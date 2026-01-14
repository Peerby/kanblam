use crate::app::App;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::Paragraph,
    Frame,
};

/// Render the status bar with project tabs and summary
pub fn render_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    // If there's a pending confirmation, show it prominently
    if let Some(ref confirmation) = app.model.ui_state.pending_confirmation {
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
            Constraint::Min(20),      // Session info
            Constraint::Length(30),   // Summary stats
        ])
        .split(area);

    // Render session info for current project
    render_session_info(frame, chunks[0], app);

    // Render summary
    render_summary(frame, chunks[1], app);
}

/// Render session info for the current project
fn render_session_info(frame: &mut Frame, area: Rect, app: &App) {
    let Some(project) = app.model.active_project() else {
        let no_project = Paragraph::new(Span::styled(
            " No project selected ",
            Style::default().fg(Color::DarkGray),
        ));
        frame.render_widget(no_project, area);
        return;
    };

    let mut spans = Vec::new();
    spans.push(Span::raw(" "));

    // Show tmux sessions for this project
    if project.tmux_sessions.is_empty() {
        spans.push(Span::styled(
            "No Claude sessions",
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::styled(
            " - press Enter on a task to start one",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        spans.push(Span::styled("Claude sessions: ", Style::default().fg(Color::Gray)));

        for (idx, _session) in project.tmux_sessions.iter().enumerate() {
            let is_active = idx == project.active_session_idx;

            let style = if is_active {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };

            // Show user-friendly session number (1-9)
            let session_text = format!(" {} ", idx + 1);
            spans.push(Span::styled(session_text, style));
        }

        // Show keyboard hint for switching
        if project.tmux_sessions.len() > 1 {
            spans.push(Span::styled(
                " [/] to switch",
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    let info = Paragraph::new(ratatui::text::Line::from(spans));
    frame.render_widget(info, area);
}

/// Render summary statistics
fn render_summary(frame: &mut Frame, area: Rect, app: &App) {
    let review_count = app.model.projects_needing_attention();

    // Check if active project has hooks installed and up to date
    let hooks_status = app.model.active_project()
        .map(|p| if p.hooks_installed { "" } else { " [hooks missing/outdated]" })
        .unwrap_or("");

    let summary = if review_count > 0 {
        Span::styled(
            format!(" Review: {}{} ", review_count, hooks_status),
            Style::default()
                .fg(Color::White)
                .bg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )
    } else if !hooks_status.is_empty() {
        Span::styled(
            format!(" {}- Ctrl-R to install ", hooks_status.trim()),
            Style::default()
                .fg(Color::Yellow),
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
