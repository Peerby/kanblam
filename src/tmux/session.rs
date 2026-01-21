#![allow(dead_code)]

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

/// Get or create the Kanblam tmux session for a project
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

/// Open a combined tmux session with three panes:
/// - Claude on left (pane 0)
/// - Shell on right (pane 1)
/// - Statusbar at bottom (pane 2) - minimal height for dev tools
/// Creates a session named "kb-{short-task-id}"
pub fn open_popup(worktree_path: &std::path::Path, session_id: Option<&str>) -> Result<()> {
    // Extract task ID from worktree path (format: .../worktrees/task-{uuid})
    let dir_name = worktree_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("claude");

    // Use short task-id suffix (e.g., "task-6cfe1853" -> "kb-6cfe")
    // Strip "task-" prefix if present and use first 4 chars of UUID
    let (short_name, full_task_id) = if let Some(stripped) = dir_name.strip_prefix("task-") {
        (&stripped[..4.min(stripped.len())], stripped)
    } else if dir_name.len() > 4 {
        (&dir_name[..4], dir_name)
    } else {
        (dir_name, dir_name)
    };
    let session_name = format!("kb-{}", short_name);

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
        // Build claude command - resume if we have a valid session_id
        let claude_cmd = match session_id {
            Some(id) => format!("claude --resume {}", id),
            None => "claude".to_string(),
        };

        // Create new detached session with Claude running in the first pane
        // Use login shell to get user's PATH (so `claude` command is found)
        let shell_cmd = format!(
            "cd '{}' && {}",
            worktree_path.to_string_lossy(),
            claude_cmd
        );

        // Use -x- and -y- to inherit current terminal size instead of default-size
        // This fixes split-window -l not being honored in detached sessions (tmux issue #3060)
        let output = Command::new("tmux")
            .args([
                "new-session",
                "-d",  // detached
                "-x-", // use current terminal width
                "-y-", // use current terminal height
                "-s", &session_name,
                "-c", &worktree_path.to_string_lossy(),
                "bash", "-l", "-c", &shell_cmd,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to create session: {}", stderr));
        }

        // Split horizontally to create right pane with shell
        let output = Command::new("tmux")
            .args([
                "split-window",
                "-t", &session_name,
                "-h",  // horizontal split (side by side)
                "-c", &worktree_path.to_string_lossy(),
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to create shell pane: {}", stderr));
        }

        // Create statusbar pane at the bottom spanning full width
        // We need to use tmux's -f flag to create a full-width split
        // Get the kanblam binary path for the statusbar command
        let kanblam_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "kanblam".to_string());

        // Build statusbar command
        let statusbar_cmd = format!(
            "cd '{}' && '{}' statusbar {}",
            worktree_path.to_string_lossy(),
            kanblam_path,
            full_task_id
        );

        // Split vertically with -f flag for full-width pane at bottom
        // -f creates a new pane spanning the full window width/height
        // Note: Don't use -l flag here - it's not honored reliably in detached sessions (tmux #3060)
        // Instead, we resize the pane immediately after creation
        let output = Command::new("tmux")
            .args([
                "split-window",
                "-t", &session_name,
                "-f",  // full-width split
                "-v",  // vertical split (stacked)
                "-c", &worktree_path.to_string_lossy(),
                "bash", "-l", "-c", &statusbar_cmd,
            ])
            .output()?;

        if !output.status.success() {
            // Statusbar pane creation failed, but that's not critical - continue without it
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("Note: Could not create statusbar pane: {}", stderr);
        }

        // Select the left pane (Claude) as the active pane
        // Use {top-left} to select the first pane regardless of base-index
        let _ = Command::new("tmux")
            .args(["select-pane", "-t", &format!("{}:.{{top-left}}", session_name)])
            .output();

        // Switch to the new session FIRST - this may cause layout recalculation
        let _ = Command::new("tmux")
            .args(["switch-client", "-t", &session_name])
            .output();

        // Small delay to let tmux finish the switch and layout calculation
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Resize statusbar pane to exactly 2 lines AFTER switching
        // Must be done after switch-client because switching recalculates layout
        let _ = Command::new("tmux")
            .args(["resize-pane", "-t", &format!("{}:.{{bottom}}", session_name), "-y", "2"])
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

/// Result of creating a detached session
pub struct DetachedSessionResult {
    pub session_name: String,
    pub was_created: bool,
}

/// Open combined tmux session in detached mode (don't switch to it)
/// Creates a session with three panes:
/// - Claude on left (pane 0)
/// - Shell on right (pane 1)
/// - Statusbar at bottom (pane 2) - minimal height for dev tools
/// Returns the session name and whether it was newly created
pub fn open_popup_detached(
    worktree_path: &std::path::Path,
    session_id: Option<&str>,
) -> Result<DetachedSessionResult> {
    // Extract task ID from worktree path (format: .../worktrees/task-{uuid})
    let dir_name = worktree_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("claude");

    // Use short task-id suffix (e.g., "task-6cfe1853" -> "kb-6cfe")
    let (short_name, full_task_id) = if let Some(stripped) = dir_name.strip_prefix("task-") {
        (&stripped[..4.min(stripped.len())], stripped)
    } else if dir_name.len() > 4 {
        (&dir_name[..4], dir_name)
    } else {
        (dir_name, dir_name)
    };
    let session_name = format!("kb-{}", short_name);

    // Check if session already exists
    let check = Command::new("tmux")
        .args(["has-session", "-t", &session_name])
        .output()?;

    let session_exists = check.status.success();

    if !session_exists {
        // Build claude command - resume if we have a valid session_id
        let claude_cmd = match session_id {
            Some(id) => format!("claude --resume {}", id),
            None => "claude".to_string(),
        };

        // Create new detached session with Claude running in the first pane
        let shell_cmd = format!(
            "cd '{}' && {}",
            worktree_path.to_string_lossy(),
            claude_cmd
        );

        // Use -x- and -y- to inherit current terminal size instead of default-size
        // This fixes split-window -l not being honored in detached sessions (tmux issue #3060)
        let output = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-x-", // use current terminal width
                "-y-", // use current terminal height
                "-s", &session_name,
                "-c", &worktree_path.to_string_lossy(),
                "bash", "-l", "-c", &shell_cmd,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to create session: {}", stderr));
        }

        // Split horizontally to create right pane with shell
        let output = Command::new("tmux")
            .args([
                "split-window",
                "-t", &session_name,
                "-h",  // horizontal split (side by side)
                "-c", &worktree_path.to_string_lossy(),
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to create shell pane: {}", stderr));
        }

        // Create statusbar pane at the bottom spanning full width
        // Get the kanblam binary path for the statusbar command
        let kanblam_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "kanblam".to_string());

        // Build statusbar command
        let statusbar_cmd = format!(
            "cd '{}' && '{}' statusbar {}",
            worktree_path.to_string_lossy(),
            kanblam_path,
            full_task_id
        );

        // Split vertically with -f flag for full-width pane at bottom
        // -f creates a new pane spanning the full window width/height
        // Note: Don't use -l flag here - it's not honored reliably in detached sessions (tmux #3060)
        // Instead, we resize the pane immediately after creation
        let output = Command::new("tmux")
            .args([
                "split-window",
                "-t", &session_name,
                "-f",  // full-width split
                "-v",  // vertical split (stacked)
                "-c", &worktree_path.to_string_lossy(),
                "bash", "-l", "-c", &statusbar_cmd,
            ])
            .output()?;

        if !output.status.success() {
            // Statusbar pane creation failed, but that's not critical - continue without it
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("Note: Could not create statusbar pane: {}", stderr);
        }

        // Small delay to let tmux finish creating the pane
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Resize statusbar pane to exactly 2 lines (minimum for tmux)
        // This works reliably unlike -l flag in split-window
        let _ = Command::new("tmux")
            .args(["resize-pane", "-t", &format!("{}:.{{bottom}}", session_name), "-y", "2"])
            .output();

        // Select the left pane (Claude) as the active pane
        // Use {top-left} to select the first pane regardless of base-index
        let _ = Command::new("tmux")
            .args(["select-pane", "-t", &format!("{}:.{{top-left}}", session_name)])
            .output();
    }

    // Don't switch - stay in current session
    Ok(DetachedSessionResult {
        session_name,
        was_created: !session_exists,
    })
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

/// Kill any detached tmux sessions associated with a task.
/// This includes the combined session: `kb-{first-4-chars-of-task-id}`
pub fn kill_task_sessions(task_id: &str) {
    // Kill combined session (kb-{first-4-chars})
    let session_name = format!("kb-{}", &task_id[..4.min(task_id.len())]);
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", &session_name])
        .output();
}

/// Result of checking Claude CLI activity state in a tmux pane
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeCliState {
    /// Claude is at prompt, waiting for input (shows ❯)
    WaitingForInput,
    /// Claude is actively working (processing, using tools)
    Working,
    /// Claude process is not running (shell prompt visible, or session doesn't exist)
    NotRunning,
    /// Could not determine state
    Unknown,
}

/// Check the state of Claude CLI in a task's tmux session.
/// Examines the pane content to determine if Claude is:
/// - At prompt (waiting for input)
/// - Actively working (processing)
/// - Not running (just a shell)
pub fn get_claude_cli_state(task_id: &str) -> ClaudeCliState {
    // Session name is kb-{first 4 chars of task_id}
    let session_name = format!("kb-{}", &task_id[..4.min(task_id.len())]);
    // Use {top-left} to get first pane regardless of base-index setting
    let target = format!("{}:.{{top-left}}", session_name); // Left pane where Claude runs

    // Check if session exists
    let check = Command::new("tmux")
        .args(["has-session", "-t", &session_name])
        .output();

    match check {
        Ok(output) if !output.status.success() => {
            // Session doesn't exist
            return ClaudeCliState::NotRunning;
        }
        Err(_) => return ClaudeCliState::Unknown,
        _ => {}
    }

    // Capture the last 20 lines of the pane
    let output = match Command::new("tmux")
        .args(["capture-pane", "-t", &target, "-p", "-S", "-20"])
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return ClaudeCliState::Unknown,
    };

    // Analyze pane content from bottom up
    for line in output.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Claude's ready prompt character is ❯ (U+276F) when waiting for input
        if trimmed.starts_with("❯") || trimmed.starts_with(">") {
            // Skip if showing loading indicator (... or spinner)
            if trimmed.contains("...") || trimmed.contains("⠋") || trimmed.contains("⠙") {
                return ClaudeCliState::Working;
            }
            return ClaudeCliState::WaitingForInput;
        }

        // Working indicators (spinners, tool use)
        if trimmed.contains("⠋") || trimmed.contains("⠙") || trimmed.contains("⠹")
            || trimmed.contains("⠸") || trimmed.contains("⠼") || trimmed.contains("⠴")
            || trimmed.contains("⠦") || trimmed.contains("⠧") || trimmed.contains("⠇")
            || trimmed.contains("⠏")
        {
            return ClaudeCliState::Working;
        }

        // Tool use output (Claude is working or just finished)
        if trimmed.starts_with("●") || trimmed.starts_with("○") {
            return ClaudeCliState::Working;
        }

        // Check for shell prompt (Claude not running) - common patterns
        // $ prompt, % prompt, user@host patterns
        if trimmed.ends_with('$') || trimmed.ends_with('%') {
            // Make sure it's not Claude's output that happens to end with these
            if trimmed.len() < 80 && !trimmed.contains("Claude") {
                return ClaudeCliState::NotRunning;
            }
        }
    }

    // If we got here, Claude might be outputting text (also "working")
    // Default to Unknown if we can't tell
    ClaudeCliState::Unknown
}

