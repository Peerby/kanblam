use crate::app::App;
use crate::model::FocusArea;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

/// Render the Claude output viewer pane
pub fn render_output(frame: &mut Frame, area: Rect, app: &App) {
    let is_focused = app.model.ui_state.focus == FocusArea::OutputViewer;

    let (title, content) = if let Some(project) = app.model.active_project() {
        if let Some(task) = project.in_progress_task() {
            // Show captured output if available
            let lines: Vec<Line> = if !project.captured_output.is_empty() {
                project.captured_output
                    .lines()
                    .map(|l| Line::from(l.to_string()))
                    .collect()
            } else {
                get_placeholder_output(&task.title)
            };

            (
                format!(" Claude: {} ", truncate(&task.title, 30)),
                lines,
            )
        } else if !project.captured_output.is_empty() {
            // Show last output even when no task in progress
            let lines: Vec<Line> = project.captured_output
                .lines()
                .map(|l| Line::from(l.to_string()))
                .collect();
            (
                " Claude Output (last run) ".to_string(),
                lines,
            )
        } else {
            (
                " Claude Output ".to_string(),
                vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "No task in progress",
                        Style::default().fg(Color::DarkGray),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Select a task and press Enter to start",
                        Style::default().fg(Color::DarkGray),
                    )),
                ],
            )
        }
    } else {
        (
            " Claude Output ".to_string(),
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No project selected",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Add a project or wait for auto-detection",
                    Style::default().fg(Color::DarkGray),
                )),
            ],
        )
    };

    let border_style = if is_focused {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(Span::styled(
            &title,
            if is_focused {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ))
        .borders(Borders::ALL)
        .border_style(border_style);

    let output = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(output, area);

    // Render status indicator in corner
    if let Some(project) = app.model.active_project() {
        if project.in_progress_task().is_some() {
            let indicator = Span::styled(
                " RUNNING ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );
            let indicator_area = Rect {
                x: area.x + area.width.saturating_sub(12),
                y: area.y,
                width: 10,
                height: 1,
            };
            frame.render_widget(Paragraph::new(indicator), indicator_area);
        } else if project.needs_attention {
            let indicator = Span::styled(
                " REVIEW ",
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            );
            let indicator_area = Rect {
                x: area.x + area.width.saturating_sub(10),
                y: area.y,
                width: 9,
                height: 1,
            };
            frame.render_widget(Paragraph::new(indicator), indicator_area);
        }
    }
}

/// Placeholder output for demo (will be replaced with actual tmux capture)
fn get_placeholder_output(task_title: &str) -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("> claude \"{}\"", task_title),
            Style::default().fg(Color::Green),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Claude is analyzing the task...",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(""),
        Line::from("This pane will show live output from the Claude Code"),
        Line::from("session running in tmux. The output is captured using"),
        Line::from("`tmux capture-pane` and streamed here in real-time."),
        Line::from(""),
        Line::from(Span::styled(
            "When Claude finishes, the task will automatically move",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "to the Review column and you'll hear a notification.",
            Style::default().fg(Color::DarkGray),
        )),
    ]
}

/// Truncate a string to a maximum length
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    } else {
        s.to_string()
    }
}
