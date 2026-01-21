//! Worktree statusbar - A minimal TUI that runs in a tmux pane alongside the shell
//! Provides git status, keybindings for common dev actions, and control of the tmux layout

#![allow(dead_code)]

use anyhow::{anyhow, Result};
use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        cursor::{Hide, Show},
        event::{self, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode},
    },
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

/// Render the statusbar
fn render(frame: &mut Frame, state: &StatusbarState) {
    let area = frame.area();

    // Build status line
    let mut spans = Vec::new();

    // Git branch indicator
    spans.push(Span::styled(
        " \u{e0a0} ", // Nerd Font git branch icon
        Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
    ));

    if let Some(ref branch) = state.branch_name {
        spans.push(Span::styled(
            branch.clone(),
            Style::default().fg(Color::Magenta),
        ));
    } else {
        spans.push(Span::styled(
            "unknown",
            Style::default().fg(Color::DarkGray),
        ));
    }

    // Ahead/behind indicator
    spans.push(Span::styled(
        " │ ",
        Style::default().fg(Color::DarkGray),
    ));

    if state.behind > 0 {
        spans.push(Span::styled(
            format!("↓{}", state.behind),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
    }

    if state.ahead > 0 {
        spans.push(Span::styled(
            format!("↑{}", state.ahead),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
    }

    if state.ahead == 0 && state.behind == 0 {
        spans.push(Span::styled(
            "✓ synced",
            Style::default().fg(Color::Green),
        ));
    }

    // Separator before keybindings
    spans.push(Span::styled(
        " │ ",
        Style::default().fg(Color::DarkGray),
    ));

    // Show status message if any, otherwise show keybinding hints
    if let Some((ref msg, _)) = state.status_message {
        spans.push(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    } else {
        // Keybinding hints
        let hints = [
            ("r", "rebase"),
            ("s", "start"),
            ("S", "stop"),
            ("c", "claude"),
            ("z", "shell"),
            ("b", "back"),
            ("k", "kill"),
        ];

        for (i, (key, action)) in hints.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" ", Style::default().fg(Color::DarkGray)));
            }
            spans.push(Span::styled(
                key.to_string(),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(":{}", action),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    // Dev running indicator
    if state.dev_running {
        spans.push(Span::styled(
            " │ ",
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::styled(
            "● running",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ));
    }

    let line = Line::from(spans);
    let paragraph = Paragraph::new(line).style(Style::default().bg(Color::Rgb(30, 30, 40)));

    frame.render_widget(paragraph, area);
}

/// Handle a key press, returns true if should quit
fn handle_key(state: &mut StatusbarState, key: KeyCode) -> bool {
    match key {
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

    // Clear the terminal area and hide cursor
    execute!(io::stdout(), Hide)?;

    let mut state = StatusbarState::new(task_id.to_string(), worktree_path, parent_session);

    // Initial git status refresh
    state.refresh_git_status();

    let result = run_loop(&mut terminal, &mut state);

    // Restore terminal
    execute!(io::stdout(), Show)?;
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
        terminal.draw(|f| render(f, state))?;

        // Handle events with timeout (for periodic refresh)
        if event::poll(Duration::from_millis(500))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if handle_key(state, key.code) {
                        break;
                    }
                }
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
