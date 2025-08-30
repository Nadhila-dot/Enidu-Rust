use futures::future;
use futures::future::join_all;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_rustls::{rustls, TlsConnector};
use reqwest::{Client, ClientBuilder};
use rustls::{
    ClientConfig, RootCertStore, DigitallySignedStruct, SignatureScheme
};
use rustls::pki_types::{ServerName, CertificateDer, UnixTime};
use rustls::client::danger::{ServerCertVerified, HandshakeSignatureValid};
use std::fmt::Debug;
//@rs-ignore
use rand::thread_rng;

use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::connect::json::daemon_json;
use crate::libs::logs::print;
use crate::types::{AppState, JobInfo, JobStatus};
use crate::env::container::get_system_stats;
use url::Url;
use bytes::BytesMut;

// Shutdown phases for controlled resource reduction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownPhase {
    Running,
    ShuttingDown,
    Draining,
    Stopped,
}

// Statistics structure to track results
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestStats {
    pub sent: i64,
    pub success: i64,
    pub errors: i64,
    pub timeouts: i64,
    pub conn_errors: i64,
}

// System resource info for auto-scaling
#[derive(Debug, Clone)]
struct SystemResources {
    cpu_usage: f32,
    memory_available: u64,
    total_memory: u64,
}

// Type alias for a boxed stream that implements AsyncRead + AsyncWrite + Send + Unpin
trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> AsyncReadWrite for T {}

type DynStream = Box<dyn AsyncReadWrite + Send + Unpin>;

// Full user agents list for better randomization
static USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:109.0) Gecko/20100101 Firefox/117.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.5 Safari/605.1.15",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36 Edg/115.0.1901.203",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like creepercloud.io) Version/16.6 Mobile/15E148 Safari/604.1",
];

// Random referer URLs
static REFERERS: &[&str] = &[
    "https://www.google.com/",
    "https://www.facebook.com/",
    "https://www.twitter.com/",
    "https://www.instagram.com/",
    "https://www.reddit.com/",
    "https://www.linkedin.com/",
    "https://www.youtube.com/",
    "https://www.amazon.com/",
    "https://www.netflix.com/",
];

// Random accept language headers
static ACCEPT_LANGUAGES: &[&str] = &[
    "en-US,en;q=0.9",
    "en-GB,en;q=0.9",
    "en-CA,en;q=0.9",
    "en-AU,en;q=0.9",
    "fr-FR,fr;q=0.9,en;q=0.8",
    "es-ES,es;q=0.9,en;q=0.8",
    "de-DE,de;q=0.9,en;q=0.8",
    "ja-JP,ja;q=0.9,en;q=0.8",
    "zh-CN,zh;q=0.9,en;q=0.8",
];

// Global atomic counters for stats tracking
struct AtomicStats {
    sent: AtomicI64,
    success: AtomicI64,
    errors: AtomicI64,
    timeouts: AtomicI64,
    conn_errors: AtomicI64,
}

impl AtomicStats {
    fn new() -> Self {
        Self {
            sent: AtomicI64::new(0),
            success: AtomicI64::new(0),
            errors: AtomicI64::new(0),
            timeouts: AtomicI64::new(0),
            conn_errors: AtomicI64::new(0),
        }
    }

    fn to_test_stats(&self) -> TestStats {
        TestStats {
            sent: self.sent.load(Ordering::Relaxed),
            success: self.success.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            conn_errors: self.conn_errors.load(Ordering::Relaxed),
        }
    }
}

impl Default for AtomicStats {
    fn default() -> Self {
        Self::new()
    }
}

// REMOVE old DummyCertVerifier block and REPLACE with:
#[derive(Debug)]
struct NoVerify;

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ED25519,
            SignatureScheme::RSA_PKCS1_SHA256,
        ]
    }
}

// NEW: Raw HTTP Client for high-performance requests
#[derive(Clone)]
pub struct RawHttpClient {
    timeout: Duration,
    connect_timeout: Duration,
    pool: Arc<ConnectionPool>,
    tls_connector: Arc<TlsConnector>,
    proxy_addr: Option<String>,
}

