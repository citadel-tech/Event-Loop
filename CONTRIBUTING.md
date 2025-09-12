# Contributing to Mill-IO

Thank you for your interest in contributing to Mill-IO! This document provides guidelines and information for contributors.

## Development Setup

### Prerequisites

- Rust 1.75+ (stable toolchain recommended)
- Git
- Platform-specific development tools for testing (varies by OS)

### Getting Started

1. Fork the repository on GitHub
2. Clone your fork locally:
   ```bash
   git clone https://github.com/your-username/Event-Loop.git
   cd Event-Loop
   ```
3. Create a new branch for your feature:
   ```bash
   git checkout -b feature/your-feature-name
   ```
4. Build and test:
   ```bash
   cargo build
   cargo test
   ```

## Code Guidelines

### Style and Formatting

- Use `rustfmt` for code formatting: `cargo fmt`
- Use `clippy` for linting: `cargo clippy`
- Follow Rust naming conventions (snake_case for functions/variables, PascalCase for types)
- Prefer explicit error handling over panics

### Documentation

- Add documentation comments (`///`) for all public APIs
- Include examples in documentation where helpful
- Update README.md if adding new features or changing public APIs

### Testing

- Write unit tests for new functionality
- Add integration tests for complex features
- Ensure tests pass on your target platform
- Aim for meaningful test coverage

Run tests with:
```bash
cargo test
cargo test --features unstable  # For unstable features
```

## Contribution Process

### Issues

- Check existing issues before creating new ones
- Use clear, descriptive titles
- Include reproduction steps for bugs
- Label appropriately (bug, enhancement, documentation, etc.)

### Pull Requests

1. **Before starting work on large changes**, open an issue to discuss the approach
2. **Keep changes focused** - one feature/fix per PR
3. **Write clear commit messages** following conventional commits format:
   - `feat: add timer wheel implementation`
   - `fix: resolve race condition in thread pool`
   - `docs: update API documentation for EventHandler`
4. **Update tests** and documentation as needed
5. **Ensure CI passes** before requesting review

### PR Requirements

- Code builds without warnings
- All tests pass
- New functionality includes tests
- Documentation updated if needed
- `cargo fmt` and `cargo clippy` pass
- No breaking changes to public API (unless discussed)

## Architecture Notes

### Core Components

- **PollHandle**: Wraps mio polling with handler registry
- **Reactor**: Main event loop coordinator
- **ThreadPool**: Task execution management
- **ObjectPool**: Memory management for frequent allocations

### Design Principles

- **Runtime-agnostic**: Avoid dependencies on specific async runtimes
- **Performance-first**: Optimize for low latency and high throughput
- **Safety**: Use Rust's type system to prevent common concurrency issues
- **Simplicity**: Keep APIs minimal and intuitive

### Feature Flags

- `unstable-mpmc`: Use multi-producer, multi-consumer channels (requires nightly)
- `unstable`: Enable all unstable features

## Roadmap

### Current Priorities

1. **Stability**: Bug fixes and API refinement
2. **Documentation**: Comprehensive guides and examples  
3. **Performance**: Benchmarking and optimizations
4. **Platform Support**: Ensure compatibility across targets

## Communication

- **GitHub Issues**: Bug reports, feature requests, questions
- **Pull Requests**: Code review and discussion
- **Community**: Checkout [our Discord community](https://discord.gg/Wz42hVmrrK)

## Recognition

Contributors will be acknowledged in:
- Repository contributors list
- Release notes for significant contributions
- Project documentation where appropriate

## Code of Conduct

We follow the [Rust Code of Conduct](https://www.rust-lang.org/policies/code-of-conduct). Please be respectful and constructive in all interactions.

## Questions?

Don't hesitate to ask questions! Open an issue with the "question" label or reach out directly. We're here to help and appreciate your interest in improving Mill-IO.