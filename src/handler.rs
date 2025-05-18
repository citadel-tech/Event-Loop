use mio::{Interest, event::Event};

pub trait Eventhandler {
    fn handle_event(&self, event: &Event);
}

pub struct HandlerEntry {
    pub handler: Box<dyn Eventhandler + Send + Sync>,
    pub interest: Interest,
}

impl HandlerEntry {
    pub fn new<H>(handler: H, interest: Interest) -> Self
    where
        H: Eventhandler + Send + Sync + 'static,
    {
        HandlerEntry {
            handler: Box::new(handler),
            interest,
        }
    }
}
