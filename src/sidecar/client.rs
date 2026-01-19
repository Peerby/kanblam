//! Unix socket client for communicating with the TypeScript sidecar

#![allow(dead_code)]

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

    /// Start a new Claude session using a fresh connection (for use from background threads)
    /// This avoids contention on the main client's connection
    pub fn start_session_standalone(
        task_id: uuid::Uuid,
        worktree_path: PathBuf,
        prompt: String,
        images: Option<Vec<String>>,
    ) -> Result<String> {
        // Create a dedicated connection for this request
        let client = Self::connect()?;
        client.start_session(task_id, &worktree_path, &prompt, images)
    }

    /// Resume an existing session
    pub fn resume_session(
        &self,
        task_id: uuid::Uuid,
        session_id: &str,
        worktree_path: &std::path::PathBuf,
        prompt: Option<&str>,
    ) -> Result<String> {
        let params = ResumeSessionParams {
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            worktree_path: worktree_path.to_string_lossy().to_string(),
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

    /// Request a short title summary for a task description
    pub fn summarize_title(&self, task_id: uuid::Uuid, title: &str) -> Result<String> {
        let params = SummarizeTitleParams {
            task_id: task_id.to_string(),
            title: title.to_string(),
        };

        let response = self.send_request("summarize_title", Some(serde_json::to_value(params)?))?;

        if let Some(error) = response.error {
            return Err(anyhow!("Sidecar error: {} (code {})", error.message, error.code));
        }

        let result: SummarizeTitleResult = serde_json::from_value(
            response.result.ok_or_else(|| anyhow!("No result in response"))?,
        )?;

        Ok(result.short_title)
    }

    /// Request a short title summary using a standalone connection (for background threads)
    pub fn summarize_title_standalone(task_id: uuid::Uuid, title: String) -> Result<String> {
        let client = Self::connect()?;
        client.summarize_title(task_id, &title)
    }

    /// Start the watcher for a project
    pub fn start_watcher(&self, project_path: &std::path::PathBuf, interval_minutes: Option<u32>) -> Result<()> {
        let params = StartWatcherParams {
            project_path: project_path.to_string_lossy().to_string(),
            interval_minutes,
        };

        let response = self.send_request("start_watcher", Some(serde_json::to_value(params)?))?;

        if let Some(error) = response.error {
            return Err(anyhow!("Sidecar error: {} (code {})", error.message, error.code));
        }

        Ok(())
    }

    /// Stop the watcher for a project
    pub fn stop_watcher(&self, project_path: &std::path::PathBuf) -> Result<()> {
        let params = StopWatcherParams {
            project_path: project_path.to_string_lossy().to_string(),
        };

        let response = self.send_request("stop_watcher", Some(serde_json::to_value(params)?))?;

        if let Some(error) = response.error {
            return Err(anyhow!("Sidecar error: {} (code {})", error.message, error.code));
        }

        Ok(())
    }

    /// Trigger an immediate watcher observation (for testing)
    pub fn trigger_watcher(&self, project_path: &std::path::PathBuf) -> Result<()> {
        let params = StopWatcherParams {
            project_path: project_path.to_string_lossy().to_string(),
        };

        let response = self.send_request("trigger_watcher", Some(serde_json::to_value(params)?))?;

        if let Some(error) = response.error {
            return Err(anyhow!("Sidecar error: {} (code {})", error.message, error.code));
        }

        Ok(())
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

        // Read responses, skipping notifications until we get our response
        let mut reader = BufReader::new(&*stream);
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;

            if line.trim().is_empty() {
                continue;
            }

            // Try to parse as a generic JSON value first to check if it's a notification
            let json_value: serde_json::Value = serde_json::from_str(&line)?;

            // Notifications have "method" but no "id"
            // Responses have "id"
            if json_value.get("id").is_some() {
                // This is a response, parse it properly
                let response: JsonRpcResponse = serde_json::from_value(json_value)?;

                // Verify response ID matches
                if response.id != id {
                    // Not our response, could be from a different request - skip it
                    // (This shouldn't happen in practice with our single-threaded usage)
                    continue;
                }

                return Ok(response);
            }
            // If no "id", it's a notification - skip it and keep reading
        }
    }
}

/// Types of notifications from the sidecar
#[derive(Debug)]
pub enum SidecarNotification {
    /// Session event (started, stopped, working, etc.)
    SessionEvent(SidecarEvent),
    /// Watcher comment from the background observer
    WatcherComment(WatcherComment),
    /// Watcher observation status (when Claude SDK starts/stops running)
    WatcherObserving(WatcherObserving),
}

/// Event receiver for async notifications from sidecar
pub struct SidecarEventReceiver {
    reader: BufReader<UnixStream>,
}

impl SidecarEventReceiver {
    /// Create a new event receiver (separate connection for notifications)
    pub fn connect() -> Result<Self> {
        let path = socket_path();
        let stream = UnixStream::connect(&path)
            .with_context(|| format!("Failed to connect to sidecar at {:?}", path))?;

        Ok(Self {
            reader: BufReader::new(stream),
        })
    }

