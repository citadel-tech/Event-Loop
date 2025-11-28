#[cfg(feature = "unstable-mpmc")]
use std::sync::mpmc as channel;
#[cfg(not(feature = "unstable-mpmc"))]
use std::sync::mpsc as channel;
use std::{
    cmp::Ordering as CmpOrdering,
    collections::BinaryHeap,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Barrier, Condvar, Mutex,
    },
    thread::{Builder, JoinHandle},
};

use crate::error::Result;

pub const DEFAULT_POOL_CAPACITY: usize = 4;

pub type Task = Box<dyn FnOnce() + Send + 'static>;

enum WorkerMessage {
    Task(Task),
    Terminate,
}

pub struct ThreadPool {
    workers: Vec<Worker>,
    senders: Vec<channel::Sender<WorkerMessage>>,
    next_worker: AtomicUsize,
}

impl Default for ThreadPool {
    fn default() -> Self {
        let default_capacity = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(DEFAULT_POOL_CAPACITY);
        Self::new(default_capacity)
    }
}

impl ThreadPool {
    pub fn new(capacity: usize) -> Self {
        let mut workers = Vec::with_capacity(capacity);
        let mut senders = Vec::with_capacity(capacity);

        for id in 0..capacity {
            let (sender, receiver) = channel::channel::<WorkerMessage>();
            workers.push(Worker::new(id, receiver));
            senders.push(sender);
        }

        Self {
            workers,
            senders,
            next_worker: AtomicUsize::new(0),
        }
    }

    pub fn exec<F>(&self, task: F) -> Result<()>
    where
        F: FnOnce() + Send + 'static,
    {
        // Round-robin dispatch
        let index = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.senders.len();
        Ok(self.senders[index].send(WorkerMessage::Task(Box::new(task)))?)
    }

    pub fn workers_len(&self) -> usize {
        self.workers.len()
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        for sender in &self.senders {
            let _ = sender.send(WorkerMessage::Terminate);
        }
        for worker in &mut self.workers {
            if let Some(t) = worker.take_thread() {
                t.join().unwrap();
            }
        }
    }
}

struct Worker {
    #[allow(dead_code)]
    id: usize,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn new(id: usize, receiver: channel::Receiver<WorkerMessage>) -> Self {
        let thread = Some(
            Builder::new()
                .name(format!("thread-pool-worker-{id}"))
                .spawn(move || {
                    while let Ok(message) = receiver.recv() {
                        match message {
                            WorkerMessage::Task(task) => task(),
                            WorkerMessage::Terminate => break,
                        }
                    }
                })
                .expect("Couldn't create the worker thread id={id}"),
        );

        Self { id, thread }
    }

    pub fn take_thread(&mut self) -> Option<JoinHandle<()>> {
        self.thread.take()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

struct PriorityTask {
    task: Task,
    priority: TaskPriority,
    sequence: u64,
}

impl PartialEq for PriorityTask {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.sequence == other.sequence
    }
}

impl Eq for PriorityTask {}

impl PartialOrd for PriorityTask {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for PriorityTask {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        match self.priority.cmp(&other.priority) {
            CmpOrdering::Equal => other.sequence.cmp(&self.sequence),
            ord => ord,
        }
    }
}

struct ComputeSharedState {
    queue: Mutex<BinaryHeap<PriorityTask>>,
    condvar: Condvar,
    shutdown: AtomicBool,
}

pub struct ComputeThreadPool {
    workers: Vec<JoinHandle<()>>,
    state: Arc<ComputeSharedState>,
    sequence: AtomicU64,
}

impl ComputeThreadPool {
    pub fn new(capacity: usize) -> Self {
        let state = Arc::new(ComputeSharedState {
            queue: Mutex::new(BinaryHeap::new()),
            condvar: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });

        let mut workers = Vec::with_capacity(capacity);
        // barrier to ensure all workers are started before returning
        let barrier = Arc::new(Barrier::new(capacity + 1));

        for id in 0..capacity {
            let state_clone = Arc::clone(&state);
            let barrier_clone = Arc::clone(&barrier);
            let thread = Builder::new()
                .name(format!("compute-worker-{id}"))
                .spawn(move || {
                    // wait for all workers to be ready
                    barrier_clone.wait();

                    loop {
                        let task = {
                            let mut queue = state_clone.queue.lock().unwrap();

                            while queue.is_empty() && !state_clone.shutdown.load(Ordering::Relaxed)
                            {
                                queue = state_clone.condvar.wait(queue).unwrap();
                            }

                            if state_clone.shutdown.load(Ordering::Relaxed) && queue.is_empty() {
                                break;
                            }

                            queue.pop()
                        };

                        if let Some(priority_task) = task {
                            (priority_task.task)();
                        }
                    }
                })
                .expect("Failed to create compute worker thread");
            workers.push(thread);
        }

        // wait for all workers to start
        barrier.wait();

        Self {
            workers,
            state,
            sequence: AtomicU64::new(0),
        }
    }

    pub fn spawn<F>(&self, task: F, priority: TaskPriority)
    where
        F: FnOnce() + Send + 'static,
    {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let priority_task = PriorityTask {
            task: Box::new(task),
            priority,
            sequence,
        };

        let mut queue = self.state.queue.lock().unwrap();
        queue.push(priority_task);
        self.state.condvar.notify_one();
    }
}

impl Drop for ComputeThreadPool {
    fn drop(&mut self) {
        self.state.shutdown.store(true, Ordering::SeqCst);
        self.state.condvar.notify_all();

        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        sync::Arc,
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
