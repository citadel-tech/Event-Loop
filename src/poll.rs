use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use mio::{Events, Interest, Poll, Token};

use crate::handler::{Eventhandler, HandlerEntry};

pub struct PollHandle {
    poller: mio::Poll,
    registery: Arc<Mutex<HashMap<Token, HandlerEntry>>>,
    waker: Arc<mio::Waker>,
}

impl PollHandle {
    pub fn new() -> Result<Self> {
        let poller = Poll::new()?;
        let waker = mio::Waker::new(poller.registry(), Token(0))?;
        let registery: Arc<Mutex<HashMap<Token, HandlerEntry>>> =
            Arc::new(Mutex::new(HashMap::new()));
        Ok(PollHandle {
            poller,
            registery,
            waker: Arc::new(waker),
        })
    }

    pub fn register<H, S>(
        &self,
        src: &mut S,
        token: Token,
        interest: Interest,
        handler: H,
    ) -> Result<()>
    where
        H: Eventhandler + Send + Sync + 'static,
        S: mio::event::Source + ?Sized,
    {
        src.register(self.poller.registry(), token, interest)?;
        let mut registery = self.registery.lock().unwrap();
        registery.insert(token, HandlerEntry::new(handler, interest));
        Ok(())
    }

    pub fn unregister(&self, token: Token) {
        let mut registery = self.registery.lock().unwrap();
        registery.remove(&token);
    }

    pub fn poll(
        &mut self,
        events: &mut Events,
        timeout: Option<std::time::Duration>,
    ) -> Result<usize> {
        self.poller.poll(events, timeout)?;
        Ok(events.iter().count())
    }

    pub fn wake(&self) -> Result<()> {
        Ok(self.waker.wake()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mio::{Events, Interest, Poll, Token};
    use std::time::Duration;

    #[test]
    fn test_poll() {
        let mut poller = PollHandle::new().unwrap();
        let mut events = Events::with_capacity(1024);
        poller
            .poll(&mut events, Some(Duration::from_secs(1)))
            .unwrap();
    }
}