impl RawHttpClient {
    pub fn builder() -> RawHttpClientBuilder {
        RawHttpClientBuilder::new()
    }

    // Send raw HTTP GET request
    pub async fn get(&self, url: &str) -> Result<RawHttpResponse, std::io::Error> {
        let headers = HashMap::new();
        self.send_request(url, "GET", headers).await
    }

    // Send request with method and headers
    pub async fn send_request(
        &self,
        url: &str,
        method: &str,
        headers: HashMap<String, String>
    ) -> Result<RawHttpResponse, std::io::Error> {
        let parsed_url = Url::parse(url)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid URL"))?;

        let host = parsed_url.host_str().unwrap_or("localhost");
        let port = parsed_url.port().unwrap_or(if parsed_url.scheme() == "https" { 443 } else { 80 });
        let use_tls = parsed_url.scheme() == "https";

        let mut stream = self.pool.get_connection(host, port, use_tls, &self.tls_connector, self.connect_timeout).await?;

        // Build path + query (Url has no path_and_query())
        let mut path = parsed_url.path().to_string();
        if path.is_empty() {
            path = "/".to_string();
        }
        if let Some(q) = parsed_url.query() {
            path.push('?');
            path.push_str(q);
        }

        let mut request = format!("{} {} HTTP/1.1\r\n", method, path);
        request.push_str(&format!("Host: {}\r\n", host));
        request.push_str("Connection: keep-alive\r\n");
        for (k, v) in headers {
            request.push_str(&format!("{}: {}\r\n", k, v));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await?;

        let read_timeout = self.timeout;
        let mut response = String::new();
        let read_result = tokio::time::timeout(read_timeout, async {
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                if let Ok(s) = std::str::from_utf8(&buf[..n]) {
                    response.push_str(s);
                    if response.contains("\r\n\r\n") {
                        // simple break after headers+body start (optional)
                    }
                }
            }
            Ok::<(), std::io::Error>(())
        }).await;

        match read_result {
            Ok(Ok(_)) => {
                self.pool.return_connection(host, port, use_tls, stream).await;
                let status_code = response
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|code| code.parse::<u16>().ok())
                    .unwrap_or(0);
                Ok(RawHttpResponse { status: status_code, body: response })
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Request timed out")),
        }
    }
}

// Response type
pub struct RawHttpResponse {
    pub status: u16,
    pub body: String,
}

impl RawHttpResponse {
    pub fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }
}

// Builder for RawHttpClient
pub struct RawHttpClientBuilder {
    timeout: Duration,
    connect_timeout: Duration,
    pool_idle_timeout: Duration,
    danger_accept_invalid_certs: bool,
    proxy_addr: Option<String>,
}

impl RawHttpClientBuilder {
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(1),          // safer default
            connect_timeout: Duration::from_millis(300),
            pool_idle_timeout: Duration::from_secs(60),
            danger_accept_invalid_certs: true,
            proxy_addr: None,
        }
    }
    
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }
    
    pub fn pool_idle_timeout(mut self, timeout: Duration) -> Self {
        self.pool_idle_timeout = timeout;
        self
    }
    
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.danger_accept_invalid_certs = accept;
        self
    }
    
    pub fn proxy(mut self, proxy_addr: &str) -> Self {
        self.proxy_addr = Some(proxy_addr.to_string());
        self
    }
    
    pub fn build(self) -> Result<RawHttpClient, std::io::Error> {
        let mut root_store = RootCertStore::empty();
        // Updated for rustls 0.23+ API - Use add_server_trust_anchors for webpki-roots
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        
        let mut config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        if self.danger_accept_invalid_certs {
            config.dangerous().set_certificate_verifier(Arc::new(NoVerify));
        }

        let tls_connector = TlsConnector::from(Arc::new(config));
        Ok(RawHttpClient {
            timeout: self.timeout,
            connect_timeout: self.connect_timeout,
            pool: Arc::new(ConnectionPool::new(self.pool_idle_timeout)),
            tls_connector: Arc::new(tls_connector),
            proxy_addr: self.proxy_addr,
        })
    }
}

