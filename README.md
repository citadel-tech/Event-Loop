# Mill-IO

A lightweight, production-ready event loop library for Rust that provides efficient non-blocking I/O management without relying on heavyweight async runtimes. Mill-IO is a reactor-based event loop implementation built on top of `mio` that offers:

- **Runtime-agnostic**: No dependency on Tokio or other async runtimes
- **Cross-platform**: Leverages mio's polling abstraction (epoll, kqueue, IOCP)
- **Thread pool integration**: Configurable worker threads for handling I/O events
- **Compute pool**: Dedicated priority-based thread pool for CPU-intensive tasks
- **High-level networking**: TCP server/client with connection management
- **Object pooling**: Reduces allocation overhead for frequent operations
- **Clean API**: Simple registration and handler interface

## Installation

Add Mill-IO to your `Cargo.toml`:

```toml
[dependencies]
mill-io = "1.0.2"
```

With networking support:

```toml
[dependencies]
mill-io = { version = "1.0.2", features = ["net"] }
```

For unstable features:

```toml
[dependencies]
mill-io = { version = "1.0.2", features = ["unstable"] }
```

## Quick Start

```rust
use mill_io::{EventLoop, EventHandler};
use mio::{net::TcpListener, Interest, Token, event::Event};

struct EchoHandler;

impl EventHandler for EchoHandler {
    fn handle_event(&self, event: &Event) {
        // Handle incoming connections
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::default();
    let mut listener = TcpListener::bind("127.0.0.1:8080".parse()?)?;
    
    event_loop.register(
        &mut listener,
        Token(1),
        Interest::READABLE,
        EchoHandler
    )?;
    
    println!("Server listening on 127.0.0.1:8080");
    event_loop.run()?;
    
    Ok(())
}
```

See [examples/scratch_echo_server.rs](examples/scratch_echo_server.rs) for a complete implementation.

## High-Level TCP Networking

Mill-IO provides a high-level TCP API that handles connection management automatically. Enable with the `net` feature.

### TCP Server

```rust
use mill_io::net::tcp::{TcpServer, TcpServerConfig, traits::*, ServerContext};
use mill_io::{EventLoop, error::Result};
use std::sync::Arc;

struct EchoHandler;

impl NetworkHandler for EchoHandler {
    fn on_connect(&self, _ctx: &ServerContext, conn_id: ConnectionId) -> Result<()> {
        println!("Client connected: {:?}", conn_id);
        Ok(())
    }

    fn on_data(&self, ctx: &ServerContext, conn_id: ConnectionId, data: &[u8]) -> Result<()> {
        // echo back the data
        ctx.send_to(conn_id, data)?;
        Ok(())
    }

    fn on_disconnect(&self, _ctx: &ServerContext, conn_id: ConnectionId) -> Result<()> {
        println!("Client disconnected: {:?}", conn_id);
        Ok(())
    }
}

fn main() -> Result<()> {
    let event_loop = Arc::new(EventLoop::default());
    
    let config = TcpServerConfig::builder()
        .address("127.0.0.1:8080".parse().unwrap())
        .buffer_size(8192)
        .max_connections(1000)
        .no_delay(true)
        .build();

    let server = Arc::new(TcpServer::new(config, EchoHandler)?);
    server.start(&event_loop, mio::Token(0))?;
    
    event_loop.run()?;
    Ok(())
}
```

### Server Context Operations

The `ServerContext` provides methods for interacting with connections:

```rust
// Send data to a specific connection
ctx.send_to(conn_id, b"Hello")?;

// Broadcast to all connections
ctx.broadcast(b"Message to all")?;

// Close a connection
ctx.close_connection(conn_id)?;
```

## Compute Thread Pool

Mill-IO includes a dedicated thread pool for CPU-intensive operations, keeping the I/O event loop responsive. Tasks support priority scheduling.

### Basic Usage

```rust
use mill_io::{EventLoop, TaskPriority};

let event_loop = EventLoop::default();

// Spawn with default (Normal) priority
event_loop.spawn_compute(|| {
    // CPU-intensive work here
    let result = expensive_calculation();
    println!("Result: {}", result);
});

// Spawn with specific priority
event_loop.spawn_compute_with_priority(|| {
    // Critical computation
}, TaskPriority::Critical);
```

