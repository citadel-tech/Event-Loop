# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.2] - 2025-10-17

### Changed
- Updated `mio-rs` to version `1.1.0` which includes the fix for macOS thread safety issues

### Fixed
- Resolved macOS thread safety issues with `mio::Event` in worker threads (initially fixed with custom Event wrapper #70, then properly resolved by updating mio-rs after upstream fix #72)

### Development
- Enhanced CI/CD with cross-platform testing workflows
- Fixed clippy linting issues
- Added unstable feature testing with nightly Rust channel
- Improved CI workflow to avoid using `unstable` features in stable & beta channels
- Added TCP tests and disabled UDS tests on Windows for better cross-platform support
- Fixed documentation errors

## [1.0.1] - 2025-9-16

### Documentation
- Fixed documentation errors throughout the codebase
- Enhanced README.md with better explanations and examples

### Development
- Bumped version for documentation fixes

## [1.0.0] - 2025-9-15

### Added
- Complete event loop implementation with reactor pattern
- Thread pool for efficient task execution
- Object pool for memory management optimization
- Polling abstraction layer with `PollHandle`
- Error handling module with custom error types
- Event loop registration and deregistration capabilities
- Multiple channel types (MPMC/MPSC) for inter-thread communication

### Examples
- **Echo Server**: Complete TCP echo server implementation
- **HTTP Server**: Basic HTTP server example
- **File Watcher**: File system monitoring example  
- **JSON-RPC Server**: JSON-RPC protocol server implementation