// REPLACEMENT: More aggressive connection pooling implementation
pub struct ConnectionPool {
    pools: dashmap::DashMap<String, tokio::sync::Mutex<Vec<DynStream>>>,
    idle_timeout: Duration,
    // Track pool metrics
    total_connections: AtomicU32,
    max_connections_per_host: usize,
}

impl ConnectionPool {
    fn new(idle_timeout: Duration) -> Self {
        Self {
            pools: dashmap::DashMap::new(),
            idle_timeout,
            total_connections: AtomicU32::new(0),
            // Massively increase connection pool size (similar to Go's 5,000,000)
            max_connections_per_host: 5_000_000,
        }
    }
    
    async fn get_connection(
        &self,
        host: &str,
        port: u16,
        use_tls: bool,
        tls_connector: &TlsConnector,
        connect_timeout: Duration,
    ) -> Result<DynStream, std::io::Error> {
        let key = format!("{}:{}:{}", host, port, use_tls);
        if let Some(pool_entry) = self.pools.get(&key) {
            let mut pool = pool_entry.lock().await;
            if let Some(conn) = pool.pop() {
                return Ok(conn);
            }
        }

        // Apply connect timeout
        let tcp_stream = match tokio::time::timeout(connect_timeout, TcpStream::connect((host, port))).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timeout")),
        };

        tcp_stream.set_nodelay(true)?;