### Task Priorities

Tasks are executed based on priority (highest first):

- `TaskPriority::Critical` - Urgent tasks, processed first
- `TaskPriority::High` - Important tasks
- `TaskPriority::Normal` - Default priority
- `TaskPriority::Low` - Background tasks

### Monitoring Metrics

```rust
let metrics = event_loop.get_compute_metrics();

println!("Tasks submitted: {}", metrics.tasks_submitted());
println!("Tasks completed: {}", metrics.tasks_completed());
println!("Tasks failed: {}", metrics.tasks_failed());
println!("Active workers: {}", metrics.active_workers());
println!("Queue depths - Low: {}, Normal: {}, High: {}, Critical: {}",
    metrics.queue_depth_low(),
    metrics.queue_depth_normal(),
    metrics.queue_depth_high(),
    metrics.queue_depth_critical()
);
println!("Total execution time: {}ms", metrics.total_execution_time_ns() / 1_000_000);
```

### Use Cases

- Cryptographic operations (hashing, encryption)
- Image/video processing
- Data compression
- Complex calculations
- File parsing

## Examples

Mill-IO includes several practical examples demonstrating different use cases (See [examples](./examples)).

## Configuration

Mill-IO provides flexible configuration options:

### Default Configuration

```rust
use mill_io::EventLoop;

// Uses CPU cores for workers, 1024 event capacity, 150ms timeout
let event_loop = EventLoop::default();
```

### Custom Configuration

```rust
use mill_io::EventLoop;

let event_loop = EventLoop::new(
    8,      // Number of worker threads
    2048,   // Maximum events per poll iteration
    50      // Poll timeout in milliseconds
)?;
```

### TCP Server Configuration

```rust
use mill_io::net::tcp::TcpServerConfig;

let config = TcpServerConfig::builder()
    .address("0.0.0.0:8080".parse().unwrap())
    .buffer_size(16384)          // Read buffer size
    .max_connections(10000)      // Connection limit
    .no_delay(true)              // Disable Nagle's algorithm
    .keep_alive(Some(Duration::from_secs(60)))
    .build();
```

### Thread Pool Sizing Guidelines

- **CPU-bound tasks**: Number of CPU cores
- **I/O-bound tasks**: 2-4x number of CPU cores
- **Mixed workloads**: Start with CPU cores + 2

## Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│                      User Application                       │
│  ┌──────────────┐          ┌─────────────────┐              │
│  │ TcpServer/   │─────────+│ NetworkHandler  │              │
│  │ TcpClient    │          │ (your handler)  │              │
│  └──────────────┘          └─────────────────┘              │
└────────────┬──────────────────────┬─────────────────────────┘
             │                      │ Callbacks
             │ Register             │ (on_connect, on_data, etc.)
             +                      │
┌─────────────────────────────────────────────────────────────┐
│                      Mill-IO EventLoop                      │
│  ┌──────────┐       ┌──────────┐       ┌──────────────┐     │
│  │ Reactor  │──────+│ I/O Pool │       │ Compute Pool │     │
│  │ (Poll)   │       │          │       │ (Priority)   │     │
│  └──────────┘       └──────────┘       └──────────────┘     │
└────────────┬────────────────────────────────────────────────┘
             │ OS Events
             +
┌─────────────────────────────────────────────────────────────┐
│              Operating System (epoll/kqueue/IOCP)           │
└─────────────────────────────────────────────────────────────┘
```

For detailed architectural documentation, see [Architecture Guide](./docs/Arch.md)

## Platform Support

Mill-IO supports all major platforms through mio:

- **Linux**: epoll-based polling
- **macOS**: kqueue-based polling  
- **Windows**: IOCP-based polling
- **FreeBSD/OpenBSD**: kqueue-based polling

Minimum supported Rust version: 1.70

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

## Contributing

Contributions are welcome! Please read our [Contributing Guide](CONTRIBUTING.md) for details on our development process, coding standards, and how to submit pull requests.

For questions or discussions, feel free to open an issue or reach out to the maintainers.
