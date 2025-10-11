// TODO: add custom error module and use it here
use lockfree::map::Map;
use mio::{Events, Interest, Poll, Token};
use std::sync::{Arc, RwLock};

use crate::{
    error::Result,
    handler::{EventHandler, HandlerEntry},
};

type Registry = Arc<Map<Token, HandlerEntry>>;

pub struct PollHandle {
    poller: Arc<RwLock<mio::Poll>>,
    registery: Registry,
    waker: Arc<mio::Waker>,
}

impl PollHandle {
    pub fn new() -> Result<Self> {
        let poller = Arc::new(RwLock::new(Poll::new()?));
        let waker = mio::Waker::new(poller.read().unwrap().registry(), Token(0))?;
        let registery: Registry = Arc::new(Map::new());
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
        H: EventHandler + Send + Sync + 'static,
        S: mio::event::Source + ?Sized,
    {
        let handler_entry = HandlerEntry::new(handler, interest);

        let poller = self
            .poller
            .read()
            .map_err(|_| "Failed to acquire poller read lock")?;
        src.register(poller.registry(), token, interest)?;

        self.registery.insert(token, handler_entry);
        Ok(())
    }

    pub fn deregister<S>(&self, source: &mut S, token: Token) -> Result<()>
    where
        S: mio::event::Source + ?Sized,
    {
        let poller = self
            .poller
            .read()
            .map_err(|_| "Failed to acquire poller read lock")?;
        poller.registry().deregister(source)?;

        self.registery.remove(&token);

        Ok(())
    }

    pub fn poll(&self, events: &mut Events, timeout: Option<std::time::Duration>) -> Result<usize> {
        let mut poller = self
            .poller
            .write()
            .map_err(|_| "Failed to acquire poller write lock")?;
        poller.poll(events, timeout)?;
        Ok(events.iter().count())
    }

    pub fn wake(&self) -> Result<()> {
        Ok(self.waker.wake()?)
    }

    pub fn get_registery(&self) -> Registry {
        self.registery.clone()
    }
}
#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "linux"))]
    use crate::handler::SafeEvent;

    use super::*;
    #[cfg(target_os = "linux")]
    use mio::event::Event;
    use mio::event::Source;
    use mio::Events;
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
        let poller = PollHandle::new().unwrap();
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
            fn handle_event(
                &self,
                #[cfg(target_os = "linux")] _event: &Event,
                #[cfg(not(target_os = "linux"))] _event: &SafeEvent,
            ) {
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
            poller.registery.iter().find(|t| t.0 == token).is_some(),
            "Token not found in registry"
        );

        assert!(
            poller.deregister(&mut source, token).is_ok(),
            "Failed to unregister source"
        );

        assert!(
            poller.registery.iter().find(|t| t.0 == token).is_none(),
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
            fn handle_event(
                &self,
                #[cfg(target_os = "linux")] _event: &Event,
                #[cfg(not(target_os = "linux"))] _event: &SafeEvent,
            ) {
            }
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

        assert_eq!(poller.registery.iter().count(), 2);
        assert!(
            poller.registery.iter().find(|t| t.0 == Token(1)).is_some(),
            "Failed to find src1"
        );
        assert!(
            poller.registery.iter().find(|t| t.0 == Token(2)).is_some(),
            "Failed to find src2"
        );
    }
}
