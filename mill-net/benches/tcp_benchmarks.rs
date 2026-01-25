use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use mill_io::{error::Result, EventLoop, TaskPriority};
use mill_net::tcp::{
    traits::{ConnectionId, NetworkHandler},
    ServerContext, TcpServer, TcpServerConfig,
};
use mio::Token;
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

#[derive(Clone)]
struct BenchHandler {
    bytes_received: Arc<AtomicU64>,
    connections: Arc<AtomicUsize>,
}

impl NetworkHandler for BenchHandler {
    fn on_connect(&self, _: &ServerContext, _: ConnectionId) -> Result<()> {
        self.connections.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_data(&self, ctx: &ServerContext, conn_id: ConnectionId, data: &[u8]) -> Result<()> {
        self.bytes_received
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        ctx.send_to(conn_id, data)?;
        Ok(())
    }

    fn on_disconnect(&self, _: &ServerContext, _: ConnectionId) -> Result<()> {
        Ok(())
    }
}

struct ServerHandle {
    event_loop: Arc<EventLoop>,
    handle: Option<thread::JoinHandle<()>>,
}

impl ServerHandle {
    fn new(event_loop: Arc<EventLoop>) -> Self {
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

fn bench_tcp_echo_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("tcp_echo");

    for msg_size in [128, 1024, 4096] {
        group.throughput(Throughput::Bytes((msg_size * 100) as u64));

        group.bench_with_input(
            BenchmarkId::new("message_size", msg_size),
            &msg_size,
            |b, &size| {
                let event_loop = Arc::new(EventLoop::default());
                let handler = BenchHandler {
                    bytes_received: Arc::new(AtomicU64::new(0)),
                    connections: Arc::new(AtomicUsize::new(0)),
                };
                let config = TcpServerConfig::builder()
                    .address("127.0.0.1:0".parse().unwrap())
                    .build();
                let server = Arc::new(TcpServer::new(config, handler.clone()).unwrap());
                let addr = server.local_addr().unwrap();

                server.clone().start(&event_loop, Token(1)).unwrap();
                let el = event_loop.clone();
                let _ = thread::spawn(move || {
                    let _ = el.run();
                });

                thread::sleep(Duration::from_millis(10));

                let mut stream = TcpStream::connect(addr).unwrap();
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
    }
    group.finish();
}

fn bench_tcp_concurrent_connections(c: &mut Criterion) {
    let mut group = c.benchmark_group("tcp_concurrent");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(15));

    for conn_count in [10, 50] {
        group.bench_with_input(
            BenchmarkId::from_parameter(conn_count),
            &conn_count,
            |b, &count| {
                // SETUP - Create server once
                let event_loop = Arc::new(EventLoop::default());
                let handler = BenchHandler {
                    bytes_received: Arc::new(AtomicU64::new(0)),
                    connections: Arc::new(AtomicUsize::new(0)),
                };
                let config = TcpServerConfig::builder()
                    .address("127.0.0.1:0".parse().unwrap())
                    .max_connections(count * 10)
                    .build();
                let server = Arc::new(TcpServer::new(config, handler).unwrap());
                let addr = server.local_addr().unwrap();

                server.clone().start(&event_loop, Token(1)).unwrap();
                let _server_handle = ServerHandle::new(event_loop);

                thread::sleep(Duration::from_millis(50));

                // MEASUREMENT - Don't use barriers, just measure sequential connection handling
                b.iter(|| {
                    let handles: Vec<_> = (0..count)
                        .map(|_| {
                            let addr_copy = addr;
                            thread::spawn(move || {
                                if let Ok(mut stream) = TcpStream::connect(addr_copy) {
                                    let _ = stream.set_nodelay(true);
                                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

                                    for _ in 0..3 {
                                        if stream.write_all(b"test").is_err() {
                                            break;
                                        }
                                        let mut buf = [0u8; 4];
                                        if stream.read_exact(&mut buf).is_err() {
                                            break;
                                        }
                                    }
                                }
                            })
                        })
                        .collect();

                    for h in handles {
                        let _ = h.join();
                    }
                });
            },
        );
    }
    group.finish();
}

fn bench_mixed_io_compute(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_io_compute");
    group.sample_size(20);

    // SETUP
    let event_loop = Arc::new(EventLoop::default());

    #[derive(Clone)]
    struct ComputeHandler {
        event_loop: Arc<EventLoop>,
        processed: Arc<AtomicUsize>,
    }

    impl NetworkHandler for ComputeHandler {
        fn on_data(&self, _ctx: &ServerContext, _conn_id: ConnectionId, data: &[u8]) -> Result<()> {
            let data_len = data.len();
            let processed = self.processed.clone();
            self.event_loop.spawn_compute_with_priority(
                move || {
                    let mut hash: u64 = 0;
                    for i in 0..data_len {
                        hash = hash.wrapping_mul(31).wrapping_add(i as u64);
                    }
                    black_box(hash);
                    processed.fetch_add(1, Ordering::Relaxed);
                },
                TaskPriority::High,
            );
            Ok(())
        }
    }

    let handler = ComputeHandler {
        event_loop: event_loop.clone(),
        processed: Arc::new(AtomicUsize::new(0)),
    };
    let config = TcpServerConfig::builder()
        .address("127.0.0.1:0".parse().unwrap())
        .build();
    let server = Arc::new(TcpServer::new(config, handler).unwrap());
    let addr = server.local_addr().unwrap();

    server.clone().start(&event_loop, Token(1)).unwrap();

    let el = event_loop.clone();
    let server_handle = thread::spawn(move || {
        let _ = el.run();
    });

    thread::sleep(Duration::from_millis(50));

    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_nodelay(true).unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    group.bench_function("io_with_compute_offload", |b| {
        b.iter(|| {
            for _ in 0..20 {
                let _ = stream.write_all(b"compute this data");
            }
        });
    });

    // CLEANUP
    event_loop.stop();
    let _ = server_handle.join();

    group.finish();
}

criterion_group!(
    tcp_benches,
    bench_tcp_echo_throughput,
    bench_tcp_concurrent_connections,
);

criterion_group!(mixed_benches, bench_mixed_io_compute);

criterion_main!(tcp_benches, mixed_benches);
