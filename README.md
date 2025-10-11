# Mill-IO

A lightweight, production-ready event loop library for Rust that provides efficient non-blocking I/O management without relying on heavyweight async runtimes. Mill-IO is a reactor-based event loop implementation built on top of `mio` that offers:

- **Runtime-agnostic**: No dependency on Tokio or other async runtimes
- **Cross-platform**: Leverages mio's polling abstraction (epoll, kqueue, IOCP)
- **Thread pool integration**: Configurable worker threads for handling I/O events
- **Object pooling**: Reduces allocation overhead for frequent operations
- **Clean API**: Simple registration and handler interface

## Installation

Add Mill-IO to your `Cargo.toml`:

```toml
[dependencies]
mill-io = "1.0.1"
```

For unstable features:

```toml
[dependencies]
mill-io = { version = "1.0.1", features = ["unstable"] }
```

## Core Components

- **Polling Abstraction**: Cross-platform event notification using mio
- **Reactor Core**: Manages event loop lifecycle and dispatches events
- **Thread Pool**: Scalable task execution with work distribution
- **Object Pool**: Memory-efficient buffer management
- **Handler Registry**: Thread-safe event handler management

## Quick Start

```rust
use mill_io::{EventLoop, EventHandler};
use mio::{net::TcpListener, Interest, Token};

struct EchoHandler;

impl EventHandler for EchoHandler {
    fn handle_event(&self, event: &UnifiedEvent) {
        // Handle incoming connections
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut event_loop = EventLoop::default();
    let mut listener = TcpListener::bind("127.0.0.1:8080")?;
    
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

See [examples/echo_server.rs](examples/echo_server.rs) for a complete implementation.

## Examples

Mill-IO includes several practical examples demonstrating different use cases:

- **Echo Server** (`examples/echo_server.rs`): Basic TCP echo server implementation
- **HTTP Server** (`examples/http_server.rs`): Simple HTTP server handling GET requests
- **File Watcher** (`examples/file_watcher.rs`): File system monitoring with inotify
- **JSON-RPC Server** (`examples/jsonrpc-server.rs`): JSON-RPC 2.0 server implementation

Run an example:

```bash
cargo run --example echo_server
```

## Configuration

Mill-IO provides flexible configuration options:

### Default Configuration

```rust
use mill_io::EventLoop;

// Uses 4 worker threads, 1024 event capacity, 100ms timeout
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

### Thread Pool Sizing Guidelines

- **CPU-bound tasks**: Number of CPU cores
- **I/O-bound tasks**: 2-4x number of CPU cores
- **Mixed workloads**: Start with CPU cores + 2

### Memory Usage

Object pooling reduces memory allocations. Pool sizes are automatically managed but can be tuned based on workload patterns.

## Why It Exists

Mill-IO was created to address the need for efficient I/O management in applications that want to avoid the complexity and overhead of full async runtimes. Specifically designed for projects like [Coinswap](https://github.com/citadel-tech/coinswap) that require:

- Fine-grained control over concurrency
- Minimal runtime dependencies  
- Predictable performance characteristics
- Runtime-agnostic architecture

Rather than forcing applications into a specific async ecosystem, Mill-IO provides the building blocks for custom I/O handling while maintaining simplicity and performance.

## Features

- [x] Cross-platform I/O polling
- [x] Configurable thread pool
- [x] Object pooling for buffers
- [x] Thread-safe handler registry
- [x] Graceful shutdown handling
- [ ] Timer wheel implementation (planned)
- [ ] Rate limiting support (planned)
- [ ] Connection pooling (planned)

## Architecture

Mill-IO follows a modular, reactor-based architecture for efficient I/O event handling:

```
EventLoop
    |
    +-- Reactor
        |
        +-- PollHandle (Cross-platform polling)
        |   |
        |   +-- Handler Registry (Token -> Handler mapping)
        |   |
        |   +-- mio::Poll (System polling interface)
        |
        +-- ThreadPool (Worker threads)
            |
            +-- Worker threads (Configurable count)
            |
            +-- Job queue (Event dispatching)
```

### Core Components

1. **Reactor Core**
   - Central event loop coordinator
   - Event polling and dispatch management
   - Configurable timeouts and capacity
   - Graceful shutdown handling

2. **Handler Registry**
   - Lock-free token-to-handler mapping
   - O(1) handler lookups
   - Thread-safe registration/deregistration

3. **Thread Pool**
   - Configurable worker thread count
   - Work stealing via shared channel
   - Automatic task distribution
   - Support for both MPSC/MPMC channels

4. **Object Pool**
   - Efficient buffer recycling
   - Reduced allocation overhead
   - Automatic sizing and cleanup
   - Thread-safe object lifecycle

### Key Features

- **Zero-copy I/O**: Efficient buffer management via object pooling
- **Cross-platform**: Unified API across epoll/kqueue/IOCP
- **Lock-free**: Minimal contention in hot paths
- **Predictable**: Direct control over I/O operations

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
