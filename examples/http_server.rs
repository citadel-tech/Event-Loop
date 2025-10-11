use mill_io::{error::Result, EventHandler, EventLoop, ObjectPool, PooledObject, UnifiedEvent};
use mio::{
    net::{TcpListener, TcpStream},
    Interest, Token,
};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::SocketAddr,
    sync::{Arc, Mutex, OnceLock, RwLock},
};

static EVENT_LOOP: OnceLock<EventLoop> = OnceLock::new();
fn event_loop() -> &'static EventLoop {
    EVENT_LOOP.get_or_init(EventLoop::default)
}
static NEXT_TOKEN: OnceLock<RwLock<TokenGenerator>> = OnceLock::new();
fn next_token_lock() -> &'static RwLock<TokenGenerator> {
    NEXT_TOKEN.get_or_init(|| RwLock::new(TokenGenerator::new()))
}

const LISTENER_TOKEN: Token = Token(1);

struct TokenGenerator {
    next: usize,
}

impl TokenGenerator {
    fn new() -> Self {
        Self { next: 2 }
    }

    fn generate(&mut self) -> Token {
        let token = Token(self.next);
        self.next += 1;
        token
    }
}

/// HTTP response builder
struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl HttpResponse {
    fn ok(body: &str) -> Self {
        Self {
            status: 200,
            headers: vec![
                ("Content-Type".to_string(), "text/html".to_string()),
                ("Content-Length".to_string(), body.len().to_string()),
            ],
            body: body.to_string(),
        }
    }

    fn not_found() -> Self {
        let body = "<h1>404 Not Found</h1>";
        Self {
            status: 404,
            headers: vec![
                ("Content-Type".to_string(), "text/html".to_string()),
                ("Content-Length".to_string(), body.len().to_string()),
            ],
            body: body.to_string(),
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 {} {}\r\n",
            self.status,
            match self.status {
                200 => "OK",
                404 => "Not Found",
                _ => "Unknown",
            }
        );

        for (key, value) in &self.headers {
            response.push_str(&format!("{}: {}\r\n", key, value));
        }

        response.push_str("\r\n");
        response.push_str(&self.body);

        response.into_bytes()
    }
}

/// Simple HTTP request parser
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
}

impl HttpRequest {
    fn parse(data: &[u8]) -> Option<Self> {
        let request_str = String::from_utf8_lossy(data);
        let lines: Vec<&str> = request_str.lines().collect();

        if lines.is_empty() {
            return None;
        }

        // parse request line (GET /path HTTP/1.1)
        let request_line_parts: Vec<&str> = lines[0].split_whitespace().collect();
        if request_line_parts.len() < 2 {
            return None;
        }

        let method = request_line_parts[0].to_string();
        let path = request_line_parts[1].to_string();

        // parse headers
        let mut headers = HashMap::new();
        for line in lines.iter().skip(1) {
            if line.is_empty() {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                headers.insert(key.trim().to_lowercase(), value.trim().to_string());
            }
        }

        Some(HttpRequest {
            method,
            path,
            headers,
        })
    }
}

pub struct HttpServer {
    listener: Arc<Mutex<TcpListener>>,
    connections: Arc<Mutex<HashMap<Token, TcpStream>>>,
    buffer_pool: ObjectPool<Vec<u8>>,
}

impl HttpServer {
    pub fn new(listener: Arc<Mutex<TcpListener>>) -> Self {
        Self {
            listener,
            connections: Arc::new(Mutex::new(HashMap::new())),
            buffer_pool: ObjectPool::new(20, || vec![0; 8192]),
        }
    }

