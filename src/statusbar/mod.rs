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

/// Tmux key bindings for pane navigation
#[derive(Clone)]
pub struct TmuxKeys {
    /// Prefix key (e.g., "C-b" or "C-a")
    pub prefix: String,
    /// Key to move to pane above
    pub pane_up: String,
    /// Key to move to pane below
    pub pane_down: String,
    /// Key to move to left pane
    pub pane_left: String,
    /// Key to move to right pane
    pub pane_right: String,
}

impl Default for TmuxKeys {
    fn default() -> Self {
        Self {
            prefix: "^b".to_string(),
            pane_up: "↑".to_string(),
            pane_down: "↓".to_string(),
            pane_left: "←".to_string(),
            pane_right: "→".to_string(),
        }
    }
}

impl TmuxKeys {
    /// Query tmux for current key bindings, falling back to defaults if unavailable
    pub fn from_tmux() -> Self {
        let mut keys = Self::default();

        // Get prefix key
        if let Some(prefix) = get_tmux_option("prefix") {
            keys.prefix = format_tmux_key(&prefix);
        }

        // Get pane navigation keys from key bindings
        // Look for select-pane bindings in the prefix table
        if let Ok(output) = Command::new("tmux")
            .args(["list-keys", "-T", "prefix"])
            .output()
        {
            if output.status.success() {
                let bindings = String::from_utf8_lossy(&output.stdout);
                for line in bindings.lines() {
                    // Parse lines like: bind-key -T prefix Up select-pane -U
                    // Format: bind-key    -T prefix    <key>    <command> [args]
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 5 && parts[0] == "bind-key" {
                        // Find the key and command
                        // Skip "bind-key -T prefix" and any flags
                        let mut key_idx = 3;
                        while key_idx < parts.len() && parts[key_idx].starts_with('-') {
                            // Skip flags like -r
                            key_idx += 1;
                            // If flag takes an argument (like -T), skip that too
                            if key_idx > 3 && !parts[key_idx - 1].starts_with('-') {
                                continue;
                            }
                        }
                        if key_idx >= parts.len() {
                            continue;
                        }
                        let key = parts[key_idx];
                        let cmd_start = key_idx + 1;
                        if cmd_start >= parts.len() {
                            continue;
                        }

                        // Check for select-pane with direction
                        if parts[cmd_start] == "select-pane" && cmd_start + 1 < parts.len() {
                            let direction = parts[cmd_start + 1];
                            let formatted_key = format_tmux_key(key);
                            match direction {
                                "-U" => keys.pane_up = formatted_key,
                                "-D" => keys.pane_down = formatted_key,
                                "-L" => keys.pane_left = formatted_key,
                                "-R" => keys.pane_right = formatted_key,
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        keys
    }

    /// Format as a compact hint string for the status bar
    /// Returns something like "C-b + ←↑↓→" or "C-a + hjkl"
    pub fn format_hint(&self) -> String {
        // Check if using arrow keys or vim-style keys
        let nav_keys = if self.is_arrow_keys() {
            "←↑↓→".to_string()
        } else if self.is_vim_keys() {
            format!("{}{}{}{}", self.pane_left, self.pane_up, self.pane_down, self.pane_right)
        } else {
            // Mixed or custom - show all keys
            format!(
                "{}{}{}{}",
                self.pane_left, self.pane_down, self.pane_up, self.pane_right
            )
        };
        format!("{} {}", self.prefix, nav_keys)
    }

    fn is_arrow_keys(&self) -> bool {
        self.pane_up == "↑"
            && self.pane_down == "↓"
            && self.pane_left == "←"
            && self.pane_right == "→"
    }

    fn is_vim_keys(&self) -> bool {
        (self.pane_up == "k" || self.pane_up == "K")
            && (self.pane_down == "j" || self.pane_down == "J")
            && (self.pane_left == "h" || self.pane_left == "H")
            && (self.pane_right == "l" || self.pane_right == "L")
    }
}

/// Get a tmux option value
fn get_tmux_option(option: &str) -> Option<String> {
    let output = Command::new("tmux")
        .args(["show-options", "-gv", option])
        .output()
        .ok()?;

    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

/// Format a tmux key for display (e.g., "C-b" becomes "^b", "Left" becomes "←")
fn format_tmux_key(key: &str) -> String {
    match key {
        "Left" => "←".to_string(),
        "Right" => "→".to_string(),
        "Up" => "↑".to_string(),
        "Down" => "↓".to_string(),
        _ if key.starts_with("C-") => format!("^{}", &key[2..]),
        _ => key.to_string(),
    }
}

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
    /// Tmux key bindings for pane navigation
    pub tmux_keys: TmuxKeys,
    /// Whether this statusbar pane is the active pane (receiving keyboard input)
    pub pane_is_active: bool,
    /// Whether the help modal is currently shown
    pub help_modal_visible: bool,
    /// Height of the help modal (for pane resizing)
    pub help_modal_height: u16,
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

        // Query tmux for keybindings
        let tmux_keys = TmuxKeys::from_tmux();

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
            tmux_keys,
            pane_is_active: false,
            help_modal_visible: false,
            help_modal_height: HELP_MODAL_HEIGHT,
        }
    }

    /// Check if the statusbar pane is the active pane in the tmux session
    pub fn check_pane_active(&mut self) {
        // Query tmux to see if the bottom pane (statusbar) is active
        let target = format!("{}:.{{bottom}}", self.session_name);
        if let Ok(output) = Command::new("tmux")
            .args(["display-message", "-t", &target, "-p", "#{pane_active}"])
            .output()
        {
            if output.status.success() {
                let active = String::from_utf8_lossy(&output.stdout).trim() == "1";
                self.pane_is_active = active;
            }
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

    /// Check pane height and resize if needed.
    /// Normal mode: 2 lines. Help modal mode: help_modal_height lines.
    /// This handles cases where the tmux layout changes (e.g., different terminal
    /// attaches to the session with different dimensions).
    pub fn enforce_pane_height(&mut self) {
        // Don't enforce height while help modal is visible - we want it to stay expanded
        if self.help_modal_visible {
            return;
        }

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

/// Height of the help modal (lines needed to display all help text)
const HELP_MODAL_HEIGHT: u16 = 16;

/// Key hint definition with full and abbreviated forms
struct KeyHint {
    key: &'static str,
    full: &'static str,
    abbrev: &'static str,
}

/// Get the ordered list of key hints
/// Order: start / stop / test / claude / shell / git / kill / rebase / commit / ?
fn get_key_hints() -> Vec<KeyHint> {
    vec![
        KeyHint { key: "s", full: "start", abbrev: "sta" },
        KeyHint { key: "S", full: "stop", abbrev: "stp" },
        KeyHint { key: "t", full: "test", abbrev: "tst" },
        KeyHint { key: "c", full: "claude", abbrev: "cla" },
        KeyHint { key: "z", full: "shell", abbrev: "shl" },
        KeyHint { key: "g", full: "git", abbrev: "git" },
        KeyHint { key: "k", full: "kill", abbrev: "kil" },
        KeyHint { key: "r", full: "rebase", abbrev: "reb" },
        KeyHint { key: "C", full: "commit", abbrev: "cmt" },
        KeyHint { key: "?", full: "help", abbrev: "?" },
    ]
}

/// Calculate the width needed for key hints with full labels
fn hints_full_width(hints: &[KeyHint]) -> usize {
    // Format: "k:action k:action ..." (space between, no trailing space)
    hints.iter().map(|h| h.key.len() + 1 + h.full.len()).sum::<usize>() + hints.len().saturating_sub(1)
}

/// Calculate the width needed for key hints with abbreviated labels
fn hints_abbrev_width(hints: &[KeyHint]) -> usize {
    hints.iter().map(|h| h.key.len() + 1 + h.abbrev.len()).sum::<usize>() + hints.len().saturating_sub(1)
}

/// Build key hint spans based on available width
fn build_key_hint_spans(available_width: usize) -> Vec<Span<'static>> {
    let hints = get_key_hints();
    let full_width = hints_full_width(&hints);
    let abbrev_width = hints_abbrev_width(&hints);

    // Decide whether to use full or abbreviated labels
    let use_abbrev = available_width < full_width;

    // If we can't even fit abbreviated, show as many as we can
    let max_hints = if available_width < abbrev_width {
        // Calculate how many hints we can fit
        let mut count = 0;
        let mut width = 0;
        for (i, hint) in hints.iter().enumerate() {
            let hint_width = hint.key.len() + 1 + hint.abbrev.len();
            let needed = if i > 0 { hint_width + 1 } else { hint_width };
            if width + needed > available_width {
                break;
            }
            width += needed;
            count += 1;
        }
        count
    } else {
        hints.len()
    };

    let mut spans = Vec::new();
    for (i, hint) in hints.iter().take(max_hints).enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(
            hint.key.to_string(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
        let label = if use_abbrev { hint.abbrev } else { hint.full };
        spans.push(Span::styled(
            format!(":{}", label),
            Style::default().fg(Color::DarkGray),
        ));
    }

    spans
}

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

/// Render the statusbar (2 lines normally, more when help modal is visible)
/// When there's enough width: 2-line KANBLAM art on the right of both lines
/// Otherwise: status on line 1, plain "KANBLAM" on line 2
fn render(frame: &mut Frame, state: &StatusbarState) {
    let area = frame.area();

    // If help modal is visible, render it instead of the normal statusbar
    if state.help_modal_visible {
        render_help_modal(frame, area);
        return;
    }
    // Use a highlighted background when this pane is active to indicate it receives keyboard input
    // Active color uses a warm tint from the mascot palette (dark magenta/orange)
    let bg_color = if state.pane_is_active {
        Color::Rgb(70, 35, 50) // Warm dark magenta - clearly active, matches mascot palette
    } else {
        Color::Rgb(30, 30, 40) // Normal dark background
    };
    let bg_style = Style::default().bg(bg_color);
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
        // Calculate available width for key hints
        // Account for: leading content (branch, ahead/behind, separator ~40 chars) + tmux hints (~20 chars) + logo (~35 chars)
        let status_prefix_width: usize = status_spans.iter().map(|s| s.content.chars().count()).sum();
        let tmux_hint_width = " │ panes:".len() + state.tmux_keys.format_hint().len();
        let logo_reservation = if area.width >= MIN_WIDTH_FOR_ART_LOGO { KANBLAM_ART_WIDTH as usize + 2 } else { 0 };
        let available_for_hints = (area.width as usize)
            .saturating_sub(status_prefix_width)
            .saturating_sub(tmux_hint_width)
            .saturating_sub(logo_reservation)
            .saturating_sub(5); // Safety margin

        // Build key hints with dynamic abbreviation
        let hint_spans = build_key_hint_spans(available_for_hints);
        status_spans.extend(hint_spans);

        // Tmux pane navigation hints
        status_spans.push(Span::styled(
            " │ panes:",
            Style::default().fg(Color::DarkGray),
        ));
        status_spans.push(Span::styled(
            state.tmux_keys.format_hint(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
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
        KeyCode::Char('t') => {
            // Run tests (auto-detected)
            if let Some(cmd) = detect_test_command(&state.worktree_path) {
                state.set_status(&format!("Running: {}", cmd));
                if let Err(e) = send_to_shell_pane(state, &cmd) {
                    state.set_status(&format!("Error: {}", e));
                }
            } else {
                state.set_status("No test command detected");
            }
        }
        KeyCode::Char('C') => {
            // Git commit - launch interactive commit in shell pane
            state.set_status("Opening git commit...");
            if let Err(e) = launch_git_commit(state) {
                state.set_status(&format!("Error: {}", e));
            }
        }
        KeyCode::Char('?') => {
            // Toggle help modal
            if state.help_modal_visible {
                // Hide help modal and resize pane back to 2 lines
                state.help_modal_visible = false;
                resize_statusbar_pane(state, 2);
            } else {
                // Show help modal and expand pane
                state.help_modal_visible = true;
                resize_statusbar_pane(state, state.help_modal_height);
            }
        }
        KeyCode::Char('q') | KeyCode::Esc => {
            // If help modal is open, close it instead of quitting
            if state.help_modal_visible {
                state.help_modal_visible = false;
                resize_statusbar_pane(state, 2);
            } else {
                // Just quit the statusbar (unusual, but allowed)
                return true;
            }
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

/// Detect the test command for the project
fn detect_test_command(worktree_path: &std::path::Path) -> Option<String> {
    // Check for package.json with test script
    if worktree_path.join("package.json").exists() {
        if let Ok(content) = std::fs::read_to_string(worktree_path.join("package.json")) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(scripts) = json.get("scripts") {
                    if scripts.get("test").is_some() {
                        return Some("npm test".to_string());
                    }
                }
            }
        }
    }

    // Check for Cargo.toml
    if worktree_path.join("Cargo.toml").exists() {
        return Some("cargo test".to_string());
    }

    // Check for Python (pytest or unittest)
    if worktree_path.join("pyproject.toml").exists() || worktree_path.join("pytest.ini").exists() {
        return Some("pytest".to_string());
    }

    // Check for Go
    if worktree_path.join("go.mod").exists() {
        return Some("go test ./...".to_string());
    }

    None
}

/// Launch git commit in the shell pane
fn launch_git_commit(state: &StatusbarState) -> Result<()> {
    let target = format!("{}:.{{top-right}}", state.session_name);

    // Clear line first (Ctrl-U), then launch git commit
    Command::new("tmux")
        .args(["send-keys", "-t", &target, "C-u"])
        .output()?;

    // Launch git commit (will open editor for commit message)
    Command::new("tmux")
        .args(["send-keys", "-t", &target, "git commit", "Enter"])
        .output()?;

    // Focus the shell pane so user can interact
    Command::new("tmux")
        .args(["select-pane", "-t", &target])
        .output()?;

    Ok(())
}

/// Resize the statusbar pane to the specified height
fn resize_statusbar_pane(state: &StatusbarState, height: u16) {
    let target = format!("{}:.{{bottom}}", state.session_name);
    let _ = Command::new("tmux")
        .args(["resize-pane", "-t", &target, "-y", &height.to_string()])
        .output();
}

/// Get detailed help text for the help modal
fn get_help_content() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("s", "start", "Run the auto-detected dev command (npm run dev, cargo run, etc.) in the shell pane"),
        ("S", "stop", "Send Ctrl-C to the shell pane to stop the running process"),
        ("t", "test", "Run the auto-detected test command (npm test, cargo test, etc.) in the shell pane"),
        ("c", "claude", "Toggle Claude pane visibility by zooming/unzooming the shell pane"),
        ("z", "shell", "Toggle shell pane visibility by zooming/unzooming the Claude pane"),
        ("g", "git", "Launch lazygit in the shell pane for interactive git operations"),
        ("k", "kill", "Kill this tmux session and return to the main Kanblam TUI"),
        ("r", "rebase", "Fetch origin/main and rebase the current branch onto it"),
        ("C", "commit", "Open git commit in the shell pane to commit staged changes"),
        ("b", "back", "Switch back to the main Kanblam TUI (keeps this session alive)"),
        ("?", "help", "Toggle this help modal"),
        ("q/Esc", "quit", "Close help modal (if open) or quit the statusbar"),
    ]
}

/// Render the help modal
fn render_help_modal(frame: &mut Frame, area: Rect) {
    let bg_color = Color::Rgb(40, 40, 55);
    let bg_style = Style::default().bg(bg_color);
    let green = Color::Rgb(80, 200, 120);
    let title_style = Style::default().fg(green).add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(Color::Yellow);
    let desc_style = Style::default().fg(Color::White);
    let dim_style = Style::default().fg(Color::DarkGray);

    let help_content = get_help_content();

    let mut lines = Vec::new();

    // Title line
    lines.push(Line::from(vec![
        Span::styled(" ", bg_style),
        Span::styled("KANBLAM STATUSBAR HELP", title_style),
        Span::styled(" (press ? or Esc to close)", dim_style),
    ]));

    // Blank line
    lines.push(Line::from(Span::styled(" ", bg_style)));

    // Help entries
    for (key, label, desc) in help_content {
        // Truncate description if needed to fit width
        let key_label_width = key.len() + 3 + label.len() + 3; // "  k  label  "
        let max_desc_width = (area.width as usize).saturating_sub(key_label_width + 2);
        let truncated_desc: String = if desc.len() > max_desc_width {
            format!("{}...", &desc[..max_desc_width.saturating_sub(3)])
        } else {
            desc.to_string()
        };

        lines.push(Line::from(vec![
            Span::styled("  ", bg_style),
            Span::styled(format!("{:>5}", key), key_style),
            Span::styled("  ", bg_style),
            Span::styled(format!("{:<8}", label), label_style),
            Span::styled(truncated_desc, desc_style),
        ]));
    }

    // Pad remaining lines with empty lines to fill the area
    while lines.len() < area.height as usize {
        lines.push(Line::from(Span::styled(" ", bg_style)));
    }

    let paragraph = Paragraph::new(lines).style(bg_style);
    frame.render_widget(paragraph, area);
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

        // Check if this pane is active (for visual feedback)
        state.check_pane_active();

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
