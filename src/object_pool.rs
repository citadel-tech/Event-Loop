#[cfg(feature = "unstable-mpmc")]
use std::sync::mpmc as channel;
#[cfg(not(feature = "unstable-mpmc"))]
use std::sync::mpsc as channel;
use std::sync::{Arc, Mutex};
const IO_BUFFER_SIZE: usize = 8192;

pub struct ObjectPool<T> {
    sender: channel::Sender<T>,
    receiver: Arc<Mutex<channel::Receiver<T>>>,
    create_fn: Box<dyn Fn() -> T + Send + Sync>,
}

impl<T: Send + 'static> ObjectPool<T> {
    pub fn new<F>(initial_size: usize, create_fn: F) -> Self
    where
        F: Fn() -> T + Send + Sync + 'static,
    {
        let (sender, receiver) = channel::channel();

        for _ in 0..initial_size {
            sender.send(create_fn()).unwrap();
        }

        Self {
            sender,
            receiver: Arc::new(Mutex::new(receiver)),
            create_fn: Box::new(create_fn),
        }
    }

    pub fn acquire(&self) -> PooledObject<T> {
        let mut object = {
            let receiver = self.receiver.lock().unwrap();
            match receiver.try_recv() {
                Ok(obj) => obj,
                Err(channel::TryRecvError::Empty) => (self.create_fn)(),
                Err(channel::TryRecvError::Disconnected) => {
                    panic!("ObjectPool sender disconnected!");
                }
            }
        };

        if let Some(vec) = (&mut object as &mut dyn std::any::Any).downcast_mut::<Vec<u8>>() {
            vec.clear();
            vec.resize(IO_BUFFER_SIZE, 0);
        }

        PooledObject {
            object: Some(object),
            pool_sender: self.sender.clone(),
        }
    }
}

pub struct PooledObject<T> {
    object: Option<T>,
    pool_sender: channel::Sender<T>,
}

impl<T> Drop for PooledObject<T> {
    fn drop(&mut self) {
        if let Some(object) = self.object.take() {
            let _ = self.pool_sender.send(object);
        }
    }
}
