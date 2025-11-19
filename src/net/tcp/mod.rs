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
//!
//! let config = TcpServerConfig::builder()
//!     .address("0.0.0.0:8080".parse().unwrap())
//!     .buffer_size(16384)              // Larger buffers for high throughput
//!     .max_connections(1000)           // Limit concurrent connections
//!     .no_delay(true)                  // Disable Nagle's algorithm
//!     .build();
//! ```
//!
//! ## Handler Implementation
//!
//! Your handler must implement NetworkHandler trait.
//!
//! ```rust
//! use mill_io::net::tcp::{traits::{NetworkHandler, ConnectionId}, ServerContext};
//! use mill_io::error::Result;
//!
//! struct MyHandler;
//!
//! impl NetworkHandler for MyHandler {
//!     fn on_data(&self, ctx: &ServerContext, conn_id: ConnectionId, data: &[u8]) -> Result<()> {
//!         println!("Received {} bytes from {:?}", data.len(), conn_id);
//!         ctx.send_to(conn_id, b"some response")?;
//!         Ok(())
//!     }
//! }
//! ```

pub mod config;
pub mod traits;

pub use config::TcpServerConfig;
pub use traits::*;

use crate::error::Result;
use crate::net::errors::{NetworkError, NetworkEvent};
use crate::{EventHandler, EventLoop, ObjectPool, PooledObject};
use lockfree::map::Map as LockfreeMap;
use mio::event::Event;
use mio::net::{TcpListener, TcpStream};
use mio::{Interest, Token};
use std::io;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex, Weak,
};

/// Context for network handlers to interact with the server.
pub struct ServerContext {
    server: Weak<dyn ServerOperations>,
    event_loop: Weak<EventLoop>,
}

impl ServerContext {
    /// Send data to a specific connection.
    pub fn send_to(&self, conn_id: ConnectionId, data: &[u8]) -> Result<()> {
        if let Some(server) = self.server.upgrade() {
            server.send_to(conn_id, data)
        } else {
            Ok(())
        }
    }

    /// Broadcast data to all connections.
    pub fn broadcast(&self, data: &[u8]) -> Result<()> {
        if let Some(server) = self.server.upgrade() {
            server.broadcast(data)
        } else {
            Ok(())
        }
    }

    /// Close a specific connection.
    pub fn close_connection(&self, conn_id: ConnectionId) -> Result<()> {
        if let Some(server) = self.server.upgrade() {
            if let Some(event_loop) = self.event_loop.upgrade() {
                server.close_connection(&event_loop, conn_id)
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }
}

/// Trait defining server operations, allowing for a weak reference from the context.
trait ServerOperations: Send + Sync {
    fn send_to(&self, conn_id: ConnectionId, data: &[u8]) -> Result<()>;
    fn broadcast(&self, data: &[u8]) -> Result<()>;
    fn close_connection(&self, event_loop: &EventLoop, conn_id: ConnectionId) -> Result<()>;
}

/// High-level TCP server
pub struct TcpServer<H: NetworkHandler> {
    listener: Arc<Mutex<TcpListener>>,
    connections: Arc<LockfreeMap<u64, TcpConnection>>,
    handler: Arc<H>,
    config: TcpServerConfig,
    buffer_pool: ObjectPool<Vec<u8>>,
    next_conn_id: Arc<AtomicU64>,
    connection_counter: Arc<AtomicUsize>,
    context: Arc<ServerContext>,
}

impl<H: NetworkHandler> TcpServer<H> {
    pub fn new(config: TcpServerConfig, handler: H) -> Result<Self> {
        let listener = TcpListener::bind(config.address)?;

        Ok(Self {
            listener: Arc::new(Mutex::new(listener)),
            connections: Arc::new(LockfreeMap::new()),
            handler: Arc::new(handler),
            buffer_pool: ObjectPool::new(20, move || vec![0; config.buffer_size]),
            next_conn_id: Arc::new(AtomicU64::new(1)),
            connection_counter: Arc::new(AtomicUsize::new(0)),
            config,
            context: Arc::new(ServerContext {
                server: Weak::<TcpServer<H>>::new(),
                event_loop: Weak::new(),
            }),
        })
    }

    /// Start the server by registering with the event loop
    pub fn start(
        self: Arc<Self>,
        event_loop: &Arc<EventLoop>,
        listener_token: Token,
    ) -> Result<()> {
        // `self` is an Arc<TcpServer<H>>, so we can create a weak pointer to it.
        let server_weak = Arc::downgrade(&self) as Weak<dyn ServerOperations>;

        // Update the context with weak references.
        let context_mut = unsafe { &mut *(Arc::as_ptr(&self.context) as *mut ServerContext) };
        context_mut.server = server_weak;
        context_mut.event_loop = Arc::downgrade(event_loop);

        let listener_handler = TcpListenerHandler {
            listener: self.listener.clone(),
            connections: self.connections.clone(),
            handler: self.handler.clone(),
            config: self.config.clone(),
            buffer_pool: self.buffer_pool.clone(),
            next_conn_id: self.next_conn_id.clone(),
            event_loop: Arc::downgrade(event_loop),
            connection_counter: self.connection_counter.clone(),
            context: self.context.clone(),
        };

        let mut listener = self.listener.lock().map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Listener mutex poisoned: {}", e),
            )
        })?;

