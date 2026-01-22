//! Worktree statusbar - A minimal TUI that runs in a tmux pane alongside the shell
//! Provides git status, keybindings for common dev actions, and control of the tmux layout

#![allow(dead_code)]

use anyhow::{anyhow, Result};
use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        cursor::{Hide, Show},
        event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind, EnableMouseCapture, DisableMouseCapture},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode},
    },
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame, Terminal,
};
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// State for the statusbar TUI
pub struct StatusbarState {
    /// Task ID this statusbar is for
    pub task_id: String,
    /// Worktree path
    pub worktree_path: PathBuf,
    /// Project directory (main repo, parent of worktrees/)
    pub project_dir: PathBuf,
    /// Current git branch name
    pub branch_name: Option<String>,
    /// Commits ahead of main
    pub ahead: usize,
    /// Commits behind main
    pub behind: usize,
    /// Last time we refreshed git status
    pub last_refresh: Instant,
    /// Whether Claude pane is visible
    pub claude_pane_visible: bool,
    /// Whether shell pane is visible
    pub shell_pane_visible: bool,
    /// Status message to show temporarily
    pub status_message: Option<(String, Instant)>,
    /// Tmux session name (e.g., "kb-e1f8")
    pub session_name: String,
    /// Parent kanblam session name (e.g., "kc-projectname")
    pub parent_session: Option<String>,
    /// Dev command (auto-detected or from settings)
    pub dev_command: Option<String>,
    /// Whether dev process is running
    pub dev_running: bool,
    /// Last time we checked pane height
    pub last_height_check: Instant,
    /// Whether we're prompting to install lazygit
    pub lazygit_install_prompt: bool,
}

impl StatusbarState {
    /// Create a new statusbar state for a task
    ///
    /// `explicit_parent_session` is the parent session name passed via command line.
    /// If not provided, falls back to searching for a kanblam process.
    pub fn new(task_id: String, worktree_path: PathBuf, explicit_parent_session: Option<String>) -> Self {
        // Derive project directory (parent of worktrees/)
        let project_dir = worktree_path
            .parent() // worktrees/
            .and_then(|p| p.parent()) // project_dir
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| worktree_path.clone());

        // task_id is now the display_id, use it directly as session name
        let session_name = task_id.clone();

        // Use explicit parent session if provided, otherwise try to detect it
        let parent_session = explicit_parent_session.or_else(|| detect_parent_session(&project_dir));

        // Detect dev command
        let dev_command = detect_dev_command(&worktree_path);

        Self {
            task_id,
            worktree_path,
            project_dir,
            branch_name: None,
            ahead: 0,
            behind: 0,
            last_refresh: Instant::now() - Duration::from_secs(60), // Force immediate refresh
            claude_pane_visible: true,
            shell_pane_visible: true,
            status_message: None,
            session_name,
            parent_session,
            dev_command,
            dev_running: false,
            last_height_check: Instant::now() - Duration::from_secs(10), // Force immediate check
            lazygit_install_prompt: false,
        }
    }

    /// Refresh git status (branch, ahead/behind)
    pub fn refresh_git_status(&mut self) {
        self.last_refresh = Instant::now();

        // Get current branch
        if let Ok(output) = Command::new("git")
            .current_dir(&self.worktree_path)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
        {
            if output.status.success() {
                self.branch_name = Some(
                    String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .to_string(),
                );
            }
        }

        // Get ahead/behind count relative to main
        // First, get merge-base with main
        let main_ref = get_main_ref(&self.project_dir);
        if let Some(ref main) = main_ref {
            // Count commits ahead (in branch but not in main)
            if let Ok(output) = Command::new("git")
                .current_dir(&self.worktree_path)
                .args(["rev-list", "--count", &format!("{}..HEAD", main)])
                .output()
            {
                if output.status.success() {
                    self.ahead = String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .parse()
                        .unwrap_or(0);
                }
            }

            // Count commits behind (in main but not in branch)
            if let Ok(output) = Command::new("git")
                .current_dir(&self.worktree_path)
                .args(["rev-list", "--count", &format!("HEAD..{}", main)])
                .output()
            {
                if output.status.success() {
                    self.behind = String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .parse()
                        .unwrap_or(0);
                }
            }
        }
    }

    /// Set a temporary status message
    pub fn set_status(&mut self, msg: &str) {
        self.status_message = Some((msg.to_string(), Instant::now()));
    }

    /// Clear status message if it's been visible long enough
    pub fn clear_old_status(&mut self) {
        if let Some((_, when)) = &self.status_message {
            if when.elapsed() > Duration::from_secs(3) {
                self.status_message = None;
            }
        }
    }

    /// Check pane height and resize to 2 lines if needed.
    /// This handles cases where the tmux layout changes (e.g., different terminal
    /// attaches to the session with different dimensions).
    pub fn enforce_pane_height(&mut self) {
        // Only check every 2 seconds to avoid excessive tmux commands
        if self.last_height_check.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_height_check = Instant::now();

        // Target the bottom pane (statusbar) in this session
        let target = format!("{}:.{{bottom}}", self.session_name);

        // Get current pane height
        let output = match Command::new("tmux")
            .args(["display-message", "-t", &target, "-p", "#{pane_height}"])
            .output()
        {
            Ok(o) => o,
            Err(_) => return, // Silently ignore errors
        };

        if !output.status.success() {
            return;
        }

        let height_str = String::from_utf8_lossy(&output.stdout);
        let height: u16 = match height_str.trim().parse() {
            Ok(h) => h,
            Err(_) => return,
        };

        // If height is not 2, resize to 2 lines
        if height != 2 {
            let _ = Command::new("tmux")
                .args(["resize-pane", "-t", &target, "-y", "2"])
                .output();
        }
    }
}

