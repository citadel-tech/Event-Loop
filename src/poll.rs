// TODO: add custom error module and use it here
use mio::{Events, Interest, Poll, Token};
use std::{
    collections::HashMap,
    error::Error,
    sync::{Arc, RwLock},
};

use crate::handler::{EventHandler, HandlerEntry};

type Registry = Arc<RwLock<HashMap<Token, HandlerEntry>>>;

pub struct PollHandle {
    poller: RwLock<mio::Poll>,
    registery: Registry,
    waker: Arc<mio::Waker>,
}

impl PollHandle {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let poller = RwLock::new(Poll::new()?);
        let waker = mio::Waker::new(poller.read().unwrap().registry(), Token(0))?;
        let registery: Registry = Arc::new(RwLock::new(HashMap::new()));
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
    ) -> Result<(), Box<dyn Error>>
    where
        H: EventHandler + Send + Sync + 'static,
        S: mio::event::Source + ?Sized,
    {
        src.register(self.poller.read().unwrap().registry(), token, interest)?;

        match self.registery.write() {
            Ok(mut registery) => {
                registery.insert(token, HandlerEntry::new(handler, interest));
            }
            Err(_) => {
                return Err("Failed to write lock registery".into());
            }
        }
        Ok(())
    }

    pub fn deregister<S>(&self, source: &mut S) -> Result<(), Box<dyn Error>>
    where
        S: mio::event::Source + ?Sized,
    {
        self.poller.write().unwrap().registry().deregister(source)?;

        Ok(())
    }

    pub fn poll<'a>(
        &self,
        events: &'a mut Events,
        timeout: Option<std::time::Duration>,
    ) -> Result<usize, Box<dyn Error>> {
        self.poller.write().unwrap().poll(events, timeout)?;
        Ok(events.iter().count())
    }

    pub fn wake(&self) -> Result<(), Box<dyn Error>> {
        Ok(self.waker.wake()?)
    }

    pub fn get_registery(&self) -> Registry {
        self.registery.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mio::Events;
    use mio::event::Source;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    struct TestSource;
    impl Source for TestSource {
        fn register(
            &mut self,
            _registry: &mio::Registry,
            _token: Token,
            _interests: Interest,
        ) -> std::io::Result<()> {
            Ok(())
        }

        fn reregister(
            &mut self,
            _registry: &mio::Registry,
            _token: Token,
            _interests: Interest,
        ) -> std::io::Result<()> {
            Ok(())
        }

        fn deregister(&mut self, _registry: &mio::Registry) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl TestSource {
        fn new() -> Self {
            TestSource
        }
    }

    #[test]
    fn test_poll() {
        let mut poller = PollHandle::new().unwrap();
        let mut events = Events::with_capacity(1024);
        poller
            .poll(&mut events, Some(Duration::from_secs(1)))
            .unwrap();
    }

    #[test]
    fn test_wake() {
        let poller = PollHandle::new().unwrap();
        assert!(poller.wake().is_ok());
    }

    #[test]
    fn test_register_unregister() {
        let poller = PollHandle::new().unwrap();
        let mut source = TestSource::new();
        let token = Token(1);

        struct TestHandler {
            called: Arc<AtomicBool>,
        }

        impl EventHandler for TestHandler {
            fn handle_event(&self, _event: &mio::event::Event) {
                self.called.store(true, Ordering::SeqCst);
            }
        }

        let handler = TestHandler {
            called: Arc::new(AtomicBool::new(false)),
        };

        assert!(
            poller
                .register(&mut source, token, Interest::READABLE, handler)
                .is_ok(),
            "Failed to register source"
        );

        assert!(
            poller.registery.read().unwrap().contains_key(&token),
            "Token not found in registry"
        );

        assert!(
            poller.deregister(&mut source).is_ok(),
            "Failed to unregister source"
        );

        assert!(
            !poller.registery.read().unwrap().contains_key(&token),
            "Token should have been removed from registry"
        );
    }

    #[test]
    fn test_multiple_handlers() {
        let poller = PollHandle::new().unwrap();
        let mut src1 = TestSource::new();
        let mut src2 = TestSource::new();

        struct NoopHandler;
        impl EventHandler for NoopHandler {
            fn handle_event(&self, _event: &mio::event::Event) {}
        }

        assert!(
            poller
                .register(&mut src1, Token(1), Interest::READABLE, NoopHandler)
                .is_ok(),
            "Failed to register src1"
        );
        assert!(
            poller
                .register(&mut src2, Token(2), Interest::WRITABLE, NoopHandler)
                .is_ok(),
            "Failed to register src2"
        );

        let registry = poller.registery.read().unwrap();
        assert_eq!(registry.len(), 2);
        assert!(registry.contains_key(&Token(1)), "Failed to find src1");
        assert!(registry.contains_key(&Token(2)), "Failed to find src2");
    }
}