    fn accept_connections(&self) -> Result<()> {
        loop {
            match self.listener.lock().unwrap().accept() {
                Ok((mut stream, addr)) => {
                    println!("New connection from: {}", addr);

                    let token = next_token_lock().write()?.generate();

                    event_loop().register(
                        &mut stream,
                        token,
                        Interest::READABLE,
                        HttpClient::new(token, self.connections.clone(), self.buffer_pool.clone()),
                    )?;

                    self.connections.lock().unwrap().insert(token, stream);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No more pending connections
                    break;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}

impl EventHandler for HttpServer {
    fn handle_event(&self, event: &UnifiedEvent) {
        if event.token() == LISTENER_TOKEN && event.is_readable() {
            if let Err(e) = self.accept_connections() {
                eprintln!("Error accepting connections: {}", e);
            }
        }
    }
}

pub struct HttpClient {
    token: Token,
    connections: Arc<Mutex<HashMap<Token, TcpStream>>>,
    buffer_pool: ObjectPool<Vec<u8>>,
}

impl HttpClient {
    fn new(
        token: Token,
        connections: Arc<Mutex<HashMap<Token, TcpStream>>>,
        buffer_pool: ObjectPool<Vec<u8>>,
    ) -> Self {
        Self {
            token,
            connections,
            buffer_pool,
        }
    }

    fn handle_http_request(&self, data: &[u8]) -> HttpResponse {
        if let Some(request) = HttpRequest::parse(data) {
            println!(
                "[INFO] Received request: method={}, path={}, headers={:?}",
                request.method, request.path, request.headers
            );

            match (request.method.as_str(), request.path.as_str()) {
                ("GET", "/") => HttpResponse::ok(
                    "<h1>Welcome to Mill-IO HTTP Server!</h1><p>A simple HTTP server built with the mill-io event loop.</p>",
                ),
                ("GET", "/about") => HttpResponse::ok(
                    "<h1>About</h1><p>This is a demonstration HTTP server using mill-io event loop library.</p>",
                ),
                ("GET", "/health") => {
                    HttpResponse::ok("<h1>Health Check</h1><p>Server is running!</p>")
                }
                _ => HttpResponse::not_found(),
            }
        } else {
            HttpResponse::not_found()
        }
    }

    fn disconnect(&self) {
        println!("Client disconnected: {:?}", self.token);
        if let Ok(mut connections) = self.connections.lock() {
            if let Some(mut stream) = connections.remove(&self.token) {
                if let Err(e) = event_loop().deregister(&mut stream, self.token) {
                    eprintln!("Failed to deregister client {:?}: {}", self.token, e);
                }
            }
        }
    }
}

impl EventHandler for HttpClient {
    fn handle_event(&self, event: &UnifiedEvent) {
        if !event.is_readable() {
            return;
        }

        let mut connections = match self.connections.lock() {
            Ok(conn) => conn,
            Err(_) => return,
        };

        let stream = match connections.get_mut(&self.token) {
            Some(s) => s,
            None => return,
        };

        let mut buffer: PooledObject<Vec<u8>> = self.buffer_pool.acquire();

        match stream.read(buffer.as_mut()) {
            Ok(0) => {
                // Client closed connection
                drop(connections); // Release lock before calling disconnect
                self.disconnect();
            }
            Ok(bytes_read) => {
                // Process HTTP request
                let response = self.handle_http_request(&buffer.as_ref()[..bytes_read]);
                let response_bytes = response.to_bytes();

                if let Err(e) = stream.write_all(&response_bytes) {
                    eprintln!("Error writing response to client {:?}: {}", self.token, e);
                    drop(connections);
                    self.disconnect();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // This is expected for non-blocking I/O
            }
            Err(e) => {
                eprintln!("Error reading from client {:?}: {}", self.token, e);
                drop(connections);
                self.disconnect();
            }
        }
    }
}

fn main() -> Result<()> {
    let addr: SocketAddr = "127.0.0.1:8080".parse()?;
    let listener = TcpListener::bind(addr)?;
    let listener = Arc::new(Mutex::new(listener));
    let addr = format!("http://{addr}");

    println!("HTTP Server starting on {}", addr);
    println!("Available routes:");
    println!("  GET {addr}/       - Home page");
    println!("  GET {addr}/about  - About page");
    println!("  GET {addr}/health - Health check");

    let server = HttpServer::new(Arc::clone(&listener));

    event_loop().register::<HttpServer, TcpListener>(
        &mut listener.lock().unwrap(),
        LISTENER_TOKEN,
        Interest::READABLE,
        server,
    )?;

    println!("Server listening on {addr}");
    event_loop().run()?;

    Ok(())
}
