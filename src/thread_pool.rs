#[allow(dead_code)]
#[cfg(feature = "unstable-mpmc")]
use std::sync::mpmc as channel;
#[cfg(not(feature = "unstable-mpmc"))]
use std::sync::mpsc as channel;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
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

/// Statistics for tracking thread pool performance
#[derive(Debug, Clone)]
pub struct PoolStats {
    pub total_tasks: usize,
    pub tasks_per_worker: HashMap<usize, usize>,
    pub active_workers: usize,
}

pub struct ThreadPool {
    workers: Vec<Worker>,
    sender: channel::Sender<WorkerMessage>,
    task_counter: Arc<AtomicUsize>,
    worker_task_counters: Arc<Mutex<HashMap<usize, Arc<AtomicUsize>>>>,
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
        let task_counter = Arc::new(AtomicUsize::new(0));
        let worker_task_counters = Arc::new(Mutex::new(HashMap::new()));

        let workers: Vec<Worker> = (0..capacity)
            .map(|id| {
                // Initialize task counter for this worker
                let worker_counter = Arc::new(AtomicUsize::new(0));
                worker_task_counters
                    .lock()
                    .unwrap()
                    .insert(id, Arc::clone(&worker_counter));

                Worker::new(id, Arc::clone(&receiver), Arc::clone(&worker_counter))
            })
            .collect();

        Self {
            workers,
            sender,
            task_counter,
            worker_task_counters,
        }
    }

    pub fn exec<F>(&self, task: F) -> Result<()>
    where
        F: FnOnce() + Send + 'static,
    {
        self.task_counter.fetch_add(1, Ordering::SeqCst);
        Ok(self.sender.send(WorkerMessage::Task(Box::new(task)))?)
    }

    pub fn workers_len(&self) -> usize {
        self.workers.len()
    }

    /// get statistics about the thread pool performance
    pub fn get_stats(&self) -> PoolStats {
        let total_tasks = self.task_counter.load(Ordering::SeqCst);
        let worker_counters = self.worker_task_counters.lock().unwrap();
        let tasks_per_worker: HashMap<usize, usize> = worker_counters
            .iter()
            .map(|(id, counter)| (*id, counter.load(Ordering::SeqCst)))
            .collect();

        let active_workers = self.workers.iter().filter(|w| w.is_active()).count();

        PoolStats {
            total_tasks,
            tasks_per_worker,
            active_workers,
        }
    }

    /// get a list of worker IDs
    pub fn worker_ids(&self) -> Vec<usize> {
        self.workers.iter().map(|w| w.id()).collect()
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
    id: usize,
    thread: Option<JoinHandle<()>>,
    task_counter: Arc<AtomicUsize>,
}

impl Worker {
    pub fn new(
        id: usize,
        receiver: Arc<Mutex<ChannelReceiver>>,
        task_counter: Arc<AtomicUsize>,
    ) -> Self {
        let task_counter_clone = Arc::clone(&task_counter);
        let thread = Some(
            Builder::new()
                .name(format!("thread-pool-worker-{id}"))
                .spawn(move || loop {
                    let task = {
                        let receiver = receiver.lock().unwrap();
                        if let Ok(message) = receiver.recv() {
                            match message {
                                WorkerMessage::Task(task) => {
                                    task_counter_clone.fetch_add(1, Ordering::SeqCst);
                                    task
                                }
                                WorkerMessage::Terminate => {
                                    break;
                                }
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
            id,
            thread,
            task_counter,
        }
    }

    pub fn take_thread(&mut self) -> Option<JoinHandle<()>> {
        self.thread.take()
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn is_active(&self) -> bool {
        self.thread.is_some()
    }

    pub fn tasks_completed(&self) -> usize {
        self.task_counter.load(Ordering::SeqCst)
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

    #[test]
    fn test_worker_ids() {
        let pool = ThreadPool::new(3);
        let ids = pool.worker_ids();
        assert_eq!(ids, vec![0, 1, 2]);
    }

    #[test]
    fn test_pool_stats() {
        let pool = ThreadPool::new(2);
        let counter = Arc::new(AtomicUsize::new(0));

        // Execute some tasks
        for _ in 0..5 {
            let counter_clone = counter.clone();
            pool.exec(move || {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(10));
            })
            .unwrap();
        }

        std::thread::sleep(Duration::from_millis(100));

        let stats = pool.get_stats();
        assert_eq!(stats.total_tasks, 5);
        assert_eq!(stats.active_workers, 2);

        // Check that tasks were distributed among workers
        let total_worker_tasks: usize = stats.tasks_per_worker.values().sum();
        assert_eq!(total_worker_tasks, 5);
    }

    #[test]
    fn test_individual_worker_task_counts() {
        let pool = ThreadPool::new(2);

        // Execute tasks and check worker-specific counts
        for _ in 0..4 {
            pool.exec(|| {
                std::thread::sleep(Duration::from_millis(10));
            })
            .unwrap();
        }

        std::thread::sleep(Duration::from_millis(100));

        let stats = pool.get_stats();

        // Verify that each worker has completed some tasks
        for (worker_id, task_count) in &stats.tasks_per_worker {
            assert!(*worker_id < 2, "Worker ID should be 0 or 1");
            assert!(
                *task_count > 0,
                "Each worker should have completed at least one task"
            );
        }
    }
}
