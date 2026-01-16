use crate::app::App;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::Paragraph,
    Frame,
};

/// Render the status bar with project info and summary
pub fn render_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    // If there's a pending confirmation, show it prominently (unless it's multiline - then it's a modal)
    if let Some(ref confirmation) = app.model.ui_state.pending_confirmation {
        // Skip multiline messages - they're rendered as modals instead
        if !confirmation.message.contains('\n') {
            let msg = Paragraph::new(Span::styled(
                format!(" {} ", confirmation.message),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            frame.render_widget(msg, area);
            return;
        }
        // For multiline, show a simple hint in the status bar
        let msg = Paragraph::new(Span::styled(
            " [y] Confirm  [n/Esc] Cancel ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(msg, area);
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
