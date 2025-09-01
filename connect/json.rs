use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct QuicMessage {
    pub r#event: String,  // Use r#event to avoid keyword conflict
    pub message: String,
}

// Daemon modal structs
#[derive(Serialize, Deserialize, Debug)]
pub struct ModalMessage {
    pub title: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inject: Option<String>,
}

/// Creates a JSON string for a daemon_json message with the specified event and message.
/// Now returns a wrapped JSON object for consistency.
/// Example: daemon_json("stop", "<message>")
/// Returns: {"type":"log","data":{"event":"stop","message":"<message>"},"ts":<timestamp>,"anti-skid":"Made by Nadhi.dev"}
pub fn daemon_json(action_type: &str, msg: &str) -> String {
    let quic_msg = QuicMessage {
        r#event: action_type.to_string(),
        message: msg.to_string(),
    };
    let data = serde_json::to_string(&quic_msg).unwrap();
    let wrapped = serde_json::json!({
        "type": "log",
        "data": data,
        "ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "anti-skid": "Made by Nadhi.dev"
    });
    serde_json::to_string(&wrapped).unwrap()
}

pub fn parse_daemon_json(json_str: &str) -> Result<QuicMessage, serde_json::Error> {
    serde_json::from_str(json_str)
}

/// Creates a JSON string for a daemon_modal message with the specified title, description, and optional href and inject.
/// Now returns a wrapped JSON object for consistency.
/// Example: daemon_modal("my modal's title", "My modal's got a cool description now mate", Some("additional href link button"), Some("custom html if we need to send any"))
/// Returns: {"type":"modal","data":{"title":"my modal's title","description":"My modal's got a cool description now mate","href":"additional href link button","inject":"custom html if we need to send any"},"ts":<timestamp>,"anti-skid":"Made by Nadhi.dev"}
pub fn daemon_modal(title: &str, description: &str, href: Option<&str>, inject: Option<&str>) -> String {
    let modal_msg = ModalMessage {
        title: title.to_string(),
        description: description.to_string(),
        href: href.map(|s| s.to_string()),
        inject: inject.map(|s| s.to_string()),
    };
    let data = serde_json::to_string(&modal_msg).unwrap();
    let wrapped = serde_json::json!({
        "type": "modal",
        "data": data,
        "ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "anti-skid": "Made by Nadhi.dev"
    });
    serde_json::to_string(&wrapped).unwrap()
}

pub fn parse_daemon_modal(json_str: &str) -> Result<ModalMessage, serde_json::Error> {
    serde_json::from_str(json_str)
}