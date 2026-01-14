mod kanban;
mod output;
mod status_bar;

use crate::app::App;
use crate::model::FocusArea;
use edtui::{EditorTheme, EditorView};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub use kanban::render_kanban;
pub use output::render_output;
pub use status_bar::render_status_bar;

/// Main view function - renders the entire UI
/// In tmux-split mode, we only render the kanban board (left pane)
/// The Claude session runs in an actual tmux pane on the right
pub fn view(frame: &mut Frame, app: &mut App) {
    // Calculate dynamic input height based on content
    // Account for wrapped lines by estimating visual rows
    let frame_width = frame.area().width.saturating_sub(4) as usize; // Account for borders

    // Determine if we're showing a preview or the editor
    let is_editing = app.model.ui_state.editing_task_id.is_some()
        || app.model.ui_state.editing_divider_id.is_some();
    let show_preview = app.model.ui_state.focus == FocusArea::KanbanBoard
        && app.model.ui_state.selected_task_idx.is_some()
        && !app.model.ui_state.selected_is_divider
        && !app.model.ui_state.selected_is_divider_above
        && !is_editing;

    let input_height = if show_preview {
        // Calculate height based on selected task content
        let task_content = app.model.active_project().and_then(|project| {
            let tasks = project.tasks_by_status(app.model.ui_state.selected_column);
            app.model.ui_state.selected_task_idx.and_then(|idx| {
                tasks.get(idx).map(|task| {
                    let mut content = task.title.clone();
                    if !task.description.is_empty() {
                        content.push_str("\n\n");
                        content.push_str(&task.description);
                    }
                    if !task.images.is_empty() {
                        content.push_str("\n\n[images]");
                    }
                    content
                })
            })
        }).unwrap_or_default();
        calculate_input_height(&task_content, frame_width)
    } else {
        // Calculate height based on editor content
        calculate_input_height(&app.model.ui_state.editor_state.lines.to_string(), frame_width)
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),            // Project bar (top)
            Constraint::Min(10),              // Main content (Kanban board)
            Constraint::Length(input_height), // Input area (dynamic)
            Constraint::Length(1),            // Status bar
        ])
        .split(frame.area());

    // Render project bar at top
    render_project_bar(frame, chunks[0], app);

    // Render kanban board (full width - tmux handles the split)
    render_kanban(frame, chunks[1], app);

    // Render task input area
    render_input(frame, chunks[2], app);

    // Render status bar
    render_status_bar(frame, chunks[3], app);

    // Render help overlay if active
    if app.model.ui_state.show_help {
        render_help(frame);
    }
}

/// Calculate the required height for the input area based on content
/// Accounts for wrapped lines and includes borders
fn calculate_input_height(content: &str, available_width: usize) -> u16 {
    const MIN_HEIGHT: u16 = 4;  // Minimum input area (2 lines + borders)
    const MAX_HEIGHT: u16 = 12; // Maximum input area to avoid taking over the screen

    if available_width == 0 {
        return MIN_HEIGHT;
    }

    let mut visual_lines = 0;
    for line in content.lines() {
        // Calculate how many visual rows this line takes when wrapped
        let line_width = line.chars().count();
        let wrapped_rows = if line_width == 0 {
            1 // Empty line still takes one row
        } else {
            (line_width + available_width - 1) / available_width
        };
        visual_lines += wrapped_rows;
    }

    // If content is empty, count as 1 line
    if visual_lines == 0 {
        visual_lines = 1;
    }

    // Add 2 for borders, and 1 extra line for cursor space
    let needed_height = (visual_lines + 3) as u16;

    needed_height.clamp(MIN_HEIGHT, MAX_HEIGHT)
}

/// Render the project bar at the top of the screen
fn render_project_bar(frame: &mut Frame, area: Rect, app: &App) {
    if app.model.projects.is_empty() {
        let no_projects = Paragraph::new(Span::styled(
            " No projects - waiting for Claude sessions... ",
            Style::default().fg(Color::DarkGray),
        ));
        frame.render_widget(no_projects, area);
        return;
    }

    let mut spans = Vec::new();
    spans.push(Span::raw(" "));

    for (idx, project) in app.model.projects.iter().enumerate() {
        let is_active = idx == app.model.active_project_idx;

        // Build project name with attention indicator
        let name = if project.needs_attention {
            format!("{}*", project.name)
        } else {
            project.name.clone()
        };

        let style = if is_active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if project.needs_attention {
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };

        // Add keyboard shortcut hint (1-9)
        let tab_text = if idx < 9 {
            format!(" [{}] {} ", idx + 1, name)
        } else {
            format!(" {} ", name)
        };

        spans.push(Span::styled(tab_text, style));

        if idx < app.model.projects.len() - 1 {
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        }
    }

    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}