        match tcp_stream.into_std() {
            Ok(std_stream) => {
                let socket = socket2::Socket::from(std_stream);
                let _ = socket.set_reuse_address(true);
                let _ = socket.set_recv_buffer_size(1_048_576);
                let _ = socket.set_send_buffer_size(1_048_576);
                let std_stream = socket.into();
                let tcp_stream = TcpStream::from_std(std_stream)?;
                self.total_connections.fetch_add(1, Ordering::Relaxed);

                if use_tls {
                    let domain = ServerName::try_from(host.to_string())
                        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid hostname"))?;
                    let tls = tls_connector.connect(domain, tcp_stream).await
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
                    Ok(Box::new(tls))
                } else {
                    Ok(Box::new(tcp_stream))
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn return_connection(
        &self,
        host: &str,
        port: u16,
        use_tls: bool,
        conn: DynStream,
    ) {
        let key = format!("{}:{}:{}", host, port, use_tls);
        let entry = self.pools.entry(key).or_insert_with(|| tokio::sync::Mutex::new(Vec::new()));
        let mut pool = entry.lock().await;
        
        // Keep much more connections in the pool, similar to Go version
        if pool.len() < self.max_connections_per_host {
            pool.push(conn);
        }
    }
    
    // Get pool stats for monitoring
    fn get_stats(&self) -> (u32, usize) {
        let total = self.total_connections.load(Ordering::Relaxed);
        let active_hosts = self.pools.len();
        (total, active_hosts)
    }
}

// Add a request template builder similar to Go version
struct RequestTemplate {
    method: String,
    path: String,
    host: String,
    scheme: String,
    port: u16,
    use_tls: bool,
    base_headers: HashMap<String, String>,
}

impl RequestTemplate {
    fn new(method: &str, url: &str) -> Result<Self, std::io::Error> {
        let parsed_url = Url::parse(url)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid URL"))?;

        let scheme = parsed_url.scheme().to_string();
        let host = parsed_url.host_str().unwrap_or("localhost").to_string();

        let mut path = parsed_url.path().to_string();
        if path.is_empty() {
            path = "/".to_string();
        }
        if let Some(q) = parsed_url.query() {
            path.push('?');
            path.push_str(q);
        }

        let port = parsed_url.port().unwrap_or(if scheme == "https" { 443 } else { 80 });
        let use_tls = scheme == "https";

        let mut base_headers = HashMap::new();
        base_headers.insert("Host".to_string(), host.clone());
        base_headers.insert("Connection".to_string(), "keep-alive".to_string());
        base_headers.insert("Accept".to_string(), "*/*".to_string());

        Ok(Self {
            method: method.to_string(),
            path,
            host,
            scheme,
            port,
            use_tls,
            base_headers,
        })
    }

    fn build_request(&self, additional: HashMap<String, String>) -> String {
        let mut request = format!("{} {} HTTP/1.1\r\n", self.method, self.path);
        for (k, v) in &self.base_headers {
            request.push_str(&format!("{}: {}\r\n", k, v));
        }
        for (k, v) in additional {
            request.push_str(&format!("{}: {}\r\n", k, v));
        }
        request.push_str("\r\n");
        request
    }
}

// Enhance RawHttpClient with retry logic and request templates
impl RawHttpClient {
    // Send request with retry logic (similar to Go version)
    pub async fn send_request_with_retry(
        &self,
        url: &str,
        method: &str,
        headers: HashMap<String, String>,
        max_retries: usize,
    ) -> Result<RawHttpResponse, std::io::Error> {
        let mut rng = rand::thread_rng();
        let backoff_base = Duration::from_millis(50);
        
        // Create request template for reuse
        let template = RequestTemplate::new(method, url)?;
        
        for retry in 0..=max_retries {
            // Check shutdown flag if we're in worker context
            let should_interrupt = CURRENT_SHUTDOWN_PHASE.with(|cell| {
                if let Some(phase) = *cell.borrow() {
                    phase >= ShutdownPhase::Draining as u32
                } else {
                    false
                }
            });
            if should_interrupt {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted, 
                    "Operation interrupted due to shutdown"
                ));
            }
            
            let result = self.send_request_with_template(&template, headers.clone()).await;
            
            match &result {
                Ok(response) if response.is_success() => return result,
                Ok(_) => {
                    // Non-2xx response, might retry depending on status
                    if retry == max_retries {
                        return result;
                    }
                },
                Err(e) if is_connection_error(e) => {
                    if retry == max_retries {
                        return result;
                    }
                    
                    // Apply exponential backoff with jitter
                    let backoff = backoff_base * 2u32.pow(retry as u32);
                    let jitter = Duration::from_millis(rng.gen_range(0..backoff.as_millis() as u64 / 2));
                    tokio::time::sleep(backoff + jitter).await;
                },
                _ => return result, // Other errors are not retryable
            }
        }
        
        // Should never reach here due to the returns above
        unreachable!()
    }
    
    // Send request using a pre-built template (more efficient)
    async fn send_request_with_template(
        &self,
        template: &RequestTemplate,
        headers: HashMap<String, String>,
    ) -> Result<RawHttpResponse, std::io::Error> {
        // Get connection with correct scheme/port
        let mut stream = self.pool
            .get_connection(&template.host, template.port, template.use_tls, &self.tls_connector, self.connect_timeout)
            .await?;

        let request = template.build_request(headers);
        stream.write_all(request.as_bytes()).await?;

        // Faster: read only headers (up to a cap)
        let read_timeout = self.timeout;
        let mut buffer = BytesMut::with_capacity(4096);
        let read_res = tokio::time::timeout(read_timeout, async {
            let mut tmp = [0u8; 1024];
            while buffer.len() < 8192 {
                let n = stream.read(&mut tmp).await?;
                if n == 0 {
                    break;
                }
                buffer.extend_from_slice(&tmp[..n]);
                if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Ok::<(), std::io::Error>(())
        }).await;

        match read_res {
            Ok(Ok(())) => {
                self.pool.return_connection(&template.host, template.port, template.use_tls, stream).await;
                let text = String::from_utf8_lossy(&buffer);
                let status_code = text
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|c| c.parse::<u16>().ok())
                    .unwrap_or(0);
                Ok(RawHttpResponse { status: status_code, body: String::new() })
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "read timeout")),
        }
    }
}

// Helper to check if an error is a connection-related error (like in Go version)
fn is_connection_error(err: &std::io::Error) -> bool {
    let error_str = err.to_string().to_lowercase();
    error_str.contains("connection") || 
        error_str.contains("reset") || 
        error_str.contains("broken pipe") || 
        error_str.contains("eof") || 
        error_str.contains("i/o timeout")
}

// Add thread_local for tracking shutdown phase in each worker thread
thread_local! {
    static CURRENT_SHUTDOWN_PHASE: std::cell::RefCell<Option<u32>> = std::cell::RefCell::new(None);
}

