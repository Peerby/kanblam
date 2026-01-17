mod interactive_modal;
mod kanban;
pub mod logo;
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
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

pub use interactive_modal::render_interactive_modal;
pub use kanban::render_kanban;
pub use output::render_output;
pub use status_bar::render_status_bar;

/// Main view function - renders the entire UI
/// In tmux-split mode, we only render the kanban board (left pane)
/// The Claude session runs in an actual tmux pane on the right
pub fn view(frame: &mut Frame, app: &mut App) {
    // Guard against extremely small terminals to prevent panics
    if frame.area().width < 20 || frame.area().height < 10 {
        let msg = Paragraph::new("Terminal too small")
            .style(Style::default().fg(Color::Red));
        frame.render_widget(msg, frame.area());
        return;
    }

    // Check if interactive modal is active - it takes over the entire screen
    if let Some(ref modal) = app.model.ui_state.interactive_modal {
        render_interactive_modal(frame, modal);
        return;
    }

    // Calculate dynamic input height based on editor content
    let frame_width = frame.area().width.saturating_sub(4) as usize; // Account for borders
    let input_height = calculate_input_height(&app.model.ui_state.editor_state.lines.to_string(), frame_width);

    // Determine header height based on available space
    // Show full 3-line logo header when terminal is wide enough and tall enough
    // (mascot overlays the project bar line to save vertical space)
    let logo_size = logo::get_logo_size(frame.area().width, frame.area().height);
    let show_full_header = matches!(logo_size, logo::LogoSize::Full | logo::LogoSize::Medium);
    let header_height = if show_full_header { 3 } else { 1 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),  // Header (project bar + optional logo)
            Constraint::Min(10),                // Main content (Kanban board)
            Constraint::Length(input_height),   // Input area (dynamic)
            Constraint::Length(1),              // Status bar
        ])
        .split(frame.area());

    // Render header area (project bar + logo)
    render_header(frame, chunks[0], app, logo_size);

    // Render kanban board (full width - tmux handles the split)
    render_kanban(frame, chunks[1], app);

    // Render mascot feet overlapping the kanban border (only when full/medium logo is shown)
    if show_full_header {
        // The feet should be rendered at the top row of the kanban area, right-aligned
        let feet_area = Rect {
            x: chunks[1].x,
            y: chunks[1].y,
            width: chunks[1].width,
            height: 1,
        };
        logo::render_mascot_feet(frame, feet_area, app.model.ui_state.logo_shimmer_frame, logo_size);
    }

    // Render task input area
    render_input(frame, chunks[2], app);

    // Render status bar
    render_status_bar(frame, chunks[3], app);

    // Render help overlay if active
    if app.model.ui_state.show_help {
        render_help(frame);
    }

    // Render queue dialog if active
    if app.model.ui_state.is_queue_dialog_open() {
        render_queue_dialog(frame, app);
    }

    // Render task preview modal if active
    if app.model.ui_state.show_task_preview {
        render_task_preview_modal(frame, app);
    }

    // Render open project dialog if active
    if app.model.ui_state.is_open_project_dialog_open() {
        render_open_project_dialog(frame, app);
    }

    // Render confirmation modal if pending confirmation has multiline message
    if let Some(ref confirmation) = app.model.ui_state.pending_confirmation {
        if confirmation.message.contains('\n') {
            render_confirmation_modal(frame, &confirmation.message);
        }
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

/// Render the header area (project bar + optional logo)
fn render_header(frame: &mut Frame, area: Rect, app: &App, logo_size: logo::LogoSize) {
    match logo_size {
        logo::LogoSize::Full => {
            // Render project bar on top-left (just first line)
            let project_bar_area = Rect {
                x: area.x,
                y: area.y,
                width: area.width.saturating_sub(logo::FULL_LOGO_WIDTH + 2),
                height: 1,
            };
            render_project_bar(frame, project_bar_area, app);

            // Render full logo using full area - it will right-align itself
            logo::render_logo_size(frame, area, app.model.ui_state.logo_shimmer_frame, logo_size, app.model.ui_state.eye_animation, app.model.ui_state.animation_frame);
        }
        logo::LogoSize::Medium => {
            // Render project bar on top-left (just first line) with more space
            let project_bar_area = Rect {
                x: area.x,
                y: area.y,
                width: area.width.saturating_sub(logo::MEDIUM_LOGO_WIDTH + 2),
                height: 1,
            };
            render_project_bar(frame, project_bar_area, app);

            // Render medium logo using full area - it will right-align itself
            logo::render_logo_size(frame, area, app.model.ui_state.logo_shimmer_frame, logo_size, app.model.ui_state.eye_animation, app.model.ui_state.animation_frame);
        }
        _ => {
            // Compact mode: project bar with inline branding
            render_project_bar_with_branding(frame, area, app);
        }
    }
}

/// Render the project bar at the top of the screen
fn render_project_bar(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = Vec::new();
    spans.push(Span::raw(" "));

    let is_focused = app.model.ui_state.focus == FocusArea::ProjectTabs;
    let selected_tab_idx = app.model.ui_state.selected_project_tab_idx;
    let shift_chars = ['!', '@', '#', '$', '%', '^', '&', '*', '(', ')'];
    let num_projects = app.model.projects.len();

    // First: Show +project button (index 0 in tab selection)
    if num_projects < 9 {
        let is_tab_selected = is_focused && selected_tab_idx == 0;
        let style = if is_tab_selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        // Show "+project" when no projects exist, just "+" otherwise
        let label = if num_projects == 0 { " [!] +project " } else { " [!] + " };
        spans.push(Span::styled(label, style));
        spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
    }

    // Show existing projects (index 1+ in tab selection)
    for (idx, project) in app.model.projects.iter().enumerate() {
        let is_active = idx == app.model.active_project_idx;
        // Tab index is idx + 1 (since 0 is +project)
        let is_tab_selected = is_focused && selected_tab_idx == idx + 1;

        // Build project name with attention indicator
        let name = if project.needs_attention {
            format!("{}*", project.name)
        } else {
            project.name.clone()
        };

        let style = if is_tab_selected {
            // Highlighted selection (when navigating with arrows in ProjectTabs focus)
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if is_active {
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

        // Keyboard shortcut: @ for first project, # for second, etc. (! is for +project)
        let tab_text = if idx + 1 < 10 {
            format!(" [{}] {} ", shift_chars[idx + 1], name)
        } else {
            format!(" {} ", name)
        };

        spans.push(Span::styled(tab_text, style));
        spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
    }

    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}

/// Render the project bar with inline branding on the right
fn render_project_bar_with_branding(frame: &mut Frame, area: Rect, app: &App) {
    let green = Color::Rgb(80, 200, 120);
    let dark_green = Color::Rgb(60, 150, 90);

    let mut spans = Vec::new();
    spans.push(Span::raw(" "));

    let is_focused = app.model.ui_state.focus == FocusArea::ProjectTabs;
    let selected_tab_idx = app.model.ui_state.selected_project_tab_idx;
    let shift_chars = ['!', '@', '#', '$', '%', '^', '&', '*', '(', ')'];
    let num_projects = app.model.projects.len();

    // First: Show +project button (index 0 in tab selection)
    if num_projects < 9 {
        let is_tab_selected = is_focused && selected_tab_idx == 0;
        let style = if is_tab_selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        // Show "+project" when no projects exist, just "+" otherwise
        let label = if num_projects == 0 { " [!] +project " } else { " [!] + " };
        spans.push(Span::styled(label, style));
        spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
    }

    // Show existing projects (index 1+ in tab selection)
    for (idx, project) in app.model.projects.iter().enumerate() {
        let is_active = idx == app.model.active_project_idx;
        // Tab index is idx + 1 (since 0 is +project)
        let is_tab_selected = is_focused && selected_tab_idx == idx + 1;

        // Build project name with attention indicator
        let name = if project.needs_attention {
            format!("{}*", project.name)
        } else {
            project.name.clone()
        };

        let style = if is_tab_selected {
            // Highlighted selection (when navigating with arrows in ProjectTabs focus)
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if is_active {
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

        // Keyboard shortcut: @ for first project, # for second, etc. (! is for +project)
        let tab_text = if idx + 1 < 10 {
            format!(" [{}] {} ", shift_chars[idx + 1], name)
        } else {
            format!(" {} ", name)
        };

        spans.push(Span::styled(tab_text, style));
        spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
    }

    // Calculate remaining space for branding
    let project_bar_len: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let remaining = (area.width as usize).saturating_sub(project_bar_len);

    // Add branding on the right if there's space
    if remaining >= logo::COMPACT_LOGO_WIDTH as usize {
        let branding = "KANBLAM";
        let padding = remaining.saturating_sub(branding.len() + 1);
        spans.push(Span::raw(" ".repeat(padding)));
        spans.push(Span::styled(branding, Style::default().fg(green)));
    }

    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}

/// Render the task input area using edtui
fn render_input(frame: &mut Frame, area: Rect, app: &mut App) {
    let is_focused = app.model.ui_state.focus == FocusArea::TaskInput;
    let is_editing_task = app.model.ui_state.editing_task_id.is_some();
    let is_feedback_mode = app.model.ui_state.feedback_task_id.is_some();

    // Choose colors based on focus and mode
    let (border_color, text_color) = if is_focused {
        let color = if is_feedback_mode {
            Color::Cyan
        } else if is_editing_task {
            Color::Magenta
        } else {
            Color::Yellow
        };
        (color, color)
    } else {
        (Color::DarkGray, Color::DarkGray)
    };

    // Choose title based on mode
    let pending_count = app.model.ui_state.pending_images.len();
    let title_style = if is_focused {
        Style::default().fg(border_color).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(border_color)
    };

    let title = if is_feedback_mode {
        Line::from(Span::styled(" Feedback ", title_style))
    } else if is_editing_task {
        Line::from(Span::styled(" Edit Task ", title_style))
    } else if pending_count > 0 {
        Line::from(Span::styled(format!(" New Task [+{} img] ", pending_count), title_style))
    } else {
        Line::from(Span::styled(" New Task ", title_style))
    };

    // Create the block for the editor
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title);

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
    // Only show full hints when focused; when unfocused show insert hint + ^V
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::DarkGray);
    let (hints, hints_width) = if !is_focused {
        // When unfocused, show insert hint and paste image hint
        (
            Line::from(vec![
                Span::styled("i", key_style),
                Span::styled("nsert  ", desc_style),
                Span::styled("^V", key_style),
                Span::styled(" img", desc_style),
            ]),
            14u16,
        )
    } else if pending_count > 0 {
        // Show image management hints when images are pending
        (
            Line::from(vec![
                Span::styled("^V", key_style),
                Span::styled("+img ", desc_style),
                Span::styled("^X", key_style),
                Span::styled("-1 ", desc_style),
                Span::styled("^U", key_style),
                Span::styled("clr ", desc_style),
                Span::styled("^G", key_style),
                Span::styled(" vim ", desc_style),
                Span::styled("⏎", key_style),
                Span::styled(" submit", desc_style),
            ]),
            38u16,
        )
    } else {
        (
            Line::from(vec![
                Span::styled("^V", key_style),
                Span::styled(" img ", desc_style),
                Span::styled("^G", key_style),
                Span::styled(" vim ", desc_style),
                Span::styled("^C", key_style),
                Span::styled(" cancel ", desc_style),
                Span::styled("⏎", key_style),
                Span::styled(" submit", desc_style),
            ]),
            38u16,
        )
    };
    let hints_area = Rect {
        x: area.x + area.width.saturating_sub(hints_width + 1),
        y: area.y + area.height.saturating_sub(1),
        width: hints_width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(hints), hints_area);
}

/// Render the task preview modal (shown with v/space/enter)
/// Phase-aware modal showing contextual information and available actions
fn render_task_preview_modal(frame: &mut Frame, app: &App) {
    let area = centered_rect(75, 80, frame.area());

    // Get the selected task
    let task = app.model.active_project().and_then(|project| {
        let tasks = project.tasks_by_status(app.model.ui_state.selected_column);
        app.model.ui_state.selected_task_idx.and_then(|idx| tasks.get(idx).copied())
    });

    let Some(task) = task else {
        return;
    };

    // Get column color for the border
    let (column_color, phase_label) = match task.status {
        crate::model::TaskStatus::Planned => (Color::Blue, "Planned"),
        crate::model::TaskStatus::Queued => (Color::Cyan, "Queued"),
        crate::model::TaskStatus::InProgress => (Color::Yellow, "In Progress"),
        crate::model::TaskStatus::NeedsInput => (Color::Red, "Needs Input"),
        crate::model::TaskStatus::Review => (Color::Magenta, "Review"),
        crate::model::TaskStatus::Accepting => (Color::Magenta, "Accepting"),
        crate::model::TaskStatus::Updating => (Color::Magenta, "Updating"),
        crate::model::TaskStatus::Applying => (Color::Magenta, "Applying"),
        crate::model::TaskStatus::Done => (Color::Green, "Done"),
    };

    let mut lines: Vec<Line> = Vec::new();
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White);
    let dim_style = Style::default().fg(Color::DarkGray);

    // ═══════════════════════════════════════════════════════════════════════
    // HEADER: Title (short_title if available, otherwise title) and phase badge
    // ═══════════════════════════════════════════════════════════════════════
    let header_title = task.short_title.as_ref().unwrap_or(&task.title);
    lines.push(Line::from(vec![
        Span::styled(header_title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(format!("[{}]", phase_label), Style::default().fg(column_color).add_modifier(Modifier::BOLD)),
    ]));

    // ═══════════════════════════════════════════════════════════════════════
    // FULL TITLE (if short_title exists, show the original title)
    // ═══════════════════════════════════════════════════════════════════════
    if task.short_title.is_some() {
        lines.push(Line::from(""));
        // Show full title in a slightly different style
        for title_line in task.title.lines() {
            lines.push(Line::from(Span::styled(title_line, Style::default().fg(Color::White))));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // DESCRIPTION (if any)
    // ═══════════════════════════════════════════════════════════════════════
    if !task.description.is_empty() {
        lines.push(Line::from(""));
        for desc_line in task.description.lines() {
            lines.push(Line::from(Span::styled(desc_line, Style::default().fg(Color::Gray))));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // ATTACHMENTS
    // ═══════════════════════════════════════════════════════════════════════
    if !task.images.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("📎 ", dim_style),
            Span::styled(format!("{} image(s) attached", task.images.len()), Style::default().fg(Color::Cyan)),
        ]));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // PHASE-SPECIFIC INFORMATION
    // ═══════════════════════════════════════════════════════════════════════
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("─".repeat(40), dim_style)));

    match task.status {
        crate::model::TaskStatus::Planned | crate::model::TaskStatus::Queued => {
            // Show creation time and queue info
            lines.push(Line::from(vec![
                Span::styled("Created: ", label_style),
                Span::styled(format_datetime(task.created_at), value_style),
            ]));

            if task.status == crate::model::TaskStatus::Queued {
                if let Some(queued_for) = task.queued_for_session {
                    if let Some(project) = app.model.active_project() {
                        if let Some(parent_task) = project.tasks.iter().find(|t| t.id == queued_for) {
                            lines.push(Line::from(vec![
                                Span::styled("Queued for: ", label_style),
                                Span::styled(&parent_task.title, Style::default().fg(Color::Yellow)),
                            ]));
                        }
                    }
                }
            }
        }

        crate::model::TaskStatus::InProgress => {
            // Show session state and timing
            if let Some(started) = task.started_at {
                let duration = chrono::Utc::now().signed_duration_since(started);
                lines.push(Line::from(vec![
                    Span::styled("Running for: ", label_style),
                    Span::styled(format_duration(duration), Style::default().fg(Color::Yellow)),
                ]));
            }

            // Session state with color
            let (state_label, state_color) = match task.session_state {
                crate::model::ClaudeSessionState::Creating => ("Creating worktree...", Color::Yellow),
                crate::model::ClaudeSessionState::Starting => ("Starting session...", Color::Yellow),
                crate::model::ClaudeSessionState::Ready => ("Ready", Color::Green),
                crate::model::ClaudeSessionState::Working => ("Working", Color::Green),
                crate::model::ClaudeSessionState::Continuing => ("Continuing", Color::Cyan),
                _ => ("Unknown", Color::DarkGray),
            };
            lines.push(Line::from(vec![
                Span::styled("Session: ", label_style),
                Span::styled(state_label, Style::default().fg(state_color)),
            ]));

            // Last tool activity
            if let Some(ref tool_name) = task.last_tool_name {
                lines.push(Line::from(vec![
                    Span::styled("Last tool: ", label_style),
                    Span::styled(tool_name, value_style),
                ]));
            }
        }

        crate::model::TaskStatus::NeedsInput => {
            // Urgent - show waiting time
            if let Some(started) = task.started_at {
                let duration = chrono::Utc::now().signed_duration_since(started);
                lines.push(Line::from(vec![
                    Span::styled("⚠ ", Style::default().fg(Color::Red)),
                    Span::styled("Waiting for input since ", label_style),
                    Span::styled(format_duration(duration), Style::default().fg(Color::Red)),
                ]));
            }

            lines.push(Line::from(vec![
                Span::styled("Session: ", label_style),
                Span::styled("Paused - needs your input", Style::default().fg(Color::Red)),
            ]));
        }

        crate::model::TaskStatus::Review | crate::model::TaskStatus::Accepting | crate::model::TaskStatus::Updating | crate::model::TaskStatus::Applying => {
            // Show timing and branch info
            if let Some(started) = task.started_at {
                let duration = chrono::Utc::now().signed_duration_since(started);
                lines.push(Line::from(vec![
                    Span::styled("Total time: ", label_style),
                    Span::styled(format_duration(duration), value_style),
                ]));
            }

            if let Some(ref branch) = task.git_branch {
                lines.push(Line::from(vec![
                    Span::styled("Branch: ", label_style),
                    Span::styled(branch, Style::default().fg(Color::Green)),
                ]));
            }

            if task.status == crate::model::TaskStatus::Accepting {
                if let Some(accept_started) = task.accepting_started_at {
                    let elapsed = chrono::Utc::now().signed_duration_since(accept_started).num_seconds();
                    let tool_info = task.last_tool_name.as_deref().unwrap_or("merging");
                    lines.push(Line::from(vec![
                        Span::styled("⟳ ", Style::default().fg(Color::Yellow)),
                        Span::styled(format!("Rebasing ({}) {}s", tool_info, elapsed), Style::default().fg(Color::Yellow)),
                    ]));
                }
            } else if task.status == crate::model::TaskStatus::Updating {
                if let Some(activity_at) = task.last_activity_at {
                    let elapsed = chrono::Utc::now().signed_duration_since(activity_at).num_seconds();
                    let tool_info = task.last_tool_name.as_deref().unwrap_or("updating");
                    lines.push(Line::from(vec![
                        Span::styled("⟳ ", Style::default().fg(Color::Cyan)),
                        Span::styled(format!("Updating ({}) {}s", tool_info, elapsed), Style::default().fg(Color::Cyan)),
                    ]));
                }
            }
        }

        crate::model::TaskStatus::Done => {
            // Show completion info
            if let Some(completed) = task.completed_at {
                lines.push(Line::from(vec![
                    Span::styled("Completed: ", label_style),
                    Span::styled(format_datetime(completed), Style::default().fg(Color::Green)),
                ]));
            }

            if let (Some(started), Some(completed)) = (task.started_at, task.completed_at) {
                let duration = completed.signed_duration_since(started);
                lines.push(Line::from(vec![
                    Span::styled("Duration: ", label_style),
                    Span::styled(format_duration(duration), value_style),
                ]));
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // GIT STATUS - For tasks with worktrees
    // ═══════════════════════════════════════════════════════════════════════
    if task.worktree_path.is_some() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("─".repeat(40), dim_style)));
        lines.push(Line::from(Span::styled("Git Status", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))));
        lines.push(Line::from(""));

        // Show branch
        if let Some(ref branch) = task.git_branch {
            lines.push(Line::from(vec![
                Span::styled("Branch: ", label_style),
                Span::styled(branch, Style::default().fg(Color::Green)),
            ]));
        }

        // Show line changes with visual bar
        let total_changes = task.git_additions + task.git_deletions;
        if total_changes > 0 {
            // Create a visual bar showing proportion of additions vs deletions
            let bar_width = 20usize;
            let add_ratio = task.git_additions as f64 / total_changes as f64;
            let add_chars = (add_ratio * bar_width as f64).round() as usize;
            let del_chars = bar_width.saturating_sub(add_chars);

            let add_bar = "█".repeat(add_chars);
            let del_bar = "█".repeat(del_chars);

            lines.push(Line::from(vec![
                Span::styled("Changes: ", label_style),
                Span::styled(format!("+{}", task.git_additions), Style::default().fg(Color::Green)),
                Span::styled(" / ", dim_style),
                Span::styled(format!("-{}", task.git_deletions), Style::default().fg(Color::Red)),
                Span::styled("  ", dim_style),
                Span::styled(add_bar, Style::default().fg(Color::Green)),
                Span::styled(del_bar, Style::default().fg(Color::Red)),
            ]));

            lines.push(Line::from(vec![
                Span::styled("Files:   ", label_style),
                Span::styled(format!("{} changed", task.git_files_changed), value_style),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("Changes: ", label_style),
                Span::styled("No changes yet", dim_style),
            ]));
        }

        // Show commits ahead/behind with status indicator
        if task.git_commits_ahead > 0 || task.git_commits_behind > 0 {
            let mut commit_spans = vec![Span::styled("Commits: ", label_style)];

            if task.git_commits_ahead > 0 {
                commit_spans.push(Span::styled(
                    format!("↑{} ahead", task.git_commits_ahead),
                    Style::default().fg(Color::Cyan),
                ));
            }

            if task.git_commits_behind > 0 {
                if task.git_commits_ahead > 0 {
                    commit_spans.push(Span::styled("  ", dim_style));
                }
                commit_spans.push(Span::styled(
                    format!("↓{} behind", task.git_commits_behind),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ));
            }

            lines.push(Line::from(commit_spans));

            // Show warning if behind main
            if task.git_commits_behind > 0 {
                lines.push(Line::from(vec![
                    Span::styled("         ", label_style),
                    Span::styled("⚠ ", Style::default().fg(Color::Yellow)),
                    Span::styled("Main has new commits - press ", Style::default().fg(Color::Yellow)),
                    Span::styled("u", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::styled(" to update", Style::default().fg(Color::Yellow)),
                ]));
            }
        } else if total_changes > 0 {
            lines.push(Line::from(vec![
                Span::styled("Commits: ", label_style),
                Span::styled("✓ Up to date with main", Style::default().fg(Color::Green)),
            ]));
        }

        // Get and show changed files (limited to top 8)
        if let Some(project) = app.model.active_project() {
            if let Ok(files) = crate::worktree::get_worktree_changed_files(&project.working_dir, task.id) {
                if !files.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled("Changed Files:", label_style)));

                    let max_files = 8;
                    for (i, file) in files.iter().take(max_files).enumerate() {
                        // Truncate long paths
                        let max_path_len = 35;
                        let display_path = if file.path.len() > max_path_len {
                            format!("...{}", &file.path[file.path.len() - max_path_len + 3..])
                        } else {
                            file.path.clone()
                        };

                        let status_indicator = if file.is_new {
                            Span::styled(" (new)", Style::default().fg(Color::Green))
                        } else if file.is_deleted {
                            Span::styled(" (del)", Style::default().fg(Color::Red))
                        } else if file.is_renamed {
                            Span::styled(" (ren)", Style::default().fg(Color::Yellow))
                        } else {
                            Span::raw("")
                        };

                        let line_spans = vec![
                            Span::styled("  ", dim_style),
                            Span::styled(display_path, value_style),
                            status_indicator,
                            Span::styled(
                                format!("  +{}", file.additions),
                                Style::default().fg(Color::Green),
                            ),
                            Span::styled("/", dim_style),
                            Span::styled(
                                format!("-{}", file.deletions),
                                Style::default().fg(Color::Red),
                            ),
                        ];

                        lines.push(Line::from(line_spans));

                        // Show "and X more..." if truncated
                        if i == max_files - 1 && files.len() > max_files {
                            lines.push(Line::from(vec![
                                Span::styled("  ", dim_style),
                                Span::styled(
                                    format!("... and {} more files", files.len() - max_files),
                                    dim_style,
                                ),
                            ]));
                        }
                    }
                }
            }
        }
    }

    // Worktree path (collapsed for active tasks, shown for debugging)
    if let Some(ref wt_path) = task.worktree_path {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Path: ", label_style),
            Span::styled(wt_path.display().to_string(), dim_style),
        ]));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // ACTIONS - Phase-specific key hints
    // ═══════════════════════════════════════════════════════════════════════
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("─".repeat(40), dim_style)));
    lines.push(Line::from(Span::styled("Actions", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))));
    lines.push(Line::from(""));

    match task.status {
        crate::model::TaskStatus::Planned => {
            lines.push(Line::from(vec![
                Span::styled(" s ", key_style), Span::styled(" Start task with worktree isolation", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" q ", key_style), Span::styled(" Queue for running session", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" e ", key_style), Span::styled(" Edit task", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" d ", key_style), Span::styled(" Delete task", label_style),
            ]));
        }

        crate::model::TaskStatus::Queued => {
            lines.push(Line::from(vec![
                Span::styled(" s ", key_style), Span::styled(" Start immediately", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" e ", key_style), Span::styled(" Edit task", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" d ", key_style), Span::styled(" Delete task", label_style),
            ]));
        }

        crate::model::TaskStatus::InProgress => {
            lines.push(Line::from(vec![
                Span::styled(" s ", key_style), Span::styled(" Switch to Claude session", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" o ", key_style), Span::styled(" Open interactive modal", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" t ", key_style), Span::styled(" Open test shell in worktree", label_style),
            ]));
            if task.git_commits_behind > 0 {
                lines.push(Line::from(vec![
                    Span::styled(" u ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::styled(" Update: rebase onto latest main", Style::default().fg(Color::Yellow)),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled(" r ", key_style), Span::styled(" Move to review", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" x ", key_style), Span::styled(" Reset (cleanup and move to Planned)", label_style),
            ]));
        }

        crate::model::TaskStatus::NeedsInput => {
            lines.push(Line::from(vec![
                Span::styled(" s ", key_style), Span::styled(" Continue / switch to session", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" o ", key_style), Span::styled(" Open interactive modal", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" t ", key_style), Span::styled(" Open test shell", label_style),
            ]));
            if task.git_commits_behind > 0 {
                lines.push(Line::from(vec![
                    Span::styled(" u ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::styled(" Update: rebase onto latest main", Style::default().fg(Color::Yellow)),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled(" r ", key_style), Span::styled(" Move to review", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" x ", key_style), Span::styled(" Reset (cleanup and move to Planned)", label_style),
            ]));
        }

        crate::model::TaskStatus::Review => {
            lines.push(Line::from(vec![
                Span::styled(" a ", key_style), Span::styled(" Apply: test changes in main worktree", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" u ", key_style), Span::styled(" Unapply: remove applied changes", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" m ", key_style), Span::styled(" Merge: finalize changes and mark done", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" d ", key_style), Span::styled(" Discard: reject changes and mark done", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" c ", key_style), Span::styled(" Check: view git diff/status report", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" f ", key_style), Span::styled(" Feedback: send follow-up instructions", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" o ", key_style), Span::styled(" Open interactive modal", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" t ", key_style), Span::styled(" Open test shell", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" x ", key_style), Span::styled(" Reset (cleanup and move to Planned)", label_style),
            ]));
        }

        crate::model::TaskStatus::Accepting => {
            lines.push(Line::from(Span::styled(
                "  Task is being rebased onto main...",
                Style::default().fg(Color::Yellow),
            )));
        }

        crate::model::TaskStatus::Updating => {
            lines.push(Line::from(Span::styled(
                "  Worktree is being updated to latest main...",
                Style::default().fg(Color::Cyan),
            )));
        }

        crate::model::TaskStatus::Applying => {
            lines.push(Line::from(Span::styled(
                "  Changes are being applied to main worktree...",
                Style::default().fg(Color::Magenta),
            )));
        }

        crate::model::TaskStatus::Done => {
            lines.push(Line::from(vec![
                Span::styled(" e ", key_style), Span::styled(" Edit task", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" d ", key_style), Span::styled(" Delete task", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" r ", key_style), Span::styled(" Move back to Review", label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" x ", key_style), Span::styled(" Reset (cleanup and move to Planned)", label_style),
            ]));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // ACTIVITY LOG - Show recent activity during Accepting/Updating
    // ═══════════════════════════════════════════════════════════════════════
    if !task.activity_log.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("─".repeat(40), dim_style)));

        // Show different header based on status
        let log_header = match task.status {
            crate::model::TaskStatus::Accepting => "Merge Activity",
            crate::model::TaskStatus::Updating => "Update Activity",
            _ => "Recent Activity",
        };
        lines.push(Line::from(Span::styled(
            log_header,
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        )));
        lines.push(Line::from(""));

        // Show the last 6 entries (most recent at bottom for natural scrolling feel)
        let entries_to_show: Vec<_> = task.activity_log.iter().rev().take(6).collect();
        for entry in entries_to_show.iter().rev() {
            // Format timestamp as relative time
            let elapsed = chrono::Utc::now().signed_duration_since(entry.timestamp);
            let time_ago = if elapsed.num_seconds() < 5 {
                "now".to_string()
            } else if elapsed.num_seconds() < 60 {
                format!("{}s ago", elapsed.num_seconds())
            } else if elapsed.num_minutes() < 60 {
                format!("{}m ago", elapsed.num_minutes())
            } else {
                format!("{}h ago", elapsed.num_hours())
            };

            // Color based on message content
            let msg_color = if entry.message.starts_with("Using ") {
                Color::Cyan
            } else if entry.message.contains("error") || entry.message.contains("failed") || entry.message.contains("cancelled") {
                Color::Red
            } else if entry.message.contains("success") || entry.message.contains("complete") {
                Color::Green
            } else {
                Color::White
            };

            lines.push(Line::from(vec![
                Span::styled(format!("{:>7} ", time_ago), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    truncate_string(&entry.message, 35),
                    Style::default().fg(msg_color)
                ),
            ]));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // FOOTER: Close hint
    // ═══════════════════════════════════════════════════════════════════════
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Esc", key_style),
        Span::styled("/", dim_style),
        Span::styled("Enter", key_style),
        Span::styled("/", dim_style),
        Span::styled("Space", key_style),
        Span::styled(" close    ", dim_style),
        Span::styled("?", key_style),
        Span::styled(" full help", dim_style),
    ]));

    let preview = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" {} ", task.status.label()))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(column_color)),
        )
        .style(Style::default().fg(Color::White))
        .wrap(ratatui::widgets::Wrap { trim: false });

    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(preview, area);
}

