use mill_io::{EventHandler, EventLoop, ObjectPool, PooledObject, error::Result};
use mio::{
    Interest, Token,
    net::{TcpListener, TcpStream},
};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::SocketAddr,
    sync::{Arc, LazyLock, Mutex, RwLock},
};

static EVENT_LOOP: LazyLock<EventLoop> = LazyLock::new(|| EventLoop::default());

const LISTENER: Token = Token(1);
static CURRENT_TOKEN: LazyLock<RwLock<NextToken>> = LazyLock::new(|| RwLock::new(NextToken::new()));
struct NextToken(usize);

impl NextToken {
    fn new() -> Self {
        NextToken(2)
    }

    fn next(&mut self) -> Token {
        let next = self.0;
        self.0 += 1;
        Token(next)
    }
}

pub struct EchoServerHandler {
    listener: Arc<Mutex<TcpListener>>,
    connections: Arc<Mutex<HashMap<Token, TcpStream>>>,
    buffer_pool: ObjectPool<Vec<u8>>,
}

impl EchoServerHandler {
    pub fn new(listener: Arc<Mutex<TcpListener>>) -> Result<Self> {
        Ok(EchoServerHandler {
            listener,
            connections: Arc::new(Mutex::new(HashMap::new())),
            buffer_pool: ObjectPool::new(10, || vec![0; 8192]),
        })
    }

    fn handle_listener_event(
        &self,
        connections: Arc<Mutex<HashMap<Token, TcpStream>>>,
        buffer_pool: &ObjectPool<Vec<u8>>,
    ) -> Result<()> {
        loop {
            println!("handling listener");
            match self.listener.lock().unwrap().accept() {
                Ok((mut stream, _)) => {
                    println!("handling stream: addr={:#?}", stream.local_addr());
                    let token = CURRENT_TOKEN.write()?.next();
                    let connections_clone = connections.clone();
                    let buffer_pool_clone = buffer_pool.clone();

                    println!("register new event: token={token:?}");

                    EVENT_LOOP.register(
                        &mut stream,
                        token,
                        Interest::READABLE | Interest::WRITABLE,
                        ClientHandler {
                            connections: connections_clone,
                            token,
                            buffer_pool: buffer_pool_clone,
                        },
                    )?;
                    println!("{token:?}:inserting...");
                    connections.lock().unwrap().insert(token, stream);
                    println!("{token:?}: new connection is insterted");
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // No more pending connections
                    println!("no more pending connections: {e}");
                    break;
                }
                Err(e) => {
                    println!("error: {e:?}");
                    return Err(e.into());
                }
            }
        }
        Ok(())
    }
}

impl EventHandler for EchoServerHandler {
    fn handle_event(&self, event: &mio::event::Event) {
        if event.token() == LISTENER {
            if let Err(e) = self.handle_listener_event(self.connections.clone(), &self.buffer_pool)
            {
                eprintln!("error accepting connection: {}", e);
            }
            println!(
                "new Connection: len={:#?}",
                self.connections.lock().unwrap().len()
            );
        }
    }
}

#[derive(Debug)]
pub struct ClientHandler {
    connections: Arc<Mutex<HashMap<Token, TcpStream>>>,
    token: Token,
    buffer_pool: ObjectPool<Vec<u8>>,
}

impl EventHandler for ClientHandler {
    fn handle_event(&self, event: &mio::event::Event) {
        println!("client is Handling event for client: {:?}", self);
        let mut connections = self.connections.lock().unwrap();
        if let Some(stream) = connections.get_mut(&self.token) {
            if event.is_readable() {
                let mut buffer: PooledObject<Vec<u8>> = self.buffer_pool.acquire();
                match stream.read(buffer.as_mut()) {
                    Ok(0) => {
                        connections.remove(&self.token);
                        println!("client disconnected: {:?}", self.token);
                    }
                    Ok(n) => {
                        println!(
                            "Received {} bytes from client {:?}: {:?}",
                            n,
                            self.token,
                            String::from_utf8_lossy(&buffer.as_ref()[..n])
                        );
                        if let Err(e) = stream.write_all(&buffer.as_ref()[..n]) {
                            eprintln!("Error writing to client {:?}: {}", self.token, e);
                            connections.remove(&self.token);
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        eprintln!("Blocked Connection Error: {}", e)
                    }
                    Err(e) => {
                        eprintln!("Error reading from client {:?}: {}", self.token, e);
                        connections.remove(&self.token);
                    }
                }
            }
        }
    }
}

fn main() -> Result<()> {
    let addr: SocketAddr = "127.0.0.1:8080".parse()?;
    let listener = TcpListener::bind(addr)?;
    let listener = Arc::new(Mutex::new(listener));

    let server_handler = EchoServerHandler::new(Arc::clone(&listener))?;

    EVENT_LOOP.register::<EchoServerHandler, TcpListener>(
        &mut listener.lock().unwrap(),
        LISTENER,
        Interest::READABLE,
        server_handler,
    )?;

    println!("Echo server listening on {}", addr);
    EVENT_LOOP.run()?;

    Ok(())
}