// Monitor system resources for auto-scaling
fn monitor_system_resources() -> SystemResources {
    let sys = get_system_stats();
    let mem_available = sys.total_memory() - sys.used_memory();
    let cpu_usage = 1.0 - (mem_available as f32 / sys.total_memory() as f32);
    
    SystemResources {
        cpu_usage,
        memory_available: mem_available,
        total_memory: sys.total_memory(),
    }
}

// Read max workers from .env file
fn get_max_workers_from_env() -> Option<u32> {
    if let Ok(env_content) = std::fs::read_to_string(".env") {
        for line in env_content.lines() {
            if let Some(value) = line.strip_prefix("MAX_WORKERS=") {
                return value.trim().parse().ok();
            }
        }
    }
    None
}

// Dynamic concurrency adjustment based on system resources
fn adjust_concurrency(
    semaphore: &tokio::sync::Semaphore, 
    resources: &SystemResources,
    target_concurrency: u32,
    current_rps: u32,
    target_rps: u32
) {
    let current_permits = semaphore.available_permits();
    
    // More aggressive memory factor
    let memory_factor = if resources.memory_available < (resources.total_memory / 10) {
        2.0
    } else if resources.memory_available < (resources.total_memory / 5) {
        0.6
    } else {
        1.2 // Actually increase when memory is good
    };
    
    // RPS-based adjustment
    let rps_factor = if current_rps < target_rps / 2 {
        1.5 // Boost when RPS is too low
    } else if current_rps > target_rps {
        0.9 // Slight reduction when target exceeded
    } else {
        1.1 // Slight increase otherwise
    };
    
    let target_permits = (target_concurrency as f32 * memory_factor * rps_factor) as usize;
    let target_permits = std::cmp::min(target_permits, 50_000_000); // Massive cap
    
    let diff = target_permits as isize - current_permits as isize;
    if diff > 0 {
        semaphore.add_permits(diff as usize);
    }
}

