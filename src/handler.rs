use crate::event::Event;
use mio::Interest;

pub trait EventHandler {
    fn handle_event(&self, event: &Event);
}
pub struct HandlerEntry {
    pub handler: Box<dyn EventHandler + Send + Sync>,
    pub interest: Interest,
}

impl HandlerEntry {
    pub fn new<H>(handler: H, interest: Interest) -> Self
    where
        H: EventHandler + Send + Sync + 'static,
    {
        HandlerEntry {
            handler: Box::new(handler),
            interest,
        }
    }
}
