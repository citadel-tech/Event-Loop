use mio::{event::Event as MioEvent, Token};
use std::fmt;

/// Unified event struct that abstracts away platform differences by wrapping mio::event::Event as MioEvent
pub struct Event {
    token: Token,
    is_readable: bool,
    is_writable: bool,
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Event")
            .field("token", &self.token)
            .field("is_readable", &self.is_readable)
            .field("is_writable", &self.is_writable)
            .finish()
    }
}

impl Event {
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

impl From<&MioEvent> for Event {
    fn from(event: &MioEvent) -> Self {
        Self {
            token: event.token(),
            is_readable: event.is_readable(),
            is_writable: event.is_writable(),
        }
    }
}