// UPDATED: Raw TCP-based HTTP tester (replaces reqwest-based one)
pub async fn http_tester(
    target_url: String,
    proxy_addr: Option<String>,
    base_concurrency: u32,
    timeout_sec: u64,
    wait_ms: u32,
    random_headers: bool,
) -> AtomicStats {
    
    // REMOVE ALL ARTIFICIAL LIMITS - GO FULL BEAST MODE
    let num_cpus = num_cpus::get() as u32;
    
    // Match Go's aggressive scaling: 15000 workers per CPU core
    let workers_per_cpu = 15_000;
    let total_workers = num_cpus * workers_per_cpu * base_concurrency;
    
    println!("🦀 RUST BEAST MODE: Launching {} workers across {} CPUs", total_workers, num_cpus);
    println!("⚡ Target: {} | Concurrency: {} | Timeout: {}s", target_url, total_workers, timeout_sec);
    
    // Create MASSIVE connection pool like Go version
    let mut client_builder = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_sec))
        .connect_timeout(Duration::from_millis(500))
        .pool_max_idle_per_host(5_000_000)  // Match Go's massive pool
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .tcp_nodelay(true)
        .http1_only()  // Disable HTTP/2 for maximum simple throughput
        .danger_accept_invalid_certs(true);
    
    if let Some(proxy) = proxy_addr {
        if let Ok(proxy_url) = reqwest::Proxy::all(&proxy) {
            client_builder = client_builder.proxy(proxy_url);
        }
    }
    
    let client = Arc::new(client_builder.build().unwrap());
    
    // Shared atomic counters
    let stats = Arc::new(AtomicStats::default());
    let stop_flag = Arc::new(AtomicBool::new(false));
    
    // NO SEMAPHORES - NO LIMITS - JUST RAW POWER
    let mut handles = Vec::with_capacity(total_workers as usize);
    
    // Launch ALL workers immediately - no artificial throttling
    for worker_id in 0..total_workers {
        let client = client.clone();
        let stats = stats.clone();
        let stop_flag = stop_flag.clone();
        let url = target_url.clone();
        
        let handle = tokio::spawn(async move {
            let mut rng = StdRng::seed_from_u64(worker_id as u64);
            let retry_max = 3;
            let backoff_base = Duration::from_millis(50);
            
            loop {
                // Check stop flag
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                
                // Build request with random headers
                let mut req_builder = client.get(&url);
                
                if random_headers {
                    req_builder = req_builder
                        .header("User-Agent", USER_AGENTS[rng.gen_range(0..USER_AGENTS.len())])
                        .header("Referer", REFERERS[rng.gen_range(0..REFERERS.len())])
                        .header("Cache-Control", "no-cache")
                        .header("Accept-Language", "en-US,en;q=0.9");
                }
                
                // Retry logic similar to Go version
                let mut success = false;
                for retry in 0..retry_max {
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    
                    match req_builder.try_clone().unwrap().send().await {
                        Ok(response) => {
                            stats.sent.fetch_add(1, Ordering::Relaxed);
                            if response.status().is_success() {
                                stats.success.fetch_add(1, Ordering::Relaxed);
                                success = true;
                                break;
                            } else {
                                stats.errors.fetch_add(1, Ordering::Relaxed);
                                break;
                            }
                        }
                        Err(e) => {
                            stats.sent.fetch_add(1, Ordering::Relaxed);
                            
                            if e.is_timeout() {
                                stats.timeouts.fetch_add(1, Ordering::Relaxed);
                            } else if e.is_connect() || e.is_request() {
                                stats.conn_errors.fetch_add(1, Ordering::Relaxed);
                            } else {
                                stats.errors.fetch_add(1, Ordering::Relaxed);
                            }
                            
                            // Only retry on connection errors
                            if !e.is_connect() || retry == retry_max - 1 {
                                break;
                            }
                            
                            // Exponential backoff with jitter
                            let backoff = backoff_base * 2u32.pow(retry);
                            let jitter = Duration::from_millis(rng.gen_range(0..backoff.as_millis() as u64 / 2));
                            sleep(backoff + jitter).await;
                        }
                    }
                }
                
                // Wait between requests (with jitter like Go version)
                if wait_ms > 0 {
                    let jitter = (wait_ms as f64 * 0.2) as u64;
                    let adjusted_wait = wait_ms as i64 + rng.gen_range(-(jitter as i64 / 2)..=(jitter as i64 / 2));
                    if adjusted_wait > 0 {
                        sleep(Duration::from_millis(adjusted_wait as u64)).await;
                    }
                }
            }
        });
        
        handles.push(handle);
    }
    
    // Stats printer task
    let stats_clone = stats.clone();
    let stop_flag_clone = stop_flag.clone();
    let stats_handle = tokio::spawn(async move {
        let start_time = Instant::now();
        let mut last_sent = 0;
        let mut last_success = 0;
        let mut last_errors = 0;
        
        while !stop_flag_clone.load(Ordering::Relaxed) {
            let current_sent = stats_clone.sent.load(Ordering::Relaxed);
            let current_success = stats_clone.success.load(Ordering::Relaxed);
            let current_errors = stats_clone.errors.load(Ordering::Relaxed);
            let current_timeouts = stats_clone.timeouts.load(Ordering::Relaxed);
            let current_conn_errors = stats_clone.conn_errors.load(Ordering::Relaxed);
            
            let duration = start_time.elapsed().as_secs_f64();
            let overall_rps = current_sent as f64 / duration;
            let current_rps = current_sent - last_sent;
            let success_rps = current_success - last_success;
            let error_rps = current_errors - last_errors;
            
            print!("\rRuntime: {:.1}s | Reqs: {} (RPS: {} | Avg: {:.1}) | Success: {} ({}/s) | Errors: {} ({}/s) | Timeouts: {} | ConnErrs: {}",
                duration,
                current_sent, current_rps, overall_rps,
                current_success, success_rps,
                current_errors, error_rps,
                current_timeouts, current_conn_errors
            );
            
            last_sent = current_sent;
            last_success = current_success;
            last_errors = current_errors;
            
            sleep(Duration::from_secs(1)).await;
        }
    });
    
    // Wait for stop signal (you'll need to implement this based on your stop mechanism)
    // For now, let's run for a demo period or until manual stop
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\n🛑 Interrupt signal detected, stopping all workers...");
        }
        _ = sleep(Duration::from_secs(300)) => {
            println!("\n⏰ Demo timeout reached, stopping...");
        }
    }
    
    // Signal all workers to stop
    stop_flag.store(true, Ordering::SeqCst);
    
    // Wait for all workers to finish
    let _ = join_all(handles).await;
    stats_handle.abort();
    
    println!("\n💥 RUST BEAST MODE completed!");
    
    // Return final stats
    AtomicStats {
        sent: AtomicI64::new(stats.sent.load(Ordering::Relaxed)),
        success: AtomicI64::new(stats.success.load(Ordering::Relaxed)),
        errors: AtomicI64::new(stats.errors.load(Ordering::Relaxed)),
        timeouts: AtomicI64::new(stats.timeouts.load(Ordering::Relaxed)),
        conn_errors: AtomicI64::new(stats.conn_errors.load(Ordering::Relaxed)),
    }
}

