//! High-level networking abstractions for Mill-IO event loop
//!
//! This module provides TCP and UDP networking components that integrate
//! seamlessly with Mill-IO's event loop architecture.

pub(crate) mod traits;

use crate::error::Result;
use mio::event::Event;
use std::io;
use traits::{ConnectionId, NetworkHandler, Logger, LogLevel, NoOpLogger};

use crate::{EventHandler, EventLoop, ObjectPool, PooledObject};
use mio::net::{TcpListener, TcpStream};
use mio::{Interest, Token};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};

/// Configuration for TCP server
#[derive(Clone)]
pub struct TcpServerConfig {
    /// Address to bind to
    pub address: SocketAddr,
    /// Number of connection buffer size
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

/// Builder for TcpServerConfig
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

/// High-level TCP server
pub struct TcpServer<H: NetworkHandler> {
    listener: Arc<Mutex<TcpListener>>,
    connections: Arc<RwLock<HashMap<ConnectionId, TcpConnection>>>,
    handler: Arc<H>,
    config: TcpServerConfig,
    buffer_pool: ObjectPool<Vec<u8>>,
    next_conn_id: Arc<Mutex<u64>>,
    logger: Arc<dyn Logger>,
}

impl<H: NetworkHandler> TcpServer<H> {
    pub fn new(config: TcpServerConfig, handler: H) -> Result<Self> {
        let listener = TcpListener::bind(config.address)?;
        let logger = config.logger.clone();

        Ok(Self {
            listener: Arc::new(Mutex::new(listener)),
            connections: Arc::new(RwLock::new(HashMap::new())),
            handler: Arc::new(handler),
            buffer_pool: ObjectPool::new(20, move || vec![0; config.buffer_size]),
            next_conn_id: Arc::new(Mutex::new(1)),
            logger,
            config,
        })
    }

    /// Start the server by registering with the event loop
    pub fn start(&self, event_loop: &EventLoop, listener_token: Token) -> Result<()> {
        let listener_handler = TcpListenerHandler {
            listener: self.listener.clone(),
            connections: self.connections.clone(),
            handler: self.handler.clone(),
            config: self.config.clone(),
            buffer_pool: self.buffer_pool.clone(),
            next_conn_id: self.next_conn_id.clone(),
            event_loop_ref: event_loop as *const EventLoop,
            logger: self.logger.clone(),
        };

        event_loop.register(
            &mut *self.listener.lock().unwrap(),
            listener_token,
            Interest::READABLE,
            listener_handler,
        )?;

        Ok(())
    }

    /// Get active connection count
    pub fn connection_count(&self) -> usize {
        self.connections.read().unwrap().len()
    }

    /// Send data to a specific connection
    pub fn send_to(&self, conn_id: ConnectionId, data: &[u8]) -> Result<()> {
        let mut connections = self.connections.write().unwrap();
        if let Some(conn) = connections.get_mut(&conn_id) {
            conn.stream.write_all(data)?;
        }
        Ok(())
    }

    /// Close a specific connection
    pub fn close_connection(&self, conn_id: ConnectionId) -> Result<()> {
        let mut connections = self.connections.write().unwrap();
        connections.remove(&conn_id);
        Ok(())
    }

