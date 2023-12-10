
use bitcoin::{
    consensus::{deserialize_partial, serialize}, p2p::{message::{RawNetworkMessage, self, NetworkMessage}, message_network::VersionMessage, ServiceFlags}, Network
};
use bytes::{Buf, BytesMut};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{tcp::OwnedReadHalf, TcpStream},
    select, signal,
    sync::{
        broadcast,
        mpsc::{self, error::SendError, UnboundedSender},
    },
    try_join,
};

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::types;
use types::*;


const EXPECTED_HANDSHAKE_MESSAGES: usize = 4;
const TIMEOUT_MILLISEC: u64 = 1000;

pub async fn perform_btc_handshake(params: HandshakeParams) -> Result<EventChain, HSError> {
    // Setup shutdown broadcast channels
    let (shutdown_tx, _) = broadcast::channel::<usize>(1);

    // Spawn the event chain task.
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let mut ev_shutdown_rx = shutdown_tx.subscribe();
    let ev_shutdown_tx = shutdown_tx.clone();
    let event_chain_id = params.address.clone();
    let event_chain_handle = tokio::spawn(async move {
        let mut event_chain = EventChain::new(event_chain_id);
        loop {
            select! {
                Some(ev) = ev_rx.recv() => {
                    event_chain.add(ev);
                }
                recv_res = ev_shutdown_rx.recv() => {
                    return match recv_res {
                        Ok(_) => Ok(event_chain),
                        Err(err) => Err(HSError::from(err)),
                    }
                }
            }
            if event_chain.len() == EXPECTED_HANDSHAKE_MESSAGES {
                event_chain.mark_as_complete();
                ev_shutdown_tx.send(1)?;
            }
        }
    });

    // Stablish TCP connection with timeout.
    let stream = tokio::time::timeout(
        Duration::from_millis(TIMEOUT_MILLISEC),
        TcpStream::connect(&params.address),
    )
    .await??;

    let (rx_stream, mut tx_stream) = stream.into_split();

    // Spawn the message writer task. This will take care of serialize all messages write to the socket.
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<RawNetworkMessage>();
    let msg_writer_ev_tx = ev_tx.clone();
    let mut msg_writer_shutdown_rx = shutdown_tx.subscribe();
    let msg_writer_handle = tokio::spawn(async move {
        loop {
            select! {
                Some(msg) = msg_rx.recv() => {
                    let msg_type = msg.cmd().to_string();
                    let data = serialize(&msg);
                    tx_stream.write_all(data.as_slice()).await?;
                    msg_writer_ev_tx.send(Event::new(msg_type, EventDirection::OUT))?;
                }
                result = msg_writer_shutdown_rx.recv() => {
                    tx_stream.shutdown().await?;
                    return match result {
                        Ok(_) => Ok(()),
                        Err(err) => Err(HSError::from(err)),
                    }
                }
            }
        }
    });

    // Spawn the message reader task
    let mut msg_reader_shutdown_rx = shutdown_tx.subscribe();
    let msg_reader_msg_tx = msg_tx.clone();
    let msg_reader_handle = tokio::spawn(async move {
        // A complete handshake is about 342 bytes. We allocate much more so we don't need
        // to do more allocations.
        let mut msg_reader = MessageReader::new(rx_stream, 1024);
        let mut handles = Vec::new();
        loop {
            select! {
                message_res = msg_reader.read_message() => {
                    match message_res {
                        Ok(opt_res) => {
                            if let Some(msg) = opt_res {
                                let handle = tokio::spawn(handle_message(msg, msg_reader_msg_tx.clone(), ev_tx.clone()));
                                handles.push(handle);
                            }
                         },
                        Err(err) => return Err(err),
                    }
                },
                result = msg_reader_shutdown_rx.recv() => {
                   return match result {
                     Ok(_) => {
                       // Ensure all message handles succeeded before ending.
                       futures::future::try_join_all(handles).await?;
                       Ok(())
                     },
                     Err(err) => Err(HSError::from(err)),
                    }
                }
            }
        }
    });

    // Start the handshake by sending the first VERSION message
    let version_message = version_message(params.address, params.user_agent);
    msg_tx.send(version_message)?;

    // Wait for external shutdown signals ctr+c ...
    let mut ext_shutdown_shutdown_rx = shutdown_tx.subscribe();
    select! {
        _ = tokio::time::sleep(Duration::from_millis(TIMEOUT_MILLISEC)) => {
            shutdown_tx.send(1)?;
        }
        val = signal::ctrl_c() => {
            if val.is_ok(){
                shutdown_tx.send(1)?;
            }
        }
        // Break this select! once an internal shutdown is invoked from any of the subs systems.
        _val = ext_shutdown_shutdown_rx.recv()=>{}
    }

    let (event_chain_res, message_writer_res, msg_reader_res) =
        try_join!(event_chain_handle, msg_writer_handle, msg_reader_handle)?;
    // Check no errors happened in message reader and writer.
    message_writer_res?;
    msg_reader_res?;
    // Finally, check the event chain was successful and return it.
    event_chain_res
}

