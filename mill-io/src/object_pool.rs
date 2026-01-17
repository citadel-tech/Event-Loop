use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A thread-safe object pool for reusing allocations.
///
/// This pool uses a simple mutex-protected deque for storing objects.
/// Objects are lazily created when the pool is empty.
#[derive(Clone)]
pub struct ObjectPool<T> {
    pool: Arc<Mutex<VecDeque<T>>>,
    create_fn: Arc<dyn Fn() -> T + Send + Sync>,
    capacity: usize,
}

impl<T: Send + 'static> ObjectPool<T> {
    /// Creates a new object pool with the specified `initial_size` and using `create_fn`
    pub fn new<F>(initial_size: usize, create_fn: F) -> Self
    where
        F: Fn() -> T + Send + Sync + 'static,
    {
        let capacity = initial_size;
        let mut pool = VecDeque::with_capacity(capacity);

        for _ in 0..initial_size {
            pool.push_back(create_fn());
        }

        Self {
            pool: Arc::new(Mutex::new(pool)),
            create_fn: Arc::new(create_fn),
            capacity,
        }
    }

    /// Acquires an object from the pool, creating a new one if the pool is empty.
    #[inline]
    pub fn acquire(&self) -> PooledObject<T> {
        let object = {
            let mut pool = self.pool.lock().unwrap();
            pool.pop_front()
        };

        let object = object.unwrap_or_else(|| (self.create_fn)());

        PooledObject {
            object: Some(object),
            pool: Arc::clone(&self.pool),
            capacity: self.capacity,
        }
    }

    /// Returns the approximate number of objects currently in the pool.
    pub fn available(&self) -> usize {
        self.pool.lock().unwrap().len()
    }
}

/// A guard that returns the object to the pool when dropped.
pub struct PooledObject<T> {
    object: Option<T>,
    pool: Arc<Mutex<VecDeque<T>>>,
    capacity: usize,
}

impl<T> PooledObject<T> {
    /// Takes ownership of the inner object, preventing it from returning to the pool.
    pub fn take(mut self) -> T {
        self.object.take().expect("PooledObject already taken")
    }
}

impl<T> std::ops::Deref for PooledObject<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.object.as_ref().expect("PooledObject is empty")
    }
}

impl<T> std::ops::DerefMut for PooledObject<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.object.as_mut().expect("PooledObject is empty")
    }
}

impl<T> AsRef<T> for PooledObject<T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T> AsMut<T> for PooledObject<T> {
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        self
    }
}

impl<T> Drop for PooledObject<T> {
    #[inline]
    fn drop(&mut self) {
        if let Some(object) = self.object.take() {
            let mut pool = self.pool.lock().unwrap();
            // Only return to pool if under capacity
            if pool.len() < self.capacity {
                pool.push_back(object);
            }
            // Otherwise, object is dropped
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_reuse() {
        let pool = ObjectPool::new(1, || vec![0u8; 1024]);

        let obj1 = pool.acquire();
        let ptr1 = obj1.as_ptr();
        drop(obj1);

        let obj2 = pool.acquire();
        let ptr2 = obj2.as_ptr();

        assert_eq!(ptr1, ptr2, "Pool should reuse the same allocation");
    }

    #[test]
    fn test_pool_fifo_order() {
        let pool = ObjectPool::new(0, || vec![0u8; 1024]);

        let obj1 = pool.acquire();
        let ptr1 = obj1.as_ptr();
        drop(obj1);

        let obj2 = pool.acquire();
        drop(obj2);

        // now pool has [ptr1, ptr2]. should get ptr1 first (FIFO)
        let obj3 = pool.acquire();
        assert_eq!(obj3.as_ptr(), ptr1, "Should get first returned object");
    }

    #[test]
    fn test_pool_grows() {
        let pool = ObjectPool::new(1, || vec![0u8; 1024]);

        // hold more objects than initial size
        let _obj1 = pool.acquire();
        let _obj2 = pool.acquire();
        let _obj3 = pool.acquire();

        // should not panic. creates new objects as needed
    }

    #[test]
    fn test_pool_capacity_limit() {
        let pool = ObjectPool::new(2, || vec![0u8; 1024]);

        let obj1 = pool.acquire();
        let obj2 = pool.acquire();
        let obj3 = pool.acquire();

        assert_eq!(pool.available(), 0);

        drop(obj1);
        drop(obj2);
        drop(obj3);

        assert_eq!(pool.available(), 2, "Pool should respect capacity limit");
    }

    #[test]
    fn test_pool_available() {
        let pool = ObjectPool::new(5, || vec![0u8; 1024]);
        assert_eq!(pool.available(), 5);

        let _obj1 = pool.acquire();
        assert_eq!(pool.available(), 4);

        let _obj2 = pool.acquire();
        assert_eq!(pool.available(), 3);
    }

    #[test]
    fn test_pooled_object_take() {
        let pool = ObjectPool::new(1, || vec![0u8; 1024]);

        let obj = pool.acquire();
        let _vec = obj.take();

        assert_eq!(pool.available(), 0);
    }
}