// Helper function to remove all the conservative resource management
pub async fn unleashed_worker_spawn(
    client: Arc<Client>,
    url: String,
    stats: Arc<AtomicStats>,
    stop_flag: Arc<AtomicBool>,
    wait_ms: u32,
    random_headers: bool,
) {
    let mut rng = StdRng::from_rng(&mut thread_rng());
    
    loop {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }
        
        // Build request
        let mut req_builder = client.get(&url);
        
        if random_headers {
            req_builder = req_builder
                .header("User-Agent", USER_AGENTS[rng.gen_range(0..USER_AGENTS.len())])
                .header("Referer", REFERERS[rng.gen_range(0..REFERERS.len())])
                .header("Cache-Control", "no-cache");
        }
        
        // Fire and forget - no retry logic to maximize speed
        match req_builder.send().await {
            Ok(response) => {
                stats.sent.fetch_add(1, Ordering::Relaxed);
                if response.status().is_success() {
                    stats.success.fetch_add(1, Ordering::Relaxed);
                } else {
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(e) => {
                stats.sent.fetch_add(1, Ordering::Relaxed);
                if e.is_timeout() {
                    stats.timeouts.fetch_add(1, Ordering::Relaxed);
                } else if e.is_connect() {
                    stats.conn_errors.fetch_add(1, Ordering::Relaxed);
                } else {
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        
        // Minimal wait with jitter
        if wait_ms > 0 {
            let jitter = rng.gen_range(0..wait_ms / 2);
            sleep(Duration::from_millis((wait_ms + jitter) as u64)).await;
        }
    }
}

// Helper function for forced cleanup
fn force_cleanup() {
    use std::alloc::GlobalAlloc;
    print("Forcing cleanup...", true);
    unsafe {
        std::alloc::System.alloc_zeroed(std::alloc::Layout::new::<u8>());
    }
}

// Improved stop_job with instant termination
pub fn stop_job(app_state: &AppState, id: &str) -> bool {
    let mut jobs = app_state.jobs.lock().unwrap();

    if let Some(job) = jobs.get_mut(id) {
        job.status = JobStatus::Stopping;

        // FIRST: Create stop files immediately
        create_stop_files();

        // SECOND: Log instant termination
        let log_channels = app_state.log_channels.lock().unwrap();
        if let Some(log_tx) = log_channels.get(id) {
            let _ = log_tx.send(daemon_json("update", "Forcibly terminating all workers"));
        }
        drop(log_channels);

        // THIRD: Broadcast stop signal to all channels
        print(&format!("Forcibly terminating all workers for job {}", id), true);
        {
            let stop_channels = app_state.stop_channels.lock().unwrap();
            if let Some(stop_tx) = stop_channels.get(id) {
                // Send multiple stop signals to ensure delivery
                for _ in 0..5 {
                    let _ = stop_tx.send("STOP_IMMEDIATELY".to_string());
                }
            }
        }

        // FOURTH: Abort the task immediately
        print(&format!("Forcibly terminating all workers for job {}", id), true);
        {
            let mut job_tasks = app_state.job_tasks.lock().unwrap();
            if let Some(handle) = job_tasks.remove(id) {
                handle.abort();
            }
        }

        true
    } else {
        false
    }
}

// Create stop files to signal shutdown
// Simple async file watcher for stop files
pub fn create_stop_file_watcher() -> tokio::sync::mpsc::Receiver<()> {
    use std::path::PathBuf;
    use tokio::sync::mpsc;
    use tokio::time::{sleep, Duration};

    let (tx, rx) = mpsc::channel(1);
    let stop_files = vec![
        PathBuf::from(".stop-runner"),
        PathBuf::from(".stop"),
        PathBuf::from("data/.stop"),
        std::env::temp_dir().join("enidu.stop"),
    ];

    // Spawn a background watcher task
    tokio::spawn(async move {
        loop {
            for path in &stop_files {
                if path.exists() {
                    let _ = tx.send(()).await;
                    return;
                }
            }
            sleep(Duration::from_millis(100)).await;
        }
    });

    rx
}

pub fn create_stop_files() {
    let stop_files = &[
        ".stop-runner",
        ".stop",
        "data/.stop",
    ];

    for &path in stop_files {
        if let Ok(mut file) = File::create(path) {
            let _ = file.write_all(b"stop");
        }
    }

    // Try temp directory
    if let Some(tmp_dir) = std::env::temp_dir().to_str() {
        let tmp_path = format!("{}/enidu.stop", tmp_dir);
        if let Ok(mut file) = File::create(tmp_path) {
            let _ = file.write_all(b"stop");
        }
    }
}

// Remove all stop files
pub fn remove_stop_files() {
    let stop_files = &[
        ".stop-runner",
        ".stop",
        "data/.stop",
    ];

    for &path in stop_files {
        let _ = std::fs::remove_file(path);
    }

    // Try temp directory
    if let Some(tmp_dir) = std::env::temp_dir().to_str() {
        let tmp_path = format!("{}/enidu.stop", tmp_dir);
        let _ = std::fs::remove_file(tmp_path);
    }
}

pub async fn handle_job(
    app_state: Arc<AppState>,
    job_info: JobInfo,
) {
    let id = job_info.id.clone();
    let id_for_task = id.clone();

    // Get broadcast channels from app state
    let log_tx = {
        let log_channels = app_state.log_channels.lock().unwrap();
        log_channels.get(&id).cloned().unwrap()
    };
    let stop_tx = {
        let stop_channels = app_state.stop_channels.lock().unwrap();
        stop_channels.get(&id).cloned().unwrap()
    };
    let stop_rx = stop_tx.subscribe();

    // Clone app_state for use after the async block
    let app_state_clone = app_state.clone();

    // Run the HTTP tester in a separate task
    let handle = tokio::spawn(async move {
        // Remove any existing stop files
        remove_stop_files();

        // Run the tester
        let message = format!(
            "Starting HTTP tester with URL: {}, Proxy: {:?}, Concurrency: {}, Timeout: {}s, Wait: {}ms, Random Headers: {}",
            job_info.url,
            job_info.proxy_addr,
            job_info.concurrency,
            job_info.timeout_sec,
            job_info.wait_ms,
            job_info.random_headers
        );
        print(&message, false);
        let stats = http_tester(
            job_info.url,
            job_info.proxy_addr,
            job_info.concurrency,
            job_info.timeout_sec,
            job_info.wait_ms,
            job_info.random_headers,
        ).await;

        // Update job status when complete
        let mut jobs = app_state.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(&id_for_task) {
            job.status = JobStatus::Complete;
        }

        // Log completion
        let final_stats_json = serde_json::json!({
            "event": "job_complete",
            "job_id": id_for_task,
            "sent": stats.sent,
            "success": stats.success,
            "errors": stats.errors
        });
        let _ = log_tx.send(final_stats_json.to_string());

        // Remove from job_tasks
        {
            let mut job_tasks = app_state.job_tasks.lock().unwrap();
            job_tasks.remove(&id_for_task);
        }
    });

    // Store the job task handle
    {
        let mut job_tasks = app_state_clone.job_tasks.lock().unwrap();
        job_tasks.insert(id, handle);
    }
}