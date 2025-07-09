use mill_io::{EventHandler, EventLoop, ObjectPool, PooledObject};
use mio::{
    Interest, Token,
    net::{TcpListener, TcpStream},
};
use std::{
    collections::HashMap,
    error::Error,
    io::{self, Read, Write},
    net::SocketAddr,
    sync::{Arc, Mutex},
};

const LISTENER: Token = Token(1);

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
    token_generator: Mutex<NextToken>,
    buffer_pool: ObjectPool<Vec<u8>>,
}

impl EchoServerHandler {
    pub fn new(listener: Arc<Mutex<TcpListener>>) -> Result<Self, Box<dyn Error>> {
        Ok(EchoServerHandler {
            listener,
            connections: Arc::new(Mutex::new(HashMap::new())),
            token_generator: Mutex::new(NextToken::new()),
            buffer_pool: ObjectPool::new(10, || vec![0; 8192]),
        })
    }

    fn handle_listener_event(
        &self,
        event_loop: &EventLoop,
        connections: Arc<Mutex<HashMap<Token, TcpStream>>>,
        token_generator: &Mutex<NextToken>,
        buffer_pool: &ObjectPool<Vec<u8>>,
    ) -> Result<(), Box<dyn Error>> {
        loop {
            match self.listener.lock().unwrap().accept() {
                Ok((mut stream, _)) => {
                    let mut next_token = token_generator.lock().unwrap();
                    let token = next_token.next();
                    let connections_clone = connections.clone();
                    let buffer_pool_clone = buffer_pool.clone();

                    event_loop.register(
                        &mut stream,
                        token,
                        Interest::READABLE | Interest::WRITABLE,
                        ClientHandler {
                            connections: connections_clone,
                            token,
                            buffer_pool: buffer_pool_clone,
                        },
                    )?;
                    connections.lock().unwrap().insert(token, stream);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // No more pending connections
                    break;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}

impl EventHandler for EchoServerHandler {
    fn handle_event(&self, event: &mio::event::Event) {
        if event.token() == LISTENER {
            if let Err(e) = self.handle_listener_event(
                &EventLoop::new(1).unwrap(),
                self.connections.clone(),
                &self.token_generator,
                &self.buffer_pool,
            ) {
                eprintln!("Error accepting connection: {}", e);
            }
        }
    }
}

pub struct ClientHandler {
    connections: Arc<Mutex<HashMap<Token, TcpStream>>>,
    token: Token,
    buffer_pool: ObjectPool<Vec<u8>>,
}

impl EventHandler for ClientHandler {
    fn handle_event(&self, event: &mio::event::Event) {
        println!("Handling event for client: {:?}", self.token);
        let mut connections = self.connections.lock().unwrap();
        if let Some(stream) = connections.get_mut(&self.token) {
            if event.is_readable() {
                let mut buffer: PooledObject<Vec<u8>> = self.buffer_pool.acquire();
                match stream.read(buffer.as_mut()) {
                    Ok(0) => {
                        connections.remove(&self.token);
                        println!("Client disconnected: {:?}", self.token);
                    }
                    Ok(n) => {
                        println!(
                            "Received {} bytes from client {:?}: {:?}",
                            n,
                            self.token,
                            String::from_utf8_lossy(&buffer.as_ref()[..n])
                        );
                        // Echo data back to the client
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

fn main() -> Result<(), Box<dyn Error>> {
    let addr: SocketAddr = "127.0.0.1:8080".parse()?;
    let listener = TcpListener::bind(addr)?;
    let listener = Arc::new(Mutex::new(listener));

    let server_handler = EchoServerHandler::new(Arc::clone(&listener))?;
    let event_loop = EventLoop::new(4)?;

    event_loop.register::<EchoServerHandler, TcpListener>(
        &mut listener.lock().unwrap(),
        LISTENER,
        Interest::READABLE,
        server_handler,
    )?;

    println!("Echo server listening on {}", addr);
    event_loop.run()?;

    Ok(())
}
