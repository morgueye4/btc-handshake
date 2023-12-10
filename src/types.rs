use std::fmt::{self, Display};

use std::{
    ops::Add,
    time::{Duration, Instant},
};
use tokio::{
    sync::{broadcast::error::RecvError, mpsc::error::SendError},
    task::*,
    time::error::Elapsed,
};

use structopt::StructOpt;

pub const HS_OK: &str = "🟩";
pub const HS_NOK: &str = "🔴";
pub const HS_IN: &str = "<<<<";
pub const HS_OUT: &str = ">>>";
pub const HS_TO: &str = "🕐"; // Timeout
pub const HS_WRNG: &str = "⚠️";// Warning


#[derive(StructOpt, Debug, Clone)]
pub struct HandshakeParams {
  #[structopt(short, long, help = "Ip addres of the BTC node.")]
  pub address: String,
  #[structopt(short, long, help = "The user agent of the BTC node ")]
  pub user_agent: String,
}

#[derive(Debug)]
pub struct HSError {
  pub err_message: String,
}

impl fmt::Display for HSError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Handshake Error: {}", self.err_message)
    }
}

impl From<SendError<Event>> for HSError {
    fn from(err: SendError<Event>) -> Self {
        HSError {
            err_message: err.to_string(),
        }
    }
}

impl From<std::io::Error> for HSError {
    fn from(err: std::io::Error) -> Self {
        HSError {
            err_message: err.to_string(),
        }
    }
}

impl From<SendError<usize>> for HSError {
    fn from(err: SendError<usize>) -> Self {
        HSError {
            err_message: err.to_string(),
        }
    }
}

impl From<RecvError> for HSError {
    fn from(err: RecvError) -> Self {
        HSError {
            err_message: err.to_string(),
        }
    }
}

impl From<tokio::sync::broadcast::error::SendError<usize>> for HSError {
    fn from(err: tokio::sync::broadcast::error::SendError<usize>) -> Self {
        HSError {
            err_message: err.to_string(),
        }
    }
}

impl From<JoinError> for HSError {
    fn from(err: JoinError) -> Self {
        HSError {
            err_message: err.to_string(),
        }
    }
}

impl From<Elapsed> for HSError {
    fn from(err: Elapsed) -> Self {
        HSError {
            err_message: err.to_string(),
        }
    }
}



pub struct HandshakeResult {
    id: String,
    result: Result<EventChain, HSError>,
}

impl HandshakeResult {
    pub fn new(id: String, result: Result<EventChain, HSError>) -> HandshakeResult {
        HandshakeResult { id, result }
    }

    pub fn id(&self) -> &str {
        self.id.as_ref()
    }

    pub fn result(&self) -> Result<&EventChain, &HSError> {
        self.result.as_ref()
    }
}

impl Display for HandshakeResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.result.is_ok() {
            true => {
                write!(f, "{}", self.result().unwrap())
            }
            false => {
                write!(
                    f,
                    "{} {}: {}",
                    HS_NOK,
                    self.id,
                    self.result().err().unwrap()
                )
            }
        }
    }
}



pub struct EventChain {
    id: String,
    complete: bool,
    events: Vec<Event>,
}

impl EventChain {
    pub fn new(id: String) -> Self {
        EventChain {
            id,
            events: Vec::new(),
            complete: false,
        }
    }

    pub fn add(&mut self, event: Event) {
        self.events.push(event);
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.len() == 0
    }

    pub fn get(&self, n: usize) -> Option<&Event> {
        self.events.get(n)
    }

    pub fn mark_as_complete(&mut self) {
        self.complete = true;
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn id(&self) -> &str {
        self.id.as_ref()
    }
}

impl Display for EventChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = if self.is_complete() {
            HS_OK
        } else {
            HS_TO
        };
        write!(f, "{} - {}", status, self.id())?;
        write!(f, " || ")?;

        let mut last_ev: Option<&Event> = None;
        let mut total_time_millis = Duration::from_millis(0);
        for ev in self.events.iter() {
            let elapsed_time = match last_ev {
                Some(l_ev) => ev.time().duration_since(l_ev.time()),
                None => Duration::from_millis(0),
            };
            total_time_millis = total_time_millis.add(elapsed_time);
            if last_ev.is_some() {
                write!(f, " -- {:#?} --> ", elapsed_time)?;
            }
            write!(f, "{}", ev)?;
            last_ev = Some(ev);
        }
        write!(f, " || total time {:#?}.", total_time_millis)
    }
}

pub struct Event {
    name: String,
    time: Instant,
    direction: EventDirection,
    data_pairs: Vec<(String, String)>,
}

impl Event {
    pub fn new(name: String, direction: EventDirection) -> Event {
        Event {
            name,
            direction,
            time: Instant::now(),
            data_pairs: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        self.name.as_ref()
    }

    pub fn time(&self) -> Instant {
        self.time
    }

    pub fn direction(&self) -> &EventDirection {
        &self.direction
    }

    pub fn data_pairs(&self) -> &[(String, String)] {
        self.data_pairs.as_ref()
    }

    pub fn set_pair(&mut self, key: String, val: String) {
        self.data_pairs.push((key, val));
    }
}

impl Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.name(), self.direction())?;
        if !self.data_pairs.is_empty() {
            let mut pairs = String::new();
            pairs.push_str(" (");
            self.data_pairs
                .iter()
                .for_each(|(k, v)| pairs.push_str(format!("{}:{} ", k, v).as_str()));
            pairs = pairs.trim_end().to_string();
            pairs.push(')');
            write!(f, "{}", pairs)?;
        }
        Ok(())
    }
}

pub enum EventDirection {
    IN,
    OUT,
}

impl Display for EventDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let direction = match self {
            EventDirection::IN => HS_IN,
            EventDirection::OUT => HS_OUT,
        };
        write!(f, "{}", direction)
    }
}
