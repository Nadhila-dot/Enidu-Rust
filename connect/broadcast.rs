use std::sync::{Once, Mutex};
use tokio::sync::broadcast::{self, Sender, Receiver};
use crate::connect::json::daemon_json;
use crate::libs::logs::print;

static INIT: Once = Once::new();
static mut BROADCAST: Option<Sender<String>> = None;
const CAP: usize = 32; 

pub fn init() {
    unsafe {
        INIT.call_once(|| {
            let (tx, _) = broadcast::channel(CAP);
            BROADCAST = Some(tx);
            print("[BROADCAST] Lightweight system initialized", false);
        });
    }
}

pub fn get_sender() -> Sender<String> {
    unsafe {
        if BROADCAST.is_none() {
            init();
        }
        BROADCAST.as_ref().unwrap().clone()
    }
}

pub fn get_receiver() -> Receiver<String> {
    get_sender().subscribe()
}

pub fn send_message(message: String) -> Result<usize, ()> {
    let tx = get_sender();
    let _ = tx.send(message.clone());
    print(&format!("[BROADCAST] SEND {}", message), false);
    Ok(1)
}

pub fn send_update(event_type: &str, message: &str) -> Result<usize, ()> {
    let msg = daemon_json(event_type, message);
    let tx = get_sender();
    let _ = tx.send(msg.clone());
    print(&format!("[BROADCAST] Lightweight UPDATE {}", msg), false);
    Ok(1)
}