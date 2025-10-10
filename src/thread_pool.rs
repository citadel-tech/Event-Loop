
#[cfg(feature = "unstable-mpmc")]
use std::sync::mpmc as channel;
#[cfg(not(feature = "unstable-mpmc"))]
use std::sync::mpsc as channel;
use std::{
    sync::{Arc, Mutex},
    thread::{Builder, JoinHandle},
};

use crate::error::Result;

pub const DEFAULT_POOL_CAPACITY: usize = 4;

type Task = Box<dyn FnOnce() + Send + 'static>;

enum WorkerMessage {
    Task(Task),
    Terminate,
}

pub struct ThreadPool {
    workers: Vec<Worker>,
    sender: channel::Sender<WorkerMessage>,
}

type ChannelReceiver = channel::Receiver<WorkerMessage>;

impl Default for ThreadPool {
    fn default() -> Self {
        Self::new(DEFAULT_POOL_CAPACITY)
    }
}

impl ThreadPool {
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = channel::channel::<WorkerMessage>();

        let receiver = Arc::new(Mutex::new(receiver));

        let workers: Vec<Worker> = (0..capacity)
            .map(|id| Worker::new(id, Arc::clone(&receiver)))
            .collect();

        Self { workers, sender }
    }

    pub fn exec<F>(&self, task: F) -> Result<()>
    where
        F: FnOnce() + Send + 'static,
    {
        Ok(self.sender.send(WorkerMessage::Task(Box::new(task)))?)
    }

    pub fn workers_len(&self) -> usize {
        self.workers.len()
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        self.workers.iter().for_each(|_| {
            let _ = self.sender.send(WorkerMessage::Terminate);
        });
        self.workers.iter_mut().for_each(|worker| {
            if let Some(t) = worker.take_thread() {
                t.join().unwrap();
            }
        });
    }
}

struct Worker {
    #[cfg(target_os = "linux")]
    id: usize,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn new(id: usize, reciever: Arc<Mutex<ChannelReceiver>>) -> Self {
        let thread = Some(
            Builder::new()
                .name(format!("thread-pool-worker-{id}"))
                .spawn(move || loop {
                    let task = {
                        let receiver = reciever.lock().unwrap();
                        if let Ok(message) = receiver.recv() {
                            match message {
                                WorkerMessage::Task(task) => task,
                                WorkerMessage::Terminate => break,
                            }
                        } else {
                            break;
                        }
                    };

                    task();
                })
                .expect(&format!("Couldn't create the worker thread id={id}")),
        );

        Self {
            #[cfg(target_os = "linux")]
            id,
            thread,
        }
    }

    pub fn take_thread(&mut self) -> Option<JoinHandle<()>> {
        self.thread.take()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use super::*;

    #[test]
    fn test_thread_pool_creation() {
        let pool = ThreadPool::new(4);
        assert_eq!(pool.workers_len(), 4);
    }

    #[test]
    fn test_task_execution() {
        let pool = ThreadPool::new(2);
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        pool.exec(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_multiple_tasks() {
        let pool = ThreadPool::new(4);
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..10 {
            let counter_clone = counter.clone();
            pool.exec(move || {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
        }

        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }

    #[test]
    fn test_pool_cleanup() {
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let pool = ThreadPool::new(2);
            let counter_clone = counter.clone();

            pool.exec(move || {
                std::thread::sleep(Duration::from_millis(50));
                counter_clone.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