        event_loop.register(
            &mut *listener,
            listener_token,
            Interest::READABLE,
            listener_handler,
        )?;

        Ok(())
    }

    /// Get active connection count
    pub fn connection_count(&self) -> usize {
        self.connection_counter.load(Ordering::SeqCst)
    }
}

impl<H: NetworkHandler> ServerOperations for TcpServer<H> {
    /// Send data to a specific connection
    fn send_to(&self, conn_id: ConnectionId, data: &[u8]) -> Result<()> {
        if let Some(conn) = self.connections.get(&conn_id.as_u64()) {
            let mut stream = conn.val().stream.lock().map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Stream mutex poisoned: {}", e),
                )
            })?;
            stream.write_all(data)?;
        }
        Ok(())
    }

    /// Close a specific connection
    fn close_connection(&self, event_loop: &EventLoop, conn_id: ConnectionId) -> Result<()> {
        if let Some(conn) = self.connections.remove(&conn_id.as_u64()) {
            let mut stream = match conn.val().stream.lock() {
                Ok(s) => s,
                Err(e) => {
                    self.handler.on_error(
                        &self.context,
                        Some(conn_id),
                        NetworkError::PoisonedLock(format!("Stream mutex on close: {}", e)),
                    );
                    e.into_inner()
                }
            };

            let _ = event_loop.deregister(&mut *stream, conn.val().token);
            let _ = stream.shutdown(std::net::Shutdown::Both);

            if let Err(e) = self.handler.on_disconnect(&self.context, conn_id) {
                self.handler.on_error(
                    &self.context,
                    Some(conn_id),
                    NetworkError::HandlerError(format!("on_disconnect: {}", e)),
                );
            }

            let _ = self
                .handler
                .on_event(&self.context, NetworkEvent::ConnectionClosed(conn_id));
        }
        Ok(())
    }

    /// Broadcast data to all connections
    fn broadcast(&self, data: &[u8]) -> Result<()> {
        for conn in self.connections.iter() {
            if let Ok(mut stream) = conn.val().stream.lock() {
                let _ = stream.write_all(data);
            }
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
    event_loop: Weak<EventLoop>,
    connection_counter: Arc<AtomicUsize>,
    context: Arc<ServerContext>,
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
            let listener = match self.listener.lock() {
                Ok(l) => l,
                Err(e) => {
                    self.handler.on_error(
                        &self.context,
                        None,
                        NetworkError::PoisonedLock(format!("Listener mutex: {}", e)),
                    );
                    return;
                }
            };

            match listener.accept() {
                Ok((stream, peer_addr)) => {
                    // Atomically check and increment the connection count.
                    if let Some(max) = self.config.max_connections {
                        if self.connection_counter.fetch_add(1, Ordering::SeqCst) >= max {
                            self.connection_counter.fetch_sub(1, Ordering::SeqCst); // Revert if limit exceeded
                            self.handler.on_error(
                                &self.context,
                                None,
                                NetworkError::MaxConnectionsReached(peer_addr),
                            );
                            continue;
                        }
                    } else {
                        self.connection_counter.fetch_add(1, Ordering::SeqCst);
                    }

                    if let Err(e) = stream.set_nodelay(self.config.no_delay) {
                        self.handler.on_error(
                            &self.context,
                            None,
                            NetworkError::Configuration(format!(
                                "Failed to set TCP_NODELAY: {}",
                                e
                            )),
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
                        event_loop: self.event_loop.clone(),
                        connection_counter: self.connection_counter.clone(),
                        context: self.context.clone(),
                    };

                    let event_loop = if let Some(arc) = self.event_loop.upgrade() {
                        arc
                    } else {
                        self.handler
                            .on_error(&self.context, None, NetworkError::EventLoopGone);
                        self.connection_counter.fetch_sub(1, Ordering::SeqCst);
                        // The stream is dropped here, closing the connection.
                        continue;
                    };

                    // let stream_clone = Arc::clone(&stream_arc);
                    let mut stream_guard = match stream_arc.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            self.handler.on_error(
                                &self.context,
                                Some(conn_id),
                                NetworkError::PoisonedLock(format!(
                                    "Stream mutex registration: {}",
                                    e
                                )),
                            );
                            continue;
                        }
                    };

                    // If registration fails, the stream is dropped and the connection is closed.
                    if let Err(e) = event_loop.register(
                        &mut *stream_guard,
                        token,
                        Interest::READABLE | Interest::WRITABLE,
                        conn_handler,
                    ) {
                        self.handler
                            .on_error(&self.context, Some(conn_id), NetworkError::Io(e));
                        self.connection_counter.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }

                    let conn = TcpConnection {
                        stream: stream_arc.clone(),
                        token,
                        peer_addr,
                    };
                    self.connections.insert(conn_id.as_u64(), conn);

                    let _ = self.handler.on_event(
                        &self.context,
                        NetworkEvent::ConnectionEstablished(conn_id, peer_addr),
                    );

                    // If the user's on_connect handler fails, we must clean up the connection.
                    if let Err(e) = self.handler.on_connect(&self.context, conn_id) {
                        self.handler.on_error(
                            &self.context,
                            Some(conn_id),
                            NetworkError::HandlerError(format!("on_connect: {}", e)),
                        );

                        // Remove the connection from the map and deregister from the event loop.
                        if let Some(conn) = self.connections.remove(&conn_id.as_u64()) {
                            if let Ok(mut stream) = conn.val().stream.lock() {
                                let _ = event_loop.deregister(&mut *stream, conn.val().token);
                            }
                        }
                        self.connection_counter.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(e) => {
                    self.handler
                        .on_error(&self.context, None, NetworkError::Accept(Box::new(e)));
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
    event_loop: Weak<EventLoop>,
    connection_counter: Arc<AtomicUsize>,
    context: Arc<ServerContext>,
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
            if let Err(e) = self.handler.on_writable(&self.context, self.conn_id) {
                self.handler.on_error(
                    &self.context,
                    Some(self.conn_id),
                    NetworkError::HandlerError(format!("on_writable: {}", e)),
                );
            }
        }
    }
}

impl<H: NetworkHandler> TcpConnectionHandler<H> {
    fn handle_read(&self) {
        let mut buffer: PooledObject<Vec<u8>> = self.buffer_pool.acquire();

        let read_result = {
            let mut stream = match self.stream.lock() {
                Ok(s) => s,
                Err(e) => {
                    self.handler.on_error(
                        &self.context,
                        Some(self.conn_id),
                        NetworkError::PoisonedLock(format!("Stream mutex read: {}", e)),
                    );
                    self.disconnect();
                    return;
                }
            };
            stream.read(buffer.as_mut())
        };

        match read_result {
            Ok(0) => {
                // connection closed
                self.disconnect();
            }
            Ok(n) => {
                if let Err(e) =
                    self.handler
                        .on_data(&self.context, self.conn_id, &buffer.as_ref()[..n])
                {
                    self.handler.on_error(
                        &self.context,
                        Some(self.conn_id),
                        NetworkError::HandlerError(format!("on_data: {}", e)),
                    );
                    self.disconnect();
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // expected for non-blocking I/O
            }
            Err(e) => {
                self.handler.on_error(
                    &self.context,
                    Some(self.conn_id),
                    NetworkError::Io(Box::new(e)),
                );
                self.disconnect();
            }
        }
    }

    fn disconnect(&self) {
        if let Some(conn) = self.connections.remove(&self.conn_id.as_u64()) {
            self.connection_counter.fetch_sub(1, Ordering::SeqCst);
            if let Some(event_loop) = self.event_loop.upgrade() {
                if let Ok(mut stream) = conn.val().stream.lock() {
                    let _ = event_loop.deregister(&mut *stream, conn.val().token);
                }
            }

            if let Err(e) = self.handler.on_disconnect(&self.context, self.conn_id) {
                self.handler.on_error(
                    &self.context,
                    Some(self.conn_id),
                    NetworkError::HandlerError(format!("on_disconnect: {}", e)),
                );
            }

            let _ = self
                .handler
                .on_event(&self.context, NetworkEvent::ConnectionClosed(self.conn_id));
        }
    }
}

/// High-level TCP client
pub struct TcpClient<H: NetworkHandler> {
    stream: Arc<Mutex<Option<TcpStream>>>,
    handler: Arc<H>,
    buffer_pool: ObjectPool<Vec<u8>>,
    conn_id: ConnectionId,
    context: Arc<ServerContext>,
}

impl<H: NetworkHandler> TcpClient<H> {
    pub fn connect(addr: SocketAddr, handler: H) -> Result<Self> {
        let stream = TcpStream::connect(addr)?;

        Ok(Self {
            stream: Arc::new(Mutex::new(Some(stream))),
            handler: Arc::new(handler),
            buffer_pool: ObjectPool::new(5, || vec![0; 8192]),
            conn_id: ConnectionId::new(1),
            context: Arc::new(ServerContext {
                server: Weak::<TcpClient<H>>::new(),
                event_loop: Weak::new(),
            }),
        })
    }

    pub fn start(&mut self, event_loop: &Arc<EventLoop>, token: Token) -> Result<()> {
        let context_mut = unsafe { &mut *(Arc::as_ptr(&self.context) as *mut ServerContext) };
        context_mut.event_loop = Arc::downgrade(event_loop);

        let handler = TcpClientHandler {
            conn_id: self.conn_id,
            stream: self.stream.clone(),
            handler: self.handler.clone(),
            buffer_pool: self.buffer_pool.clone(),
            event_loop: Arc::downgrade(event_loop),
            context: self.context.clone(),
        };

        let mut stream_guard = self.stream.lock().map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Client stream mutex poisoned: {}", e),
            )
        })?;

        let stream = stream_guard
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "TCP stream is None"))?;

        event_loop.register(
            stream,
            token,
            Interest::READABLE | Interest::WRITABLE,
            handler,
        )?;

        self.handler.on_connect(&self.context, self.conn_id)?;

        Ok(())
    }

    pub fn send(&self, data: &[u8]) -> Result<()> {
        if let Ok(mut stream_guard) = self.stream.lock() {
            if let Some(stream) = stream_guard.as_mut() {
                stream.write_all(data)?;
            }
        }
        Ok(())
    }

    pub fn disconnect(&self) -> Result<()> {
        if let Ok(mut stream_guard) = self.stream.lock() {
            *stream_guard = None;
        }
        self.handler.on_disconnect(&self.context, self.conn_id)?;
        Ok(())
    }
}

