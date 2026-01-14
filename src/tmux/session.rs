use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

/// Switch to a specific pane - handles both same-session and different-session cases
pub fn switch_to_session(pane_id: &str) -> Result<()> {
    // Get the session name for the target pane
    let output = Command::new("tmux")
        .args(["display-message", "-t", pane_id, "-p", "#{session_name}"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to get session name: {}", stderr));
    }

    let target_session = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Get current session name
    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .output()?;

    let current_session = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if target_session == current_session {
        // Same session - use select-window and select-pane
        let _ = Command::new("tmux")
            .args(["select-window", "-t", pane_id])
            .output();
        let _ = Command::new("tmux")
            .args(["select-pane", "-t", pane_id])
            .output();
    } else {
        // Different session - use switch-client
        let _ = Command::new("tmux")
            .args(["switch-client", "-t", &target_session])
            .output();
    }

    Ok(())
}

/// Represents a detected Claude Code session in tmux
#[derive(Debug, Clone)]
pub struct ClaudeSession {
    pub session_name: String,
    pub window_index: u32,
    pub pane_id: String,
    pub working_dir: PathBuf,
    pub current_command: String,
}

/// Detect all tmux panes that are running Claude Code
/// Uses a reliable method: find "claude" processes and match their TTY to tmux panes
pub fn detect_claude_sessions() -> Result<Vec<ClaudeSession>> {
    let mut sessions = Vec::new();

    // Step 1: Find all processes running "claude" command and get their TTY
    // ps output format: TTY and working directory
    let ps_output = Command::new("ps")
        .args(["-eo", "tty,command"])
        .output()?;

    if !ps_output.status.success() {
        return Ok(sessions);
    }

    // Build a map of TTY -> true for panes running claude
    let mut claude_ttys: HashMap<String, bool> = HashMap::new();
    let ps_stdout = String::from_utf8_lossy(&ps_output.stdout);

    for line in ps_stdout.lines() {
        let line = line.trim();
        // Look for lines where command starts with "claude" (the actual claude process)
        if line.contains(" claude") && !line.contains("grep") && !line.contains("kanclaude") {
            // Extract TTY (first column)
            if let Some(tty) = line.split_whitespace().next() {
                // Convert TTY format: "s006" -> "/dev/ttys006"
                let full_tty = if tty.starts_with("s") {
                    format!("/dev/tty{}", tty)
                } else if tty.starts_with("tty") {
                    format!("/dev/{}", tty)
                } else {
                    continue;
                };
                claude_ttys.insert(full_tty, true);
            }
        }
    }

    // Step 2: Get all tmux panes with their TTY
    let tmux_output = Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{pane_id}:#{pane_tty}:#{session_name}:#{window_index}:#{pane_current_path}",
        ])
        .output();

    let tmux_output = match tmux_output {
        Ok(o) => o,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Ok(sessions);
            }
            return Err(anyhow!("Failed to run tmux: {}", e));
        }
    };

    if !tmux_output.status.success() {
        return Ok(sessions);
    }

    // Step 3: Match tmux panes to claude processes by TTY
    let tmux_stdout = String::from_utf8_lossy(&tmux_output.stdout);

    for line in tmux_stdout.lines() {
        let parts: Vec<&str> = line.splitn(5, ':').collect();
        if parts.len() < 5 {
            continue;
        }

        let pane_id = parts[0].to_string();
        let pane_tty = parts[1].to_string();
        let session_name = parts[2].to_string();
        let window_index: u32 = parts[3].parse().unwrap_or(0);
        let current_path = parts[4].to_string();

        // Only include panes that have a claude process running
        if claude_ttys.contains_key(&pane_tty) {
            sessions.push(ClaudeSession {
                session_name,
                window_index,
                pane_id,
                working_dir: PathBuf::from(current_path),
                current_command: "claude".to_string(),
            });
        }
    }

    Ok(sessions)
}

/// Get the current working directory of a tmux session
pub fn get_session_cwd(session_name: &str) -> Result<PathBuf> {
    let output = Command::new("tmux")
        .args([
            "display-message",
            "-t",
            session_name,
            "-p",
            "#{pane_current_path}",
        ])
        .output()?;

    if !output.status.success() {
        return Err(anyhow!("Failed to get session cwd"));
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PathBuf::from(path))
}

/// Create a new tmux session for a project
pub fn create_session(session_name: &str, working_dir: &PathBuf) -> Result<String> {
    // Check if session already exists
    let check = Command::new("tmux")
        .args(["has-session", "-t", session_name])
        .output()?;

    if check.status.success() {
        // Session exists, get the pane ID
        let output = Command::new("tmux")
            .args([
                "list-panes",
                "-t",
                session_name,
                "-F",
                "#{pane_id}",
            ])
            .output()?;

        let pane_id = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("%0")
            .to_string();

        return Ok(pane_id);
    }

    // Create new detached session
    let output = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            session_name,
            "-c",
            &working_dir.to_string_lossy(),
            "-P",
            "-F",
            "#{pane_id}",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to create tmux session: {}", stderr));
    }

    let pane_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(pane_id)
}

