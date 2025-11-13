//! TCP server and client implementations with lockfree connection management.
//!
//! This module provides high-performance TCP networking built on Mill-IO's event loop.
//! 
//! Each TCP connection is assigned a unique ConnectionId generated atomically.
//! The connection state is stored in a lockfree map, allowing concurrent access
//! from multiple worker threads without blocking.
//!
//! ```text
//! Connection Storage:
//!   LockfreeMap<u64, TcpConnection>
//!        │
//!        ├──> ConnId(1) ──> TcpConnection { stream, token, addr }
//!        ├──> ConnId(2) ──> TcpConnection { stream, token, addr }
//!        └──> ConnId(N) ──> TcpConnection { stream, token, addr }
//! ```
//!
//! ## Event Handling Pipeline
//!
//! ```text
//! 1. Listener Events:
//!    New Connection ──> TcpListenerHandler::handle_event()
//!        - accept() ──> Create ConnectionId
//!        - Register TcpConnectionHandler with EventLoop
//!        - Insert into LockfreeMap
//!        - Call handler.on_connect()
//!
//! 2. Connection Events:
//!    Readable Event ──> TcpConnectionHandler::handle_event()
//!        - Read from stream into pooled buffer
//!        - Call handler.on_data()
//!        - If EOF: disconnect()
//!
//!    Writable Event ──> TcpConnectionHandler::handle_event()
//!        - Call handler.on_writable()
//!
//! 3. Disconnection:
//!    disconnect() ──> Remove from LockfreeMap
//!        - Deregister from EventLoop
//!        - Call handler.on_disconnect()
//! ```
//!
//! ## Configuration
//!
//! TcpServerConfig uses the builder pattern for ergonomic configuration:
//!
//! ```rust
//! use mill_io::net::tcp::config::TcpServerConfig;
//! # use std::sync::Arc;
//! # use mill_io::net::tcp::traits::NoOpLogger;
//!
//! let config = TcpServerConfig::builder()
//!     .address("0.0.0.0:8080".parse().unwrap())
//!     .buffer_size(16384)              // Larger buffers for high throughput
//!     .max_connections(1000)           // Limit concurrent connections
//!     .no_delay(true)                  // Disable Nagle's algorithm
//!     .logger(Arc::new(NoOpLogger))    // Custom logging implementation
//!     .build();
//! ```
//!
//! ## Handler Implementation
//!
//! Your handler must implement both NetworkHandler and Logger traits. The Logger
//! trait requirement allows handlers to emit their own logging without coupling
//! to a specific logging framework.
//!
//! ```rust
//! use mill_io::net::tcp::traits::{NetworkHandler, ConnectionId, Logger, LogLevel};
//! use mill_io::error::Result;
//!
//! struct MyHandler;
//!
//! impl Logger for MyHandler {
//!     fn log(&self, level: LogLevel, message: &str) {
//!         match level {
//!             LogLevel::Error => eprintln!("{}", message),
//!             LogLevel::Info => println!("{}", message),
//!             _ => {}
//!         }
//!     }
//! }
//!
//! impl NetworkHandler for MyHandler {
//!     fn on_data(&self, conn_id: ConnectionId, data: &[u8]) -> Result<()> {
//!         self.log(LogLevel::Info, &format!("Received {} bytes from {:?}", data.len(), conn_id));
//!         Ok(())
//!     }
//! }
//! ```

pub mod config;
pub mod traits;

use crate::error::Result;
use mio::event::Event;
use std::io;
use traits::{ConnectionId, LogLevel, Logger, NetworkHandler, NoOpLogger};

use crate::{EventHandler, EventLoop, ObjectPool, PooledObject};
use config::TcpServerConfig;
use lockfree::map::Map as LockfreeMap;
use mio::net::{TcpListener, TcpStream};
use mio::{Interest, Token};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

/// High-level TCP server
pub struct TcpServer<H: NetworkHandler> {
    listener: Arc<Mutex<TcpListener>>,
    connections: Arc<LockfreeMap<u64, TcpConnection>>,
    handler: Arc<H>,
    config: TcpServerConfig,
    buffer_pool: ObjectPool<Vec<u8>>,
    next_conn_id: Arc<AtomicU64>,
    logger: Arc<dyn Logger>,
}

