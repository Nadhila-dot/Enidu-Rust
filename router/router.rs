use axum::{
    extract::{Path, State},
    extract::ws::{WebSocketUpgrade, Message},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
    http::StatusCode,
};
use futures::TryFutureExt;
use serde::Deserialize;
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use uuid::Uuid;
use tokio::sync::broadcast;
use crate::{libs::logs::print, types::{AppState, JobInfo, JobStatus}};  
use futures_util::{StreamExt, SinkExt};

use crate::env::container::get_system_stats;
use crate::connect::{handle_job, stop_job as connector_stop_job, create_stop_files};
use crate::connect::json::{daemon_json, daemon_modal};
use crate::connect::broadcast::get_receiver;
use crate::env::shutdown::stop_everything;

#[derive(Deserialize)]
struct WsControlMsg {
    action: String,
    target: String,
}

#[derive(Deserialize)]
pub struct CreateJobRequest {
    url: String,
    proxy_addr: Option<String>,
    concurrency: Option<u32>,
    timeout_sec: Option<u64>,
    wait_ms: Option<u32>,
    random_headers: Option<bool>,
}

pub async fn start_web_server(port: &str, state: crate::types::AppState) -> Result<(), Box<dyn std::error::Error>> {
    // Remove: let state = AppState::default(); // Now passed in

    let app = Router::new()
        .route("/api/v1/", get(root))
        .route("/", get(index))
        .route("/api/v1/jobs", post(create_job).get(list_jobs).delete(stop_all_jobs))
        
        // best if u don't touch these mate
        .route("/api/v1/jobs/{id}", get(get_job).delete(stop_job))
        .route("/api/v1/ws/{id}", get(ws_handler))

        .with_state(state)
        .layer(CorsLayer::permissive());

    let addr: SocketAddr = format!("0.0.0.0:{}", port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    
    Ok(())
}

async fn root() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "200",
        "program": "Enidu-Rust",
        "version": "0.1.0"
    }))
}

async fn index() -> impl IntoResponse {
    // Use that container thing to get system stats
    // this is prob a heavy crate soo best if we cache this
    // Caching will help us js instantly send things that never change.
    let sys = get_system_stats();

    Json(serde_json::json!({
        "status": "200",
        "Container": {
            "total_memory": sys.total_memory(),
            "used_memory": sys.used_memory(),
            "total_swap": sys.total_swap(),
            "used_swap": sys.used_swap(),
            "total cores": sys.cpus().len(),
            "os": sysinfo::System::os_version()
        },
        "message": "Enidu-Rust Instance",
        "version": "0.1.0"

    }))
}

async fn create_job(
    State(state): State<crate::types::AppState>,
    Json(payload): Json<CreateJobRequest>,
) -> impl IntoResponse {
    if payload.url.is_empty() {
        print("Missing URL parameter", false);
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Missing URL parameter"})));
    }

    let id = Uuid::new_v4().to_string();
   let job = JobInfo {
        id: id.clone(),
        url: payload.url.clone(),  // Assuming url is still used; adjust if target_urls replaces it
        concurrency: payload.concurrency.unwrap_or(100),
        timeout_sec: payload.timeout_sec.unwrap_or(10),
        wait_ms: payload.wait_ms.unwrap_or(0),
        random_headers: payload.random_headers.unwrap_or(false),
        proxy_addr: payload.proxy_addr.clone(),
        status: JobStatus::Running,
        target_urls: vec![payload.url.clone()],  // Default: single URL in a vector
        custom_body: None,  // Default: no custom body
        method: reqwest::Method::GET.to_string(),  // Default: GET method
        disable_rate_limit: false,  // Default: rate limiting enabled
    };

    // Increase capacities for high-throughput logs/stops
    let (log_tx, log_rx) = mpsc::channel::<String>(10000);  // Use mpsc instead of broadcast
    let (stop_tx, _) = broadcast::channel::<String>(100);  // Keep broadcast for stop (unchanged)
    state.jobs.lock().unwrap().insert(id.clone(), job.clone());
    state.log_channels.lock().unwrap().insert(id.clone(), (log_tx.clone(), log_rx));  // Store sender and receiver as tuple
    state.stop_channels.lock().unwrap().insert(id.clone(), stop_tx.clone());

    // Offload to jobs runtime via channel (no direct spawn)
    let job_clone = job.clone();
    let _ = state.job_sender.send(job_clone).await; // Send to jobs runtime

    print(&format!("Job created {}", id), false);
        (StatusCode::OK, Json(serde_json::json!({
            "id": id,
            "wsUrl": format!("/api/v1/ws/{}", id),
            "status": "running",
            "info": job
        })))
    }

async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<JobInfo>, (StatusCode, Json<serde_json::Value>)> {
    let jobs = state.jobs.lock().unwrap();
    if let Some(job) = jobs.get(&id) {
        Ok(Json(job.clone()))
    } else {
        Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Job not found"}))))
    }
}

async fn list_jobs(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.jobs.lock().unwrap();
    Json(serde_json::json!({
        "jobs": jobs.values().collect::<Vec<_>>(),
        "count": jobs.len()
    }))
}