/// Send keys/command to a tmux pane
pub fn send_to_pane(pane_id: &str, text: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["send-keys", "-t", pane_id, text])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to send keys: {}", stderr));
    }

    Ok(())
}

/// Reload Claude session to pick up new hooks
/// Sends /exit then `claude --continue` to restart with fresh config
pub fn reload_claude_session(pane_id: &str) -> Result<()> {
    // Send /exit command
    let output = Command::new("tmux")
        .args(["send-keys", "-t", pane_id, "/exit", "Enter"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to send /exit: {}", stderr));
    }

    // Wait for Claude to exit
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Restart with --continue to resume the session with fresh hooks
    let output = Command::new("tmux")
        .args(["send-keys", "-t", pane_id, "claude --continue", "Enter"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to restart Claude: {}", stderr));
    }

    Ok(())
}

/// Send a task to an already-running Claude Code session
pub fn start_claude_task(pane_id: &str, task_description: &str, images: &[PathBuf]) -> Result<()> {
    // Claude is already running - just send the task text directly

    // If there are images, include their paths for Claude to read
    let mut task = task_description.to_string();
    if !images.is_empty() {
        task.push_str("\n\nPlease read and analyze these images:");
        for image in images {
            task.push_str(&format!("\n{}", image.display()));
        }
    }

    // Use send-keys with -l flag for literal text (handles special chars)
    let output = Command::new("tmux")
        .args(["send-keys", "-t", pane_id, "-l", &task])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to send task: {}", stderr));
    }

    // Send Enter to submit
    let output = Command::new("tmux")
        .args(["send-keys", "-t", pane_id, "Enter"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to send Enter: {}", stderr));
    }

    Ok(())
}

/// Spawn a new Claude session in a new tmux window within the current session
/// Returns the pane ID of the new window
pub fn spawn_claude_session(working_dir: &PathBuf, project_name: &str) -> Result<String> {
    // Get the current tmux session name
    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .output()?;

    if !output.status.success() {
        return Err(anyhow!("Not running inside tmux"));
    }

    let session_name = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Create a new window in the current session with the project's working directory
    let output = Command::new("tmux")
        .args([
            "new-window",
            "-t", &session_name,
            "-n", project_name,
            "-c", &working_dir.to_string_lossy(),
            "-P",
            "-F", "#{pane_id}",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to create new window: {}", stderr));
    }

    let pane_id = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Start Claude in the new pane
    let start_output = Command::new("tmux")
        .args(["send-keys", "-t", &pane_id, "claude", "Enter"])
        .output()?;

    if !start_output.status.success() {
        let stderr = String::from_utf8_lossy(&start_output.stderr);
        return Err(anyhow!("Failed to start Claude: {}", stderr));
    }

    Ok(pane_id)
}

/// Check if Claude is currently running/busy in a pane
pub fn is_claude_busy(pane_id: &str) -> Result<bool> {
    // Check the current command
    let output = Command::new("tmux")
        .args([
            "display-message",
            "-t",
            pane_id,
            "-p",
            "#{pane_current_command}",
        ])
        .output()?;

    if !output.status.success() {
        return Ok(false);
    }

    let cmd = String::from_utf8_lossy(&output.stdout).trim().to_lowercase();

    // If the command is claude or node (running claude), it's busy
    if cmd.contains("claude") || cmd.contains("node") {
        return Ok(true);
    }

    // Also check the last line of output for prompt
    let content_output = Command::new("tmux")
        .args(["capture-pane", "-t", pane_id, "-p", "-l", "3"])
        .output()?;

    if content_output.status.success() {
        let content = String::from_utf8_lossy(&content_output.stdout);
        let last_line = content.lines().rev().find(|l| !l.trim().is_empty());

        if let Some(line) = last_line {
            // If the last line is a prompt, Claude is idle
            if line.trim().starts_with('>') || line.trim().ends_with('$') || line.trim().ends_with('%') {
                return Ok(false);
            }
        }
    }

    Ok(false)
}

// ============================================================================
// Worktree-based task session management
// ============================================================================

/// Get or create the KanClaude tmux session for a project
/// This session will contain windows for each active task.
pub fn get_or_create_project_session(project_slug: &str) -> Result<String> {
    let session_name = format!("kc-{}", project_slug);

    // Check if session already exists
    let check = Command::new("tmux")
        .args(["has-session", "-t", &session_name])
        .output()?;

    if check.status.success() {
        return Ok(session_name);
    }

    // Create new detached session
    let output = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            &session_name,
            "-n",
            "main",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to create session: {}", stderr));
    }

    Ok(session_name)
}

/// Create a new tmux window for a task within the project session
/// Returns the window name
pub fn create_task_window(
    project_slug: &str,
    task_id: &str,
    worktree_path: &std::path::Path,
) -> Result<String> {
    let session_name = get_or_create_project_session(project_slug)?;
    let window_name = format!("task-{}", &task_id[..8.min(task_id.len())]);

    // Check if window already exists
    let check = Command::new("tmux")
        .args([
            "list-windows",
            "-t",
            &session_name,
            "-F",
            "#{window_name}",
        ])
        .output()?;

    if check.status.success() {
        let windows = String::from_utf8_lossy(&check.stdout);
        if windows.lines().any(|w| w == window_name) {
            // Window already exists, return its name
            return Ok(window_name);
        }
    }

    // Create new window in the session
    let output = Command::new("tmux")
        .args([
            "new-window",
            "-t",
            &session_name,
            "-n",
            &window_name,
            "-c",
            &worktree_path.to_string_lossy(),
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to create window: {}", stderr));
    }

    Ok(window_name)
}

/// Start Claude in a task window
pub fn start_claude_in_window(project_slug: &str, window_name: &str) -> Result<()> {
    let session_name = format!("kc-{}", project_slug);
    let target = format!("{}:{}", session_name, window_name);

    // Start Claude
    let output = Command::new("tmux")
        .args(["send-keys", "-t", &target, "claude", "Enter"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to start Claude: {}", stderr));
    }

    Ok(())
}

/// Wait for Claude to be ready (shows prompt) with timeout
pub fn wait_for_claude_ready(project_slug: &str, window_name: &str, timeout_ms: u64) -> Result<bool> {
    let session_name = format!("kc-{}", project_slug);
    let target = format!("{}:{}", session_name, window_name);

    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);

    loop {
        if start.elapsed() > timeout {
            return Ok(false);
        }

        // Capture pane content
        let output = Command::new("tmux")
            .args(["capture-pane", "-t", &target, "-p", "-l", "10"])
            .output()?;

        if output.status.success() {
            let content = String::from_utf8_lossy(&output.stdout);

            // Look for Claude's prompt patterns
            // Claude Code shows ">" when ready for input
            // Also check for common ready patterns
            for line in content.lines().rev() {
                let trimmed = line.trim();
                if trimmed.starts_with(">") && !trimmed.contains("...") {
                    return Ok(true);
                }
                if trimmed.contains("What would you like") {
                    return Ok(true);
                }
                if trimmed.contains("How can I help") {
                    return Ok(true);
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Send task description to Claude in a window
pub fn send_task_to_window(
    project_slug: &str,
    window_name: &str,
    task_description: &str,
    images: &[std::path::PathBuf],
) -> Result<()> {
    let session_name = format!("kc-{}", project_slug);
    let target = format!("{}:{}", session_name, window_name);

    // Build the full task with image paths
    let mut task = task_description.to_string();
    if !images.is_empty() {
        task.push_str("\n\nPlease read and analyze these images:");
        for image in images {
            task.push_str(&format!("\n{}", image.display()));
        }
    }

    // Send using literal mode for special characters
    let output = Command::new("tmux")
        .args(["send-keys", "-t", &target, "-l", &task])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to send task: {}", stderr));
    }

    // Send Enter to submit
    let output = Command::new("tmux")
        .args(["send-keys", "-t", &target, "Enter"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to submit: {}", stderr));
    }

    Ok(())
}

/// Focus (select) a task window
pub fn focus_task_window(project_slug: &str, window_name: &str) -> Result<()> {
    let session_name = format!("kc-{}", project_slug);
    let target = format!("{}:{}", session_name, window_name);

    // Select the window
    let output = Command::new("tmux")
        .args(["select-window", "-t", &target])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to focus window: {}", stderr));
    }

    Ok(())
}

/// Switch to a task's tmux window (from another tmux client)
pub fn switch_to_task_window(project_slug: &str, window_name: &str) -> Result<()> {
    let session_name = format!("kc-{}", project_slug);
    let target = format!("{}:{}", session_name, window_name);

    // Switch client to this session/window
    let _ = Command::new("tmux")
        .args(["switch-client", "-t", &target])
        .output();

    // Select the window in case client is already in the session
    let _ = Command::new("tmux")
        .args(["select-window", "-t", &target])
        .output();

    Ok(())
}

/// Kill a task window
pub fn kill_task_window(project_slug: &str, window_name: &str) -> Result<()> {
    let session_name = format!("kc-{}", project_slug);
    let target = format!("{}:{}", session_name, window_name);

    let output = Command::new("tmux")
        .args(["kill-window", "-t", &target])
        .output()?;

    // Ignore errors if window doesn't exist
    let _ = output;

    Ok(())
}

/// Check if a task window exists
pub fn task_window_exists(project_slug: &str, window_name: &str) -> bool {
    let session_name = format!("kc-{}", project_slug);

    let output = Command::new("tmux")
        .args([
            "list-windows",
            "-t",
            &session_name,
            "-F",
            "#{window_name}",
        ])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let windows = String::from_utf8_lossy(&output.stdout);
            return windows.lines().any(|w| w == window_name);
        }
    }

    false
}

/// Capture output from a task window
pub fn capture_task_output(project_slug: &str, window_name: &str, lines: u32) -> Result<String> {
    let session_name = format!("kc-{}", project_slug);
    let target = format!("{}:{}", session_name, window_name);

    let output = Command::new("tmux")
        .args([
            "capture-pane",
            "-t",
            &target,
            "-p",
            "-l",
            &lines.to_string(),
        ])
        .output()?;

    if !output.status.success() {
        return Ok(String::new());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