impl<H: NetworkHandler> ServerOperations for TcpClient<H> {
    fn send_to(&self, conn_id: ConnectionId, data: &[u8]) -> Result<()> {
        if conn_id == self.conn_id {
            self.send(data)
        } else {
            Ok(())
        }
    }

    fn broadcast(&self, _data: &[u8]) -> Result<()> {
        // No-op for a client
        Ok(())
    }

    fn close_connection(&self, _event_loop: &EventLoop, conn_id: ConnectionId) -> Result<()> {
        if conn_id == self.conn_id {
            self.disconnect()
        } else {
            Ok(())
        }
    }
}

struct TcpClientHandler<H: NetworkHandler> {
    conn_id: ConnectionId,
    stream: Arc<Mutex<Option<TcpStream>>>,
    handler: Arc<H>,
    buffer_pool: ObjectPool<Vec<u8>>,
    event_loop: Weak<EventLoop>,
    context: Arc<ServerContext>,
}

impl<H: NetworkHandler> EventHandler for TcpClientHandler<H> {
    fn handle_event(&self, event: &Event) {
        if event.is_readable() {
            self.handle_read();
        }
        if event.is_writable() {
            if let Err(e) = self.handler.on_writable(&self.context, self.conn_id) {
                self.handler.on_error(
                    &self.context,
                    Some(self.conn_id),
                    NetworkError::HandlerError(format!("on_writable: {}", e)),
                );
            }
        }
    }
}

