use crate::{env::container::clear_cache, env::config::{get_threads, get_http_threads, get_auto_tune}, libs::logs::print};
use tokio::runtime::Builder;
use tokio::sync::mpsc;
use std::sync::Arc;

mod env;
mod router;
mod libs;
mod connect;
pub mod types;
mod scale;
use rustls::crypto::CryptoProvider;

fn main() {
    let (num_threads, http_threads) = configure_threads();

  

    rustls::crypto::ring::default_provider()
    .install_default()
    .unwrap();

    print(&format!("[SERVER] Tokio starting (WEBSERVER) with worker {} threads", num_threads), false);
    print(&format!("[SERVER] Tokio starting (HTTP) with worker {} threads", http_threads), false);
    print(&format!("[SERVER] Attempting to clear cache"), false);
    clear_cache().unwrap();
    // Archive old logs cuz it sucks to have old logs
    // The log folder will always have the latest log
    libs::logs::archive_old_logs();

    let port = env::checkport::get_container_port();

    print(&format!("[SERVER] Listening on port: {}", port), false);

    

    // Create channel for job requests
    // We will listen to it
    let (job_tx, job_rx) = mpsc::channel::<crate::types::JobInfo>(100);
 
    let state = crate::types::AppState {
        jobs: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        log_channels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        stop_channels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        job_tasks: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        job_sender: job_tx,
    };

   
    let jobs_runtime = Arc::new(
        Builder::new_multi_thread()
            .worker_threads(http_threads)
            .enable_time()
            .enable_io()
            .build()
            .unwrap()
    );

    // Clone for the job listener
    let state_clone = Arc::new(state.clone());
    let jobs_runtime_clone = Arc::clone(&jobs_runtime);

    // Build main runtime with dynamic worker threads
    let rt = Builder::new_multi_thread()
        .worker_threads(num_threads)
        .enable_time()
        .enable_io()
        .build()
        .unwrap();

    // Run the async main function on the runtime
    rt.block_on(async_main(state, port, job_rx, state_clone, jobs_runtime_clone));
}

fn configure_threads() -> (usize, usize) {
    if get_auto_tune() {
        print("[AUTO-TUNE] Running on Auto tune mode.", false);
        // Use a temporary runtime to await the async function
        let rt = tokio::runtime::Runtime::new().unwrap();
        let http_threads = rt.block_on(scale::threads::auto_tune_threads());
        let num_threads = (http_threads as f64 * 0.5).ceil() as usize;
        (num_threads, http_threads)
    } else {
        print("[AUTO-TUNE] Running on manual mode using .env", false);
        let num_threads = get_threads().unwrap_or(4) as usize;
        let http_threads = get_http_threads().unwrap_or(8) as usize;
        (num_threads, http_threads)
    }
}

async fn async_main(
    state: crate::types::AppState,
    port: String,
    job_rx: mpsc::Receiver<crate::types::JobInfo>,
    state_clone: Arc<crate::types::AppState>,
    jobs_runtime_clone: Arc<tokio::runtime::Runtime>,
) {
    tokio::spawn(async move {
        let mut job_rx = job_rx;
        while let Some(job_info) = job_rx.recv().await {
            let job_id = job_info.id.clone();
            let state_clone_inner = Arc::clone(&state_clone);

            // Extract log_tx (mpsc sender) from the tuple
            let log_tx = {
                let log_channels = state_clone_inner.log_channels.lock().unwrap();
                match log_channels.get(&job_id) {
                    Some((tx, _)) => tx.clone(),  // Extract sender from tuple
                    None => {
                        print(&format!("Error: No log channel for job: {}", job_id), true);
                        continue;
                    }
                }
            };
            
            let stop_rx = {
                let stop_channels = state_clone_inner.stop_channels.lock().unwrap();
                match stop_channels.get(&job_id) {
                    Some(tx) => tx.subscribe(),
                    None => {
                        print(&format!("Error: No stop channel for job: {}", job_id), true);
                        continue;
                    }
                }
            };

            let handle = jobs_runtime_clone.spawn(async move {
                crate::connect::handle_job(job_info.clone(), state_clone_inner, log_tx, stop_rx, job_info.concurrency as usize).await;
            });
            
            // Store the handle in job_tasks
            let mut job_tasks = state_clone.job_tasks.lock().unwrap();
            job_tasks.insert(job_id, handle);
        }
    });

    print(&format!("[SERVER] is running on http://localhost:{}", port), false);
      let _broadcast_sender = connect::broadcast::init(); // initialize broadcast sender
    router::router::start_web_server(&port, state).await; // Pass state to webserver
}