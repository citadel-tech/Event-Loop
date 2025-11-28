#[cfg(feature = "unstable-mpmc")]
use std::sync::mpmc as channel;
#[cfg(not(feature = "unstable-mpmc"))]
use std::sync::mpsc as channel;
use std::{
    cmp::Ordering as CmpOrdering,
    collections::BinaryHeap,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Barrier, Condvar, Mutex,
    },
    thread::{Builder, JoinHandle},
    time::Instant,
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

#[derive(Debug, Default)]
pub struct ComputePoolMetrics {
    pub tasks_submitted: AtomicU64,
    pub tasks_completed: AtomicU64,
    pub tasks_failed: AtomicU64,
    pub active_workers: AtomicUsize,
    pub queue_depth_low: AtomicUsize,
    pub queue_depth_normal: AtomicUsize,
    pub queue_depth_high: AtomicUsize,
    pub queue_depth_critical: AtomicUsize,
    pub total_execution_time_ns: AtomicU64,
}

impl ComputePoolMetrics {
    pub fn tasks_submitted(&self) -> u64 {
        self.tasks_submitted.load(Ordering::Relaxed)
    }

    pub fn tasks_completed(&self) -> u64 {
        self.tasks_completed.load(Ordering::Relaxed)
    }

    pub fn tasks_failed(&self) -> u64 {
        self.tasks_failed.load(Ordering::Relaxed)
    }

    pub fn active_workers(&self) -> usize {
        self.active_workers.load(Ordering::Relaxed)
    }

    pub fn queue_depth_low(&self) -> usize {
        self.queue_depth_low.load(Ordering::Relaxed)
    }

    pub fn queue_depth_normal(&self) -> usize {
        self.queue_depth_normal.load(Ordering::Relaxed)
    }

    pub fn queue_depth_high(&self) -> usize {
        self.queue_depth_high.load(Ordering::Relaxed)
    }

    pub fn queue_depth_critical(&self) -> usize {
        self.queue_depth_critical.load(Ordering::Relaxed)
    }

    pub fn total_execution_time_ns(&self) -> u64 {
        self.total_execution_time_ns.load(Ordering::Relaxed)
    }
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
    metrics: Arc<ComputePoolMetrics>,
}

impl Default for ComputeThreadPool {
    fn default() -> Self {
        let default_capacity = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(DEFAULT_POOL_CAPACITY);
        Self::new(default_capacity)
    }
}

impl ComputeThreadPool {
    pub fn new(capacity: usize) -> Self {
        let state = Arc::new(ComputeSharedState {
            queue: Mutex::new(BinaryHeap::new()),
            condvar: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        let metrics = Arc::new(ComputePoolMetrics::default());

        let mut workers = Vec::with_capacity(capacity);
        // barrier to ensure all workers are started before returning
        let barrier = Arc::new(Barrier::new(capacity + 1));

        for id in 0..capacity {
            let state_clone = Arc::clone(&state);
            let barrier_clone = Arc::clone(&barrier);
            let metrics_clone = Arc::clone(&metrics);
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

                            let t = queue.pop();
                            if let Some(ref pt) = t {
                                match pt.priority {
                                    TaskPriority::Low => metrics_clone.queue_depth_low.fetch_sub(1, Ordering::Relaxed),
                                    TaskPriority::Normal => metrics_clone.queue_depth_normal.fetch_sub(1, Ordering::Relaxed),
                                    TaskPriority::High => metrics_clone.queue_depth_high.fetch_sub(1, Ordering::Relaxed),
                                    TaskPriority::Critical => metrics_clone.queue_depth_critical.fetch_sub(1, Ordering::Relaxed),
                                };
                            }
                            t
                        };

                        if let Some(priority_task) = task {
                            metrics_clone.active_workers.fetch_add(1, Ordering::Relaxed);
                            let start = Instant::now();

                            let result = catch_unwind(AssertUnwindSafe(|| (priority_task.task)()));

                            let duration = start.elapsed();
                            metrics_clone.total_execution_time_ns.fetch_add(duration.as_nanos() as u64, Ordering::Relaxed);
                            metrics_clone.active_workers.fetch_sub(1, Ordering::Relaxed);

                            if result.is_ok() {
                                metrics_clone.tasks_completed.fetch_add(1, Ordering::Relaxed);
                            } else {
                                metrics_clone.tasks_failed.fetch_add(1, Ordering::Relaxed);
                            }
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
            metrics,
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

        self.metrics.tasks_submitted.fetch_add(1, Ordering::Relaxed);
        match priority {
            TaskPriority::Low => self.metrics.queue_depth_low.fetch_add(1, Ordering::Relaxed),
            TaskPriority::Normal => self.metrics.queue_depth_normal.fetch_add(1, Ordering::Relaxed),
            TaskPriority::High => self.metrics.queue_depth_high.fetch_add(1, Ordering::Relaxed),
            TaskPriority::Critical => self.metrics.queue_depth_critical.fetch_add(1, Ordering::Relaxed),
        };

        let mut queue = self.state.queue.lock().unwrap();
        queue.push(priority_task);
        self.state.condvar.notify_one();
    }

    pub fn metrics(&self) -> Arc<ComputePoolMetrics> {
        self.metrics.clone()
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
        sync::{Arc, Barrier, Mutex},
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
    fn test_compute_pool_priority() {
        let pool = ComputeThreadPool::new(1); // Single thread to ensure order execution
        let result = Arc::new(Mutex::new(Vec::new()));

        // use a barrier to ensure the first task is running and blocking the worker
        let barrier = Arc::new(Barrier::new(2));
        let b_clone = barrier.clone();

        let r1 = result.clone();
        pool.spawn(
            move || {
                b_clone.wait(); // signal that we started
                std::thread::sleep(Duration::from_millis(50)); // block worker
                r1.lock().unwrap().push(1);
            },
            TaskPriority::Low,
        );

        // wait for Task 1 to start
        barrier.wait();

        // these should be queued while the first one runs
        let r2 = result.clone();
        pool.spawn(
            move || {
                r2.lock().unwrap().push(2);
            },
            TaskPriority::Low,
        );

        let r3 = result.clone();
        pool.spawn(
            move || {
                r3.lock().unwrap().push(3);
            },
            TaskPriority::High,
        );

        let r4 = result.clone();
        pool.spawn(
            move || {
                r4.lock().unwrap().push(4);
            },
            TaskPriority::Normal,
        );

        // wait for tasks to finish
        std::thread::sleep(Duration::from_millis(200));

        let res = result.lock().unwrap();
        // 1 runs first (started immediately).
        // Then 3 (High), 4 (Normal), 2 (Low).
        assert_eq!(*res, vec![1, 3, 4, 2]);
    }
}
