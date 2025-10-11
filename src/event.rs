use mio::{event::Event, Token};
use std::fmt;

/// Unified event struct that abstracts away platform differences by wrapping mio::event::Event
pub struct UnifiedEvent {
    token: Token,
    is_readable: bool,
    is_writable: bool,
}

impl fmt::Debug for UnifiedEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnifiedEvent")
            .field("token", &self.token)
            .field("is_readable", &self.is_readable)
            .field("is_writable", &self.is_writable)
            .finish()
    }
}

impl UnifiedEvent {
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

impl From<&Event> for UnifiedEvent {
    fn from(event: &Event) -> Self {
        Self {
            token: event.token(),
            is_readable: event.is_readable(),
            is_writable: event.is_writable(),
        }
    }
}