    /// Read the next notification (blocking) - can be session event or watcher comment
    pub fn recv_notification(&mut self) -> Result<SidecarNotification> {
        let mut line = String::new();
        self.reader.read_line(&mut line)?;

        let notification: JsonRpcNotification = serde_json::from_str(&line)?;

        match notification.method.as_str() {
            "session_event" => {
                let params: SessionEventParams = serde_json::from_value(
                    notification.params.ok_or_else(|| anyhow!("No params in notification"))?,
                )?;
                let event = params.try_into().map_err(|e| anyhow!("Invalid task_id: {}", e))?;
                Ok(SidecarNotification::SessionEvent(event))
            }
            "watcher_comment" => {
                let params: WatcherCommentParams = serde_json::from_value(
                    notification.params.ok_or_else(|| anyhow!("No params in notification"))?,
                )?;
                let comment = params.try_into().map_err(|e| anyhow!("Invalid watcher comment: {}", e))?;
                Ok(SidecarNotification::WatcherComment(comment))
            }
            "watcher_observing" => {
                let params: WatcherObservingParams = serde_json::from_value(
                    notification.params.ok_or_else(|| anyhow!("No params in notification"))?,
                )?;
                Ok(SidecarNotification::WatcherObserving(params.into()))
            }
            _ => Err(anyhow!("Unexpected notification method: {}", notification.method)),
        }
    }

    /// Read the next event (blocking) - for backwards compatibility, ignores watcher notifications
    pub fn recv(&mut self) -> Result<SidecarEvent> {
        loop {
            match self.recv_notification()? {
                SidecarNotification::SessionEvent(event) => return Ok(event),
                SidecarNotification::WatcherComment(_) => continue, // Skip watcher comments
                SidecarNotification::WatcherObserving(_) => continue, // Skip observing status
            }
        }
    }

    /// Try to read a notification with timeout
    pub fn try_recv_notification(&mut self, timeout: Duration) -> Result<Option<SidecarNotification>> {
        // Use minimum 1ms timeout to avoid issues with zero timeout
        let actual_timeout = if timeout.is_zero() {
            Duration::from_millis(1)
        } else {
            timeout
        };
        self.reader.get_ref().set_read_timeout(Some(actual_timeout))?;

        match self.recv_notification() {
            Ok(notif) => Ok(Some(notif)),
            Err(e) => {
                let err_str = e.to_string().to_lowercase();
                // Check if it was a timeout or would-block error
                if err_str.contains("timed out")
                    || err_str.contains("would block")
                    || err_str.contains("resource temporarily unavailable")
                {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Try to read an event with timeout - for backwards compatibility
    pub fn try_recv(&mut self, timeout: Duration) -> Result<Option<SidecarEvent>> {
        // Use minimum 1ms timeout to avoid issues with zero timeout
        let actual_timeout = if timeout.is_zero() {
            Duration::from_millis(1)
        } else {
            timeout
        };
        self.reader.get_ref().set_read_timeout(Some(actual_timeout))?;

        match self.recv() {
            Ok(event) => Ok(Some(event)),
            Err(e) => {
                let err_str = e.to_string().to_lowercase();
                // Check if it was a timeout or would-block error
                if err_str.contains("timed out")
                    || err_str.contains("would block")
                    || err_str.contains("resource temporarily unavailable")
                {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }
}

/// Find the sidecar main.cjs path
fn find_sidecar_path() -> Option<std::path::PathBuf> {
    // Try production path first (next to executable)
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let prod_path = exe_dir.join("sidecar").join("dist").join("main.cjs");
            if prod_path.exists() {
                return Some(prod_path);
            }
        }
    }

    // Try development path (relative to Cargo manifest)
    // During cargo build/run, CARGO_MANIFEST_DIR is set
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let dev_path = std::path::PathBuf::from(&manifest_dir)
            .join("sidecar")
            .join("dist")
            .join("main.cjs");
        if dev_path.exists() {
            return Some(dev_path);
        }
    }

    // Try walking up from executable to find sidecar
    if let Ok(exe_path) = std::env::current_exe() {
        let mut dir = exe_path.parent();
        while let Some(parent) = dir {
            let candidate = parent.join("sidecar").join("dist").join("main.cjs");
            if candidate.exists() {
                return Some(candidate);
            }
            dir = parent.parent();
        }
    }

    None
}

/// Spawn the sidecar process if not already running
/// Returns the Child handle if we spawned a new process (caller should kill on exit)
/// Returns None if sidecar was already running
pub fn ensure_sidecar_running() -> Result<Option<std::process::Child>> {
    if SidecarClient::is_available() {
        // Try to ping to verify it's actually responding
        if let Ok(client) = SidecarClient::connect() {
            if client.ping().is_ok() {
                return Ok(None); // Already running, no child to track
            }
        }
    }

    // Find the sidecar
    let sidecar_path = find_sidecar_path()
        .ok_or_else(|| anyhow!("Sidecar not found. Looked in exe dir, CARGO_MANIFEST_DIR, and parent directories."))?;

    // Spawn node process in background
    let child = std::process::Command::new("node")
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
                    return Ok(Some(child)); // Return handle so caller can kill on exit
                }
            }
        }
    }

    Err(anyhow!("Sidecar failed to start within timeout"))
}