async fn handle_message(
    message: RawNetworkMessage,
    msg_writer: UnboundedSender<RawNetworkMessage>,
    event_publisher: UnboundedSender<Event>,
) -> Result<(), HSError> {
    let msg_type = message.cmd().to_string();
    match message.payload() {
        message::NetworkMessage::Verack => {
            let event = Event::new(msg_type, EventDirection::IN);
            event_publisher.send(event)?;
            Ok(())
        }
        message::NetworkMessage::Version(v) => {
            let mut event = Event::new(msg_type, EventDirection::IN);
            event.set_pair("vers".to_string(), v.version.to_string());
            event.set_pair("user-agent".to_string(), v.user_agent.clone());
            event_publisher.send(event)?;
            msg_writer.send(verack_message())?;
            Ok(())
        }
        _ => {
            println!(
                "{}  received message type not part of handshake: {}",
                HS_WRNG, msg_type
            );
            Ok(())
        }
    }
}

struct MessageReader {
    stream: OwnedReadHalf,
    buffer: BytesMut,
}

impl MessageReader {
    pub fn new(stream: OwnedReadHalf, buff_size: usize) -> MessageReader {
        MessageReader {
            stream,
            buffer: BytesMut::with_capacity(buff_size),
        }
    }
    pub async fn read_message(&mut self) -> Result<Option<RawNetworkMessage>, HSError> {
        loop {
            if let Ok((message, count)) = deserialize_partial::<RawNetworkMessage>(&self.buffer) {
                self.buffer.advance(count);
                return Ok(Some(message));
            }

            if 0 == self.stream.read_buf(&mut self.buffer).await? {
                if self.buffer.is_empty() {
                    return Ok(None);
                } else {
                    return Err(HSError {
                        err_message: "connection reset by peer".into(),
                    });
                }
            }
        }
    }
}

pub fn verack_message() -> RawNetworkMessage {
    RawNetworkMessage::new(
        Network::Bitcoin.magic(),
         NetworkMessage::Verack,
    )
}

pub fn version_message(dest_socket: String, user_agent: String) -> RawNetworkMessage {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let no_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
    let node_socket = SocketAddr::from_str(&dest_socket).unwrap();

    let btc_version = VersionMessage::new(
        ServiceFlags::NONE,
        now,
        bitcoin::p2p::Address::new(&node_socket, ServiceFlags::NONE),
        bitcoin::p2p::Address::new(&no_address, ServiceFlags::NONE),
        now as u64,
        user_agent,
        0,
    );

    RawNetworkMessage::new(
         Network::Bitcoin.magic(),
         NetworkMessage::Version(btc_version))
    
}

impl From<SendError<RawNetworkMessage>> for HSError {
    fn from(send_err: SendError<RawNetworkMessage>) -> Self {
        HSError {
            err_message: send_err.to_string(),
        }
    }
}
