use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

mod mill_io_impl {
    use super::*;
    use mill_io::{error::Result, EventLoop};
    use mill_net::tcp::{
        traits::{ConnectionId, NetworkHandler},
        ServerContext, TcpServer, TcpServerConfig,
    };
    use mio::Token;

    #[derive(Clone)]
    pub struct EchoHandler {
        pub bytes_received: Arc<AtomicU64>,
    }

    impl NetworkHandler for EchoHandler {
        fn on_data(&self, ctx: &ServerContext, conn_id: ConnectionId, data: &[u8]) -> Result<()> {
            self.bytes_received
                .fetch_add(data.len() as u64, Ordering::Relaxed);
            ctx.send_to(conn_id, data)?;
            Ok(())
        }
    }

    pub struct ServerHandle {
        event_loop: Arc<EventLoop>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl ServerHandle {
        pub fn new(event_loop: Arc<EventLoop>) -> Self {
            let el = event_loop.clone();
            let handle = thread::spawn(move || {
                let _ = el.run();
            });
            Self {
                event_loop,
                handle: Some(handle),
            }
        }
    }

    impl Drop for ServerHandle {
        fn drop(&mut self) {
            self.event_loop.stop();
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    /// thread pool dispatch
    pub fn setup_server() -> (String, ServerHandle, Arc<AtomicU64>) {
        let event_loop = Arc::new(EventLoop::default());
        setup_server_with_event_loop(event_loop)
    }

    /// direct dispatch
    pub fn setup_server_low_latency() -> (String, ServerHandle, Arc<AtomicU64>) {
        let event_loop = Arc::new(EventLoop::new_low_latency(1024, 10).unwrap());
        setup_server_with_event_loop(event_loop)
    }

    fn setup_server_with_event_loop(
        event_loop: Arc<EventLoop>,
    ) -> (String, ServerHandle, Arc<AtomicU64>) {
        let bytes_received = Arc::new(AtomicU64::new(0));

        let handler = EchoHandler {
            bytes_received: bytes_received.clone(),
        };

        let config = TcpServerConfig::builder()
            .address("127.0.0.1:0".parse().unwrap())
            .build();

        let server = Arc::new(TcpServer::new(config, handler).unwrap());
        let addr = server.local_addr().unwrap().to_string();

        server.start(&event_loop, Token(1)).unwrap();
        let handle = ServerHandle::new(event_loop);

        thread::sleep(Duration::from_millis(50));

        (addr, handle, bytes_received)
    }
}

mod tokio_impl {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        runtime::Runtime,
        sync::oneshot,
    };

    pub struct ServerHandle {
        _runtime: Runtime,
        shutdown: Option<oneshot::Sender<()>>,
    }

    impl Drop for ServerHandle {
        fn drop(&mut self) {
            if let Some(tx) = self.shutdown.take() {
                let _ = tx.send(());
            }
        }
    }

    pub fn setup_server() -> (String, ServerHandle, Arc<AtomicU64>) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();

        let bytes_received = Arc::new(AtomicU64::new(0));
        let bytes_clone = bytes_received.clone();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let addr = runtime.block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap().to_string();

            tokio::spawn(async move {
                tokio::select! {
                    _ = async {
                        loop {
                            if let Ok((mut socket, _)) = listener.accept().await {
                                let bytes = bytes_clone.clone();
                                tokio::spawn(async move {
                                    let mut buf = vec![0u8; 8192];
                                    loop {
                                        match socket.read(&mut buf).await {
                                            Ok(0) => break,
                                            Ok(n) => {
                                                bytes.fetch_add(n as u64, Ordering::Relaxed);
                                                if socket.write_all(&buf[..n]).await.is_err() {
                                                    break;
                                                }
                                            }
                                            Err(_) => break,
                                        }
                                    }
                                });
                            }
                        }
                    } => {}
                    _ = shutdown_rx => {}
                }
            });

            addr
        });

        thread::sleep(Duration::from_millis(50));

        (
            addr,
            ServerHandle {
                _runtime: runtime,
                shutdown: Some(shutdown_tx),
            },
            bytes_received,
        )
    }
}

mod async_std_impl {
    use super::*;
    use async_std::{
        io::{ReadExt, WriteExt},
        net::TcpListener,
        task,
    };

    pub struct ServerHandle {
        _marker: (),
    }

    pub fn setup_server() -> (String, ServerHandle, Arc<AtomicU64>) {
        let bytes_received = Arc::new(AtomicU64::new(0));
        let bytes_clone = bytes_received.clone();

        let (tx, rx) = std::sync::mpsc::channel();

        task::spawn(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            tx.send(addr).unwrap();

            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let bytes = bytes_clone.clone();
                    task::spawn(async move {
                        let mut buf = vec![0u8; 8192];
                        loop {
                            match socket.read(&mut buf).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    bytes.fetch_add(n as u64, Ordering::Relaxed);
                                    if socket.write_all(&buf[..n]).await.is_err() {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    });
                }
            }
        });

        let addr = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        thread::sleep(Duration::from_millis(50));

        (addr, ServerHandle { _marker: () }, bytes_received)
    }
}

mod smol_impl {
    use super::*;
    use smol::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    pub struct ServerHandle {
        _marker: (),
    }

    pub fn setup_server() -> (String, ServerHandle, Arc<AtomicU64>) {
        let bytes_received = Arc::new(AtomicU64::new(0));
        let bytes_clone = bytes_received.clone();

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            smol::block_on(async {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap().to_string();
                tx.send(addr).unwrap();

                loop {
                    if let Ok((mut socket, _)) = listener.accept().await {
                        let bytes = bytes_clone.clone();
                        smol::spawn(async move {
                            let mut buf = vec![0u8; 8192];
                            loop {
                                match socket.read(&mut buf).await {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        bytes.fetch_add(n as u64, Ordering::Relaxed);
                                        if socket.write_all(&buf[..n]).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        })
                        .detach();
                    }
                }
            });
        });

        let addr = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        thread::sleep(Duration::from_millis(50));

        (addr, ServerHandle { _marker: () }, bytes_received)
    }
}

