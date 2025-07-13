#![cfg_attr(feature = "unstable-mpmc", feature(mpmc_channel))]

use mio::{Interest, Token};
pub mod error;
pub mod handler;
pub mod object_pool;
pub mod poll;
pub mod reactor;
pub mod thread_pool;

pub use handler::EventHandler;
pub use object_pool::{ObjectPool, PooledObject};

use crate::{
    error::Result,
    reactor::{DEFAULT_EVENTS_CAPACITY, DEFAULT_POLL_TIMEOUT_MS},
    thread_pool::DEFAULT_POOL_CAPACITY,
};

pub struct EventLoop {
    reactor: reactor::Reactor,
}

impl Default for EventLoop {
    fn default() -> Self {
        let reactor = reactor::Reactor::new(
            DEFAULT_POOL_CAPACITY,
            DEFAULT_EVENTS_CAPACITY,
            DEFAULT_POLL_TIMEOUT_MS,
        )
        .unwrap();
        Self { reactor }
    }
}

impl EventLoop {
    pub fn new(workers: usize, events_capacity: usize, poll_timeout_ms: u64) -> Result<Self> {
        let reactor = reactor::Reactor::new(workers, events_capacity, poll_timeout_ms)?;
        Ok(Self { reactor })
    }

    pub fn register<H, S>(
        &self,
        source: &mut S,
        token: Token,
        interests: Interest,
        handler: H,
    ) -> Result<()>
    where
        H: EventHandler + Send + Sync + 'static,
        S: mio::event::Source + ?Sized,
    {
        self.reactor
            .poll_handle
            .register(source, token, interests, handler)
    }

    pub fn deregister<S>(&self, source: &mut S) -> Result<()>
    where
        S: mio::event::Source + ?Sized,
    {
        self.reactor.poll_handle.deregister(source)
    }

    pub fn run(&self) -> Result<()> {
        self.reactor.run()
    }

    pub fn stop(&self) {
        let shutdown_handler = self.reactor.get_shutdown_handle();
        shutdown_handler.shutdown();
    }
}
