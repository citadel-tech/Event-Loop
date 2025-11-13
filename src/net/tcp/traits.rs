use crate::error::Result;
use std::io::Error;
/// Unique identifier for connections
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionId(pub u64);

impl ConnectionId {
    pub fn new(id: u64) -> Self {
        ConnectionId(id)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// Trait for handling network events with typed data
pub trait NetworkHandler: Send + Sync + Logger + 'static {
    /// Called when connection is established (TCP only)
    fn on_connect(&self, conn_id: ConnectionId) -> Result<()> {
        let _ = conn_id;
        Ok(())
    }

    /// Called when data is received
    fn on_data(&self, conn_id: ConnectionId, data: &[u8]) -> Result<()>;

    /// Called when connection is closed (TCP only)
    fn on_disconnect(&self, conn_id: ConnectionId) -> Result<()> {
        let _ = conn_id;
        Ok(())
    }

    /// Called on write readiness (for backpressure handling)
    fn on_writable(&self, conn_id: ConnectionId) -> Result<()> {
        let _ = conn_id;
        Ok(())
    }

    /// Called on errors
    fn on_error(&self, conn_id: ConnectionId, error: Error) {
        self.log(
            LogLevel::Error,
            &format!("Connection {:?} error: {}", conn_id, error),
        );
    }
}

/// Log levels for network events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// Logger trait for network events
///
/// Library users can implement this trait to handle logging however they prefer.
pub trait Logger: Send + Sync {
    fn log(&self, level: LogLevel, message: &str);
}

/// Default no-op logger that discards all messages
#[derive(Default, Clone)]
pub struct NoOpLogger;

impl Logger for NoOpLogger {
    fn log(&self, _level: LogLevel, _message: &str) {
        // Do nothing
    }
}