impl<H: NetworkHandler> TcpServer<H> {
    pub fn new(config: TcpServerConfig, handler: H) -> Result<Self> {
        let listener = TcpListener::bind(config.address)?;
        let logger = config.logger.clone();

        Ok(Self {
            listener: Arc::new(Mutex::new(listener)),
            connections: Arc::new(LockfreeMap::new()),
            handler: Arc::new(handler),
            buffer_pool: ObjectPool::new(20, move || vec![0; config.buffer_size]),
            next_conn_id: Arc::new(AtomicU64::new(1)),
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
        self.connections.iter().count()
    }

    /// Send data to a specific connection
    pub fn send_to(&self, conn_id: ConnectionId, data: &[u8]) -> Result<()> {
        if let Some(mut conn) = self.connections.get(&conn_id.as_u64()) {
            conn.val().stream.lock().unwrap().write_all(data)?;
        }
        Ok(())
    }

    /// Close a specific connection
    pub fn close_connection(&self, conn_id: ConnectionId) -> Result<()> {
        self.connections.remove(&conn_id.as_u64());
        Ok(())
    }

    /// Broadcast data to all connections
    pub fn broadcast(&self, data: &[u8]) -> Result<()> {
        for conn in self.connections.iter() {
            let _ = conn.val().stream.lock().unwrap().write_all(data);
        }
        Ok(())
    }
}

/// Internal TCP connection representation
struct TcpConnection {
    stream: Arc<Mutex<TcpStream>>,
    token: Token,
    #[allow(dead_code)]
    peer_addr: SocketAddr,
}

/// Handler for accepting new connections
struct TcpListenerHandler<H: NetworkHandler> {
    listener: Arc<Mutex<TcpListener>>,
    connections: Arc<LockfreeMap<u64, TcpConnection>>,
    handler: Arc<H>,
    config: TcpServerConfig,
    buffer_pool: ObjectPool<Vec<u8>>,
    next_conn_id: Arc<AtomicU64>,
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
                Ok((stream, peer_addr)) => {
                    if let Some(max) = self.config.max_connections {
                        if self.connections.iter().count() >= max {
                            self.logger.log(
                                LogLevel::Warn,
                                &format!("Max connections reached, rejecting {}", peer_addr),
                            );
                            continue;
                        }
                    }

                    if let Err(e) = stream.set_nodelay(self.config.no_delay) {
                        self.logger.log(
                            LogLevel::Error,
                            &format!("Failed to set TCP_NODELAY: {}", e),
                        );
                    }

                    let conn_id = ConnectionId(self.next_conn_id.fetch_add(1, Ordering::SeqCst));
                    let token = Token(conn_id.as_u64() as usize);

                    let stream_arc = Arc::new(Mutex::new(stream));

                    let conn_handler = TcpConnectionHandler {
                        conn_id,
                        stream: stream_arc.clone(),
                        connections: self.connections.clone(),
                        handler: self.handler.clone(),
                        buffer_pool: self.buffer_pool.clone(),
                        event_loop_ref: self.event_loop_ref,
                        logger: self.logger.clone(),
                    };

                    let event_loop = unsafe { &*self.event_loop_ref };
                    if let Err(e) = event_loop.register(
                        &mut *stream_arc.lock().unwrap(),
                        token,
                        Interest::READABLE | Interest::WRITABLE,
                        conn_handler,
                    ) {
                        self.logger.log(
                            LogLevel::Error,
                            &format!("Failed to register connection: {}", e),
                        );
                        continue;
                    }

                    let conn = TcpConnection {
                        stream: stream_arc,
                        token,
                        peer_addr,
                    };
                    self.connections.insert(conn_id.as_u64(), conn);

                    if let Err(e) = self.handler.on_connect(conn_id) {
                        self.logger
                            .log(LogLevel::Error, &format!("Handler on_connect error: {}", e));
                    }

                    self.logger.log(
                        LogLevel::Info,
                        &format!("New connection: {} (id: {:?})", peer_addr, conn_id),
                    );
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(e) => {
                    self.logger
                        .log(LogLevel::Error, &format!("Accept error: {}", e));
                    break;
                }
            }
        }
    }
}

/// Handler for individual TCP connections
struct TcpConnectionHandler<H: NetworkHandler> {
    conn_id: ConnectionId,
    stream: Arc<Mutex<TcpStream>>,
    connections: Arc<LockfreeMap<u64, TcpConnection>>,
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
                self.logger.log(
                    LogLevel::Error,
                    &format!("Handler on_writable error: {}", e),
                );
            }
        }
    }
}

