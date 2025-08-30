use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct QuicMessage {
    pub r#event: String,  // Use r#event to avoid keyword conflict
    pub message: String,
}

/// Creates a JSON string for a QUIC message with the specified event and message.
/// Example: daemon_json("stop", "<message>")
/// Returns: {"event":"stop","message":"<message>"}
pub fn daemon_json(action_type: &str, msg: &str) -> String {
    let quic_msg = QuicMessage {
        r#event: action_type.to_string(),
        message: msg.to_string(),
    };
    serde_json::to_string(&quic_msg).unwrap()
}

/// Parses a JSON string back into a QuicMessage struct.
/// Useful for receiving and deserializing QUIC messages.
pub fn parse_daemon_json(json_str: &str) -> Result<QuicMessage, serde_json::Error> {
    serde_json::from_str(json_str)
}