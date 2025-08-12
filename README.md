


A lightweight, production-ready event loop library for Rust that provides efficient non-blocking I/O management without relying on heavyweight async runtimes. mill-io is a reactor-based event loop implementation built on top of `mio` that offers:

- **Runtime-agnostic**: No dependency on Tokio or other async runtimes
- **Cross-platform**: Leverages mio's polling abstraction (epoll, kqueue, IOCP)
- **Thread pool integration**: Configurable worker threads for handling I/O events
- **Object pooling**: Reduces allocation overhead for frequent operations
- **Clean API**: Simple registration and handler interface

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
    fn handle_event(&self, event: &mio::event::Event) {
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

Mill-IO follows a modular, reactor-based architecture for efficient I/O event handling. For detailed architectural documentation, see [Architecture Guide](./docs/Arch.md).

## Acknowledgments

This project was developed as part of the **Summer of Bitcoin 2025** program. Special thanks to:

- **Citadel-tech** and the **Coinswap** project for providing the use case and requirements
- **Summer of Bitcoin** organizers and mentors for their guidance
- The **mio** project for providing the foundational polling abstractions
- The Rust community for excellent async I/O resources and documentation

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

## Contributing

Contributions are welcome! Please read our [Contributing Guide](CONTRIBUTING.md) for details on our development process, coding standards, and how to submit pull requests.

For questions or discussions, feel free to open an issue or reach out to the maintainers.