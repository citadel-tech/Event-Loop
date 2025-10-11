#[cfg(not(target_os = "linux"))]
use crate::handler::SafeEvent;
use crate::{error::Result, poll::PollHandle, thread_pool::ThreadPool};
use mio::{event::Event, Events};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};

pub const DEFAULT_EVENTS_CAPACITY: usize = 1024;
pub const DEFAULT_POLL_TIMEOUT_MS: u64 = 150;

pub struct Reactor {
    pub(crate) poll_handle: PollHandle,
    events: Arc<RwLock<Events>>,
    pool: ThreadPool,
    running: AtomicBool,
    poll_timeout_ms: u64,
}

impl Default for Reactor {
    fn default() -> Self {
        Self {
            poll_handle: PollHandle::new().unwrap(),
            events: Arc::new(RwLock::new(Events::with_capacity(DEFAULT_EVENTS_CAPACITY))),
            pool: ThreadPool::default(),
            running: AtomicBool::new(false),
            poll_timeout_ms: DEFAULT_POLL_TIMEOUT_MS,
        }
    }
}

impl Reactor {
    pub fn new(pool_size: usize, events_capacity: usize, poll_timeout_ms: u64) -> Result<Self> {
        Ok(Self {
            poll_handle: PollHandle::new()?,
            events: Arc::new(RwLock::new(Events::with_capacity(events_capacity))),
            pool: ThreadPool::new(pool_size),
            running: AtomicBool::new(false),
            poll_timeout_ms,
        })
    }

    pub fn run(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);

        while self.running.load(Ordering::SeqCst) {
            let _ = self.poll_handle.poll(
                &mut self.events.write().unwrap(),
                Some(Duration::from_millis(self.poll_timeout_ms)),
            )?;

            for event in self.events.read().unwrap().iter() {
                self.dispatch_event(event.clone())?;
            }
        }
        Ok(())
    }

    pub fn get_shutdown_handle(&'_ self) -> ShutdownHandle<'_> {
        ShutdownHandle {
            running: &self.running,
            poll_handle: &self.poll_handle,
        }
    }

    pub fn dispatch_event(&self, event: Event) -> Result<()> {
        let token = event.token();
        let is_readable = event.is_readable();
        let is_writable = event.is_writable();

        #[cfg(not(target_os = "linux"))]
        let safe_event = SafeEvent::from(&event);

        let registry = self.poll_handle.get_registery();

        self.pool.exec(move || {
            let entry = registry.get(&token);
            if let Some(entry) = entry {
                let interest = entry.1.interest;
                let handler = entry.1.handler.as_ref();
                if (interest.is_readable() && is_readable)
                    || (interest.is_writable() && is_writable)
                {
                    handler.handle_event(&safe_event);
                }
            }
        })
    }

    pub fn get_events(&self) -> Arc<RwLock<Events>> {
        self.events.clone()
    }
}

pub struct ShutdownHandle<'a> {
    running: &'a AtomicBool,
    poll_handle: &'a PollHandle,
}

impl<'a> ShutdownHandle<'a> {
    pub fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.poll_handle.wake().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::*;
    use mio::{Interest, Token};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    #[derive(Clone)]
    struct TestHandler {
        counter: Arc<Mutex<usize>>,
        condition: Arc<Condvar>,
    }

    impl EventHandler for TestHandler {
        fn handle_event(
            &self,
            #[cfg(target_os = "linux")] _event: &Event,
            #[cfg(not(target_os = "linux"))] _event: &SafeEvent,
        ) {
            let mut count = self.counter.lock().unwrap();
            *count += 1;
            self.condition.notify_one();
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_reactor_start_stop() {
        let reactor = Arc::new(Reactor::default());
        let shutdown_handle = reactor.get_shutdown_handle();

        let reactor_clone = Arc::clone(&reactor);
        let handle = std::thread::spawn(move || {
            reactor_clone.run().unwrap();
        });

        std::thread::sleep(Duration::from_millis(100));

        shutdown_handle.shutdown();

        handle.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_with_pipe() -> std::io::Result<()> {
        use mio::net::UnixStream;

        let reactor =
            Arc::new(Reactor::new(2, DEFAULT_EVENTS_CAPACITY, DEFAULT_POLL_TIMEOUT_MS).unwrap());
        let counter = Arc::new(Mutex::new(0));
        let condition = Arc::new(Condvar::new());

        let (mut stream1, mut stream2) = UnixStream::pair()?;

        let handler = TestHandler {
            counter: Arc::clone(&counter),
            condition: Arc::clone(&condition),
        };

        let token = Token(1);

        reactor
            .poll_handle
            .register(&mut stream1, token, Interest::READABLE, handler)
            .unwrap();

        let reactor_clone = Arc::clone(&reactor);
        let handle = std::thread::spawn(move || {
            // Poll once
            let events_result = {
                let mut events = reactor_clone.events.write().unwrap();
                reactor_clone
                    .poll_handle
                    .poll(&mut *events, Some(Duration::from_millis(100)))
            };

            if let Ok(_) = events_result {
                let events = reactor_clone.events.read().unwrap();
                for event in events.iter() {
                    let _ = reactor_clone.dispatch_event(event.clone());
                }
            }
        });

        std::io::Write::write_all(&mut stream2, b"test data")?;

        handle.join().unwrap();

        let count = counter.lock().unwrap();
        let result = condition
            .wait_timeout(count, Duration::from_millis(500))
            .unwrap();

        if !result.1.timed_out() {
            assert_eq!(*result.0, 1);
        }

        Ok(())
    }

    #[test]
    fn test_with_tcp() -> std::io::Result<()> {
        use mio::net::{TcpListener, TcpStream};
        use std::net::SocketAddr;

        let reactor =
            Arc::new(Reactor::new(2, DEFAULT_EVENTS_CAPACITY, DEFAULT_POLL_TIMEOUT_MS).unwrap());
        let counter = Arc::new(Mutex::new(0));
        let condition = Arc::new(Condvar::new());

        // Create a TCP listener on localhost
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut listener = TcpListener::bind(addr)?;
        let listener_addr = listener.local_addr()?;

        let handler = TestHandler {
            counter: Arc::clone(&counter),
            condition: Arc::clone(&condition),
        };

        let token = Token(1);

        reactor
            .poll_handle
            .register(&mut listener, token, Interest::READABLE, handler)
            .unwrap();

        let reactor_clone = Arc::clone(&reactor);
        let handle = std::thread::spawn(move || {
            // Poll once
            let events_result = {
                let mut events = reactor_clone.events.write().unwrap();
                reactor_clone
                    .poll_handle
                    .poll(&mut *events, Some(Duration::from_millis(100)))
            };

            if let Ok(_) = events_result {
                let events = reactor_clone.events.read().unwrap();
                for event in events.iter() {
                    let _ = reactor_clone.dispatch_event(event.clone());
                }
            }
        });

        // Connect to trigger the event
        let _stream = TcpStream::connect(listener_addr)?;

        handle.join().unwrap();

        let count = counter.lock().unwrap();
        let result = condition
            .wait_timeout(count, Duration::from_millis(500))
            .unwrap();

        if !result.1.timed_out() {
            assert_eq!(*result.0, 1);
        }

        Ok(())
    }
}
