//! JSON-RPC 2.0 protocol types for Rust <-> TypeScript sidecar communication

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
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
    pub worktree_path: String,
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

#[derive(Debug, Serialize)]
pub struct SummarizeTitleParams {
    pub task_id: String,
    pub title: String,
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

#[derive(Debug, Deserialize)]
pub struct SummarizeTitleResult {
    pub short_title: String,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_json_rpc_request_serialization() {
        let request = JsonRpcRequest::new(1, "test_method", Some(json!({"key": "value"})));
        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"test_method\""));
        assert!(json.contains("\"params\":{\"key\":\"value\"}"));
    }

    #[test]
    fn test_json_rpc_request_without_params() {
        let request = JsonRpcRequest::new(42, "ping", None);
        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"id\":42"));
        assert!(json.contains("\"method\":\"ping\""));
        assert!(!json.contains("params")); // params should be skipped
    }

    #[test]
    fn test_json_rpc_response_deserialization() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"session_id":"abc123"}}"#;
        let response: JsonRpcResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(response.id, 1);
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[test]
    fn test_json_rpc_error_response() {
        let json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"Session not found"}}"#;
        let response: JsonRpcResponse = serde_json::from_str(json).unwrap();

        assert!(response.result.is_none());
        assert!(response.error.is_some());

        let error = response.error.unwrap();
        assert_eq!(error.code, error_codes::SESSION_NOT_FOUND);
        assert_eq!(error.message, "Session not found");
    }

    #[test]
    fn test_json_rpc_notification_deserialization() {
        let json = r#"{"jsonrpc":"2.0","method":"session_event","params":{"task_id":"abc","event":"started"}}"#;
        let notification: JsonRpcNotification = serde_json::from_str(json).unwrap();

        assert_eq!(notification.method, "session_event");
        assert!(notification.params.is_some());
    }

    #[test]
    fn test_start_session_params_serialization() {
        let params = StartSessionParams {
            task_id: "task-123".to_string(),
            worktree_path: "/path/to/worktree".to_string(),
            prompt: "Implement feature X".to_string(),
            images: Some(vec!["/path/to/image.png".to_string()]),
        };

        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"task_id\":\"task-123\""));
        assert!(json.contains("\"worktree_path\":\"/path/to/worktree\""));
        assert!(json.contains("\"prompt\":\"Implement feature X\""));
        assert!(json.contains("\"images\":[\"/path/to/image.png\"]"));
    }

    #[test]
    fn test_start_session_params_without_images() {
        let params = StartSessionParams {
            task_id: "task-123".to_string(),
            worktree_path: "/path/to/worktree".to_string(),
            prompt: "Implement feature X".to_string(),
            images: None,
        };

        let json = serde_json::to_string(&params).unwrap();
        assert!(!json.contains("images")); // should be skipped
    }

    #[test]
    fn test_session_event_type_deserialization() {
        let test_cases = vec![
            ("\"started\"", SessionEventType::Started),
            ("\"stopped\"", SessionEventType::Stopped),
            ("\"ended\"", SessionEventType::Ended),
            ("\"needs_input\"", SessionEventType::NeedsInput),
            ("\"working\"", SessionEventType::Working),
            ("\"tool_use\"", SessionEventType::ToolUse),
            ("\"output\"", SessionEventType::Output),
        ];

        for (json, expected) in test_cases {
            let parsed: SessionEventType = serde_json::from_str(json).unwrap();
            assert_eq!(parsed, expected, "Failed for {}", json);
        }
    }

    #[test]
    fn test_session_event_params_full() {
        let json = r#"{
            "task_id": "550e8400-e29b-41d4-a716-446655440000",
            "event": "tool_use",
            "session_id": "session-abc",
            "message": "Using tool",
            "tool_name": "Read",
            "output": "File contents here"
        }"#;

        let params: SessionEventParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.task_id, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(params.event, SessionEventType::ToolUse);
        assert_eq!(params.session_id.as_deref(), Some("session-abc"));
        assert_eq!(params.tool_name.as_deref(), Some("Read"));
    }

    #[test]
    fn test_session_event_params_minimal() {
        let json = r#"{"task_id": "task-123", "event": "started"}"#;
        let params: SessionEventParams = serde_json::from_str(json).unwrap();

        assert_eq!(params.task_id, "task-123");
        assert_eq!(params.event, SessionEventType::Started);
        assert!(params.session_id.is_none());
        assert!(params.message.is_none());
    }

    #[test]
    fn test_sidecar_event_conversion() {
        let params = SessionEventParams {
            task_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            event: SessionEventType::Started,
            session_id: Some("session-123".to_string()),
            message: None,
            tool_name: None,
            output: None,
        };

        let event: SidecarEvent = params.try_into().unwrap();
        assert_eq!(event.task_id.to_string(), "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(event.event_type, SessionEventType::Started);
        assert_eq!(event.session_id.as_deref(), Some("session-123"));
    }

    #[test]
    fn test_sidecar_event_invalid_uuid() {
        let params = SessionEventParams {
            task_id: "not-a-valid-uuid".to_string(),
            event: SessionEventType::Started,
            session_id: None,
            message: None,
            tool_name: None,
            output: None,
        };

        let result: Result<SidecarEvent, _> = params.try_into();
        assert!(result.is_err());
    }

    #[test]
    fn test_resume_session_params() {
        let params = ResumeSessionParams {
            task_id: "task-123".to_string(),
            session_id: "session-456".to_string(),
            worktree_path: "/path/to/worktree".to_string(),
            prompt: Some("Continue working".to_string()),
        };

        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"session_id\":\"session-456\""));
        assert!(json.contains("\"worktree_path\":\"/path/to/worktree\""));
        assert!(json.contains("\"prompt\":\"Continue working\""));
    }

    #[test]
    fn test_get_session_result() {
        let json = r#"{"session_id": "sess-123", "is_active": true}"#;
        let result: GetSessionResult = serde_json::from_str(json).unwrap();

        assert_eq!(result.session_id, "sess-123");
        assert!(result.is_active);
    }
}