impl<H: NetworkHandler> TcpConnectionHandler<H> {
    fn handle_read(&self) {
        let mut buffer: PooledObject<Vec<u8>> = self.buffer_pool.acquire();

        let read_result = {
            let mut stream = self.stream.lock().unwrap();
            stream.read(buffer.as_mut())
        };

        match read_result {
            Ok(0) => {
                // connection closed
                self.disconnect();
            }
            Ok(n) => {
                if let Err(e) = self.handler.on_data(self.conn_id, &buffer.as_ref()[..n]) {
                    self.logger
                        .log(LogLevel::Error, &format!("Handler on_data error: {}", e));
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
        if let Some(conn) = self.connections.remove(&self.conn_id.as_u64()) {
            let event_loop = unsafe { &*self.event_loop_ref };
            let _ =
                event_loop.deregister(&mut *conn.val().stream.lock().unwrap(), conn.val().token);

            if let Err(e) = self.handler.on_disconnect(self.conn_id) {
                self.logger.log(
                    LogLevel::Error,
                    &format!("Handler on_disconnect error: {}", e),
                );
            }
        }
    }
}

/// High-level TCP client
pub struct TcpClient<H: NetworkHandler> {
    stream: Arc<Mutex<Option<TcpStream>>>,
    handler: Arc<H>,
    buffer_pool: ObjectPool<Vec<u8>>,
    conn_id: ConnectionId,
    logger: Arc<dyn Logger>,
}

impl<H: NetworkHandler> TcpClient<H> {
    pub fn connect(addr: SocketAddr, handler: H) -> Result<Self> {
        Self::connect_with_logger(addr, handler, Arc::new(NoOpLogger))
    }

    pub fn connect_with_logger(
        addr: SocketAddr,
        handler: H,
        logger: Arc<dyn Logger>,
    ) -> Result<Self> {
        let stream = TcpStream::connect(addr)?;

        Ok(Self {
            stream: Arc::new(Mutex::new(Some(stream))),
            handler: Arc::new(handler),
            buffer_pool: ObjectPool::new(5, || vec![0; 8192]),
            conn_id: ConnectionId::new(1),
            logger,
        })
    }

    pub fn start(&mut self, event_loop: &EventLoop, token: Token) -> Result<()> {
        let handler = TcpClientHandler {
            conn_id: self.conn_id,
            stream: self.stream.clone(),
            handler: self.handler.clone(),
            buffer_pool: self.buffer_pool.clone(),
            logger: self.logger.clone(),
        };

        event_loop.register(
            self.stream.lock().unwrap().as_mut().unwrap(),
            token,
            Interest::READABLE | Interest::WRITABLE,
            handler,
        )?;

        self.handler.on_connect(self.conn_id)?;

        Ok(())
    }

    pub fn send(&mut self, data: &[u8]) -> Result<()> {
        if let Some(stream) = self.stream.lock().unwrap().as_mut() {
            stream.write_all(data)?;
        }
        Ok(())
    }

    pub fn disconnect(&mut self) -> Result<()> {
        let mut stream_guard = self.stream.lock().unwrap();
        *stream_guard = None;
        self.handler.on_disconnect(self.conn_id)?;
        Ok(())
    }
}

struct TcpClientHandler<H: NetworkHandler> {
    conn_id: ConnectionId,
    stream: Arc<Mutex<Option<TcpStream>>>,
    handler: Arc<H>,
    buffer_pool: ObjectPool<Vec<u8>>,
    logger: Arc<dyn Logger>,
}

impl<H: NetworkHandler> EventHandler for TcpClientHandler<H> {
    fn handle_event(&self, event: &Event) {
        if event.is_readable() {
            self.handle_read();
        }
        if event.is_writable() {
            if let Err(e) = self.handler.on_writable(self.conn_id) {
                self.logger.log(
                    LogLevel::Error,
                    &format!("Handler on_writable error: {}", e),
                );
            }
        }
    }
}

impl<H: NetworkHandler> TcpClientHandler<H> {
    fn handle_read(&self) {
        let mut buffer: PooledObject<Vec<u8>> = self.buffer_pool.acquire();

        let read_result = {
            let mut stream_guard = self.stream.lock().unwrap();
            if let Some(stream) = stream_guard.as_mut() {
                stream.read(buffer.as_mut())
            } else {
                return;
            }
        };

        match read_result {
            Ok(0) => {
                // connection closed by server
                self.logger.log(LogLevel::Info, "Server closed connection");
                self.disconnect();
            }
            Ok(n) => {
                if let Err(e) = self.handler.on_data(self.conn_id, &buffer.as_ref()[..n]) {
                    self.logger
                        .log(LogLevel::Error, &format!("Handler on_data error: {}", e));
                    self.disconnect();
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // expected for non-blocking I/O
            }
            Err(e) => {
                self.logger
                    .log(LogLevel::Error, &format!("Read error: {}", e));
                self.handler.on_error(self.conn_id, e);
                self.disconnect();
            }
        }
    }

    fn disconnect(&self) {
        let mut stream_guard = self.stream.lock().unwrap();
        *stream_guard = None;

        if let Err(e) = self.handler.on_disconnect(self.conn_id) {
            self.logger.log(
                LogLevel::Error,
                &format!("Handler on_disconnect error: {}", e),
            );
        }
    }
}
