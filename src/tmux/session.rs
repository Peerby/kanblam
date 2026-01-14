use anyhow::{anyhow, Result};
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

/// Send a prompt to a tmux pane using paste-buffer for reliable submission.
/// This is more reliable than send-keys because:
/// 1. set-buffer stores the text atomically on the tmux server
/// 2. paste-buffer inserts all text at once (no character-by-character race)
/// 3. Enter is sent after paste completes
fn send_prompt_via_paste_buffer(target: &str, text: &str) -> Result<()> {
    // Step 1: Set the tmux buffer with our prompt text
    let output = Command::new("tmux")
        .args(["set-buffer", "--", text])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to set buffer: {}", stderr));
    }

    // Step 2: Paste the buffer into the target pane
    let output = Command::new("tmux")
        .args(["paste-buffer", "-t", target])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to paste buffer: {}", stderr));
    }

    // Step 3: Send Enter to submit the prompt
    let output = Command::new("tmux")
        .args(["send-keys", "-t", target, "Enter"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to send Enter: {}", stderr));
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

    // Use paste-buffer for reliable prompt submission
    // This is more reliable than send-keys because paste-buffer is atomic
    send_prompt_via_paste_buffer(pane_id, &task)
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

    // Start Claude - trust is pre-configured via ~/.claude.json by pre_trust_worktree()
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

        // Capture pane content (use -S for start line, negative = from bottom)
        let output = Command::new("tmux")
            .args(["capture-pane", "-t", &target, "-p", "-S", "-15"])
            .output()?;

        if output.status.success() {
            let content = String::from_utf8_lossy(&output.stdout);

            // Look for Claude's prompt patterns
            // Claude Code shows "❯" (U+276F) when ready for input
            for line in content.lines().rev() {
                let trimmed = line.trim();
                // Claude's actual prompt character is ❯ (U+276F)
                if trimmed.starts_with("❯") || trimmed.starts_with(">") {
                    // Skip if showing loading/thinking indicator
                    if !trimmed.contains("...") {
                        return Ok(true);
                    }
                }
                if trimmed.contains("What would you like") {
                    return Ok(true);
                }
                if trimmed.contains("How can I help") {
                    return Ok(true);
                }
                // "Try" suggestions indicate ready state
                if trimmed.contains("Try \"") {
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

    // Use paste-buffer for reliable prompt submission
    send_prompt_via_paste_buffer(&target, &task)
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

/// Create a test shell window for a task and switch to it
/// The window is created in the same session as the task, with a "test-" prefix
pub fn create_test_shell(
    project_slug: &str,
    task_id: &str,
    worktree_path: &std::path::Path,
) -> Result<String> {
    let session_name = format!("kc-{}", project_slug);
    let window_name = format!("test-{}", &task_id[..8.min(task_id.len())]);

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

    let window_exists = if check.status.success() {
        let windows = String::from_utf8_lossy(&check.stdout);
        windows.lines().any(|w| w == window_name)
    } else {
        false
    };

    if !window_exists {
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
            return Err(anyhow!("Failed to create test window: {}", stderr));
        }
    }

    // Switch to the test window
    let target = format!("{}:{}", session_name, window_name);
    let _ = Command::new("tmux")
        .args(["switch-client", "-t", &target])
        .output();
    let _ = Command::new("tmux")
        .args(["select-window", "-t", &target])
        .output();

    Ok(window_name)
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
