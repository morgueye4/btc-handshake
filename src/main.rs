mod types;
mod chain;
use std::process::exit;
use structopt::StructOpt;
use types::{HandshakeParams, HandshakeResult, HSError};


pub async fn perform_handshake(params: HandshakeParams) -> Result<HandshakeResult, HSError> {
    let join_handle = tokio::spawn(chain::perform_btc_handshake(params.clone())).await?;     
    let res = HandshakeResult::new(params.address, join_handle);
    Ok(res)
}



#[tokio::main]
async fn main() {
    let config = HandshakeParams::from_args();

    match perform_handshake(config).await {
        Ok(handshake_result) =>  println!("{}", handshake_result),
        Err(err) => {
            println!("{}", err);
            exit(1)
        }
    }
}