    /// Broadcast data to all connections
    pub fn broadcast(&self, data: &[u8]) -> Result<()> {
        let mut connections = self.connections.write().unwrap();
        for conn in connections.values_mut() {
            let _ = conn.stream.write_all(data);
        }
        Ok(())
    }
}

/// Internal TCP connection representation
struct TcpConnection {
    stream: TcpStream,
    token: Token,
    #[allow(dead_code)]
    peer_addr: SocketAddr,
}

/// Handler for accepting new connections
struct TcpListenerHandler<H: NetworkHandler> {
    listener: Arc<Mutex<TcpListener>>,
    connections: Arc<RwLock<HashMap<ConnectionId, TcpConnection>>>,
    handler: Arc<H>,
    config: TcpServerConfig,
    buffer_pool: ObjectPool<Vec<u8>>,
    next_conn_id: Arc<Mutex<u64>>,
    event_loop_ref: *const EventLoop,
    logger: Arc<dyn Logger>,
}

// Safety: We ensure event_loop_ref is valid for the lifetime of the handler
unsafe impl<H: NetworkHandler> Send for TcpListenerHandler<H> {}
unsafe impl<H: NetworkHandler> Sync for TcpListenerHandler<H> {}

impl<H: NetworkHandler> EventHandler for TcpListenerHandler<H> {
    fn handle_event(&self, event: &Event) {
        if !event.is_readable() {
            return;
        }

        loop {
            match self.listener.lock().unwrap().accept() {
                Ok((mut stream, peer_addr)) => {
                    if let Some(max) = self.config.max_connections {
                        if self.connections.read().unwrap().len() >= max {
                            self.logger.log(LogLevel::Warn, &format!("Max connections reached, rejecting {}", peer_addr));
                            continue;
                        }
                    }

                    if let Err(e) = stream.set_nodelay(self.config.no_delay) {
                        self.logger.log(LogLevel::Error, &format!("Failed to set TCP_NODELAY: {}", e));
                    }

                    let conn_id = ConnectionId({
                        let mut next = self.next_conn_id.lock().unwrap();
                        let id = *next;
                        *next += 1;
                        id
                    });

                    let token = Token(conn_id.as_u64() as usize);

                    let conn_handler = TcpConnectionHandler {
                        conn_id,
                        connections: self.connections.clone(),
                        handler: self.handler.clone(),
                        buffer_pool: self.buffer_pool.clone(),
                        event_loop_ref: self.event_loop_ref,
                        logger: self.logger.clone(),
                    };

                    let event_loop = unsafe { &*self.event_loop_ref };
                    if let Err(e) = event_loop.register(
                        &mut stream,
                        token,
                        Interest::READABLE | Interest::WRITABLE,
                        conn_handler,
                    ) {
                        self.logger.log(LogLevel::Error, &format!("Failed to register connection: {}", e));
                        continue;
                    }

                    let conn = TcpConnection {
                        stream,
                        token,
                        peer_addr,
                    };
                    self.connections.write().unwrap().insert(conn_id, conn);

                    if let Err(e) = self.handler.on_connect(conn_id) {
                        self.logger.log(LogLevel::Error, &format!("Handler on_connect error: {}", e));
                    }

                    self.logger.log(LogLevel::Info, &format!("New connection: {} (id: {:?})", peer_addr, conn_id));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(e) => {
                    self.logger.log(LogLevel::Error, &format!("Accept error: {}", e));
                    break;
                }
            }
        }
    }
}

/// Handler for individual TCP connections
struct TcpConnectionHandler<H: NetworkHandler> {
    conn_id: ConnectionId,
    connections: Arc<RwLock<HashMap<ConnectionId, TcpConnection>>>,
    handler: Arc<H>,
    buffer_pool: ObjectPool<Vec<u8>>,
    event_loop_ref: *const EventLoop,
    logger: Arc<dyn Logger>,
}

unsafe impl<H: NetworkHandler> Send for TcpConnectionHandler<H> {}
unsafe impl<H: NetworkHandler> Sync for TcpConnectionHandler<H> {}

impl<H: NetworkHandler> EventHandler for TcpConnectionHandler<H> {
    fn handle_event(&self, event: &Event) {
        let is_readable = event.is_readable();
        let is_writable = event.is_writable();

        if is_readable {
            self.handle_read();
        }

        if is_writable {
            if let Err(e) = self.handler.on_writable(self.conn_id) {
                self.logger.log(LogLevel::Error, &format!("Handler on_writable error: {}", e));
            }
        }
    }
}

impl<H: NetworkHandler> TcpConnectionHandler<H> {
    fn handle_read(&self) {
        let mut buffer: PooledObject<Vec<u8>> = self.buffer_pool.acquire();

        let read_result = {
            let mut connections = self.connections.write().unwrap();
            if let Some(conn) = connections.get_mut(&self.conn_id) {
                conn.stream.read(buffer.as_mut())
            } else {
                return;
            }
        };

        match read_result {
            Ok(0) => {
                // connection closed
                self.disconnect();
            }
            Ok(n) => {
                if let Err(e) = self.handler.on_data(self.conn_id, &buffer.as_ref()[..n]) {
                    self.logger.log(LogLevel::Error, &format!("Handler on_data error: {}", e));
                    self.disconnect();
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // expected for non-blocking I/O
            }
            Err(e) => {
                self.handler.on_error(self.conn_id, e);
                self.disconnect();
            }
        }
    }

    fn disconnect(&self) {
        let mut connections = self.connections.write().unwrap();
        if let Some(mut conn) = connections.remove(&self.conn_id) {
            let event_loop = unsafe { &*self.event_loop_ref };
            let _ = event_loop.deregister(&mut conn.stream, conn.token);

            if let Err(e) = self.handler.on_disconnect(self.conn_id) {
                self.logger.log(LogLevel::Error, &format!("Handler on_disconnect error: {}", e));
            }
        }
    }
}