/// Render the task input area using edtui, or show task preview when a task is selected
fn render_input(frame: &mut Frame, area: Rect, app: &mut App) {
    let is_focused = app.model.ui_state.focus == FocusArea::TaskInput;
    let is_editing_task = app.model.ui_state.editing_task_id.is_some();
    let is_editing_divider = app.model.ui_state.editing_divider_id.is_some();
    let is_editing = is_editing_task || is_editing_divider;

    // Check if we should show a task preview instead of the editor
    // Show preview when: focused on kanban, task selected (not divider), not editing
    let show_preview = app.model.ui_state.focus == FocusArea::KanbanBoard
        && app.model.ui_state.selected_task_idx.is_some()
        && !app.model.ui_state.selected_is_divider
        && !app.model.ui_state.selected_is_divider_above
        && !is_editing;

    if show_preview {
        render_task_preview(frame, area, app);
        return;
    }

    // Choose title based on whether we're editing or creating, show image indicator
    let pending_count = app.model.ui_state.pending_images.len();
    let title = if is_editing_divider {
        " Divider Title ".to_string()
    } else if is_editing_task {
        " Edit Task ".to_string()
    } else if pending_count > 0 {
        format!(" New Task [+{} img] ", pending_count)
    } else {
        " New Task ".to_string()
    };

    // Choose colors based on focus and edit state
    let (border_color, text_color) = if is_focused {
        let color = if is_editing { Color::Magenta } else { Color::Yellow };
        (color, color)
    } else {
        (Color::DarkGray, Color::DarkGray)
    };

    // Create the block for the editor
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            title,
            if is_focused {
                Style::default().fg(border_color).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(border_color)
            },
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Configure the editor theme
    let theme = EditorTheme::default()
        .base(Style::default().fg(text_color))
        .cursor_style(if is_focused {
            Style::default().bg(text_color).fg(Color::Black)
        } else {
            Style::default()
        });

    // Render the editor with wrap enabled
    let editor_state = &mut app.model.ui_state.editor_state;
    EditorView::new(editor_state)
        .wrap(true)
        .theme(theme)
        .render(inner, frame.buffer_mut());

    // Render hints at bottom-right of the border
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::DarkGray);
    let hints = if pending_count > 0 {
        // Show image management hints when images are pending
        Line::from(vec![
            Span::styled("^V", key_style),
            Span::styled("+img ", desc_style),
            Span::styled("^X", key_style),
            Span::styled("-1 ", desc_style),
            Span::styled("^U", key_style),
            Span::styled("clr ", desc_style),
            Span::styled("⏎", key_style),
            Span::styled(" submit ", desc_style),
            Span::styled("\\⏎", key_style),
            Span::styled(" newline ", desc_style),
        ])
    } else {
        Line::from(vec![
            Span::styled("^V", key_style),
            Span::styled(" img  ", desc_style),
            Span::styled("^C", key_style),
            Span::styled(" cancel  ", desc_style),
            Span::styled("⏎", key_style),
            Span::styled(" submit ", desc_style),
            Span::styled("\\⏎", key_style),
            Span::styled(" newline ", desc_style),
        ])
    };
    let hints_width = 38u16; // Approximate width of hints text
    let hints_area = Rect {
        x: area.x + area.width.saturating_sub(hints_width + 1),
        y: area.y + area.height.saturating_sub(1),
        width: hints_width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(hints), hints_area);
}

