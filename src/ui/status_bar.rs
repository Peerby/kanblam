use crate::app::App;
use crate::model::MainWorktreeOperation;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Render the status bar with project info and summary
pub fn render_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    // If there's a pending confirmation, show it prominently (unless it's multiline - then it's a modal)
    if let Some(ref confirmation) = app.model.ui_state.pending_confirmation {
        // Skip multiline messages - they're rendered as modals instead
        if !confirmation.message.contains('\n') {
            render_confirmation_prompt(frame, area, &confirmation.message, confirmation.animation_tick);
            return;
        }
        // For multiline, show a simple hint in the status bar
        render_confirmation_prompt(frame, area, " [y] Confirm  [n/Esc] Cancel ", confirmation.animation_tick);
        return;
    }

    // If there's a status message, show it
    if let Some(ref msg) = app.model.ui_state.status_message {
        let status = Paragraph::new(Span::styled(
            format!(" {} ", msg),
            Style::default()
                .fg(Color::White)
                .bg(Color::Blue),
        ));
        frame.render_widget(status, area);
        return;
    }

    // Show startup navigation hints for the first ~10 seconds
    if let Some(remaining) = app.model.ui_state.startup_hint_until_tick {
        render_startup_hints(frame, area, remaining);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(20),      // Project info
            Constraint::Length(30),   // Summary stats
        ])
        .split(area);

    // Render project info
    render_project_info(frame, chunks[0], app);

    // Render summary
    render_summary(frame, chunks[1], app);
}

/// Render project info for the current project
fn render_project_info(frame: &mut Frame, area: Rect, app: &App) {
    let Some(project) = app.model.active_project() else {
        let no_project = Paragraph::new(Span::styled(
            " No project selected ",
            Style::default().fg(Color::DarkGray),
        ));
        frame.render_widget(no_project, area);
        return;
    };

    // Count active tasks
    let active_count = project.tasks.iter()
        .filter(|t| t.session_state.is_active())
        .count();

    // Get current git branch
    let branch_name = get_current_branch(&project.working_dir);

    let mut spans = Vec::new();
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        &project.name,
        Style::default().fg(Color::Cyan),
    ));

    // Show git branch if available
    if let Some(branch) = branch_name {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            "\u{e0a0}", // Nerd Font git branch icon
            Style::default().fg(Color::Magenta),
        ));
        spans.push(Span::styled(
            format!(" {}", branch),
            Style::default().fg(Color::Magenta),
        ));
    }

    if active_count > 0 {
        spans.push(Span::styled(
            format!(" ({} active Claude session{})", active_count, if active_count == 1 { "" } else { "s" }),
            Style::default().fg(Color::Green),
        ));
    }

    // Show main worktree lock indicator if locked
    if let Some((task_id, operation, task_title)) = project.main_worktree_lock_info() {
        let op_text = match operation {
            MainWorktreeOperation::Accepting => "accepting",
            MainWorktreeOperation::Applying => "applying",
        };
        // Format as "[abc123] title trunc.."
        let short_id = &task_id.to_string()[..6];
        let truncated_title: String = if task_title.len() > 15 {
            format!("{}..", &task_title[..13])
        } else {
            task_title
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("{} [{}] {}", op_text, short_id, truncated_title),
            Style::default()
                .fg(Color::Rgb(0, 0, 100))  // Dark blue for better contrast on yellow
                .bg(Color::Yellow),
        ));
    }

    let info = Paragraph::new(ratatui::text::Line::from(spans));
    frame.render_widget(info, area);
}

