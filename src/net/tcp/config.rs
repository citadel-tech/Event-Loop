use std::{net::SocketAddr, sync::Arc};

use crate::net::tcp::traits::{Logger, NoOpLogger};

/// Configuration for TCP server.
///
/// Controls server behavior including bind address, buffer sizes, connection limits,
/// and socket options. Use TcpServerConfig::builder() for ergonomic construction.
///
/// ## Socket Options
///
/// - no_delay: When enabled (default), disables Nagle's algorithm for lower latency
/// - keep_alive: Configures SO_KEEPALIVE to detect dead connections
///
/// ## Resource Limits
///
/// - buffer_size: Size of read buffers allocated from the pool
/// - max_connections: Hard limit on concurrent connections (None for unlimited)
#[derive(Clone)]
pub struct TcpServerConfig {
    /// Address to bind to
    pub address: SocketAddr,
    /// Size of connection buffer
    pub buffer_size: usize,
    /// Maximum number of connections
    pub max_connections: Option<usize>,
    /// Enable TCP_NODELAY
    pub no_delay: bool,
    /// SO_KEEPALIVE setting
    pub keep_alive: Option<std::time::Duration>,
    /// Logger for network events
    pub logger: Arc<dyn Logger>,
}

impl TcpServerConfig {
    /// Create a new builder for TcpServerConfig
    pub fn builder() -> TcpServerConfigBuilder {
        TcpServerConfigBuilder::new()
    }
}

impl Default for TcpServerConfig {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:8080".parse().unwrap(),
            buffer_size: 8192,
            max_connections: None,
            no_delay: true,
            keep_alive: Some(std::time::Duration::from_secs(60)),
            logger: Arc::new(NoOpLogger),
        }
    }
}

/// Builder for TcpServerConfig using the builder pattern.
///
/// All fields are optional and will use defaults from TcpServerConfig::default()
/// if not explicitly set.
pub struct TcpServerConfigBuilder {
    address: Option<SocketAddr>,
    buffer_size: Option<usize>,
    max_connections: Option<usize>,
    no_delay: Option<bool>,
    keep_alive: Option<Option<std::time::Duration>>,
    logger: Option<Arc<dyn Logger>>,
}

impl TcpServerConfigBuilder {
    /// Create a new builder with default values
    pub fn new() -> Self {
        Self {
            address: None,
            buffer_size: None,
            max_connections: None,
            no_delay: None,
            keep_alive: None,
            logger: None,
        }
    }

    /// Set the address to bind to
    pub fn address(mut self, address: SocketAddr) -> Self {
        self.address = Some(address);
        self
    }

    /// Set the buffer size for connections
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = Some(size);
        self
    }

    /// Set the maximum number of connections
    pub fn max_connections(mut self, max: usize) -> Self {
        self.max_connections = Some(max);
        self
    }

    /// Enable or disable TCP_NODELAY
    pub fn no_delay(mut self, enabled: bool) -> Self {
        self.no_delay = Some(enabled);
        self
    }

    /// Set SO_KEEPALIVE duration
    pub fn keep_alive(mut self, duration: Option<std::time::Duration>) -> Self {
        self.keep_alive = Some(duration);
        self
    }

    /// Set the logger implementation
    pub fn logger(mut self, logger: Arc<dyn Logger>) -> Self {
        self.logger = Some(logger);
        self
    }

    /// Build the TcpServerConfig
    pub fn build(self) -> TcpServerConfig {
        let default = TcpServerConfig::default();
        TcpServerConfig {
            address: self.address.unwrap_or(default.address),
            buffer_size: self.buffer_size.unwrap_or(default.buffer_size),
            max_connections: self.max_connections.or(default.max_connections),
            no_delay: self.no_delay.unwrap_or(default.no_delay),
            keep_alive: self.keep_alive.unwrap_or(default.keep_alive),
            logger: self.logger.unwrap_or(default.logger),
        }
    }
}

impl Default for TcpServerConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}
