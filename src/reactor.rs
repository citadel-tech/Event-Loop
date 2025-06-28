use std::{
    error::Error,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use crate::{poll::PollHandle, thread_pool::ThreadPool};
use mio::{Events, event::Event};

const EVENTS_CAPACITY: usize = 1024;
const POLL_TIMEOUT_MS: u64 = 150;

pub struct Reactor {
    poll_handle: PollHandle,
    events: Events,
    pool: ThreadPool,
    running: AtomicBool,
}

impl Reactor {
    pub fn new(pool_size: usize) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            poll_handle: PollHandle::new()?,
            events: Events::with_capacity(EVENTS_CAPACITY),
            pool: ThreadPool::new(pool_size),
            running: AtomicBool::new(false),
        })
    }

    pub fn run(&mut self) -> Result<(), Box<dyn Error>> {
        self.running.store(true, Ordering::SeqCst);

        while self.running.load(Ordering::SeqCst) {
            println!("{}", self.running.load(Ordering::SeqCst));
            let _ = self.poll_handle.poll(
                &mut self.events,
                Some(Duration::from_millis(POLL_TIMEOUT_MS)),
            )?;

            for event in self.events.iter() {
                self.dispatch_event(event.clone())?;
            }
        }
        Ok(())
    }

    pub fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.poll_handle.wake().unwrap();
        todo!("UNIMPLEMENTED CORRECTLY");
    }

    pub fn dispatch_event(&self, event: Event) -> Result<(), Box<dyn Error>> {
        let token = event.token();

        let registry = self.poll_handle.get_registery();

        self.pool.exec(move || {
            let registry = registry.read().unwrap();

            if let Some(entry) = registry.get(&token) {
                if (entry.interest.is_readable() && event.is_readable())
                    || (entry.interest.is_writable() && event.is_writable())
                {
                    entry.handler.handle_event(&event);
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use mio::{Interest, Token, event::Source};

    use super::*;
    use crate::handler::*;
    use std::sync::{Arc, Mutex};

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

    #[derive(Clone)]
    struct TestHandler {
        counter: Arc<Mutex<usize>>,
    }

    impl EventHandler for TestHandler {
        fn handle_event(&self, _event: &Event) {
            let mut count = self.counter.lock().unwrap();
            *count += 1;
        }
    }

    #[test]
    fn test_reactor_creation() {
        let reactor = Reactor::new(4);
        assert!(reactor.is_ok());
    }

    #[test]
    fn test_reactor_start_stop() {
        todo!("PANIC ON THE LOCK");
        let reactor = Arc::new(Mutex::new(Reactor::new(4).unwrap()));

        let reactor_clone = Arc::clone(&reactor);
        println!("running");
        let handle = std::thread::spawn(move || {
            println!("running");
            let mut unlocked = reactor_clone.lock().unwrap();
            unlocked.run().unwrap();
        });

        std::thread::sleep(Duration::from_millis(100));
        let unlocked = reactor.lock().unwrap();
        unlocked.shutdown();
        handle.join().unwrap();
    }

    #[test]
    fn test_event_dispatch() {
        let mut reactor = Reactor::new(2).unwrap();
        let counter = Arc::new(Mutex::new(0));
        let mut source = TestSource;

        let handler = TestHandler {
            counter: counter.clone(),
        };

        let token = Token(1);

        reactor
            .poll_handle
            .register(&mut source, token, Interest::READABLE, handler)
            .unwrap();

        let _ = reactor.poll_handle.poll(&mut reactor.events, None).unwrap();
        for event in reactor.events.iter() {
            reactor.dispatch_event(event.clone()).unwrap();
        }

        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(*counter.lock().unwrap(), 1);
    }

    #[test]
    fn test_multiple_events() {
        let mut reactor = Reactor::new(4).unwrap();
        let counter = Arc::new(Mutex::new(0));
        let mut source = TestSource;

        for i in 0..3 {
            let handler = TestHandler {
                counter: counter.clone(),
            };

            let token = Token(i);
            reactor
                .poll_handle
                .register(
                    &mut source,
                    token,
                    Interest::READABLE | Interest::WRITABLE,
                    handler.clone(),
                )
                .unwrap();

            reactor
                .poll_handle
                .register(&mut source, token, Interest::READABLE, handler.clone())
                .unwrap();

            let _ = reactor.poll_handle.poll(&mut reactor.events, None).unwrap();
        }

        for event in reactor.events.iter() {
            reactor.dispatch_event(event.clone()).unwrap();
        }

        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(*counter.lock().unwrap(), 6);
    }
}