impl<H: NetworkHandler> TcpClientHandler<H> {
    fn handle_read(&self) {
        let mut buffer: PooledObject<Vec<u8>> = self.buffer_pool.acquire();

        let read_result = {
            let mut stream_guard = match self.stream.lock() {
                Ok(s) => s,
                Err(e) => {
                    self.handler.on_error(
                        &self.context,
                        Some(self.conn_id),
                        NetworkError::PoisonedLock(format!("Client stream mutex read: {}", e)),
                    );
                    self.disconnect();
                    return;
                }
            };
            if let Some(stream) = stream_guard.as_mut() {
                stream.read(buffer.as_mut())
            } else {
                return;
            }
        };

        match read_result {
            Ok(0) => {
                // connection closed by server
                let _ = self
                    .handler
                    .on_event(&self.context, NetworkEvent::ConnectionClosed(self.conn_id));
                self.disconnect();
            }
            Ok(n) => {
                if let Err(e) =
                    self.handler
                        .on_data(&self.context, self.conn_id, &buffer.as_ref()[..n])
                {
                    self.handler.on_error(
                        &self.context,
                        Some(self.conn_id),
                        NetworkError::HandlerError(format!("on_data: {}", e)),
                    );
                    self.disconnect();
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // expected for non-blocking I/O
            }
            Err(e) => {
                self.handler.on_error(
                    &self.context,
                    Some(self.conn_id),
                    NetworkError::Io(Box::new(e)),
                );
                self.disconnect();
            }
        }
    }

    fn disconnect(&self) {
        let mut stream_guard = match self.stream.lock() {
            Ok(s) => s,
            Err(e) => {
                self.handler.on_error(
                    &self.context,
                    Some(self.conn_id),
                    NetworkError::PoisonedLock(format!("Client stream mutex disconnect: {}", e)),
                );
                e.into_inner()
            }
        };

        if let Some(stream) = stream_guard.as_mut() {
            if let Some(event_loop) = self.event_loop.upgrade() {
                let _ = event_loop.deregister(stream, Token(self.conn_id.as_u64() as usize));
            }
        }

        *stream_guard = None;

        if let Err(e) = self.handler.on_disconnect(&self.context, self.conn_id) {
            self.handler.on_error(
                &self.context,
                Some(self.conn_id),
                NetworkError::HandlerError(format!("on_disconnect: {}", e)),
            );
        }
    }
}
