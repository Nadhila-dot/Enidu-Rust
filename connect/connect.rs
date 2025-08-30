use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use futures_util::future::join_all;
use rand::seq::SliceRandom;

use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT},
    Client, Method, RequestBuilder,
};
use tokio::{
    fs::File,
    io::AsyncWriteExt,
    sync::{broadcast, mpsc, Semaphore},
    task::JoinHandle,
    time::{self, sleep},
};
use rand::{Rng};
use rand_distr::{Distribution, Poisson, Uniform};
use crate::{
    libs::logs::print,
    types::{AppState, JobInfo, JobStatus},
};

// Constants for benchmarking
const STATS_UPDATE_INTERVAL_MS: u64 = 250;
const BUFFER_POOL_SIZE: usize = 1024;
const BUFFER_SIZE: usize = 16 * 1024; // 16KB buffers
const MAX_RETRIES: u32 = 3;
const HTTP_PIPELINE_DEPTH: usize = 16;

// Job state
#[derive(Debug, Clone, PartialEq)]
enum JobState {
    Running,
    ShuttingDown,
    Draining,
    Stopped,
}

// Statistics for performance tracking
struct JobStats {
    start_time: Instant,
    requests: AtomicU64,
    successes: AtomicU64,
    errors: AtomicU64,
    timeouts: AtomicU64,
    conn_errors: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    last_rps: AtomicU64,
    
    // EWMA for smoothed metrics
    latency_ewma: Mutex<f64>,
    rps_ewma: Mutex<f64>,
    
    // Latency tracking
    latency_min: AtomicU64,
    latency_max: AtomicU64,
    latency_sum: AtomicU64,
}

impl JobStats {
    fn new() -> Self {
        Self {
            start_time: Instant::now(),
            requests: AtomicU64::new(0),
            successes: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
            conn_errors: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            last_rps: AtomicU64::new(0),
            latency_ewma: Mutex::new(0.0),
            rps_ewma: Mutex::new(0.0),
            latency_min: AtomicU64::new(u64::MAX),
            latency_max: AtomicU64::new(0),
            latency_sum: AtomicU64::new(0),
        }
    }

    fn update_latency(&self, latency_ms: u64) {
        // Update min/max latency with atomic compare-and-swap
        let mut current_min = self.latency_min.load(Ordering::Relaxed);
        while latency_ms < current_min {
            match self.latency_min.compare_exchange_weak(
                current_min,
                latency_ms,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_min = actual,
            }
        }

        let mut current_max = self.latency_max.load(Ordering::Relaxed);
        while latency_ms > current_max {
            match self.latency_max.compare_exchange_weak(
                current_max,
                latency_ms,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_max = actual,
            }
        }

        // Update sum for average calculation
        self.latency_sum.fetch_add(latency_ms, Ordering::Relaxed);

        // Update EWMA (Exponentially Weighted Moving Average) with 0.1 alpha
        let alpha = 0.1;
        let mut ewma = self.latency_ewma.lock().unwrap();
        if *ewma == 0.0 {
            *ewma = latency_ms as f64;
        } else {
            *ewma = alpha * (latency_ms as f64) + (1.0 - alpha) * *ewma;
        }
    }

    fn update_rps(&self, rps: u64) {
        self.last_rps.store(rps, Ordering::Relaxed);
        
        // Update EWMA for RPS
        let alpha = 0.1;
        let mut ewma = self.rps_ewma.lock().unwrap();
        if *ewma == 0.0 {
            *ewma = rps as f64;
        } else {
            *ewma = alpha * (rps as f64) + (1.0 - alpha) * *ewma;
        }
    }