// Async handler for stopping jobs (fixed from previous version)
async fn stop_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Check if job exists
    let exists = {
        let jobs = state.jobs.lock().unwrap();
        jobs.contains_key(&id)
    };

    if exists {
        // Offload stop logic to a blocking task, return immediately
        let state_clone = state.clone();
        let id_clone = id.clone();
        tokio::spawn(async move {
            tokio::task::spawn_blocking(move || {
                connector_stop_job(state_clone.into(), &id_clone);
            }).await.ok();
        });

        (StatusCode::OK, Json(serde_json::json!({"status": "stopping", "id": id})))
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Job not found"})))
    }
}

async fn stop_all_jobs(State(state): State<AppState>) -> impl IntoResponse {
    let mut jobs = state.jobs.lock().unwrap();
    let count = jobs.len();
    let job_ids: Vec<String> = jobs.keys().cloned().collect();
    drop(jobs);

    // Offload all stops to background tasks
    for id in job_ids {
        let state_clone = state.clone();
        tokio::spawn(async move {
            tokio::task::spawn_blocking(move || {
                connector_stop_job(state_clone.into(), &id);
            }).await.ok();
        });
    }

    print(&format!("Stopping all jobs, count: {}", count), false);
    (StatusCode::OK, Json(serde_json::json!({
        "status": "stopping_all",
        "count": count
    })))
}

// WebSocket handler for real-time logs
async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, id, state))
}


// WebSocket handler implementation

use tokio::sync::mpsc;

async fn handle_socket(mut socket: axum::extract::ws::WebSocket, id: String, state: AppState) {
    // Retrieve the receiver for this job from state.log_channels
    let mut log_rx = {
        let mut log_channels = state.log_channels.lock().unwrap();
        match log_channels.remove(&id) {
            Some((_tx, rx)) => rx, // Only keep the Receiver part
            None => return,
        }
    };

    let (mut sender, mut receiver) = socket.split();

    // Channel for control feedback messages
    let mut broadcast_rx = get_receiver();
    let (feedback_tx, mut feedback_rx) = mpsc::unbounded_channel::<String>();
    // Channel for event messages
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<(String, String)>(); // (event_type, data)

    // Spawn a task to forward log, feedback, and event messages to the client
    let send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(msg) = log_rx.recv() => {
                    if sender.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
                Ok(broadcast_msg) = broadcast_rx.recv() => {
                    if sender.send(Message::Text(broadcast_msg.into())).await.is_err() {
                        break;
                    }
                }
                Some(feedback) = feedback_rx.recv() => {
                    if sender.send(Message::Text(feedback.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Listen for incoming control messages
    print("Listening for control messages", false);
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(txt) = msg {
            if let Ok(cmd) = serde_json::from_str::<WsControlMsg>(&txt) {
                if cmd.action == "stop" {
                    if cmd.target == "all" {
                        print("Stopping all jobs signal received from websocket", true);
                        // that daemon message
                        crate::connect::create_stop_files(&state);
                        let _ = feedback_tx.send(daemon_modal(
                            "Rail-line Stopping...",
                            "All jobs and workers have been forcefully stopped.",
                            Some("https://nadhi.dev"),
                            None,
                        ));
                        stop_everything(state.clone().into());
                        let job_ids: Vec<String> = {
                            let jobs = state.jobs.lock().unwrap();
                            jobs.keys().cloned().collect()
                        };
                        for jid in job_ids {
                            let state_clone = state.clone();
                            tokio::spawn(async move {
                                tokio::task::spawn_blocking(move || {
                                   // connector_stop_job(state_clone.clone().into(), &jid);
                                   // crate::connect::create_stop_files(&state_clone);
                                   crate::env::shutdown::stop_everything(state_clone.clone().into());
                                }).await.ok();
                            });
                        }
                        
                        // Werid ass message removed
                     //   let _ = feedback_tx.send("🛑 Panic Stop activated for all jobs".into());
                        let _ = feedback_tx.send(daemon_modal("Stopping Daemon", "Enidu instance is launching a stop sequence", Some("https://nadhi.dev"), None));
                        stop_everything(state.clone().into());
                        

                        
                        
                    } else {
                        print(&format!("Stopping specific job signal received from websocket for job id: {}", cmd.target), true);
                        let state_clone = state.clone();
                        let job_id = cmd.target.clone();
                        tokio::spawn(async move {
                            tokio::task::spawn_blocking(move || {
                                connector_stop_job(state_clone.into(), &job_id);
                            }).await.ok();
                        });
                    //    let _ = feedback_tx.send(format!("🛑 Stop signal sent for job {}", cmd.target));
                        let _ = feedback_tx.send(daemon_json("warn", &format!("Signaling job {} to stop..", cmd.target)));

                       
                        
                    }
                }
                
            }
         
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) {
                if let Some(event) = val.get("event").and_then(|e| e.as_str()) {
                    if let Some(data) = val.get("data").and_then(|d| d.as_str()) {
                        // Forward any event sent by client to the event channel
                        let _ = event_tx.send((event.to_string(), data.to_string()));
                    }
                }
            }
        }
    }

    // Ensure log forwarding task is stopped
    let _ = send_task.await;
}