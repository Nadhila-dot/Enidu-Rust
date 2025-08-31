// Magic happens yuh

use crate::types::{AppState, JobInfo, JobStatus};
use crate::libs::logs::print;
use std::fs;
use std::sync::Arc;
use tokio::sync::broadcast;
use reqwest::Url;
use crate::connect::http::hammer_http_max_load;

pub async fn handle_job(
    job: JobInfo,
     state: Arc<AppState>,
    log_tx: broadcast::Sender<String>,
    mut stop_rx: broadcast::Receiver<String>,
    concurrency: usize,
) {
    print(&format!("Starting job {}", job.id), false);
    let _ = log_tx.send(format!("Starting job {}", job.id));

    // Parse URL to extract host, port, and TLS
    let url = Url::parse(&job.url).unwrap();
    let host = url.host_str().unwrap().to_string();
    let port = url.port().unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
    let use_tls = url.scheme() == "https";

    // Use concurrency as num_requests
    let num_requests = job.concurrency as usize;
    let drain_response = true;

    // Call the hammering function
    hammer_http_max_load(&host, port, num_requests, drain_response, use_tls, true, concurrency);

    // Listen for stop signals or check stop file
    let stop_file = format!("/tmp/enidu_stop_{}", job.id);
    loop {
        tokio::select! {
            Ok(_) = stop_rx.recv() => {
                print(&format!("Stop signal received for job {}", job.id), false);
                let _ = log_tx.send(format!("Stop signal received for job {}", job.id));
                break;
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                if fs::metadata(&stop_file).is_ok() {
                    print(&format!("Stop file detected for job {}", job.id), false);
                    let _ = log_tx.send(format!("Stop file detected for job {}", job.id));
                    break;
                }
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

    print(&format!("Job {} completed", job.id), false);
    let _ = log_tx.send(format!("Job {} completed", job.id));
}

pub fn stop_job(state: &AppState, id: &str) {
    print(&format!("Stopping job {}", id), false);

    // Create stop file
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
    let jobs = state.jobs.lock().unwrap();
    for id in jobs.keys() {
        let stop_file = format!("/tmp/enidu_stop_{}", id);
        fs::write(&stop_file, "stop").ok();
    }
}