/// Kill the Claude CLI session for a task (if it exists).
/// This allows restarting with fresh state after SDK has done work.
pub fn kill_claude_cli_session(task_id: &str) -> Result<()> {
    let session_name = format!("kb-{}", &task_id[..4.min(task_id.len())]);

    let output = Command::new("tmux")
        .args(["kill-session", "-t", &session_name])
        .output()?;

    if !output.status.success() {
        // Session might not exist, which is fine
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("can't find session") && !stderr.contains("no server running") {
            return Err(anyhow!("Failed to kill session: {}", stderr));
        }
    }

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

/// Open a new pane to the right of the current pane and start a fresh Claude CLI session.
/// This splits the current pane horizontally and runs `claude` in the new pane.
pub fn split_pane_with_claude(working_dir: &std::path::Path) -> Result<()> {
    // Split the current pane horizontally (creates pane to the right)
    // -h = horizontal split (side by side)
    // -c = start directory
    let output = Command::new("tmux")
        .args([
            "split-window",
            "-h",
            "-c",
            &working_dir.to_string_lossy(),
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to split pane: {}", stderr));
    }

    // The new pane is now active, send the claude command
    // Use bash -l -c to get login shell environment (for PATH)
    let output = Command::new("tmux")
        .args(["send-keys", "claude", "Enter"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to start Claude: {}", stderr));
    }

    Ok(())
}

/// Check if Claude's last output in the tmux pane looks like a question
/// This is used to determine if Claude is waiting for user input vs just finished.
pub fn claude_output_contains_question(project_slug: &str, window_name: &str) -> bool {
    // Capture the last 30 lines to get Claude's recent output
    let content = match capture_task_output(project_slug, window_name, 30) {
        Ok(c) => c,
        Err(_) => return false,
    };

    // Look for question patterns in the content
    // We check the last ~20 non-empty lines to find Claude's last message
    let lines: Vec<&str> = content.lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(20)
        .collect();

    // Skip the prompt line (❯ or >) at the very end
    let message_lines: Vec<&str> = lines.iter()
        .skip_while(|l| {
            let trimmed = l.trim();
            trimmed.starts_with('❯') || (trimmed.starts_with('>') && trimmed.len() < 3)
        })
        .copied()
        .collect();

    // Check for question patterns in Claude's last output
    for line in &message_lines {
        let lower = line.to_lowercase();

        // Direct question marks
        if line.contains('?') {
            return true;
        }

        // Question phrases
        if lower.contains("would you like")
            || lower.contains("should i ")
            || lower.contains("do you want")
            || lower.contains("shall i ")
            || lower.contains("can you ")
            || lower.contains("could you ")
            || lower.contains("what would you")
            || lower.contains("how would you")
            || lower.contains("which option")
            || lower.contains("let me know")
            || lower.contains("please confirm")
            || lower.contains("please provide")
            || lower.contains("please specify")
            || lower.contains("what do you think")
            || lower.contains("your thoughts")
            || lower.contains("your preference")
        {
            return true;
        }
    }

    false
}
