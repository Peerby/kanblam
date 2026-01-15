//! JSON-RPC 2.0 protocol types for Rust <-> TypeScript sidecar communication

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// JSON-RPC 2.0 Request
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: &'static str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method,
            params,
        }
    }
}

/// JSON-RPC 2.0 Response
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 Error
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 Notification (no id, no response expected)
#[derive(Debug, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

// Request parameter types

#[derive(Debug, Serialize)]
pub struct StartSessionParams {
    pub task_id: String,
    pub worktree_path: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct ResumeSessionParams {
    pub task_id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SendPromptParams {
    pub task_id: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct StopSessionParams {
    pub task_id: String,
}

#[derive(Debug, Serialize)]
pub struct GetSessionParams {
    pub task_id: String,
}

// Response result types

#[derive(Debug, Deserialize)]
pub struct StartSessionResult {
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ResumeSessionResult {
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct GetSessionResult {
    pub session_id: String,
    pub is_active: bool,
}

// Session event types (notifications from sidecar)

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEventType {
    Started,
    Stopped,
    Ended,
    NeedsInput,
    Working,
    ToolUse,
    Output,
}

#[derive(Debug, Deserialize)]
pub struct SessionEventParams {
    pub task_id: String,
    pub event: SessionEventType,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
}

/// Parsed session event ready for use in app logic
#[derive(Debug, Clone)]
pub struct SidecarEvent {
    pub task_id: Uuid,
    pub event_type: SessionEventType,
    pub session_id: Option<String>,
    pub message: Option<String>,
    pub tool_name: Option<String>,
    pub output: Option<String>,
}

impl TryFrom<SessionEventParams> for SidecarEvent {
    type Error = uuid::Error;

    fn try_from(params: SessionEventParams) -> Result<Self, Self::Error> {
        Ok(Self {
            task_id: Uuid::parse_str(&params.task_id)?,
            event_type: params.event,
            session_id: params.session_id,
            message: params.message,
            tool_name: params.tool_name,
            output: params.output,
        })
    }
}

// Error codes matching TypeScript
pub mod error_codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
    pub const SESSION_NOT_FOUND: i32 = -32000;
    pub const SESSION_ALREADY_EXISTS: i32 = -32001;
    pub const SDK_ERROR: i32 = -32002;
}