/// Get the current git branch name for a directory
fn get_current_branch(working_dir: &std::path::Path) -> Option<String> {
    std::process::Command::new("git")
        .current_dir(working_dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Render summary statistics
fn render_summary(frame: &mut Frame, area: Rect, app: &App) {
    let review_count = app.model.projects_needing_attention();

    let summary = if review_count > 0 {
        Span::styled(
            format!(" Review: {} ", review_count),
            Style::default()
                .fg(Color::White)
                .bg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " Ready ",
            Style::default().fg(Color::Green),
        )
    };

    let summary_widget = Paragraph::new(summary).alignment(Alignment::Right);
    frame.render_widget(summary_widget, area);
}

/// Render startup navigation hints (shown for first ~10 seconds)
/// remaining: ticks remaining (100 = just started, 0 = about to disappear)
fn render_startup_hints(frame: &mut Frame, area: Rect, remaining: usize) {
    let width = area.width as usize;

    // Animation: light sweeps from left to right during first ~2 seconds (20 ticks)
    // The light position moves across the width of the screen
    let ticks_elapsed = 100_usize.saturating_sub(remaining);
    let animation_duration = 20; // ticks for the sweep animation
    let light_pos = if ticks_elapsed < animation_duration {
        // Map elapsed ticks to position across screen width
        (ticks_elapsed * width) / animation_duration
    } else {
        usize::MAX // Animation complete, no light
    };
    let light_width = 8; // Width of the bright spot

    // First, fill the entire bar with yellow background
    let bg_style = Style::default().bg(Color::Yellow);
    let full_bg = " ".repeat(width);
    frame.render_widget(
        Paragraph::new(Span::styled(&full_bg, bg_style)),
        area,
    );

    // Build the hint text with character-by-character styling for the light effect
    let hint_text = " ↑↓←→ navigate   enter select   esc back   ^s settings   ? help ";
    let hint_chars: Vec<char> = hint_text.chars().collect();

    // Center the hint text
    let hint_width = hint_chars.len();
    let start_x = if width > hint_width {
        (width - hint_width) / 2
    } else {
        0
    };

    // Keys that should be bold (character positions in hint_text)
    let bold_ranges: Vec<(usize, usize)> = vec![
        (1, 5),   // ↑↓←→
        (17, 22), // enter
        (32, 35), // esc
        (43, 45), // ^s
        (56, 57), // ?
    ];

    let is_bold = |pos: usize| -> bool {
        bold_ranges.iter().any(|(start, end)| pos >= *start && pos < *end)
    };

    // Create spans for each character with appropriate styling
    let mut spans = Vec::new();

    for (i, ch) in hint_chars.iter().enumerate() {
        let screen_x = start_x + i;
        let dist_from_light = if light_pos == usize::MAX {
            usize::MAX
        } else if screen_x >= light_pos {
            screen_x - light_pos
        } else {
            light_pos - screen_x
        };

        // Determine background color based on distance from light
        let bg = if dist_from_light == 0 {
            Color::White // Brightest at center
        } else if dist_from_light <= light_width / 2 {
            Color::Rgb(255, 255, 200) // Near-white yellow
        } else if dist_from_light <= light_width {
            Color::Rgb(255, 255, 150) // Bright yellow
        } else {
            Color::Yellow // Base yellow
        };

        // Dark blue provides better contrast on yellow than black
        let mut style = Style::default().fg(Color::Rgb(0, 0, 100)).bg(bg);
        if is_bold(i) {
            style = style.add_modifier(Modifier::BOLD);
        }

        spans.push(Span::styled(ch.to_string(), style));
    }

    // Render centered hint text
    let hints = Line::from(spans);
    let hint_area = Rect {
        x: area.x + start_x as u16,
        y: area.y,
        width: hint_width.min(width) as u16,
        height: 1,
    };
    frame.render_widget(Paragraph::new(hints), hint_area);
}

/// Render confirmation prompt with highlight sweep animation
/// animation_tick: starts at 20, counts down to 0. Animation runs while > 0.
fn render_confirmation_prompt(frame: &mut Frame, area: Rect, message: &str, animation_tick: usize) {
    let width = area.width as usize;
    let animation_duration: usize = 20; // ticks for the sweep animation

    // Calculate light position based on animation tick
    // tick starts at 20, so elapsed = 20 - tick
    let ticks_elapsed = animation_duration.saturating_sub(animation_tick);
    let light_pos = if ticks_elapsed < animation_duration {
        // Map elapsed ticks to position across screen width
        (ticks_elapsed * width) / animation_duration
    } else {
        usize::MAX // Animation complete, no light
    };
    let light_width = 8; // Width of the bright spot

    // First, fill the entire bar with yellow background
    let bg_style = Style::default().bg(Color::Yellow);
    let full_bg = " ".repeat(width);
    frame.render_widget(
        Paragraph::new(Span::styled(&full_bg, bg_style)),
        area,
    );

    // Build the message text with character-by-character styling for the light effect
    let message_text = format!(" {} ", message);
    let message_chars: Vec<char> = message_text.chars().collect();
    let message_width = message_chars.len();

    // Left-align the message (start at x=0)
    let start_x = 0_usize;

    // Create spans for each character with appropriate styling
    let mut spans = Vec::new();

    for (i, ch) in message_chars.iter().enumerate() {
        let screen_x = start_x + i;
        let dist_from_light = if light_pos == usize::MAX {
            usize::MAX
        } else if screen_x >= light_pos {
            screen_x - light_pos
        } else {
            light_pos - screen_x
        };

        // Determine background color based on distance from light
        let bg = if dist_from_light == 0 {
            Color::White // Brightest at center
        } else if dist_from_light <= light_width / 2 {
            Color::Rgb(255, 255, 200) // Near-white yellow
        } else if dist_from_light <= light_width {
            Color::Rgb(255, 255, 150) // Bright yellow
        } else {
            Color::Yellow // Base yellow
        };

        // Dark blue provides better contrast on yellow than black
        let style = Style::default()
            .fg(Color::Rgb(0, 0, 100))
            .bg(bg)
            .add_modifier(Modifier::BOLD);

        spans.push(Span::styled(ch.to_string(), style));
    }

    // Render the message
    let message_line = Line::from(spans);
    let message_area = Rect {
        x: area.x + start_x as u16,
        y: area.y,
        width: message_width.min(width) as u16,
        height: 1,
    };
    frame.render_widget(Paragraph::new(message_line), message_area);
}