    fn get_stats_json(&self) -> serde_json::Value {
        let elapsed_secs = self.start_time.elapsed().as_secs_f64();
        let requests = self.requests.load(Ordering::Relaxed);
        let successes = self.successes.load(Ordering::Relaxed);
        let errors = self.errors.load(Ordering::Relaxed);
        let timeouts = self.timeouts.load(Ordering::Relaxed);
        let conn_errors = self.conn_errors.load(Ordering::Relaxed);
        
        let avg_rps = if elapsed_secs > 0.0 {
            (requests as f64) / elapsed_secs
        } else {
            0.0
        };
        
        let latency_min = self.latency_min.load(Ordering::Relaxed);
        let latency_max = self.latency_max.load(Ordering::Relaxed);
        let latency_sum = self.latency_sum.load(Ordering::Relaxed);
        let latency_avg = if requests > 0 {
            (latency_sum as f64) / (requests as f64)
        } else {
            0.0
        };
        
        let ewma_latency = *self.latency_ewma.lock().unwrap();
        let ewma_rps = *self.rps_ewma.lock().unwrap();
        
        serde_json::json!({
            "elapsed_secs": elapsed_secs,
            "requests": {
                "total": requests,
                "successes": successes,
                "errors": errors,
                "timeouts": timeouts,
                "conn_errors": conn_errors,
            },
            "throughput": {
                "current_rps": self.last_rps.load(Ordering::Relaxed),
                "avg_rps": avg_rps,
                "ewma_rps": ewma_rps,
            },
            "latency_ms": {
                "min": if latency_min == u64::MAX { 0 } else { latency_min },
                "max": latency_max,
                "avg": latency_avg,
                "ewma": ewma_latency,
            },
            "transfer": {
                "bytes_sent": self.bytes_sent.load(Ordering::Relaxed),
                "bytes_received": self.bytes_received.load(Ordering::Relaxed),
            }
        })
    }
}

// User-Agent generators
const USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/96.0.4664.110 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.1 Safari/605.1.15",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:94.0) Gecko/20100101 Firefox/94.0",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 15_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.1 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (Linux; Android 12; Pixel 6) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/96.0.4664.104 Mobile Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/96.0.4664.110 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/96.0.4664.110 Safari/537.36",
    "Mozilla/5.0 (iPad; CPU OS 15_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.1 Mobile/15E148 Safari/604.1",
];

const ACCEPT_LANGUAGES: &[&str] = &[
    "en-US,en;q=0.9", "en-GB,en;q=0.8", "de-DE,de;q=0.9,en;q=0.8", 
    "fr-FR,fr;q=0.9,en;q=0.8", "ja-JP,ja;q=0.9,en;q=0.8", "es-ES,es;q=0.9",
    "zh-CN,zh;q=0.9,en;q=0.8", "ru-RU,ru;q=0.9,en;q=0.8",
];

const REFERRERS: &[&str] = &[
    "https://www.google.com/", "https://www.bing.com/", "https://duckduckgo.com/", 
    "https://www.facebook.com/", "https://twitter.com/", "https://www.linkedin.com/",
    "https://www.reddit.com/", "https://news.ycombinator.com/",
];

// BufferPool for efficient memory reuse
struct BufferPool {
    pool: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl BufferPool {
    fn new(capacity: usize, buffer_size: usize) -> Self {
        let mut pool = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            pool.push(vec![0; buffer_size]);
        }
        
        Self {
            pool: Arc::new(Mutex::new(pool)),
        }
    }
    
    fn get_buffer(&self) -> Vec<u8> {
        let mut pool = self.pool.lock().unwrap();
        if let Some(buffer) = pool.pop() {
            buffer
        } else {
            // If pool is empty, create new buffer
            vec![0; BUFFER_SIZE]
        }
    }
    
    fn return_buffer(&self, mut buffer: Vec<u8>) {
        // Clear buffer and return to pool
        buffer.clear();
        buffer.resize(BUFFER_SIZE, 0);
        let mut pool = self.pool.lock().unwrap();
        if pool.len() < BUFFER_POOL_SIZE {
            pool.push(buffer);
        }
        // If pool is full, buffer will be dropped
    }
}