/// Format a datetime for display
fn format_datetime(dt: chrono::DateTime<chrono::Utc>) -> String {
    let local = dt.with_timezone(&chrono::Local);
    local.format("%b %d, %H:%M").to_string()
}

/// Truncate a string to a maximum length with ellipsis
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 3 {
        "...".to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

/// Format a duration for display (human-readable)
fn format_duration(duration: chrono::Duration) -> String {
    let total_secs = duration.num_seconds();
    if total_secs < 60 {
        format!("{}s", total_secs)
    } else if total_secs < 3600 {
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        if secs > 0 {
            format!("{}m {}s", mins, secs)
        } else {
            format!("{}m", mins)
        }
    } else {
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        if mins > 0 {
            format!("{}h {}m", hours, mins)
        } else {
            format!("{}h", hours)
        }
    }
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
        Line::from("  h/l        Move left/right between columns"),
        Line::from("  j/k        Move down/up within column"),
        Line::from("  1-6        Jump to column (Planned/Queued/InProgress/Needs/Review/Done)"),
        Line::from("  Tab        Cycle focus: Board → Input → Tabs"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Task Actions", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  v/Space    View task details"),
        Line::from("  i          New task (focus input)"),
        Line::from("  e          Edit task"),
        Line::from("  s          Start (Planned/Queued) / Continue (Review/NeedsInput)"),
        Line::from("  d          Delete task"),
        Line::from("  r          Move to Review (InProgress/NeedsInput/Done)"),
        Line::from("  x          Reset: cleanup & move to Planned"),
        Line::from("  +/-        Reorder task up/down"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Review Column", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  a          Accept: merge changes and mark done"),
        Line::from("  d          Decline: discard changes and mark done"),
        Line::from("  f          Feedback: send follow-up instructions"),
        Line::from("  u          Unapply task changes (if applied)"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Input Mode", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  Enter      Submit task"),
        Line::from("  \\Enter    Newline (line continuation)"),
        Line::from("  Ctrl-V     Paste image"),
        Line::from("  Ctrl-X/U   Remove last / clear all images"),
        Line::from("  Esc        Cancel / unfocus"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Projects", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  1-9        Switch to project N / open new project"),
        Line::from("  Ctrl-D     Close current project"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Sessions", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  o/O        Open Claude modal (O: detached)"),
        Line::from("  t/T        Open test shell (T: detached)"),
        Line::from("  q          Queue task (Planned) / Quit"),
        Line::from("  ?          Toggle this help"),
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

/// Render queue dialog for selecting a session to queue a task for
fn render_queue_dialog(frame: &mut Frame, app: &App) {
    let area = centered_rect(50, 50, frame.area());

    // Get the running sessions
    let sessions: Vec<_> = if let Some(project) = app.model.active_project() {
        project.tasks_with_active_sessions()
            .iter()
            .map(|t| (t.id, t.title.clone(), t.session_state))
            .collect()
    } else {
        Vec::new()
    };

    // Get the task being queued
    let queuing_task_title = app.model.ui_state.queue_dialog_task_id
        .and_then(|id| {
            app.model.active_project()
                .and_then(|p| p.tasks.iter().find(|t| t.id == id))
                .map(|t| t.title.clone())
        })
        .unwrap_or_else(|| "Task".to_string());

    let selected_idx = app.model.ui_state.queue_dialog_selected_idx;

    // Build the dialog content
    let mut lines = vec![
        Line::from(Span::styled(
            "Queue Task For Session",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("Queuing: "),
            Span::styled(&queuing_task_title, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Select a running session:",
            Style::default().add_modifier(Modifier::UNDERLINED),
        )),
    ];

    for (i, (_id, title, state)) in sessions.iter().enumerate() {
        let is_selected = i == selected_idx;
        let prefix = if is_selected { "► " } else { "  " };
        let state_str = format!(" [{}]", state.label());

        let style = if is_selected {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        lines.push(Line::from(vec![
            Span::styled(prefix.to_string(), style),
            Span::styled(title.clone(), style),
            Span::styled(state_str, Style::default().fg(Color::DarkGray)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "j/k: Navigate  Enter: Confirm  q/Esc: Cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let dialog = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Queue Task ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .style(Style::default().fg(Color::White));

    // Clear area first
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(dialog, area);
}

/// Render the open project dialog
fn render_open_project_dialog(frame: &mut Frame, app: &App) {
    let area = centered_rect(70, 70, frame.area());

    let slot = app.model.ui_state.open_project_dialog_slot.unwrap_or(0);

    // Clear area first
    frame.render_widget(ratatui::widgets::Clear, area);

    // Split the area: title at top, current path, directory list in middle, hints at bottom
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // Title
            Constraint::Length(2),  // Current path
            Constraint::Min(10),    // Directory list
            Constraint::Length(3),  // Hints
        ])
        .split(area);

    // Render title
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" Open project in slot [{}] ", slot + 1),
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
        ),
    ]));
    frame.render_widget(title, chunks[0]);

    // Render directory browser
    if let Some(ref browser) = app.model.ui_state.directory_browser {
        // Current path display
        let path_display = Paragraph::new(Line::from(vec![
            Span::styled(" 📁 ", Style::default().fg(Color::Yellow)),
            Span::styled(
                browser.cwd().display().to_string(),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ]));
        frame.render_widget(path_display, chunks[1]);

        // Build list items from directory entries
        let items: Vec<ListItem> = browser
            .entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                let icon = if entry.name == ".." {
                    "↩ "
                } else {
                    "📂 "
                };
                let style = if idx == browser.selected_idx {
                    Style::default().bg(Color::Blue).fg(Color::White)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(icon, style),
                    Span::styled(&entry.name, style),
                ]))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Green))
                    .title(" Select Directory "),
            );
        frame.render_widget(list, chunks[2]);
    }

    // Render hints
    let hints = Paragraph::new(vec![
        Line::from(Span::styled(
            "↑↓/jk: Navigate  Space/Enter/l: Open dir  Backspace/h: Parent  Esc: Cancel",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "o: Open selected directory as project",
            Style::default().fg(Color::Yellow),
        )),
    ]);
    frame.render_widget(hints, chunks[3]);
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

