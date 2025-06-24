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