/// Get the main branch reference (origin/main, origin/master, main, or master)
fn get_main_ref(project_dir: &PathBuf) -> Option<String> {
    // Try origin/main first
    for ref_name in &["origin/main", "origin/master", "main", "master"] {
        if let Ok(output) = Command::new("git")
            .current_dir(project_dir)
            .args(["rev-parse", "--verify", ref_name])
            .output()
        {
            if output.status.success() {
                return Some(ref_name.to_string());
            }
        }
    }
    None
}

/// Detect the parent kanblam session by searching for any tmux session
/// running the kanblam TUI (not statusbar).
///
/// This is a fallback for when the parent session name wasn't passed explicitly.
fn detect_parent_session(_project_dir: &PathBuf) -> Option<String> {
    // Search for any session running kanblam (not statusbar)
    if let Ok(output) = Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{session_name} #{pane_current_command}",
        ])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    let session = parts[0];
                    let cmd = parts[1];
                    // Look for kanblam TUI (not statusbar)
                    // The command might be "kanblam" or the full path
                    if cmd.contains("kanblam") && !cmd.contains("statusbar") {
                        return Some(session.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Check if lazygit is installed
fn is_lazygit_installed() -> bool {
    Command::new("which")
        .arg("lazygit")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Detect the dev command for the project
fn detect_dev_command(worktree_path: &PathBuf) -> Option<String> {
    // Check for package.json with dev script
    if worktree_path.join("package.json").exists() {
        if let Ok(content) = std::fs::read_to_string(worktree_path.join("package.json")) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(scripts) = json.get("scripts") {
                    if scripts.get("dev").is_some() {
                        return Some("npm run dev".to_string());
                    }
                    if scripts.get("start").is_some() {
                        return Some("npm start".to_string());
                    }
                }
            }
        }
    }

    // Check for Cargo.toml
    if worktree_path.join("Cargo.toml").exists() {
        return Some("cargo run".to_string());
    }

    // Check for Python
    if worktree_path.join("pyproject.toml").exists() {
        return Some("python -m main".to_string());
    }

    // Check for Go
    if worktree_path.join("go.mod").exists() {
        return Some("go run .".to_string());
    }

    None
}

/// Width of the plain "KANBLAM" text
const KANBLAM_TEXT_WIDTH: u16 = 7; // "KANBLAM" = 7 chars

/// Width of the 2-line KANBLAM wordmark (same as main app wordmark, 30 chars + trailing space)
const KANBLAM_ART_WIDTH: u16 = 31;

/// 2-line KANBLAM wordmark derived from the main app's 3-line version
/// - K, A, N, A, M: Use lines 1 and 2 (drop line 3)
/// - L: Use line 2 shifted up, with █▄▄ base from line 3
/// - B: Both lines have holes using ▖ with black fg on green bg
// Top line split around B hole: "  █ █ ▄▀█ █▄ █ █" + ▖ (styled) + "▄ █   ▄▀█ █▀▄▀█"
const KANBLAM_ART_TOP_PRE: &str    = "  █ █ ▄▀█ █▄ █ █";  // Before B hole
const KANBLAM_ART_TOP_POST: &str   = "▄ █   ▄▀█ █▀▄▀█";   // After B hole
// Bottom line split around B hole: "█▀▄ █▀█ █ ▀█ █" + ▖ (styled) + "█ █▄▄ █▀█ █ ▀ █"
const KANBLAM_ART_BOTTOM_PRE: &str  = "█▀▄ █▀█ █ ▀█ █";   // Before B hole
const KANBLAM_ART_BOTTOM_POST: &str = "█ █▄▄ █▀█ █ ▀ █";  // After B hole

/// Render the statusbar (2 lines)
/// When there's enough width: 2-line KANBLAM art on the right of both lines
/// Otherwise: status on line 1, plain "KANBLAM" on line 2
fn render(frame: &mut Frame, state: &StatusbarState) {
    let area = frame.area();
    let bg_style = Style::default().bg(Color::Rgb(30, 30, 40));
    let green = Color::Rgb(80, 200, 120);
    let kanblam_style = Style::default().fg(green).add_modifier(Modifier::BOLD);

    // Determine if we have space for the 2-line art logo
    // Need enough width for: status content (~80 chars) + gap (2) + logo (32) = ~115 chars minimum
    // Using MIN_WIDTH_FOR_ART_LOGO (120) to be safe and prevent overlap on narrow screens
    let use_art_logo = area.width >= MIN_WIDTH_FOR_ART_LOGO && area.height >= 2;

    // === Build status spans (used on line 1) ===
    let mut status_spans = Vec::new();

    // Git branch indicator
    status_spans.push(Span::styled(
        " \u{e0a0} ", // Nerd Font git branch icon
        Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
    ));

    if let Some(ref branch) = state.branch_name {
        status_spans.push(Span::styled(
            branch.clone(),
            Style::default().fg(Color::Magenta),
        ));
    } else {
        status_spans.push(Span::styled(
            "unknown",
            Style::default().fg(Color::DarkGray),
        ));
    }

    // Ahead/behind indicator
    status_spans.push(Span::styled(
        " │ ",
        Style::default().fg(Color::DarkGray),
    ));

    if state.behind > 0 {
        status_spans.push(Span::styled(
            format!("↓{}", state.behind),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
        status_spans.push(Span::raw(" "));
    }

    if state.ahead > 0 {
        status_spans.push(Span::styled(
            format!("↑{}", state.ahead),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
    }

    if state.ahead == 0 && state.behind == 0 {
        status_spans.push(Span::styled(
            "✓ synced",
            Style::default().fg(Color::Green),
        ));
    }

    // Separator before keybindings
    status_spans.push(Span::styled(
        " │ ",
        Style::default().fg(Color::DarkGray),
    ));

    // Show status message if any, otherwise show keybinding hints
    if let Some((ref msg, _)) = state.status_message {
        status_spans.push(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    } else {
        // Keybinding hints
        let hints = [
            ("g", "git"),
            ("r", "rebase"),
            ("s", "start"),
            ("S", "stop"),
            ("c", "claude"),
            ("z", "shell"),
            ("b", "back to Kanblam"),
            ("k", "kill"),
        ];

        for (i, (key, action)) in hints.iter().enumerate() {
            if i > 0 {
                status_spans.push(Span::styled(" ", Style::default().fg(Color::DarkGray)));
            }
            status_spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ));
            status_spans.push(Span::styled(
                format!(":{}", action),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    // Dev running indicator
    if state.dev_running {
        status_spans.push(Span::styled(
            " │ ",
            Style::default().fg(Color::DarkGray),
        ));
        status_spans.push(Span::styled(
            "● running",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ));
    }

    if use_art_logo {
        // === 2-LINE ART MODE: Logo on right side of both lines ===
        let logo_padding = area.width.saturating_sub(KANBLAM_ART_WIDTH + 1);

        // LINE 1: Status on left, top of logo on right
        if area.height >= 1 {
            let line1_area = Rect { x: area.x, y: area.y, width: area.width, height: 1 };

            // Calculate status content width
            let status_width: usize = status_spans.iter().map(|s| s.content.chars().count()).sum();
            let available_for_status = logo_padding.saturating_sub(2) as usize; // Leave gap before logo

            let mut line1_spans = status_spans.clone();

            // Add padding between status and logo
            if status_width < available_for_status {
                let gap = available_for_status - status_width;
                line1_spans.push(Span::styled(" ".repeat(gap), bg_style));
            }

            // Add top line of logo with B hole
            // B hole style: black foreground on green background (same as main wordmark)
            let hole_style = Style::default().fg(Color::Black).bg(green);
            line1_spans.push(Span::styled(KANBLAM_ART_TOP_PRE, kanblam_style));
            line1_spans.push(Span::styled("▖", hole_style));  // B top hole
            line1_spans.push(Span::styled(KANBLAM_ART_TOP_POST, kanblam_style));
            line1_spans.push(Span::styled(" ", bg_style));

            let line1 = Line::from(line1_spans);
            let para1 = Paragraph::new(line1).style(bg_style);
            frame.render_widget(para1, line1_area);
        }

        // LINE 2: Empty left side, bottom of logo on right
        if area.height >= 2 {
            let line2_area = Rect { x: area.x, y: area.y + 1, width: area.width, height: 1 };

            // B bottom middle: horizontal line (black line on green background)
            let bottom_b_style = Style::default().fg(Color::Black).bg(green);

            let mut line2_spans = Vec::new();
            line2_spans.push(Span::styled(" ".repeat(logo_padding as usize), bg_style));
            line2_spans.push(Span::styled(KANBLAM_ART_BOTTOM_PRE, kanblam_style));
            line2_spans.push(Span::styled("━", bottom_b_style));  // B bottom middle line
            line2_spans.push(Span::styled(KANBLAM_ART_BOTTOM_POST, kanblam_style));
            line2_spans.push(Span::styled(" ", bg_style));

            let line2 = Line::from(line2_spans);
            let para2 = Paragraph::new(line2).style(bg_style);
            frame.render_widget(para2, line2_area);
        }
    } else {
        // === PLAIN TEXT MODE: Status on line 1, "KANBLAM" on line 2 ===

        // LINE 1: Status content
        if area.height >= 1 {
            let line1_area = Rect { x: area.x, y: area.y, width: area.width, height: 1 };
            let line1 = Line::from(status_spans);
            let para1 = Paragraph::new(line1).style(bg_style);
            frame.render_widget(para1, line1_area);
        }

        // LINE 2: Plain "KANBLAM" right-aligned
        if area.height >= 2 {
            let line2_area = Rect { x: area.x, y: area.y + 1, width: area.width, height: 1 };

            let padding = area.width.saturating_sub(KANBLAM_TEXT_WIDTH + 1);
            let mut line2_spans = Vec::new();
            if padding > 0 {
                line2_spans.push(Span::styled(" ".repeat(padding as usize), bg_style));
            }
            line2_spans.push(Span::styled("KANBLAM", kanblam_style));
            line2_spans.push(Span::styled(" ", bg_style));

            let line2 = Line::from(line2_spans);
            let para2 = Paragraph::new(line2).style(bg_style);
            frame.render_widget(para2, line2_area);
        }
    }
}

/// Minimum width to show the 2-line art logo (must match render function threshold)
const MIN_WIDTH_FOR_ART_LOGO: u16 = 120;

/// Get the column position where KANBLAM text/logo starts (for click detection)
/// Returns (start_col, end_col) - clicks anywhere on line 2 in this range trigger "back"
fn get_kanblam_click_region(area_width: u16) -> (u16, u16) {
    let use_art = area_width >= MIN_WIDTH_FOR_ART_LOGO;
    let text_width = if use_art { KANBLAM_ART_WIDTH } else { KANBLAM_TEXT_WIDTH };
    let start_col = area_width.saturating_sub(text_width + 1);
    (start_col, area_width)
}

/// Handle a key press, returns true if should quit
fn handle_key(state: &mut StatusbarState, key: KeyCode) -> bool {
    // Handle lazygit install prompt
    if state.lazygit_install_prompt {
        match key {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                state.lazygit_install_prompt = false;
                state.set_status("Installing lazygit...");
                match install_lazygit() {
                    Ok(true) => {
                        state.set_status("lazygit installed! Launching...");
                        if let Err(e) = launch_lazygit(state) {
                            state.set_status(&format!("Launch error: {}", e));
                        }
                    }
                    Ok(false) => {
                        state.set_status("Installation failed");
                    }
                    Err(e) => {
                        state.set_status(&format!("{}", e));
                    }
                }
                return false;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                state.lazygit_install_prompt = false;
                state.status_message = None;
                return false;
            }
            _ => return false,
        }
    }

    match key {
        KeyCode::Char('g') => {
            // Launch lazygit focused on commits since branch point
            if is_lazygit_installed() {
                // Show base commit info in status message
                let base_info = get_merge_base(state)
                    .map(|sha| format!(" (base: {}, press * to filter)", sha))
                    .unwrap_or_default();
                state.set_status(&format!("Launching lazygit...{}", base_info));
                if let Err(e) = launch_lazygit(state) {
                    state.set_status(&format!("Error: {}", e));
                }
            } else {
                // Prompt to install
                state.lazygit_install_prompt = true;
                state.set_status("lazygit not found. Install via brew? (y/n)");
            }
        }
        KeyCode::Char('r') => {
            // Rebase onto main
            state.set_status("Rebasing...");
            match do_rebase(state) {
                Ok(true) => state.set_status("Rebase successful!"),
                Ok(false) => state.set_status("Rebase failed - conflicts detected"),
                Err(e) => state.set_status(&format!("Rebase error: {}", e)),
            }
            state.refresh_git_status();
        }
        KeyCode::Char('s') => {
            // Start/restart dev
            if let Some(cmd) = state.dev_command.clone() {
                state.set_status(&format!("Starting: {}", cmd));
                if let Err(e) = send_to_shell_pane(state, &cmd) {
                    state.set_status(&format!("Error: {}", e));
                } else {
                    state.dev_running = true;
                }
            } else {
                state.set_status("No dev command detected");
            }
        }
        KeyCode::Char('S') => {
            // Stop dev (send Ctrl-C to shell pane)
            state.set_status("Stopping...");
            if let Err(e) = send_ctrl_c_to_shell_pane(state) {
                state.set_status(&format!("Error: {}", e));
            } else {
                state.dev_running = false;
                state.set_status("Stopped");
            }
        }
        KeyCode::Char('c') => {
            // Toggle Claude pane
            if let Err(e) = toggle_claude_pane(state) {
                state.set_status(&format!("Error: {}", e));
            }
        }
        KeyCode::Char('z') => {
            // Toggle shell pane
            if let Err(e) = toggle_shell_pane(state) {
                state.set_status(&format!("Error: {}", e));
            }
        }
        KeyCode::Char('b') => {
            // Back to main Kanblam session (keep this session alive)
            if let Some(parent) = state.parent_session.clone() {
                state.set_status("Switching to Kanblam...");
                let _ = Command::new("tmux")
                    .args(["switch-client", "-t", &parent])
                    .output();
            } else {
                state.set_status("No parent Kanblam session found");
            }
        }
        KeyCode::Char('k') => {
            // Kill this session and return to Kanblam
            if let Some(ref parent) = state.parent_session {
                // Switch first, then kill (kill will happen after we exit)
                let _ = Command::new("tmux")
                    .args(["switch-client", "-t", parent])
                    .output();
            }
            // Signal to kill this session
            let _ = Command::new("tmux")
                .args(["kill-session", "-t", &state.session_name])
                .output();
            return true; // Quit
        }
        KeyCode::Char('q') | KeyCode::Esc => {
            // Just quit the statusbar (unusual, but allowed)
            return true;
        }
        _ => {}
    }
    false
}

/// Perform a rebase onto main
fn do_rebase(state: &StatusbarState) -> Result<bool> {
    // Fetch latest
    let _ = Command::new("git")
        .current_dir(&state.project_dir)
        .args(["fetch", "origin", "main"])
        .output();

    // Get main ref
    let main_ref = get_main_ref(&state.project_dir)
        .ok_or_else(|| anyhow!("Could not find main branch"))?;

    // Try rebase
    let result = Command::new("git")
        .current_dir(&state.worktree_path)
        .args(["rebase", &main_ref])
        .output()?;

    if result.status.success() {
        Ok(true)
    } else {
        // Abort the failed rebase
        let _ = Command::new("git")
            .current_dir(&state.worktree_path)
            .args(["rebase", "--abort"])
            .output();
        Ok(false)
    }
}

/// Send a command to the shell pane
fn send_to_shell_pane(state: &StatusbarState, cmd: &str) -> Result<()> {
    // Use {top-right} token to target shell pane regardless of base-index setting
    let target = format!("{}:.{{top-right}}", state.session_name);

    // Clear line first (Ctrl-U), then send command
    Command::new("tmux")
        .args(["send-keys", "-t", &target, "C-u"])
        .output()?;

    Command::new("tmux")
        .args(["send-keys", "-t", &target, cmd, "Enter"])
        .output()?;

    Ok(())
}

/// Send Ctrl-C to the shell pane
fn send_ctrl_c_to_shell_pane(state: &StatusbarState) -> Result<()> {
    let target = format!("{}:.{{top-right}}", state.session_name);
    Command::new("tmux")
        .args(["send-keys", "-t", &target, "C-c"])
        .output()?;
    Ok(())
}

/// Toggle Claude pane visibility
fn toggle_claude_pane(state: &mut StatusbarState) -> Result<()> {
    // tmux doesn't support hiding panes, so we zoom the shell pane instead
    let shell_target = format!("{}:.{{top-right}}", state.session_name);

    if state.claude_pane_visible {
        // Zoom the shell pane (hides claude)
        Command::new("tmux")
            .args(["resize-pane", "-t", &shell_target, "-Z"])
            .output()?;
        state.claude_pane_visible = false;
        state.set_status("Shell zoomed (c to unzoom)");
    } else {
        // Unzoom to show all panes
        Command::new("tmux")
            .args(["resize-pane", "-t", &shell_target, "-Z"])
            .output()?;
        state.claude_pane_visible = true;
        state.set_status("Restored layout");
    }

    Ok(())
}

/// Toggle shell pane visibility
fn toggle_shell_pane(state: &mut StatusbarState) -> Result<()> {
    let claude_target = format!("{}:.{{top-left}}", state.session_name);

    if state.shell_pane_visible {
        // Zoom the Claude pane
        Command::new("tmux")
            .args(["resize-pane", "-t", &claude_target, "-Z"])
            .output()?;
        state.shell_pane_visible = false;
        state.set_status("Claude zoomed (c to unzoom)");
    } else {
        // Unzoom to show all panes
        Command::new("tmux")
            .args(["resize-pane", "-t", &claude_target, "-Z"])
            .output()?;
        state.shell_pane_visible = true;
        state.set_status("Restored layout");
    }

    Ok(())
}

/// Get the merge-base commit between the worktree branch and main.
/// Returns the short SHA if found.
fn get_merge_base(state: &StatusbarState) -> Option<String> {
    // Try to find the main branch reference
    let main_ref = get_main_ref(&state.project_dir)?;

    // Get merge-base between current HEAD and main
    let output = Command::new("git")
        .current_dir(&state.worktree_path)
        .args(["merge-base", "HEAD", &main_ref])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let full_sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if full_sha.is_empty() {
        return None;
    }

    // Get short SHA for display
    let short_output = Command::new("git")
        .current_dir(&state.worktree_path)
        .args(["rev-parse", "--short", &full_sha])
        .output()
        .ok()?;

    if short_output.status.success() {
        Some(String::from_utf8_lossy(&short_output.stdout).trim().to_string())
    } else {
        // Fallback to first 7 chars
        Some(full_sha.chars().take(7).collect())
    }
}

/// Launch lazygit in the shell pane, focused on commits since the branch point.
fn launch_lazygit(state: &StatusbarState) -> Result<()> {
    let target = format!("{}:.{{top-right}}", state.session_name);

    // Clear line first (Ctrl-U), then launch lazygit with 'log' to focus on commits
    Command::new("tmux")
        .args(["send-keys", "-t", &target, "C-u"])
        .output()?;

    // Launch lazygit with 'log' positional arg to focus on commits panel
    Command::new("tmux")
        .args(["send-keys", "-t", &target, "lazygit log", "Enter"])
        .output()?;

    // Focus the shell pane so user can interact with lazygit
    Command::new("tmux")
        .args(["select-pane", "-t", &target])
        .output()?;

    Ok(())
}

/// Install lazygit using Homebrew
fn install_lazygit() -> Result<bool> {
    // Check if Homebrew is available
    let brew_available = Command::new("which")
        .arg("brew")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !brew_available {
        return Err(anyhow!("Homebrew not found - install lazygit manually"));
    }

    // Install lazygit via Homebrew
    let result = Command::new("brew")
        .args(["install", "lazygit"])
        .output()?;

    Ok(result.status.success())
}

/// Run the statusbar TUI
///
/// `parent_session` is the explicit parent session name passed via `--parent` argument.
pub fn run(task_id: &str, worktree_path: PathBuf, parent_session: Option<String>) -> Result<()> {
    // Setup terminal - DON'T use alternate screen for a minimal statusbar
    // Alternate screen mode doesn't work well with very small panes
    enable_raw_mode()?;
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Clear the terminal area, hide cursor, and enable mouse capture for click handling
    execute!(io::stdout(), Hide, EnableMouseCapture)?;

    let mut state = StatusbarState::new(task_id.to_string(), worktree_path, parent_session);

    // Initial git status refresh
    state.refresh_git_status();

    let result = run_loop(&mut terminal, &mut state);

    // Restore terminal
    execute!(io::stdout(), Show, DisableMouseCapture)?;
    disable_raw_mode()?;

    result
}

/// Main event loop
fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut StatusbarState,
) -> Result<()> {
    loop {
        // Auto-refresh git status every 30 seconds
        if state.last_refresh.elapsed() > Duration::from_secs(30) {
            state.refresh_git_status();
        }

        // Clear old status messages
        state.clear_old_status();

        // Ensure pane stays at 2 lines (handles screen dimension changes)
        state.enforce_pane_height();

        // Render
        let area = terminal.draw(|f| render(f, state))?.area;

        // Handle events with timeout (for periodic refresh)
        if event::poll(Duration::from_millis(500))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        if handle_key(state, key.code) {
                            break;
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    // Handle click on KANBLAM logo/text (right side of statusbar)
                    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                        let row = mouse.row;
                        let col = mouse.column;
                        let (start_col, end_col) = get_kanblam_click_region(area.width);
                        let use_art = area.width >= MIN_WIDTH_FOR_ART_LOGO;

                        // When using 2-line art, clicks on either line in the logo region count
                        // When using plain text, only clicks on line 2 count
                        let is_logo_click = if use_art {
                            (row == 0 || row == 1) && col >= start_col && col < end_col
                        } else {
                            row == 1 && area.height >= 2 && col >= start_col && col < end_col
                        };

                        if is_logo_click {
                            // Click on KANBLAM - go back to Kanblam
                            if let Some(parent) = state.parent_session.clone() {
                                state.set_status("Switching to Kanblam...");
                                let _ = Command::new("tmux")
                                    .args(["switch-client", "-t", &parent])
                                    .output();
                            } else {
                                state.set_status("No parent Kanblam session found");
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Entry point for the statusbar subcommand
pub fn main(args: &[String]) -> Result<()> {
    if args.is_empty() {
        return Err(anyhow!("Usage: kanblam statusbar <task-id> [--parent <session-name>]"));
    }

    let task_id = &args[0];

    // Parse optional --parent argument
    let parent_session = if args.len() >= 3 && args[1] == "--parent" {
        Some(args[2].clone())
    } else {
        None
    };

    // Get worktree path from current directory or construct it
    let worktree_path = std::env::current_dir()?;

    run(task_id, worktree_path, parent_session)
}
