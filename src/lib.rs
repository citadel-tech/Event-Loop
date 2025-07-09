#![cfg_attr(feature = "unstable-mpmc", feature(mpmc_channel))]
use std::error::Error;

use mio::{Interest, Token};
pub mod handler;
pub mod object_pool;
pub mod poll;
pub mod reactor;
pub mod thread_pool;

pub use handler::EventHandler;
pub use object_pool::{ObjectPool, PooledObject};

pub struct EventLoop {
    reactor: reactor::Reactor,
}

impl EventLoop {
    pub fn new(workers: usize) -> Result<Self, Box<dyn Error>> {
        let reactor = reactor::Reactor::new(workers)?;
        Ok(Self { reactor })
    }

    pub fn register<H, S>(
        &self,
        source: &mut S,
        token: Token,
        interests: Interest,
        handler: H,
    ) -> Result<(), Box<dyn Error>>
    where
        H: EventHandler + Send + Sync + 'static,
        S: mio::event::Source + ?Sized,
    {
        self.reactor
            .poll_handle
            .register(source, token, interests, handler)
    }

    pub fn deregister<S>(&self, source: &mut S) -> Result<(), Box<dyn Error>>
    where
        S: mio::event::Source + ?Sized,
    {
        self.reactor.poll_handle.deregister(source)
    }

    pub fn run(&self) -> Result<(), Box<dyn Error>> {
        self.reactor.run()
    }

    pub fn stop(&self) {
        let shutdown_handler = self.reactor.get_shutdown_handle();
        shutdown_handler.shutdown();
    }
}