// Create stop files to signal job termination
pub fn create_stop_files(job_id: &str) -> Result<(), std::io::Error> {
    // Create a directory for stop files if it doesn't exist
    std::fs::create_dir_all("./stops")?;
    
    // Create a stop file with the job ID
    let path = format!("./stops/{}.stop", job_id);
    std::fs::write(&path, "STOP")?;
    
    print(&format!("Created stop file: {}", path), false);
    Ok(())
}

// Stop a running job
pub fn stop_job(state: &AppState, job_id: &str) {
    // First update job status to indicate stopping
    {
        let mut jobs = state.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(job_id) {
            job.status = JobStatus::Stopping;
            print(&format!("Stopping job: {}", job_id), false);
        } else {
            print(&format!("Job not found for stopping: {}", job_id), false);
            return;
        }
    }
    
    // Create stop file
    if let Err(e) = create_stop_files(job_id) {
        print(&format!("Error creating stop file: {}", e), true);
    }
    
    // Send stop signal via channel
    if let Some(tx) = state.stop_channels.lock().unwrap().get(job_id) {
        let _ = tx.send(String::from("STOP"));
    }
}

// Generate random headers for requests
fn generate_random_headers(random_headers: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    
    if !random_headers {
        // Default basic headers
        headers.insert(USER_AGENT, HeaderValue::from_static("Enidu-Benchmark/1.0"));
        return headers;
    }
    
    let mut rng = rand::rng();
    
    // Add User-Agent
    let index = rng.gen_range(0..USER_AGENTS.len());
    let user_agent = USER_AGENTS[index];
    headers.insert(USER_AGENT, HeaderValue::from_str(user_agent).unwrap());
    
    // Add Accept-Language
    let index = rng.gen_range(0..ACCEPT_LANGUAGES.len());
    let accept_lang = ACCEPT_LANGUAGES[index];
    headers.insert(
        HeaderName::from_static("accept-language"),
        HeaderValue::from_str(accept_lang).unwrap(),
    );
    
    // Add Referer with 50% probability
    if rng.gen_bool(0.5) {
        let index = rng.gen_range(0..REFERRERS.len());
        let referer = REFERRERS[index];
        headers.insert(
            HeaderName::from_static("referer"),
            HeaderValue::from_str(referer).unwrap(),
        );
    }
    
    // Add cache control with 40% probability
    if rng.gen_bool(0.4) {
        headers.insert(
            HeaderName::from_static("cache-control"),
            HeaderValue::from_static("no-cache"),
        );
    }
    
    // Add random X-headers with 30% probability
    if rng.gen_bool(0.3) {
        headers.insert(
            HeaderName::from_static("x-requested-with"),
            HeaderValue::from_static("XMLHttpRequest"),
        );
    }
    
    headers
}