/// Render a confirmation modal for multiline messages (like merge check reports)
fn render_confirmation_modal(frame: &mut Frame, message: &str) {
    // Calculate size based on content
    let line_count = message.lines().count();
    let max_line_width = message.lines().map(|l| l.len()).max().unwrap_or(40);

    // Size the modal to fit content with some padding
    let height_percent = ((line_count + 4) * 100 / frame.area().height as usize).min(80).max(30) as u16;
    let width_percent = ((max_line_width + 6) * 100 / frame.area().width as usize).min(90).max(50) as u16;

    let area = centered_rect(width_percent, height_percent, frame.area());

    // Build lines with styling
    let mut lines: Vec<Line> = Vec::new();
    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White);
    let verdict_merged = Style::default().fg(Color::Green).add_modifier(Modifier::BOLD);
    let verdict_not_merged = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let warning_style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);

    for line in message.lines() {
        let styled_line = if line.starts_with("===") {
            Line::from(Span::styled(line, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)))
        } else if line.starts_with("VERDICT: MERGED") {
            Line::from(Span::styled(line, verdict_merged))
        } else if line.starts_with("VERDICT: NOT MERGED") || line.starts_with("VERDICT: CANNOT") {
            Line::from(Span::styled(line, verdict_not_merged))
        } else if line.starts_with("VERDICT: HAS UNCOMMITTED") || line.contains("UNCOMMITTED CHANGES") || line.starts_with("NOT safe") || line.contains("uncommitted") {
            Line::from(Span::styled(line, warning_style))
        } else if line.starts_with("Branch:") || line.starts_with("Commits") || line.starts_with("Diff") || line.starts_with("Worktree:") {
            // Split label and value
            if let Some(colon_pos) = line.find(':') {
                let (label, value) = line.split_at(colon_pos + 1);
                Line::from(vec![
                    Span::styled(label, label_style),
                    Span::styled(value, value_style),
                ])
            } else {
                Line::from(Span::styled(line, value_style))
            }
        } else if line.starts_with("---") {
            Line::from(Span::styled(line, Style::default().fg(Color::DarkGray)))
        } else if line.contains("'y'") || line.contains("'n'") {
            Line::from(Span::styled(line, Style::default().fg(Color::Yellow)))
        } else {
            Line::from(Span::styled(line, value_style))
        };
        lines.push(styled_line);
    }

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" Merge Check ")
                .title_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        )
        .wrap(ratatui::widgets::Wrap { trim: false });

    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(modal, area);
}
