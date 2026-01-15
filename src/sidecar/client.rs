//! Unix socket client for communicating with the TypeScript sidecar

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use super::protocol::*;

/// Path to the sidecar socket
fn socket_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".kanclaude")
        .join("sidecar.sock")
}

/// Client for communicating with the sidecar
pub struct SidecarClient {
    stream: Arc<Mutex<UnixStream>>,
    request_id: AtomicU64,
}

impl SidecarClient {
    /// Connect to the sidecar
    pub fn connect() -> Result<Self> {
        let path = socket_path();
        let stream = UnixStream::connect(&path)
            .with_context(|| format!("Failed to connect to sidecar at {:?}", path))?;

        // Set read timeout for responses
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;

        Ok(Self {
            stream: Arc::new(Mutex::new(stream)),
            request_id: AtomicU64::new(1),
        })
    }

    /// Check if sidecar is available
    pub fn is_available() -> bool {
        socket_path().exists()
    }

    /// Send a ping to verify connection
    pub fn ping(&self) -> Result<bool> {
        let response = self.send_request("ping", None)?;
        Ok(response.result.is_some())
    }

    /// Start a new Claude session
    pub fn start_session(
        &self,
        task_id: uuid::Uuid,
        worktree_path: &PathBuf,
        prompt: &str,
        images: Option<Vec<String>>,
    ) -> Result<String> {
        let params = StartSessionParams {
            task_id: task_id.to_string(),
            worktree_path: worktree_path.to_string_lossy().to_string(),
            prompt: prompt.to_string(),
            images,
        };

        let response = self.send_request("start_session", Some(serde_json::to_value(params)?))?;

        if let Some(error) = response.error {
            return Err(anyhow!("Sidecar error: {} (code {})", error.message, error.code));
        }

        let result: StartSessionResult = serde_json::from_value(
            response.result.ok_or_else(|| anyhow!("No result in response"))?,
        )?;

        Ok(result.session_id)
    }

    /// Resume an existing session
    pub fn resume_session(
        &self,
        task_id: uuid::Uuid,
        session_id: &str,
        prompt: Option<&str>,
    ) -> Result<String> {
        let params = ResumeSessionParams {
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            prompt: prompt.map(|s| s.to_string()),
        };

        let response = self.send_request("resume_session", Some(serde_json::to_value(params)?))?;

        if let Some(error) = response.error {
            return Err(anyhow!("Sidecar error: {} (code {})", error.message, error.code));
        }

        let result: ResumeSessionResult = serde_json::from_value(
            response.result.ok_or_else(|| anyhow!("No result in response"))?,
        )?;

        Ok(result.session_id)
    }

    /// Send a prompt to an existing session
    pub fn send_prompt(
        &self,
        task_id: uuid::Uuid,
        prompt: &str,
        images: Option<Vec<String>>,
    ) -> Result<()> {
        let params = SendPromptParams {
            task_id: task_id.to_string(),
            prompt: prompt.to_string(),
            images,
        };

        let response = self.send_request("send_prompt", Some(serde_json::to_value(params)?))?;

        if let Some(error) = response.error {
            return Err(anyhow!("Sidecar error: {} (code {})", error.message, error.code));
        }

        Ok(())
    }

    /// Stop a session
    pub fn stop_session(&self, task_id: uuid::Uuid) -> Result<()> {
        let params = StopSessionParams {
            task_id: task_id.to_string(),
        };

        let response = self.send_request("stop_session", Some(serde_json::to_value(params)?))?;

        if let Some(error) = response.error {
            return Err(anyhow!("Sidecar error: {} (code {})", error.message, error.code));
        }

        Ok(())
    }

    /// Get session info
    pub fn get_session(&self, task_id: uuid::Uuid) -> Result<Option<GetSessionResult>> {
        let params = GetSessionParams {
            task_id: task_id.to_string(),
        };

        let response = self.send_request("get_session", Some(serde_json::to_value(params)?))?;

        if let Some(error) = response.error {
            if error.code == error_codes::SESSION_NOT_FOUND {
                return Ok(None);
            }
            return Err(anyhow!("Sidecar error: {} (code {})", error.message, error.code));
        }

        let result: GetSessionResult = serde_json::from_value(
            response.result.ok_or_else(|| anyhow!("No result in response"))?,
        )?;

        Ok(Some(result))
    }

    /// Send a request and wait for response
    fn send_request(
        &self,
        method: &'static str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);

        let mut stream = self.stream.lock().map_err(|_| anyhow!("Lock poisoned"))?;

        // Send request
        let request_json = serde_json::to_string(&request)?;
        writeln!(stream, "{}", request_json)?;
        stream.flush()?;

        // Read response
        let mut reader = BufReader::new(&*stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;

        let response: JsonRpcResponse = serde_json::from_str(&line)?;

        // Verify response ID matches
        if response.id != id {
            return Err(anyhow!(
                "Response ID mismatch: expected {}, got {}",
                id,
                response.id
            ));
        }

        Ok(response)
    }
}

/// Event receiver for async notifications from sidecar
pub struct SidecarEventReceiver {
    stream: UnixStream,
}

impl SidecarEventReceiver {
    /// Create a new event receiver (separate connection for notifications)
    pub fn connect() -> Result<Self> {
        let path = socket_path();
        let stream = UnixStream::connect(&path)
            .with_context(|| format!("Failed to connect to sidecar at {:?}", path))?;

        Ok(Self { stream })
    }

    /// Read the next event (blocking)
    pub fn recv(&mut self) -> Result<SidecarEvent> {
        let mut reader = BufReader::new(&self.stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;

        let notification: JsonRpcNotification = serde_json::from_str(&line)?;

        if notification.method != "session_event" {
            return Err(anyhow!("Unexpected notification method: {}", notification.method));
        }

        let params: SessionEventParams = serde_json::from_value(
            notification.params.ok_or_else(|| anyhow!("No params in notification"))?,
        )?;

        params.try_into().map_err(|e| anyhow!("Invalid task_id: {}", e))
    }

    /// Try to read an event with timeout
    pub fn try_recv(&mut self, timeout: Duration) -> Result<Option<SidecarEvent>> {
        self.stream.set_read_timeout(Some(timeout))?;

        match self.recv() {
            Ok(event) => Ok(Some(event)),
            Err(e) => {
                // Check if it was a timeout
                if e.to_string().contains("timed out") {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }
}

/// Spawn the sidecar process if not already running
pub fn ensure_sidecar_running() -> Result<()> {
    if SidecarClient::is_available() {
        // Try to ping to verify it's actually responding
        if let Ok(client) = SidecarClient::connect() {
            if client.ping().is_ok() {
                return Ok(());
            }
        }
    }

    // Start the sidecar
    let sidecar_path = std::env::current_exe()?
        .parent()
        .ok_or_else(|| anyhow!("Cannot determine executable directory"))?
        .join("sidecar")
        .join("dist")
        .join("main.js");

    if !sidecar_path.exists() {
        return Err(anyhow!("Sidecar not found at {:?}", sidecar_path));
    }

    // Spawn node process in background
    std::process::Command::new("node")
        .arg(&sidecar_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Failed to spawn sidecar process")?;

    // Wait for socket to become available
    for _ in 0..50 {
        thread::sleep(Duration::from_millis(100));
        if SidecarClient::is_available() {
            if let Ok(client) = SidecarClient::connect() {
                if client.ping().is_ok() {
                    return Ok(());
                }
            }
        }
    }

    Err(anyhow!("Sidecar failed to start within timeout"))
}