// Main job execution function
pub async fn handle_job(job: JobInfo, log_tx: broadcast::Sender<String>, stop_rx: broadcast::Receiver<String>) {
    print(&format!("Starting job: {}", job.id), false);
    
    // Send initial log message
    let _ = log_tx.send(format!("🚀 Starting benchmark with {} workers", job.concurrency));
    
    // Create shared state for job
    let stats = Arc::new(JobStats::new());
    let stop_flag = Arc::new(AtomicBool::new(false));
    let job_state = Arc::new(Mutex::new(JobState::Running));
    let buffer_pool = Arc::new(BufferPool::new(BUFFER_POOL_SIZE, BUFFER_SIZE));
    
    // Create semaphore to limit concurrency precisely
    let concurrency_limit = Arc::new(Semaphore::new(job.concurrency as usize));
    
    // Clone job.id before moving job into closures
    let job_id = job.id.clone();
    let job_id_for_stop = job_id.clone();
    let job_id_clone = job_id.clone();
    
    // Create a stop file watcher
    let stop_file_path = format!("./stops/{}.stop", job_id_for_stop);
    let stop_flag_clone = Arc::clone(&stop_flag);
    let job_state_clone = Arc::clone(&job_state);
    tokio::spawn(async move {
        let mut stop_rx = stop_rx;
        loop {
            // Check for stop signal from channel
            if let Ok(_) = stop_rx.try_recv() {
                print(&format!("Received stop signal for job: {}", job_id_for_stop), false);
                stop_flag_clone.store(true, Ordering::SeqCst);
                *job_state_clone.lock().unwrap() = JobState::ShuttingDown;
                break;
            }
            
            // Check for stop file
            if tokio::fs::metadata(&stop_file_path).await.is_ok() {
                print(&format!("Found stop file for job: {}", job_id_for_stop), false);
                stop_flag_clone.store(true, Ordering::SeqCst);
                *job_state_clone.lock().unwrap() = JobState::ShuttingDown;
                // Clean up stop file
                let _ = tokio::fs::remove_file(&stop_file_path).await;
                break;
            }
            
            // Sleep a bit before checking again
            sleep(Duration::from_millis(100)).await;
        }
    });
    
    // Spawn stats reporter task
    let log_tx_clone = log_tx.clone();
    let stats_clone = Arc::clone(&stats);
    let job_state_clone = Arc::clone(&job_state);
    // let job_id_clone = job_id.clone(); // already cloned above
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_millis(STATS_UPDATE_INTERVAL_MS));
        let mut last_requests = 0;
        
        loop {
            interval.tick().await;
            
            // Calculate current RPS
            let current_requests = stats_clone.requests.load(Ordering::Relaxed);
            let requests_delta = current_requests.saturating_sub(last_requests);
            let rps = (requests_delta as f64 / (STATS_UPDATE_INTERVAL_MS as f64 / 1000.0)) as u64;
            stats_clone.update_rps(rps);
            last_requests = current_requests;
            
            // Generate stats JSON and send to log channel
            let stats_json = stats_clone.get_stats_json();
            let state = {
                let state = job_state_clone.lock().unwrap();
                format!("{:?}", *state)
            };
            
            let log_msg = format!(
                "📊 [{}] State: {} | RPS: {}/s | Success: {} | Errors: {} | Avg Latency: {:.2}ms",
                job_id_clone,
                state,
                rps,
                stats_clone.successes.load(Ordering::Relaxed),
                stats_clone.errors.load(Ordering::Relaxed),
                *stats_clone.latency_ewma.lock().unwrap()
            );
            
            let _ = log_tx_clone.send(log_msg);
            
            // Send detailed stats as JSON every second
            if current_requests % 4 == 0 {  // Every ~1 second (250ms * 4)
                let _ = log_tx_clone.send(format!("📈 STATS: {}", serde_json::to_string(&stats_json).unwrap()));
            }
            
            // Break if job is stopped
            if *job_state_clone.lock().unwrap() == JobState::Stopped {
                break;
            }
        }
    });
    
    // Create HTTP client with optimal settings
    let client = create_optimized_client(job.timeout_sec);
    
    // Create channel for work distribution
    let (work_tx, _) = broadcast::channel(job.concurrency as usize * 2);
    
    // Spawn work generator
    let urls = job.target_urls.clone();
    let work_tx_clone = work_tx.clone();
    let stop_flag_clone = Arc::clone(&stop_flag);
    let job_state_clone = Arc::clone(&job_state);
    let job_state_clone_for_signal = Arc::clone(&job_state);
    tokio::spawn(async move {
        // Use spawn_blocking to ensure RNG is thread-safe
        tokio::task::spawn_blocking(move || {
            let mut rng = rand::thread_rng();
            
            // Create Poisson distribution for realistic timing if wait_ms is set
            let poisson_dist = if job.wait_ms > 0 {
                Some(Poisson::new(job.wait_ms as f64).unwrap())
            } else {
                None
            };
            
            loop {
                // Stop generating work if shutdown requested
                if stop_flag_clone.load(Ordering::SeqCst) || 
                   *job_state_clone.lock().unwrap() != JobState::Running {
                    break;
                }
                
                // Select random URL from target list
                let index = rng.gen_range(0..urls.len());
                let url = urls[index].clone();
                
                // Send work item to all subscribers (workers)
                if work_tx_clone.send(url).is_err() {
                    break;
                }
                
                // Wait based on specified delay or distribution
                if let Some(dist) = &poisson_dist {
                    let wait_time = dist.sample(&mut rng);
                    sleep(Duration::from_millis(wait_time as u64));
                } else if job.wait_ms > 0 {
                    // Fixed wait with small jitter
                    let jitter = if job.wait_ms > 10 {
                        Uniform::new(0, job.wait_ms / 10).unwrap().sample(&mut rng)
                    } else {
                        0
                    };
                    sleep(Duration::from_millis((job.wait_ms + jitter) as u64));
                }
            }
        }).await.unwrap();
        
        // Signal shutdown complete
        *job_state_clone_for_signal.lock().unwrap() = JobState::Draining;
    });
    
    // Spawn worker pool
    let mut workers = Vec::with_capacity(job.concurrency as usize);
    
    for worker_id in 0..job.concurrency {
        let mut work_rx = work_tx.subscribe();
        let client_clone = client.clone();
        let stats_clone = Arc::clone(&stats);
        let stop_flag_clone = Arc::clone(&stop_flag);
        let job_state_clone = Arc::clone(&job_state);
        let concurrency_limit_clone = Arc::clone(&concurrency_limit);
        let buffer_pool_clone = Arc::clone(&buffer_pool);
        let log_tx_clone = log_tx.clone();
        let random_headers = job.random_headers;
        let method_str = job.method.clone();
        let custom_body = job.custom_body.clone();
        
        let worker = tokio::spawn(async move {
            // Parse HTTP method
            let method = match method_str.as_str() {
                "GET" => Method::GET,
                "POST" => Method::POST,
                "PUT" => Method::PUT,
                "DELETE" => Method::DELETE,
                "HEAD" => Method::HEAD,
                _ => Method::GET,
            };
            
            // Setup exponential backoff parameters
            let mut backoff_ms: u64 = 10;
            let max_backoff_ms: u64 = 1000;
            
            while !stop_flag_clone.load(Ordering::SeqCst) {
                // Acquire semaphore permit to maintain concurrency limit
                let permit = match concurrency_limit_clone.acquire().await {
                    Ok(permit) => permit,
                    Err(_) => break, // Semaphore closed, exit
                };
                
                // Check if we should be draining instead of processing new work
                let current_state = {
                    let state = job_state_clone.lock().unwrap();
                    (*state).clone()
                };
                
                if current_state != JobState::Running {
                    // Release permit and exit if not running
                    drop(permit);
                    break;
                }
                
                // Get next URL to process
                let url = match work_rx.recv().await {
                    Ok(url) => url,
                    Err(_) => break,
                };
                
                // Get buffer from pool
                let buffer = buffer_pool_clone.get_buffer();
                
                // Generate headers
                let headers = generate_random_headers(random_headers);
                
                // Track timing
                let request_start = Instant::now();
                
                // Build request with proper method and body
                let mut req_builder = client_clone.request(method.clone(), &url)
                    .headers(headers);
                
                // Add body for POST/PUT if specified
                if (method == Method::POST || method == Method::PUT) && custom_body.is_some() {
                    let body = custom_body.as_ref().unwrap();
                    req_builder = req_builder.body(body.clone());
                    stats_clone.bytes_sent.fetch_add(body.len() as u64, Ordering::Relaxed);
                }
                
                // Track request count
                stats_clone.requests.fetch_add(1, Ordering::Relaxed);
                
                // Execute request with retry logic
                let mut retry_count = 0;
                let mut _success = false;
                
                while retry_count <= MAX_RETRIES {
                    match req_builder.try_clone().unwrap().send().await {
                        Ok(response) => {
                            // Track success
                            stats_clone.successes.fetch_add(1, Ordering::Relaxed);
                            
                            // Try to read response body into buffer
                            match response.bytes().await {
                                Ok(bytes) => {
                                    stats_clone.bytes_received.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                                },
                                Err(_) => {
                                    // Failed to read body, but request succeeded
                                }
                            }
                            
                            // Reset backoff on success
                            backoff_ms = 10;
                            _success = true;
                            break;
                        },
                        Err(e) => {
                            if e.is_timeout() {
                                stats_clone.timeouts.fetch_add(1, Ordering::Relaxed);
                            } else if e.is_connect() {
                                stats_clone.conn_errors.fetch_add(1, Ordering::Relaxed);
                            } else {
                                stats_clone.errors.fetch_add(1, Ordering::Relaxed);
                            }
                            
                            retry_count += 1;
                            
                            // Only retry if we haven't hit max retries and stop flag is not set
                            if retry_count <= MAX_RETRIES && !stop_flag_clone.load(Ordering::SeqCst) {
                                // Apply exponential backoff with jitter
                                let jitter = rand::thread_rng().gen_range(0..=backoff_ms / 4);
                                let sleep_ms = backoff_ms.saturating_add(jitter);
                                
                                if worker_id % 10 == 0 {  // Log only from some workers to reduce noise
                                    let _ = log_tx_clone.send(format!(
                                        "⚠️ Retry {} for request: {} ({}ms backoff) - Error: {}", 
                                        retry_count, url, sleep_ms, e
                                    ));
                                }
                                
                                sleep(Duration::from_millis(sleep_ms)).await;
                                
                                // Exponential backoff with cap
                                backoff_ms = (backoff_ms * 2).min(max_backoff_ms);
                            } else {
                                break;
                            }
                        }
                    }
                }
                
                // Measure and record latency
                let latency_ms = request_start.elapsed().as_millis() as u64;
                stats_clone.update_latency(latency_ms);
                
                // Return buffer to pool
                buffer_pool_clone.return_buffer(buffer);
                
                // Release concurrency permit
                drop(permit);
                
                // Small yield to allow other tasks to run
                if worker_id % 10 == 0 {
                    tokio::task::yield_now().await;
                }
            }
        });
        
        workers.push(worker);
    }
    
    // Wait for stop signal before waiting for workers
    let log_tx_clone = log_tx.clone();
    let job_state_clone = Arc::clone(&job_state);
    
    // Wait for all workers to complete
    tokio::spawn(async move {
        // Wait for job to enter draining state
        loop {
            let state = {
                let state = job_state_clone.lock().unwrap();
                (*state).clone()
            };
            
            if state == JobState::Draining {
                break;
            }
            
            sleep(Duration::from_millis(100)).await;
        }
        
        let _ = log_tx_clone.send("🔄 Job draining - waiting for workers to complete...".to_string());
        
        // Wait for all workers to complete
        for (i, worker) in workers.into_iter().enumerate() {
            if i % 100 == 0 || i == 0 || i + 1 == job.concurrency as usize {
                let _ = log_tx_clone.send(format!("⏳ Waiting for worker {}/{} to complete", i + 1, job.concurrency));
            }
            let _ = worker.await;
        }
        
        let _ = log_tx_clone.send("✅ All workers stopped, job complete".to_string());
        
        // Set job state to stopped
        *job_state_clone.lock().unwrap() = JobState::Stopped;
    });
}

// Create an optimized HTTP client
fn create_optimized_client(timeout_sec: u64) -> Client {
    let timeout = Duration::from_secs(timeout_sec);
    
    // Configure client with optimal settings for high-throughput benchmarking
    reqwest::Client::builder()
        .timeout(timeout)
        .pool_max_idle_per_host(HTTP_PIPELINE_DEPTH)
        .pool_idle_timeout(Some(Duration::from_secs(30)))
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .tcp_nodelay(true)  // Disable Nagle's algorithm for lower latency
        .connect_timeout(Duration::from_secs(5))
        .http2_keep_alive_interval(Some(Duration::from_secs(5)))
        .http2_keep_alive_timeout(Duration::from_secs(10))
        .http2_adaptive_window(true)
        .build()
        .unwrap()
}