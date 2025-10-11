use mio::{event::Event, Interest};

#[cfg(not(target_os = "linux"))]
use mio::Token;
pub trait EventHandler {
    #[cfg(not(target_os = "linux"))]
    fn handle_event(&self, event: &SafeEvent);

    #[cfg(target_os = "linux")]
    fn handle_event(&self, event: &Event);
}

#[cfg(not(target_os = "linux"))]
pub struct SafeEvent {
    token: Token,
    is_readable: bool,
    is_writable: bool,
}

#[cfg(not(target_os = "linux"))]
impl SafeEvent {
    pub fn token(&self) -> Token {
        self.token
    }

    pub fn is_readable(&self) -> bool {
        self.is_readable
    }

    pub fn is_writable(&self) -> bool {
        self.is_writable
    }
}

#[cfg(not(target_os = "linux"))]
impl From<&Event> for SafeEvent {
    fn from(value: &Event) -> Self {
        Self {
            token: value.token(),
            is_readable: value.is_readable(),
            is_writable: value.is_writable(),
        }
    }
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