fn bench_echo_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("echo_throughput");

    for msg_size in [128, 1024, 4096] {
        group.throughput(Throughput::Bytes((msg_size * 100) as u64));

        // Mill-IO (default mode - thread pool dispatch)
        group.bench_with_input(
            BenchmarkId::new("mill-io", msg_size),
            &msg_size,
            |b, &size| {
                let (addr, _handle, _) = mill_io_impl::setup_server();
                let mut stream = TcpStream::connect(&addr).unwrap();
                stream.set_nodelay(true).unwrap();
                let data = vec![42u8; size];
                let mut response = vec![0u8; size];

                b.iter(|| {
                    for _ in 0..100 {
                        stream.write_all(&data).unwrap();
                        stream.read_exact(&mut response).unwrap();
                    }
                    black_box(&response);
                });
            },
        );

        // Mill-IO (low-latency mode - direct dispatch)
        group.bench_with_input(
            BenchmarkId::new("mill-io-fast", msg_size),
            &msg_size,
            |b, &size| {
                let (addr, _handle, _) = mill_io_impl::setup_server_low_latency();
                let mut stream = TcpStream::connect(&addr).unwrap();
                stream.set_nodelay(true).unwrap();
                let data = vec![42u8; size];
                let mut response = vec![0u8; size];

                b.iter(|| {
                    for _ in 0..100 {
                        stream.write_all(&data).unwrap();
                        stream.read_exact(&mut response).unwrap();
                    }
                    black_box(&response);
                });
            },
        );

        // Tokio
        group.bench_with_input(
            BenchmarkId::new("tokio", msg_size),
            &msg_size,
            |b, &size| {
                let (addr, _handle, _) = tokio_impl::setup_server();
                let mut stream = TcpStream::connect(&addr).unwrap();
                stream.set_nodelay(true).unwrap();
                let data = vec![42u8; size];
                let mut response = vec![0u8; size];

                b.iter(|| {
                    for _ in 0..100 {
                        stream.write_all(&data).unwrap();
                        stream.read_exact(&mut response).unwrap();
                    }
                    black_box(&response);
                });
            },
        );

        // async-std
        group.bench_with_input(
            BenchmarkId::new("async-std", msg_size),
            &msg_size,
            |b, &size| {
                let (addr, _handle, _) = async_std_impl::setup_server();
                let mut stream = TcpStream::connect(&addr).unwrap();
                stream.set_nodelay(true).unwrap();
                let data = vec![42u8; size];
                let mut response = vec![0u8; size];

                b.iter(|| {
                    for _ in 0..100 {
                        stream.write_all(&data).unwrap();
                        stream.read_exact(&mut response).unwrap();
                    }
                    black_box(&response);
                });
            },
        );

        // smol
        group.bench_with_input(BenchmarkId::new("smol", msg_size), &msg_size, |b, &size| {
            let (addr, _handle, _) = smol_impl::setup_server();
            let mut stream = TcpStream::connect(&addr).unwrap();
            stream.set_nodelay(true).unwrap();
            let data = vec![42u8; size];
            let mut response = vec![0u8; size];

            b.iter(|| {
                for _ in 0..100 {
                    stream.write_all(&data).unwrap();
                    stream.read_exact(&mut response).unwrap();
                }
                black_box(&response);
            });
        });
    }

    group.finish();
}

fn bench_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("ping_pong_latency");

    // Mill-IO (default mode)
    group.bench_function("mill-io", |b| {
        let (addr, _handle, _) = mill_io_impl::setup_server();
        let mut stream = TcpStream::connect(&addr).unwrap();
        stream.set_nodelay(true).unwrap();

        b.iter(|| {
            stream.write_all(b"ping").unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).unwrap();
            black_box(&buf);
        });
    });

    // Mill-IO (low-latency mode)
    group.bench_function("mill-io-fast", |b| {
        let (addr, _handle, _) = mill_io_impl::setup_server_low_latency();
        let mut stream = TcpStream::connect(&addr).unwrap();
        stream.set_nodelay(true).unwrap();

        b.iter(|| {
            stream.write_all(b"ping").unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).unwrap();
            black_box(&buf);
        });
    });

    // Tokio
    group.bench_function("tokio", |b| {
        let (addr, _handle, _) = tokio_impl::setup_server();
        let mut stream = TcpStream::connect(&addr).unwrap();
        stream.set_nodelay(true).unwrap();

        b.iter(|| {
            stream.write_all(b"ping").unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).unwrap();
            black_box(&buf);
        });
    });

    // async-std
    group.bench_function("async-std", |b| {
        let (addr, _handle, _) = async_std_impl::setup_server();
        let mut stream = TcpStream::connect(&addr).unwrap();
        stream.set_nodelay(true).unwrap();

        b.iter(|| {
            stream.write_all(b"ping").unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).unwrap();
            black_box(&buf);
        });
    });

    // smol
    group.bench_function("smol", |b| {
        let (addr, _handle, _) = smol_impl::setup_server();
        let mut stream = TcpStream::connect(&addr).unwrap();
        stream.set_nodelay(true).unwrap();

        b.iter(|| {
            stream.write_all(b"ping").unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).unwrap();
            black_box(&buf);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_echo_throughput, bench_latency);
criterion_main!(benches);
