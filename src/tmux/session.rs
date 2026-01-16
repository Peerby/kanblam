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

    // Step 3: Brief delay to ensure paste is fully processed before Enter
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Step 4: Send Enter to submit the prompt
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

/// Start Claude with --resume in a task window (for CLI handoff from SDK)
pub fn send_resume_command(project_slug: &str, window_name: &str, session_id: &str) -> Result<()> {
    let session_name = format!("kc-{}", project_slug);
    let target = format!("{}:{}", session_name, window_name);

    // Send claude --resume <session_id> command
    let resume_cmd = format!("claude --resume {}", session_id);
    let output = Command::new("tmux")
        .args(["send-keys", "-t", &target, &resume_cmd, "Enter"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to send resume command: {}", stderr));
    }

    Ok(())
}

/// Start Claude fresh in a task window (for when there's no resumable session)
pub fn send_start_command(project_slug: &str, window_name: &str) -> Result<()> {
    let session_name = format!("kc-{}", project_slug);
    let target = format!("{}:{}", session_name, window_name);

    // Just start claude without --resume
    let output = Command::new("tmux")
        .args(["send-keys", "-t", &target, "claude", "Enter"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to send start command: {}", stderr));
    }

    Ok(())
}

/// Resize a tmux pane to specific dimensions
pub fn resize_pane(target: &str, width: u16, height: u16) -> Result<()> {
    // Resize width
    let output = Command::new("tmux")
        .args(["resize-pane", "-t", target, "-x", &width.to_string()])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to resize pane width: {}", stderr));
    }

    // Resize height
    let output = Command::new("tmux")
        .args(["resize-pane", "-t", target, "-y", &height.to_string()])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to resize pane height: {}", stderr));
    }

    Ok(())
}

/// Send SIGWINCH to a tmux pane to trigger terminal resize handling
pub fn send_sigwinch(target: &str) -> Result<()> {
    // Use tmux refresh-client to signal window size change
    let output = Command::new("tmux")
        .args(["refresh-client", "-t", target, "-S"])
        .output()?;

    if !output.status.success() {
        // Try alternative: send resize-pane with current size to trigger redraw
        let _ = Command::new("tmux")
            .args(["resize-pane", "-t", target, "-Z"])  // Toggle zoom to force redraw
            .output();
        let _ = Command::new("tmux")
            .args(["resize-pane", "-t", target, "-Z"])  // Toggle back
            .output();
    }

    Ok(())
}

/// Get the dimensions of a tmux pane
pub fn get_pane_size(target: &str) -> Result<(u16, u16)> {
    let output = Command::new("tmux")
        .args(["display-message", "-t", target, "-p", "#{pane_width} #{pane_height}"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to get pane size: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = stdout.trim().split_whitespace().collect();
    if parts.len() != 2 {
        return Err(anyhow!("Unexpected pane size output: {}", stdout));
    }

    let width: u16 = parts[0].parse().map_err(|e| anyhow!("Invalid width: {}", e))?;
    let height: u16 = parts[1].parse().map_err(|e| anyhow!("Invalid height: {}", e))?;

    Ok((width, height))
}

/// Open Claude in a separate tmux session (non-blocking)
/// Creates a new session for Claude and switches to it, allowing normal tmux navigation
pub fn open_popup(worktree_path: &std::path::Path, session_id: Option<&str>) -> Result<()> {
    // Extract task ID from worktree path (format: .../worktrees/task-{uuid})
    let dir_name = worktree_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("claude");

    // Use full task-id suffix for uniqueness (e.g., "task-6cfe1853" -> "cl-6cfe1853")
    // Strip "task-" prefix if present and use first 8 chars of UUID
    let short_name = if let Some(stripped) = dir_name.strip_prefix("task-") {
        &stripped[..8.min(stripped.len())]
    } else if dir_name.len() > 8 {
        &dir_name[..8]
    } else {
        dir_name
    };
    let session_name = format!("cl-{}", short_name);

    // Build claude command - resume if we have a valid session_id
    let claude_cmd = match session_id {
        Some(id) => format!("claude --resume {}", id),
        None => "claude".to_string(),
    };

    // Check if session already exists
    let check = Command::new("tmux")
        .args(["has-session", "-t", &session_name])
        .output()?;

    if check.status.success() {
        // Session exists, just switch to it
        let _ = Command::new("tmux")
            .args(["switch-client", "-t", &session_name])
            .output();
    } else {
        // Create new detached session with Claude running
        // Use login shell to get user's PATH (so `claude` command is found)
        let shell_cmd = format!(
            "cd '{}' && {}",
            worktree_path.to_string_lossy(),
            claude_cmd
        );

        let output = Command::new("tmux")
            .args([
                "new-session",
                "-d",  // detached
                "-s", &session_name,
                "-c", &worktree_path.to_string_lossy(),
                "bash", "-l", "-c", &shell_cmd,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to create Claude session: {}", stderr));
        }

        // Switch to the new session
        let _ = Command::new("tmux")
            .args(["switch-client", "-t", &session_name])
            .output();
    }

    Ok(())
}

/// Send a key sequence to a tmux pane (for interactive modal)
pub fn send_key_to_pane(target: &str, key: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["send-keys", "-t", target, key])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to send key: {}", stderr));
    }

    Ok(())
}

/// Capture pane content with ANSI escape codes (for terminal rendering)
pub fn capture_pane_with_escapes(target: &str) -> Result<String> {
    let output = Command::new("tmux")
        .args(["capture-pane", "-t", target, "-p", "-e"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to capture pane: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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

/// Create a test shell session for a task and switch to it
/// Each task gets its own dedicated tmux session named "kb-{short-task-id}"
/// If the session already exists, we reconnect to it
pub fn create_test_shell(
    _project_slug: &str,
    task_id: &str,
    worktree_path: &std::path::Path,
) -> Result<String> {
    let session_name = format!("kb-{}", &task_id[..4.min(task_id.len())]);

    // Check if session already exists
    let check = Command::new("tmux")
        .args(["has-session", "-t", &session_name])
        .output()?;

    let session_exists = check.status.success();

    if !session_exists {
        // Create new detached session starting in the worktree directory
        let output = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                &session_name,
                "-c",
                &worktree_path.to_string_lossy(),
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to create test session: {}", stderr));
        }
    }

    // Switch to the test session
    let _ = Command::new("tmux")
        .args(["switch-client", "-t", &session_name])
        .output();

    Ok(session_name)
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
