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

    let mut spans = Vec::new();
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        &project.name,
        Style::default().fg(Color::Cyan),
    ));

    if active_count > 0 {
        spans.push(Span::styled(
            format!(" ({} active Claude session{})", active_count, if active_count == 1 { "" } else { "s" }),
            Style::default().fg(Color::Green),
        ));
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
