
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc};

#[derive(Clone, Serialize, Deserialize)]
pub enum JobStatus {
    Running,
    Stopping,
    Complete,
    Error,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct JobInfo {
    pub id: String,
    pub url: String,
    pub concurrency: u32,
    pub timeout_sec: u64,
    pub wait_ms: u32,
    pub random_headers: bool,
    pub proxy_addr: Option<String>,
    pub target_urls: Vec<String>,
    pub custom_body: Option<String>,
    pub method: String,
    pub status: crate::types::JobStatus, // Add this line
    pub disable_rate_limit: bool,
}
#[derive(Clone)] // Removed Default derive
pub struct AppState {
    pub jobs: Arc<Mutex<HashMap<String, JobInfo>>>,
    pub log_channels: Arc<Mutex<HashMap<String, (mpsc::Sender<String>, mpsc::Receiver<String>)>>>,  // Store both sender and receiver
    pub stop_channels: Arc<Mutex<HashMap<String, broadcast::Sender<String>>>>,
    pub job_tasks: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    pub job_sender: tokio::sync::mpsc::Sender<JobInfo>,
}