/// Render a preview of the currently selected task
fn render_task_preview(frame: &mut Frame, area: Rect, app: &App) {
    // Get the selected task
    let task = app.model.active_project().and_then(|project| {
        let tasks = project.tasks_by_status(app.model.ui_state.selected_column);
        app.model.ui_state.selected_task_idx.and_then(|idx| tasks.get(idx).copied())
    });

    let Some(task) = task else {
        // No task selected, show empty preview
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(" Task Preview ", Style::default().fg(Color::DarkGray)));
        frame.render_widget(block, area);
        return;
    };

    // Get column color for the border
    let column_color = match app.model.ui_state.selected_column {
        crate::model::TaskStatus::Planned => Color::Blue,
        crate::model::TaskStatus::Queued => Color::Cyan,
        crate::model::TaskStatus::InProgress => Color::Yellow,
        crate::model::TaskStatus::NeedsInput => Color::Red,
        crate::model::TaskStatus::Review => Color::Magenta,
        crate::model::TaskStatus::Done => Color::Green,
    };

    // Build the title with step number and task status
    let title = match app.model.ui_state.selected_column {
        crate::model::TaskStatus::Planned => " 1. Planned ",
        crate::model::TaskStatus::Queued => " 2. Queued ",
        crate::model::TaskStatus::InProgress => " 3. In Progress ",
        crate::model::TaskStatus::NeedsInput => " 4. Needs Input ",
        crate::model::TaskStatus::Review => " 5. Review ",
        crate::model::TaskStatus::Done => " 6. Done ",
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(column_color))
        .title(Span::styled(
            title,
            Style::default().fg(column_color).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Build the preview content
    let mut lines: Vec<Line> = Vec::new();

    // Task title (bold)
    lines.push(Line::from(Span::styled(
        &task.title,
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    )));

    // Description if present
    if !task.description.is_empty() {
        lines.push(Line::from("")); // Empty line
        for desc_line in task.description.lines() {
            lines.push(Line::from(Span::styled(
                desc_line,
                Style::default().fg(Color::Gray),
            )));
        }
    }

    // Image count if present
    if !task.images.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("[{} image(s) attached]", task.images.len()),
            Style::default().fg(Color::Cyan),
        )));
    }

    let paragraph = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, inner);

    // Render action hints at bottom-right
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::DarkGray);
    let hints = Line::from(vec![
        Span::styled("e", key_style),
        Span::styled(" edit  ", desc_style),
        Span::styled("⏎", key_style),
        Span::styled(" start  ", desc_style),
        Span::styled("d", key_style),
        Span::styled(" delete ", desc_style),
    ]);
    let hints_width = 28u16;
    let hints_area = Rect {
        x: area.x + area.width.saturating_sub(hints_width + 1),
        y: area.y + area.height.saturating_sub(1),
        width: hints_width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(hints), hints_area);
}

/// Render help overlay
fn render_help(frame: &mut Frame) {
    let area = centered_rect(60, 80, frame.area());

    let help_text = vec![
        Line::from(Span::styled(
            "KanClaude Keyboard Shortcuts",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Navigation", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  h/l        Move left/right in row"),
        Line::from("  j/k        Move down/up in column"),
        Line::from("  !@#$%^     Jump to column 1-6"),
        Line::from("  Tab        Switch focus area"),
        Line::from("  1-9        Switch to project N"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Actions", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  i          Add new task"),
        Line::from("  e          Edit selected task"),
        Line::from("  Enter      Start selected task"),
        Line::from("  d          Delete selected task"),
        Line::from("  r          Move to Review"),
        Line::from("  x          Mark as Done"),
        Line::from("  +/-        Move task up/down"),
        Line::from("  |          Toggle divider below"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Input Editing (Vim-style)", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  Enter      Submit task"),
        Line::from("  \\+Enter   Line continuation (newline)"),
        Line::from("  Ctrl-V     Paste image from clipboard"),
        Line::from("  Ctrl-X     Remove last pasted image"),
        Line::from("  Ctrl-U     Clear all pasted images"),
        Line::from("  Ctrl-C     Cancel / unfocus"),
        Line::from("  Esc        Normal mode (vim)"),
        Line::from("  i/a/A      Insert mode"),
        Line::from("  h/l/w/b    Move cursor"),
        Line::from("  x/dd       Delete char/line"),
        Line::from("  u          Undo"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Sessions", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  o          Switch to Claude session"),
        Line::from("  [/]        Prev/Next session in project"),
        Line::from("  Ctrl-N     Spawn new Claude session"),
        Line::from("  R          Refresh (re-scan tmux)"),
        Line::from("  Ctrl-R     Install hooks & reload Claude"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Other", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  ?          Toggle this help"),
        Line::from("  q          Quit"),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to close",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let help = Paragraph::new(help_text)
        .block(
            Block::default()
                .title(" Help ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .style(Style::default().fg(Color::White));

    // Clear area first
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(help, area);
}

/// Helper function to create a centered rect
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
