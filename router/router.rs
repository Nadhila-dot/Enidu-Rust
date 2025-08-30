use axum::{
    extract::{Path, State},
    extract::ws::{WebSocketUpgrade, Message},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
    http::StatusCode,
};
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
    State(state): State<crate::types::AppState>, // Use the passed state
    Json(payload): Json<CreateJobRequest>,
) -> impl IntoResponse {
    if payload.url.is_empty() {
        print("Missing URL parameter", false);
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Missing URL parameter"})));
    }

    let id = Uuid::new_v4().to_string();
    let job = JobInfo {
        id: id.clone(),
        url: payload.url,
        concurrency: payload.concurrency.unwrap_or(100),
        timeout_sec: payload.timeout_sec.unwrap_or(10),
        wait_ms: payload.wait_ms.unwrap_or(0),
        random_headers: payload.random_headers.unwrap_or(false),
        proxy_addr: payload.proxy_addr,
        status: JobStatus::Running,
    };

    // Increase capacities for high-throughput logs/stops
    let (log_tx, _) = broadcast::channel::<String>(10000); // Increased from 1000
    let (stop_tx, _) = broadcast::channel::<String>(100); // Increased from 10
    state.jobs.lock().unwrap().insert(id.clone(), job.clone());
    state.log_channels.lock().unwrap().insert(id.clone(), log_tx.clone());
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
                connector_stop_job(&state_clone, &id_clone);
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
                connector_stop_job(&state_clone, &id);
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
    let log_rx = {
        let log_channels = state.log_channels.lock().unwrap();
        match log_channels.get(&id) {
            Some(tx) => tx.subscribe(),
            None => return,
        }
    };

    let (mut sender, mut receiver) = socket.split();
    let mut log_rx = log_rx;

    // Channel for control feedback messages
    let (feedback_tx, mut feedback_rx) = mpsc::unbounded_channel::<String>();

    // Spawn a task to forward log and feedback messages to the client
    let send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                Ok(msg) = log_rx.recv() => {
                    let json_msg = serde_json::json!({
                        "type": "log",
                        "data": msg,
                        "ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
                    });
                    if sender.send(Message::Text(json_msg.to_string().into())).await.is_err() {
                        break;
                    }
                }
                Some(feedback) = feedback_rx.recv() => {
                    let json_msg = serde_json::json!({
                        "type": "control",
                        "data": feedback,
                        "ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
                    });
                    if sender.send(Message::Text(json_msg.to_string().into())).await.is_err() {
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
                        print("Stopping all jobs signal received from websocket", false);
                        let job_ids: Vec<String> = {
                            let jobs = state.jobs.lock().unwrap();
                            jobs.keys().cloned().collect()
                        };
                        for jid in job_ids {
                            let state_clone = state.clone();
                            tokio::spawn(async move {
                                tokio::task::spawn_blocking(move || {
                                    connector_stop_job(&state_clone, &jid);
                                }).await.ok();
                            });
                        }
                        // Send feedback via channel
                        let _ = feedback_tx.send("🛑 Panic Stop activated for all jobs".into());
                    } else {
                        print(&format!("Stopping specific job signal received from websocket for job id: {}", cmd.target), false);
                        let state_clone = state.clone();
                        let job_id = cmd.target.clone();
                        tokio::spawn(async move {
                            tokio::task::spawn_blocking(move || {
                                connector_stop_job(&state_clone, &job_id);
                            }).await.ok();
                        });
                        let _ = feedback_tx.send(format!("🛑 Stop signal sent for job {}", cmd.target));
                    }
                }
            }
        }
    }

    // Ensure log forwarding task is stopped
    let _ = send_task.await;
}