use crate::thread_pool::DEFAULT_POOL_CAPACITY;

pub fn get_default_capacity() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(DEFAULT_POOL_CAPACITY)
}
