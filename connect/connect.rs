use crate::types::{AppState, JobInfo, JobStatus};
use crate::libs::logs::print;
use std::fs;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use reqwest::Url;
use crate::connect::http::hammer_http_max_load;
use tokio::task::JoinHandle;
use crate::connect::json::{daemon_json, daemon_modal};


pub async fn handle_job(
    job: JobInfo,
    state: Arc<AppState>,
    log_tx: mpsc::Sender<String>, 
    mut stop_rx: broadcast::Receiver<String>,
    concurrency: usize,
) {
    print(&format!("Starting job {} with {} workers", job.id, concurrency), false);

    //let _ = log_tx.send(daemon_modal(&format!("Created Job {}", job.id)));
    let _ = log_tx.send(daemon_modal("New job Created!", &format!("A new Job with id {} created with {} workers", job.id, concurrency), Some("https://nadhi.dev"), None));

    // Parse URL to extract host, port, and TLS
    let url = Url::parse(&job.url).unwrap();
    let host = url.host_str().unwrap().to_string();
    let port = url.port().unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
    let use_tls = url.scheme() == "https";

    // Each worker handles the full number of requests
    let num_requests = job.concurrency as usize;
    let drain_response = true;

    // Spawn worker tasks
    let mut worker_handles: Vec<JoinHandle<()>> = Vec::new();
    
    for worker_id in 0..concurrency {
        let worker_host = host.clone();
        let worker_log_tx = log_tx.clone();
        let worker_job_id = job.id.clone();
        
        let handle = tokio::spawn(async move {
            print(&format!("Worker {} starting for job {}", worker_id, worker_job_id), false);
            crate::connect::broadcast::send_update("update", &format!("Worker {} starting for job {}", worker_id, worker_job_id))
                .unwrap_or_else(|_| 0);

            // 🔨 
            hammer_http_max_load(
                &worker_host, 
                port, 
                num_requests, // Each worker handles ALL requests (Very stressful)
                drain_response, 
                use_tls, 
                true, 
                concurrency
            );
            
            print(&format!("Worker {} completed for job {}", worker_id, worker_job_id), false);
            crate::connect::broadcast::send_update("update", &format!("Worker {} completed for job {}", worker_id, worker_job_id))
                .unwrap_or_else(|_| 0);
        });
        
        worker_handles.push(handle);
    }

    // Monitor for stop signals while workers are running
    let stop_file = format!("/tmp/enidu_stop_{}", job.id);
    let mut workers_completed = false;
    
    loop {
        tokio::select! {
            // Check for stop signal
            Ok(_) = stop_rx.recv() => {
                print(&format!("Stop signal received for job {}", job.id), false);
                let _ = log_tx.send(format!("Stop signal received for job {}", job.id));
                
                // Abort all worker tasks
                for handle in &mut worker_handles {
                    handle.abort();
                }
                workers_completed = true;
                break;
            }
            
            // Check for stop file
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                if fs::metadata(&stop_file).is_ok() {
                    print(&format!("Stop file detected for job {}", job.id), false);
                    let _ = log_tx.send(format!("Stop file detected for job {}", job.id));
                    
                    // Abort all worker tasks
                    for handle in &mut worker_handles {
                        handle.abort();
                    }
                    workers_completed = true;
                    break;
                }
                
                // Check if all workers are done
                let all_done = worker_handles.iter().all(|h| h.is_finished());
                if all_done {
                    workers_completed = true;
                    break;
                }
            }
        }
    }

    // If workers completed naturally, wait for them to finish properly
    if workers_completed && !worker_handles.is_empty() {
        // Only wait if they weren't aborted
        let first_handle_aborted = worker_handles.first().map_or(true, |h| h.is_finished());
        if !first_handle_aborted {
            for handle in worker_handles {
                let _ = handle.await;
            }
        }
    }

    // Update job status to completed or stopped
    {
        let mut jobs = state.jobs.lock().unwrap();
        if let Some(j) = jobs.get_mut(&job.id) {
            j.status = JobStatus::Stopping;
        }
    }

    // Clean up stop file
    let _ = fs::remove_file(&stop_file);

    print(&format!("Job {} completed", job.id), false);
    crate::connect::broadcast::send_update("update", &format!("Job {} completed", job.id))
        .unwrap_or_else(|_| 0);
}

pub fn stop_job(state: Arc<AppState>, id: &str) {
    print(&format!("Stopping job {}", id), false);

   

    crate::connect::create_stop_files(&state);

     crate::env::shutdown::stop_everything(state.clone());

    // Create stop file
    print("MAKING STOP FILES DISK", true);
    let stop_file = format!("/tmp/enidu_stop_{}", id);
    fs::write(&stop_file, "stop").ok();

    // Update job status
    {
        let mut jobs = state.jobs.lock().unwrap();
        if let Some(j) = jobs.get_mut(id) {
            j.status = JobStatus::Complete;
        }
    }

    // Send stop signal
    if let Some(stop_tx) = state.stop_channels.lock().unwrap().get(id) {
        let _ = stop_tx.send("stop".to_string());
    }
}

pub fn create_stop_files(state: &AppState) {
    print("[STOP FILES] Creating stop files for all jobs", true);
    let jobs = state.jobs.lock().unwrap();
    for id in jobs.keys() {
        let stop_file = format!("/tmp/enidu_stop_{}", id);
        // Spawn a tokio task to create the stop file asynchronously
        let stop_file = stop_file.clone();
        tokio::spawn(async move {
            let _ = tokio::fs::write(&stop_file, "stop").await;
        });
    }
}