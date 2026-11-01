//! PROTOTYPE stub of a session engine: pure `std` code, capabilities injected.
//! Not the session model (issue #19), just enough std surface to be realistic.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod corpus;
pub use corpus::pure_corpus;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Event {
    User(String),
    Assistant(String),
    Tool { name: String, output: serde_json::Value },
}

/// Injected capabilities: the only way the core reaches the outside world.
pub trait Provider: Send + Sync {
    fn complete(&self, history: &[Event]) -> Result<String, String>;
}
pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

pub struct Session {
    provider: Arc<dyn Provider>,
    clock: Arc<dyn Clock>,
    log: Mutex<Vec<(Duration, Event)>>,
    meta: HashMap<String, String>,
}

impl Session {
    pub fn new(provider: Arc<dyn Provider>, clock: Arc<dyn Clock>) -> Self {
        Self { provider, clock, log: Mutex::new(Vec::new()), meta: HashMap::new() }
    }
    pub fn turn(&self, user: &str) -> Result<String, String> {
        let mut log = self.log.lock().map_err(|e| e.to_string())?;
        log.push((self.clock.now(), Event::User(user.into())));
        let hist: Vec<Event> = log.iter().map(|(_, e)| e.clone()).collect();
        let reply = self.provider.complete(&hist)?;
        log.push((self.clock.now(), Event::Assistant(reply.clone())));
        Ok(reply)
    }
    pub fn export(&self) -> String {
        let log = self.log.lock().unwrap();
        serde_json::to_string(&log.iter().map(|(_, e)| e).collect::<Vec<_>>()).unwrap()
    }
    pub fn meta(&self) -> &HashMap<String, String> { &self.meta }
}
