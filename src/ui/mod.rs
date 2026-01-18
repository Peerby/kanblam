mod interactive_modal;
mod kanban;
pub mod logo;
mod output;
mod status_bar;

use crate::app::App;
use crate::model::{DirEntry, FocusArea, MillerColumn, SpecialEntry, TaskStatus};
use edtui::{EditorMode, EditorTheme, EditorView};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Widget,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
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

    // Render status bar (includes git status)
    render_status_bar(frame, chunks[3], app);

    // Render help overlay if active
    if app.model.ui_state.show_help {
        render_help(frame, app.model.ui_state.help_scroll_offset);
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

    // Render configuration modal if active
    if app.model.ui_state.is_config_modal_open() {
        render_config_modal(frame, app);
    }

    // Render stash modal if active
    if app.model.ui_state.show_stash_modal {
        render_stash_modal(frame, app);
    }

    // Render confirmation modal if pending confirmation has multiline message
    if let Some(ref confirmation) = app.model.ui_state.pending_confirmation {
        if confirmation.message.contains('\n') {
            render_confirmation_modal(frame, &confirmation.message, app.model.ui_state.confirmation_scroll_offset, &confirmation.action);
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

    // Handle empty content
    if content.is_empty() {
        visual_lines = 1;
    } else {
        // Split by newlines manually to correctly handle trailing newlines
        // str.lines() ignores trailing newlines, but we need to count them for the editor
        // For example, "hello\n" should show 2 lines (one for "hello", one for the cursor)
        for line in content.split('\n') {
            // Calculate how many visual rows this line takes when wrapped
            let line_width = line.chars().count();
            let wrapped_rows = if line_width == 0 {
                1 // Empty line still takes one row
            } else {
                (line_width + available_width - 1) / available_width
            };
            visual_lines += wrapped_rows;
        }
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
        let attention_count = project.attention_count();

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
        } else {
            Style::default().fg(Color::Gray)
        };

        // Keyboard shortcut: @ for first project, # for second, etc. (! is for +project)
        let tab_text = if idx + 1 < 10 {
            format!(" [{}] {} ", shift_chars[idx + 1], project.name)
        } else {
            format!(" {} ", project.name)
        };

        spans.push(Span::styled(tab_text, style));

        // Add red badge for projects with tasks needing attention
        if attention_count > 0 {
            spans.push(Span::styled(
                format!(" {} ", attention_count),
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ));
        }

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
        let attention_count = project.attention_count();

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
        } else {
            Style::default().fg(Color::Gray)
        };

        // Keyboard shortcut: @ for first project, # for second, etc. (! is for +project)
        let tab_text = if idx + 1 < 10 {
            format!(" [{}] {} ", shift_chars[idx + 1], project.name)
        } else {
            format!(" {} ", project.name)
        };

        spans.push(Span::styled(tab_text, style));

        // Add red badge for projects with tasks needing attention
        if attention_count > 0 {
            spans.push(Span::styled(
                format!(" {} ", attention_count),
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ));
        }

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

    // Check if feedback is for a live (InProgress) task
    let is_live_feedback = app.model.ui_state.feedback_task_id.and_then(|task_id| {
        app.model.active_project().and_then(|project| {
            project.tasks.iter().find(|t| t.id == task_id).map(|t| t.status == TaskStatus::InProgress)
        })
    }).unwrap_or(false);

    // Choose colors based on focus and mode
    let (border_color, text_color) = if is_focused {
        let color = if is_live_feedback {
            Color::Green  // Green for live feedback to running task
        } else if is_feedback_mode {
            Color::Cyan   // Cyan for feedback to paused task
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

    let title = if is_live_feedback {
        Line::from(Span::styled(" Live Feedback ", title_style))
    } else if is_feedback_mode {
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
    // Show mode-specific hints when focused
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::DarkGray);

    // Get the configured editor name for the hint
    let editor_name = app.model.global_settings.default_editor.name().to_lowercase();
    let editor_hint = format!(" {} ", editor_name);
    let editor_hint_len = editor_hint.len() as u16;

    // Check current editor mode for focused hints
    let is_insert_mode = matches!(
        app.model.ui_state.editor_state.mode,
        EditorMode::Insert | EditorMode::Search | EditorMode::Visual
    );

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
    } else if is_insert_mode {
        // INSERT MODE hints
        if pending_count > 0 {
            // With pending images: "^V+img ^X-1 ^Uclr ⏎ line esc→⏎ submit"
            // Width: 2+5+2+3+2+4+1+6+3+1+1+7 = 37
            (
                Line::from(vec![
                    Span::styled("^V", key_style),
                    Span::styled("+img ", desc_style),
                    Span::styled("^X", key_style),
                    Span::styled("-1 ", desc_style),
                    Span::styled("^U", key_style),
                    Span::styled("clr ", desc_style),
                    Span::styled("⏎", key_style),
                    Span::styled(" line ", desc_style),
                    Span::styled("esc", key_style),
                    Span::styled("→", desc_style),
                    Span::styled("⏎", key_style),
                    Span::styled(" submit", desc_style),
                ]),
                37u16,
            )
        } else {
            // No pending images: "^V img ^G vim ⏎ line esc→⏎ submit"
            // Width: 2+5+2+editor+1+6+3+1+1+7 = 28 + editor_hint_len
            (
                Line::from(vec![
                    Span::styled("^V", key_style),
                    Span::styled(" img ", desc_style),
                    Span::styled("^G", key_style),
                    Span::styled(editor_hint.clone(), desc_style),
                    Span::styled("⏎", key_style),
                    Span::styled(" line ", desc_style),
                    Span::styled("esc", key_style),
                    Span::styled("→", desc_style),
                    Span::styled("⏎", key_style),
                    Span::styled(" submit", desc_style),
                ]),
                28 + editor_hint_len,
            )
        }
    } else {
        // NORMAL MODE hints
        if pending_count > 0 {
            // With pending images: "^V+img ^X-1 ^Uclr hjkl←↓↑→ aio edit ⏎ submit"
            // Width: 2+5+2+3+2+4+4+5+3+6+1+7 = 44
            (
                Line::from(vec![
                    Span::styled("^V", key_style),
                    Span::styled("+img ", desc_style),
                    Span::styled("^X", key_style),
                    Span::styled("-1 ", desc_style),
                    Span::styled("^U", key_style),
                    Span::styled("clr ", desc_style),
                    Span::styled("hjkl", key_style),
                    Span::styled("←↓↑→ ", desc_style),
                    Span::styled("aio", key_style),
                    Span::styled(" edit ", desc_style),
                    Span::styled("⏎", key_style),
                    Span::styled(" submit", desc_style),
                ]),
                44u16,
            )
        } else {
            // No pending images: "^V img ^G vim hjkl←↓↑→ aio edit ⏎ submit"
            // Width: 2+5+2+editor+4+5+3+6+1+7 = 35 + editor_hint_len
            (
                Line::from(vec![
                    Span::styled("^V", key_style),
                    Span::styled(" img ", desc_style),
                    Span::styled("^G", key_style),
                    Span::styled(editor_hint, desc_style),
                    Span::styled("hjkl", key_style),
                    Span::styled("←↓↑→ ", desc_style),
                    Span::styled("aio", key_style),
                    Span::styled(" edit ", desc_style),
                    Span::styled("⏎", key_style),
                    Span::styled(" submit", desc_style),
                ]),
                35 + editor_hint_len,
            )
        }
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

    let current_tab = app.model.ui_state.task_detail_tab;
    let mut lines: Vec<Line> = Vec::new();
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White);
    let dim_style = Style::default().fg(Color::DarkGray);

    // ═══════════════════════════════════════════════════════════════════════
    // TAB BAR
    // ═══════════════════════════════════════════════════════════════════════
    let tab_bar = render_task_detail_tab_bar(current_tab);
    lines.push(tab_bar);
    lines.push(Line::from(""));

    // ═══════════════════════════════════════════════════════════════════════
    // TAB CONTENT
    // ═══════════════════════════════════════════════════════════════════════
    match current_tab {
        crate::model::TaskDetailTab::General => {
            render_general_tab(&mut lines, task, app, &label_style, &value_style, &dim_style);
        }
        crate::model::TaskDetailTab::Git => {
            render_git_tab(&mut lines, task, app, &label_style, &value_style, &dim_style, &key_style);
        }
        crate::model::TaskDetailTab::Claude => {
            render_claude_tab(&mut lines, task, &label_style, &value_style, &dim_style);
        }
        crate::model::TaskDetailTab::Activity => {
            render_activity_tab(&mut lines, task, &label_style, &dim_style);
        }
        crate::model::TaskDetailTab::Help => {
            render_help_tab(&mut lines, task, &key_style, &label_style, &dim_style);
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // FOOTER: Navigation and close hints
    // ═══════════════════════════════════════════════════════════════════════
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("←/h", key_style),
        Span::styled(" ", dim_style),
        Span::styled("→/l", key_style),
        Span::styled(" tabs    ", dim_style),
        Span::styled("Esc", key_style),
        Span::styled("/", dim_style),
        Span::styled("Enter", key_style),
        Span::styled("/", dim_style),
        Span::styled("Space", key_style),
        Span::styled(" close", dim_style),
    ]));

    // Build title: [phase] short_title
    let short_title = task.short_title.as_ref().unwrap_or(&task.title);
    let title = format!(" [{}] {} ", phase_label, truncate_string(short_title, 40));

    let preview = Paragraph::new(lines)
        .block(
            Block::default()
                .title(Span::styled(title, Style::default().fg(Color::White)))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(column_color)),
        )
        .style(Style::default().fg(Color::White))
        .wrap(ratatui::widgets::Wrap { trim: false });

    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(preview, area);
}

/// Render the tab bar for the task detail modal
fn render_task_detail_tab_bar(current_tab: crate::model::TaskDetailTab) -> Line<'static> {
    let tabs = crate::model::TaskDetailTab::all();
    let mut spans = Vec::new();

    for (i, tab) in tabs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
        }

        let style = if *tab == current_tab {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        spans.push(Span::styled(tab.label(), style));
    }

    Line::from(spans)
}

/// Render the General tab content
fn render_general_tab<'a>(
    lines: &mut Vec<Line<'a>>,
    task: &crate::model::Task,
    app: &App,
    label_style: &Style,
    value_style: &Style,
    dim_style: &Style,
) {
    // Title (full if short_title exists)
    if task.short_title.is_some() {
        for title_line in task.title.lines() {
            lines.push(Line::from(Span::styled(title_line.to_string(), Style::default().fg(Color::White))));
        }
        lines.push(Line::from(""));
    }

    // Description
    if !task.description.is_empty() {
        for desc_line in task.description.lines() {
            lines.push(Line::from(Span::styled(desc_line.to_string(), Style::default().fg(Color::Gray))));
        }
        lines.push(Line::from(""));
    }

    // Attachments
    if !task.images.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("📎 ", *dim_style),
            Span::styled(format!("{} image(s) attached", task.images.len()), Style::default().fg(Color::Cyan)),
        ]));
        lines.push(Line::from(""));
    }

    // Phase-specific timing info
    lines.push(Line::from(Span::styled("─".repeat(40), *dim_style)));

    match task.status {
        crate::model::TaskStatus::Planned | crate::model::TaskStatus::Queued => {
            lines.push(Line::from(vec![
                Span::styled("Created: ", *label_style),
                Span::styled(format_datetime(task.created_at), *value_style),
            ]));

            if task.status == crate::model::TaskStatus::Queued {
                if let Some(queued_for) = task.queued_for_session {
                    if let Some(project) = app.model.active_project() {
                        if let Some(parent_task) = project.tasks.iter().find(|t| t.id == queued_for) {
                            lines.push(Line::from(vec![
                                Span::styled("Queued for: ", *label_style),
                                Span::styled(parent_task.title.clone(), Style::default().fg(Color::Yellow)),
                            ]));
                        }
                    }
                }
            }
        }

        crate::model::TaskStatus::InProgress => {
            if let Some(started) = task.started_at {
                let duration = chrono::Utc::now().signed_duration_since(started);
                lines.push(Line::from(vec![
                    Span::styled("Running for: ", *label_style),
                    Span::styled(format_duration(duration), Style::default().fg(Color::Yellow)),
                ]));
            }

            let (state_label, state_color) = match task.session_state {
                crate::model::ClaudeSessionState::Creating => ("Creating worktree...", Color::Yellow),
                crate::model::ClaudeSessionState::Starting => ("Starting session...", Color::Yellow),
                crate::model::ClaudeSessionState::Ready => ("Ready", Color::Green),
                crate::model::ClaudeSessionState::Working => ("Working", Color::Green),
                crate::model::ClaudeSessionState::Continuing => ("Continuing", Color::Cyan),
                _ => ("Unknown", Color::DarkGray),
            };
            lines.push(Line::from(vec![
                Span::styled("Session: ", *label_style),
                Span::styled(state_label, Style::default().fg(state_color)),
            ]));

            if let Some(ref tool_name) = task.last_tool_name {
                lines.push(Line::from(vec![
                    Span::styled("Last tool: ", *label_style),
                    Span::styled(tool_name.clone(), *value_style),
                ]));
            }
        }

        crate::model::TaskStatus::NeedsInput => {
            if let Some(started) = task.started_at {
                let duration = chrono::Utc::now().signed_duration_since(started);
                lines.push(Line::from(vec![
                    Span::styled("⚠ ", Style::default().fg(Color::Red)),
                    Span::styled("Waiting for input since ", *label_style),
                    Span::styled(format_duration(duration), Style::default().fg(Color::Red)),
                ]));
            }

            lines.push(Line::from(vec![
                Span::styled("Session: ", *label_style),
                Span::styled("Paused - needs your input", Style::default().fg(Color::Red)),
            ]));
        }

        crate::model::TaskStatus::Review | crate::model::TaskStatus::Accepting | crate::model::TaskStatus::Updating | crate::model::TaskStatus::Applying => {
            if let Some(started) = task.started_at {
                let duration = chrono::Utc::now().signed_duration_since(started);
                lines.push(Line::from(vec![
                    Span::styled("Total time: ", *label_style),
                    Span::styled(format_duration(duration), *value_style),
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
            if let Some(completed) = task.completed_at {
                lines.push(Line::from(vec![
                    Span::styled("Completed: ", *label_style),
                    Span::styled(format_datetime(completed), Style::default().fg(Color::Green)),
                ]));
            }

            if let (Some(started), Some(completed)) = (task.started_at, task.completed_at) {
                let duration = completed.signed_duration_since(started);
                lines.push(Line::from(vec![
                    Span::styled("Duration: ", *label_style),
                    Span::styled(format_duration(duration), *value_style),
                ]));
            }
        }
    }

    // Worktree path
    if let Some(ref wt_path) = task.worktree_path {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Path: ", *label_style),
            Span::styled(wt_path.display().to_string(), *dim_style),
        ]));
    }
}

/// Render the Git tab content
fn render_git_tab<'a>(
    lines: &mut Vec<Line<'a>>,
    task: &crate::model::Task,
    app: &App,
    label_style: &Style,
    _value_style: &Style,
    dim_style: &Style,
    key_style: &Style,
) {
    if task.worktree_path.is_none() {
        lines.push(Line::from(Span::styled("No worktree for this task", *dim_style)));
        return;
    }

    // Show summary header (branch, changes, commits)
    if let Some(ref branch) = task.git_branch {
        lines.push(Line::from(vec![
            Span::styled("Branch: ", *label_style),
            Span::styled(branch.clone(), Style::default().fg(Color::Green)),
        ]));
    }

    // Show line changes with visual bar (compact)
    let total_changes = task.git_additions + task.git_deletions;
    if total_changes > 0 {
        let bar_width = 16usize;
        let add_ratio = task.git_additions as f64 / total_changes as f64;
        let add_chars = (add_ratio * bar_width as f64).round() as usize;
        let del_chars = bar_width.saturating_sub(add_chars);

        let add_bar = "█".repeat(add_chars);
        let del_bar = "█".repeat(del_chars);

        lines.push(Line::from(vec![
            Span::styled(format!("+{}", task.git_additions), Style::default().fg(Color::Green)),
            Span::styled("/", *dim_style),
            Span::styled(format!("-{}", task.git_deletions), Style::default().fg(Color::Red)),
            Span::styled(format!(" in {} files ", task.git_files_changed), *dim_style),
            Span::styled(add_bar, Style::default().fg(Color::Green)),
            Span::styled(del_bar, Style::default().fg(Color::Red)),
        ]));
    } else {
        lines.push(Line::from(Span::styled("No changes yet", *dim_style)));
    }

    // Show commits behind warning if applicable
    if task.git_commits_behind > 0 {
        lines.push(Line::from(vec![
            Span::styled("⚠ ", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("{} commits behind main - ", task.git_commits_behind),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("u", key_style.fg(Color::Cyan)),
            Span::styled(" to update", Style::default().fg(Color::Yellow)),
        ]));
    }

    // Separator and scroll hint
    lines.push(Line::from(Span::styled("─".repeat(50), *dim_style)));
    lines.push(Line::from(vec![
        Span::styled("j", *key_style),
        Span::styled("/", *dim_style),
        Span::styled("k", *key_style),
        Span::styled(" scroll  ", *dim_style),
        Span::styled("PgUp", *key_style),
        Span::styled("/", *dim_style),
        Span::styled("PgDn", *key_style),
        Span::styled(" page  ", *dim_style),
        Span::styled("Home", *key_style),
        Span::styled("/", *dim_style),
        Span::styled("End", *key_style),
        Span::styled(" jump", *dim_style),
    ]));
    lines.push(Line::from(""));

    // Get git diff from cache or show loading message
    let scroll_offset = app.model.ui_state.git_diff_scroll_offset;

    if let Some((cached_task_id, ref diff_content)) = app.model.ui_state.git_diff_cache {
        if cached_task_id == task.id {
            // Parse and render the diff with colors
            render_git_diff_content(lines, diff_content, scroll_offset, dim_style);
        } else {
            lines.push(Line::from(Span::styled("Loading diff...", *dim_style)));
        }
    } else {
        lines.push(Line::from(Span::styled("Loading diff...", *dim_style)));
    }
}

/// Parse and render git diff content with syntax highlighting
fn render_git_diff_content<'a>(
    lines: &mut Vec<Line<'a>>,
    diff_content: &str,
    scroll_offset: usize,
    dim_style: &Style,
) {
    let diff_lines: Vec<&str> = diff_content.lines().collect();
    let total_lines = diff_lines.len();

    if total_lines == 0 {
        lines.push(Line::from(Span::styled("No diff content", *dim_style)));
        return;
    }

    // Show scroll position indicator
    if total_lines > 20 {
        let percentage = if total_lines > 0 {
            ((scroll_offset as f64 / total_lines as f64) * 100.0) as usize
        } else {
            0
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("Lines {}-{} of {} ({}%)",
                    scroll_offset + 1,
                    (scroll_offset + 30).min(total_lines),
                    total_lines,
                    percentage
                ),
                *dim_style,
            ),
        ]));
        lines.push(Line::from(""));
    }

    // Render visible diff lines with colors
    let visible_lines = 25; // How many lines to show
    for line in diff_lines.iter().skip(scroll_offset).take(visible_lines) {
        let styled_line = style_diff_line(line);
        lines.push(styled_line);
    }

    // Show "more below" indicator if there's more content
    let remaining = total_lines.saturating_sub(scroll_offset + visible_lines);
    if remaining > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("... {} more lines below ...", remaining),
            *dim_style,
        )));
    }
}

/// Style a single diff line with appropriate colors
fn style_diff_line(line: &str) -> Line<'static> {
    let line_owned = line.to_string();

    // File header lines (diff --git, index, ---, +++)
    if line_owned.starts_with("diff --git") {
        return Line::from(Span::styled(
            line_owned,
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
    }

    if line_owned.starts_with("index ") {
        return Line::from(Span::styled(
            line_owned,
            Style::default().fg(Color::DarkGray),
        ));
    }

    if line_owned.starts_with("--- ") {
        return Line::from(Span::styled(
            line_owned,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }

    if line_owned.starts_with("+++ ") {
        return Line::from(Span::styled(
            line_owned,
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ));
    }

    // Hunk header (@@ ... @@)
    if line_owned.starts_with("@@") {
        return Line::from(Span::styled(
            line_owned,
            Style::default().fg(Color::Magenta),
        ));
    }

    // Added lines
    if line_owned.starts_with('+') {
        return Line::from(Span::styled(
            line_owned,
            Style::default().fg(Color::Green),
        ));
    }

    // Removed lines
    if line_owned.starts_with('-') {
        return Line::from(Span::styled(
            line_owned,
            Style::default().fg(Color::Red),
        ));
    }

    // Context lines (unchanged)
    Line::from(Span::styled(
        line_owned,
        Style::default().fg(Color::White),
    ))
}

/// Render the Claude tab content (SDK logs)
fn render_claude_tab<'a>(
    lines: &mut Vec<Line<'a>>,
    task: &crate::model::Task,
    label_style: &Style,
    value_style: &Style,
    dim_style: &Style,
) {
    // Session info
    if let Some(ref session_id) = task.claude_session_id {
        lines.push(Line::from(vec![
            Span::styled("Session ID: ", *label_style),
            Span::styled(session_id.clone(), *value_style),
        ]));
    }

    let is_cli_mode = matches!(
        task.session_mode,
        crate::model::SessionMode::CliInteractive | crate::model::SessionMode::CliActivelyWorking
    );

    let mode_str = match task.session_mode {
        crate::model::SessionMode::SdkManaged => "SDK Managed",
        crate::model::SessionMode::CliInteractive => "CLI Interactive",
        crate::model::SessionMode::CliActivelyWorking => "CLI Working",
        crate::model::SessionMode::WaitingForCliExit => "Waiting for CLI Exit",
    };

    if is_cli_mode {
        // Show tmux session name when in CLI mode
        let task_id_str = task.id.to_string();
        let short_id = &task_id_str[..4.min(task_id_str.len())];
        let session_name = format!("kb-{}", short_id);

        lines.push(Line::from(vec![
            Span::styled("Mode: ", *label_style),
            Span::styled(mode_str, *value_style),
            Span::styled(format!("  (tmux: {})", session_name), Style::default().fg(Color::Cyan)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("      ", *label_style),
            Span::styled("Press ", *dim_style),
            Span::styled("o", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled(" to open terminal", *dim_style),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Mode: ", *label_style),
            Span::styled(mode_str, *value_style),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("SDK Commands: ", *label_style),
        Span::styled(task.sdk_command_count.to_string(), *value_style),
    ]));

    if let Some(ref tool_name) = task.last_tool_name {
        lines.push(Line::from(vec![
            Span::styled("Last Tool: ", *label_style),
            Span::styled(tool_name.clone(), *value_style),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("─".repeat(40), *dim_style)));
    lines.push(Line::from(Span::styled("SDK Activity Log", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))));
    lines.push(Line::from(""));

    // Show SDK activity from the activity log (filter for SDK-related entries)
    if task.activity_log.is_empty() {
        lines.push(Line::from(Span::styled("No activity logged yet", *dim_style)));
    } else {
        // Show all activity entries (up to 20 for the claude tab)
        let entries_to_show: Vec<_> = task.activity_log.iter().rev().take(20).collect();
        for entry in entries_to_show.iter().rev() {
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

            let msg_color = if entry.message.starts_with("Using ") || entry.message.starts_with("Tool:") {
                Color::Cyan
            } else if entry.message.contains("error") || entry.message.contains("failed") || entry.message.contains("cancelled") {
                Color::Red
            } else if entry.message.contains("success") || entry.message.contains("complete") || entry.message.contains("started") {
                Color::Green
            } else if entry.message.contains("Working") || entry.message.contains("Waiting") {
                Color::Yellow
            } else {
                Color::White
            };

            lines.push(Line::from(vec![
                Span::styled(format!("{:>7} ", time_ago), Style::default().fg(Color::DarkGray)),
                Span::styled(truncate_string(&entry.message, 45), Style::default().fg(msg_color)),
            ]));
        }
    }
}

/// Render the Activity tab content (user actions + SDK commands)
fn render_activity_tab<'a>(
    lines: &mut Vec<Line<'a>>,
    task: &crate::model::Task,
    label_style: &Style,
    dim_style: &Style,
) {
    lines.push(Line::from(Span::styled("Command History", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))));
    lines.push(Line::from(""));

    if task.activity_log.is_empty() {
        lines.push(Line::from(Span::styled("No activity logged yet", *dim_style)));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Activity will appear here as you:", *label_style)));
        lines.push(Line::from(Span::styled("  • Start and stop tasks", *dim_style)));
        lines.push(Line::from(Span::styled("  • Send feedback to Claude", *dim_style)));
        lines.push(Line::from(Span::styled("  • Open terminals and modals", *dim_style)));
        lines.push(Line::from(Span::styled("  • Merge or discard changes", *dim_style)));
    } else {
        // Show all activity entries (up to 25 for the activity tab)
        let entries_to_show: Vec<_> = task.activity_log.iter().rev().take(25).collect();
        for entry in entries_to_show.iter().rev() {
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

            // Categorize and color activity entries
            let (icon, msg_color) = if entry.message.starts_with("Using ") || entry.message.starts_with("Tool:") {
                ("🔧", Color::Cyan)
            } else if entry.message.contains("started") || entry.message.contains("Starting") {
                ("▶", Color::Green)
            } else if entry.message.contains("stopped") || entry.message.contains("ended") || entry.message.contains("Ended") {
                ("⏹", Color::Yellow)
            } else if entry.message.contains("Waiting") || entry.message.contains("input") {
                ("⏸", Color::Yellow)
            } else if entry.message.contains("Working") {
                ("⚙", Color::Green)
            } else if entry.message.contains("feedback") || entry.message.contains("Feedback") {
                ("💬", Color::Magenta)
            } else if entry.message.contains("merge") || entry.message.contains("Merge") || entry.message.contains("Rebasing") {
                ("🔀", Color::Magenta)
            } else if entry.message.contains("error") || entry.message.contains("failed") || entry.message.contains("cancelled") {
                ("✗", Color::Red)
            } else if entry.message.contains("success") || entry.message.contains("complete") {
                ("✓", Color::Green)
            } else {
                ("•", Color::White)
            };

            lines.push(Line::from(vec![
                Span::styled(format!("{:>7} ", time_ago), Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{} ", icon), Style::default().fg(msg_color)),
                Span::styled(truncate_string(&entry.message, 42), Style::default().fg(msg_color)),
            ]));
        }
    }
}

/// Render the Help tab content (phase-specific actions)
fn render_help_tab<'a>(
    lines: &mut Vec<Line<'a>>,
    task: &crate::model::Task,
    key_style: &Style,
    label_style: &Style,
    dim_style: &Style,
) {
    lines.push(Line::from(Span::styled("Available Actions", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))));
    lines.push(Line::from(""));

    match task.status {
        crate::model::TaskStatus::Planned => {
            lines.push(Line::from(vec![
                Span::styled(" s ", *key_style), Span::styled(" Start task with worktree isolation", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" q ", *key_style), Span::styled(" Queue for running session", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" e ", *key_style), Span::styled(" Edit task", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" d ", *key_style), Span::styled(" Delete task", *label_style),
            ]));
        }

        crate::model::TaskStatus::Queued => {
            lines.push(Line::from(vec![
                Span::styled(" s ", *key_style), Span::styled(" Start immediately", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" e ", *key_style), Span::styled(" Edit task", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" d ", *key_style), Span::styled(" Delete task", *label_style),
            ]));
        }

        crate::model::TaskStatus::InProgress => {
            lines.push(Line::from(vec![
                Span::styled(" s ", *key_style), Span::styled(" Switch to Claude session", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" o ", *key_style), Span::styled(" Open interactive modal", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" t ", *key_style), Span::styled(" Open test shell in worktree", *label_style),
            ]));
            if task.git_commits_behind > 0 {
                lines.push(Line::from(vec![
                    Span::styled(" u ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::styled(" Update: rebase onto latest main", Style::default().fg(Color::Yellow)),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled(" r ", *key_style), Span::styled(" Move to review", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" x ", *key_style), Span::styled(" Reset (cleanup and move to Planned)", *label_style),
            ]));
        }

        crate::model::TaskStatus::NeedsInput => {
            lines.push(Line::from(vec![
                Span::styled(" s ", *key_style), Span::styled(" Continue / switch to session", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" o ", *key_style), Span::styled(" Open interactive modal", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" t ", *key_style), Span::styled(" Open test shell", *label_style),
            ]));
            if task.git_commits_behind > 0 {
                lines.push(Line::from(vec![
                    Span::styled(" u ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::styled(" Update: rebase onto latest main", Style::default().fg(Color::Yellow)),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled(" r ", *key_style), Span::styled(" Move to review", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" x ", *key_style), Span::styled(" Reset (cleanup and move to Planned)", *label_style),
            ]));
        }

        crate::model::TaskStatus::Review => {
            lines.push(Line::from(vec![
                Span::styled(" a ", *key_style), Span::styled(" Apply: test changes in main worktree", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" u ", *key_style), Span::styled(" Unapply: remove applied changes", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" r ", *key_style), Span::styled(" Rebase: update worktree to latest main", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" m ", *key_style), Span::styled(" Merge: finalize changes and mark done", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" d ", *key_style), Span::styled(" Discard: reject changes and mark done", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" c ", *key_style), Span::styled(" Check: view git diff/status report", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" f ", *key_style), Span::styled(" Feedback: send follow-up instructions", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" o ", *key_style), Span::styled(" Open interactive modal", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" t ", *key_style), Span::styled(" Open test shell", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" x ", *key_style), Span::styled(" Reset (cleanup and move to Planned)", *label_style),
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
                Span::styled(" e ", *key_style), Span::styled(" Edit task", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" d ", *key_style), Span::styled(" Delete task", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" r ", *key_style), Span::styled(" Move back to Review", *label_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled(" x ", *key_style), Span::styled(" Reset (cleanup and move to Planned)", *label_style),
            ]));
        }
    }

    // General navigation help
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("─".repeat(40), *dim_style)));
    lines.push(Line::from(Span::styled("Navigation", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" ←/h ", *key_style), Span::styled(" Previous tab", *label_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" →/l ", *key_style), Span::styled(" Next tab", *label_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" Esc ", *key_style), Span::styled(" Close modal", *label_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  ?  ", *key_style), Span::styled(" Full help overlay", *label_style),
    ]));
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

/// Render help overlay with scrolling support
fn render_help(frame: &mut Frame, scroll_offset: usize) {
    // Minimum width to fit the longest help text line plus borders
    const MIN_WIDTH: u16 = 58;

    let mut area = centered_rect(60, 80, frame.area());

    // Enforce minimum width (centered within screen)
    if area.width < MIN_WIDTH {
        let screen = frame.area();
        let actual_width = MIN_WIDTH.min(screen.width);
        area.width = actual_width;
        area.x = screen.x + (screen.width.saturating_sub(actual_width)) / 2;
    }

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
            Span::styled("InProgress Column", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  f          Live feedback: send message to running task"),
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
        Line::from(""),
        Line::from(vec![
            Span::styled("Git", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  P          Pull from remote"),
        Line::from("  p          Push to remote (when commits ahead)"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Other", Style::default().add_modifier(Modifier::UNDERLINED)),
        ]),
        Line::from("  Ctrl-S     Settings (editor, commands)"),
        Line::from("  ?          Toggle this help"),
        Line::from(""),
        Line::from(Span::styled(
            "j/k to scroll, any other key to close",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    // Calculate if scrolling is needed and show indicator
    let content_height = help_text.len();
    // Account for border (2 lines: top + bottom)
    let visible_height = area.height.saturating_sub(2) as usize;
    let can_scroll = content_height > visible_height;
    let at_bottom = scroll_offset + visible_height >= content_height;

    // Build title with scroll indicator
    let title = if can_scroll {
        if scroll_offset > 0 && !at_bottom {
            " Help [↑↓] "
        } else if scroll_offset > 0 {
            " Help [↑] "
        } else {
            " Help [↓] "
        }
    } else {
        " Help "
    };

    let help = Paragraph::new(help_text)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .style(Style::default().fg(Color::White))
        .scroll((scroll_offset as u16, 0));

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
    let area = centered_rect(85, 75, frame.area());

    let slot = app.model.ui_state.open_project_dialog_slot.unwrap_or(0);
    let is_creating = app.model.ui_state.create_folder_input.is_some();

    // Clear area first
    frame.render_widget(ratatui::widgets::Clear, area);

    // Split the area: title, breadcrumb path, columns, create input (optional), hints
    let chunks = if is_creating {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),  // Title
                Constraint::Length(1),  // Breadcrumb path
                Constraint::Min(8),     // Miller columns
                Constraint::Length(3),  // Create folder input
                Constraint::Length(2),  // Hints
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),  // Title
                Constraint::Length(1),  // Breadcrumb path
                Constraint::Min(10),    // Miller columns
                Constraint::Length(2),  // Hints
            ])
            .split(area)
    };

    // Render title
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" Open project in slot [{}] ", slot + 1),
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
        ),
    ]));
    frame.render_widget(title, chunks[0]);

    // Render directory browser with Miller columns
    if let Some(ref browser) = app.model.ui_state.directory_browser {
        // Breadcrumb path display
        let path_str = browser
            .cwd()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~".to_string());
        let path_display = Paragraph::new(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                path_str,
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        frame.render_widget(path_display, chunks[1]);

        // Render three Miller columns
        render_miller_columns(frame, chunks[2], browser, app);
    }

    // Render create folder input if in create mode
    if let Some(ref input) = app.model.ui_state.create_folder_input {
        let input_area = chunks[3];
        let input_widget = Paragraph::new(Line::from(vec![
            Span::styled(" New folder: ", Style::default().fg(Color::Cyan)),
            Span::styled(input.as_str(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("█", Style::default().fg(Color::White)), // Cursor
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" Create New Project Folder (git init) "),
        );
        frame.render_widget(input_widget, input_area);

        // Render hints for create mode
        let hints = Paragraph::new(Line::from(Span::styled(
            "Enter: Create folder  Esc: Cancel",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(hints, chunks[4]);
    } else {
        // Render normal hints
        let hints = Paragraph::new(Line::from(Span::styled(
            "↑↓: Navigate  ←→: Columns  Enter: Open project  Esc: Cancel  Type letter to jump",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(hints, chunks[3]);
    }
}

/// Render Miller columns (directory browser with preview)
fn render_miller_columns(
    frame: &mut Frame,
    area: Rect,
    browser: &crate::model::DirectoryBrowser,
    _app: &App,
) {
    // Get preview entries for the selected directory
    let preview_entries = browser.get_preview_entries();

    // Determine which columns have content (up to and including active column)
    // Don't show empty columns on the left when at root
    let mut columns_to_show: Vec<(usize, &MillerColumn)> = Vec::new();
    for col_idx in 0..=browser.active_column {
        if let Some(ref column) = browser.columns[col_idx] {
            columns_to_show.push((col_idx, column));
        }
    }

    // Always show at least 2 columns: active + preview/empty on right
    let num_content_columns = columns_to_show.len();
    let total_columns = num_content_columns + 1; // +1 for preview column on right

    // Calculate column widths - distribute evenly
    let pct = 100 / total_columns as u16;
    let mut constraints: Vec<Constraint> = Vec::new();
    for i in 0..total_columns {
        if i > 0 {
            constraints.push(Constraint::Length(1)); // Separator
        }
        // Last column gets remaining percentage
        if i == total_columns - 1 {
            constraints.push(Constraint::Percentage(100 - pct * (total_columns as u16 - 1)));
        } else {
            constraints.push(Constraint::Percentage(pct));
        }
    }

    let column_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    // Render content columns (areas at indices 0, 2, 4, ... are columns; 1, 3, 5, ... are separators)
    for (display_idx, (col_idx, column)) in columns_to_show.iter().enumerate() {
        let chunk_idx = display_idx * 2; // Skip separator indices
        let is_active = *col_idx == browser.active_column;
        render_miller_column(frame, column_chunks[chunk_idx], column, is_active);
    }

    // Render separators between content columns
    for sep_idx in 0..num_content_columns {
        let chunk_idx = sep_idx * 2 + 1;
        if chunk_idx < column_chunks.len() {
            render_column_separator(frame, column_chunks[chunk_idx]);
        }
    }

    // Render preview column on the right (always shown)
    let preview_chunk_idx = num_content_columns * 2;
    if preview_chunk_idx < column_chunks.len() {
        if let Some(ref entries) = preview_entries {
            render_preview_column(frame, column_chunks[preview_chunk_idx], entries, browser);
        } else {
            // Empty preview column
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));
            frame.render_widget(block, column_chunks[preview_chunk_idx]);
        }
    }
}

/// Render a single Miller column
fn render_miller_column(
    frame: &mut Frame,
    area: Rect,
    column: &MillerColumn,
    is_active: bool,
) {
    let items: Vec<ListItem> = column
        .entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let is_selected = idx == column.selected_idx;

            // Determine display text and suffix
            let (display_text, suffix) = match entry.special {
                SpecialEntry::NewProjectHere => ("[New Project Here]".to_string(), ""),
                SpecialEntry::ParentDir => ("..".to_string(), " ↩"),
                SpecialEntry::None => (entry.name.clone(), if entry.is_dir { " →" } else { "" }),
            };

            // Styling based on selection and active state
            let style = if is_selected && is_active {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else if is_selected {
                Style::default().fg(Color::Cyan)
            } else if entry.special == SpecialEntry::NewProjectHere {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {}{} ", display_text, suffix), style),
            ]))
        })
        .collect();

    let border_color = if is_active { Color::Yellow } else { Color::DarkGray };

    // Get directory name for title
    let title = column
        .dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| format!(" {} ", s))
        .unwrap_or_else(|| " / ".to_string());

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(title),
    );

    let mut list_state = ListState::default().with_selected(Some(column.selected_idx));
    frame.render_stateful_widget(list, area, &mut list_state);
}

/// Render a vertical separator between columns
fn render_column_separator(frame: &mut Frame, area: Rect) {
    let sep = Paragraph::new(
        (0..area.height)
            .map(|_| Line::from("│"))
            .collect::<Vec<_>>(),
    )
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(sep, area);
}

/// Render a preview column showing contents of selected directory
fn render_preview_column(
    frame: &mut Frame,
    area: Rect,
    entries: &[DirEntry],
    browser: &crate::model::DirectoryBrowser,
) {
    // Get the selected entry name for the title
    let title = browser
        .selected()
        .map(|e| format!(" {} ", e.name))
        .unwrap_or_else(|| " Preview ".to_string());

    let items: Vec<ListItem> = entries
        .iter()
        .map(|entry| {
            let suffix = if entry.is_dir { " →" } else { "" };
            let style = Style::default().fg(Color::DarkGray);
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {}{} ", entry.name, suffix), style),
            ]))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(title),
    );

    frame.render_widget(list, area);
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

/// Render a confirmation modal for multiline messages (like merge check reports or conflict details)
fn render_confirmation_modal(frame: &mut Frame, message: &str, scroll_offset: usize, action: &crate::model::PendingAction) {
    use crate::model::PendingAction;

    // Calculate size based on content
    let line_count = message.lines().count();
    let max_line_width = message.lines().map(|l| l.len()).max().unwrap_or(40);

    // Size the modal to fit content with some padding, but cap at 80% height
    let height_percent = ((line_count + 4) * 100 / frame.area().height as usize).min(80).max(30) as u16;
    let width_percent = ((max_line_width + 6) * 100 / frame.area().width as usize).min(90).max(50) as u16;

    let area = centered_rect(width_percent, height_percent, frame.area());

    // Check if content is scrollable (more lines than visible area)
    let visible_height = area.height.saturating_sub(2) as usize; // Account for borders
    let is_scrollable = line_count > visible_height;

    // Build lines with styling
    let mut lines: Vec<Line> = Vec::new();
    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White);
    let verdict_merged = Style::default().fg(Color::Green).add_modifier(Modifier::BOLD);
    let verdict_not_merged = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let warning_style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
    let conflict_style = Style::default().fg(Color::Red);
    let file_path_style = Style::default().fg(Color::Cyan);
    let error_style = Style::default().fg(Color::LightRed);

    // Determine if this is a conflict modal for special styling
    let is_conflict_modal = matches!(action, PendingAction::ApplyConflict { .. });

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
        } else if line.contains("[Y]") || line.contains("[N]") || line.contains("'y'") || line.contains("'n'") {
            Line::from(Span::styled(line, Style::default().fg(Color::Yellow)))
        } else if is_conflict_modal {
            // Special styling for conflict output
            if line.contains("error:") || line.contains("CONFLICT") {
                Line::from(Span::styled(line, conflict_style))
            } else if line.starts_with("Applying:") || line.contains("patch does not apply") || line.contains("Applied patch") {
                Line::from(Span::styled(line, error_style))
            } else if line.contains('/') && (line.ends_with(".rs") || line.ends_with(".ts") || line.ends_with(".js") || line.ends_with(".json") || line.ends_with(".toml") || line.ends_with(".md")) {
                // Likely a file path
                Line::from(Span::styled(line, file_path_style))
            } else {
                Line::from(Span::styled(line, value_style))
            }
        } else {
            Line::from(Span::styled(line, value_style))
        };
        lines.push(styled_line);
    }

    // Determine title based on action type
    let title = if is_conflict_modal {
        " Apply Conflict "
    } else {
        " Merge Check "
    };

    // Add scroll indicator to title if scrollable
    let title_with_scroll = if is_scrollable {
        let current_line = scroll_offset + 1;
        let total_lines = line_count;
        format!("{} [{}/{}] ↑↓ to scroll ", title.trim(), current_line, total_lines)
    } else {
        title.to_string()
    };

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(title_with_scroll)
                .title_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        )
        .wrap(ratatui::widgets::Wrap { trim: false })
        .scroll((scroll_offset as u16, 0));

    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(modal, area);
}

/// Render the configuration modal
fn render_config_modal(frame: &mut Frame, app: &App) {
    use crate::model::{ConfigField, Editor};

    let area = centered_rect(65, 70, frame.area());

    let Some(ref config) = app.model.ui_state.config_modal else {
        return;
    };

    let project_name = app.model.active_project()
        .map(|p| p.name.as_str())
        .unwrap_or("No Project");

    // Build the modal content
    let mut lines = vec![
        Line::from(Span::styled(
            "Configuration",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    // Section: Global Settings
    lines.push(Line::from(vec![
        Span::styled("Global Settings", Style::default().fg(Color::Cyan).add_modifier(Modifier::UNDERLINED)),
    ]));
    lines.push(Line::from(""));

    // Default Editor field
    let is_selected = config.selected_field == ConfigField::DefaultEditor;
    let is_editing = is_selected && config.editing;

    let editor_value = if is_editing {
        // Show all editors with current selection highlighted
        let editors: Vec<String> = Editor::all().iter().map(|e| {
            if *e == config.temp_editor {
                format!("[{}]", e.name())
            } else {
                e.name().to_string()
            }
        }).collect();
        editors.join("  ")
    } else {
        config.temp_editor.name().to_string()
    };

    let (prefix, style, value_style) = if is_selected {
        (
            "► ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            if is_editing {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            }
        )
    } else {
        ("  ", Style::default(), Style::default().fg(Color::DarkGray))
    };

    lines.push(Line::from(vec![
        Span::styled(prefix, style),
        Span::styled("Default Editor: ", style),
        Span::styled(editor_value, value_style),
    ]));
    if is_selected {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(ConfigField::DefaultEditor.hint(), Style::default().fg(Color::DarkGray)),
        ]));
    }
    lines.push(Line::from(""));

    // Section: Project Commands
    lines.push(Line::from(vec![
        Span::styled(
            format!("Project: {}", project_name),
            Style::default().fg(Color::Magenta).add_modifier(Modifier::UNDERLINED)
        ),
    ]));
    lines.push(Line::from(""));

    // Command fields
    let command_fields = [
        (ConfigField::CheckCommand, &config.temp_commands.check),
        (ConfigField::RunCommand, &config.temp_commands.run),
        (ConfigField::TestCommand, &config.temp_commands.test),
        (ConfigField::FormatCommand, &config.temp_commands.format),
        (ConfigField::LintCommand, &config.temp_commands.lint),
    ];

    for (field, value) in command_fields {
        let is_selected = config.selected_field == field;
        let is_editing = is_selected && config.editing;

        let display_value = if is_editing {
            if config.edit_buffer.is_empty() {
                "_".to_string()
            } else {
                format!("{}_", config.edit_buffer)
            }
        } else {
            value.clone().unwrap_or_else(|| "(auto-detect)".to_string())
        };

        let (prefix, style, value_style) = if is_selected {
            (
                "► ",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                if is_editing {
                    Style::default().fg(Color::Green)
                } else if value.is_some() {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::DarkGray)
                }
            )
        } else {
            (
                "  ",
                Style::default(),
                if value.is_some() {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Rgb(80, 80, 80))
                }
            )
        };

        lines.push(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(format!("{}: ", field.label()), style),
            Span::styled(display_value, value_style),
        ]));

        if is_selected {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(field.hint(), Style::default().fg(Color::DarkGray)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(""));

    // Footer with keybindings
    let editing_hints = if config.editing {
        "Enter confirm  Esc cancel"
    } else {
        "j/k navigate  Enter/l edit  r reset to defaults  Esc/q save & close"
    };
    lines.push(Line::from(Span::styled(
        editing_hints,
        Style::default().fg(Color::DarkGray),
    )));

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Settings ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .style(Style::default().fg(Color::White));

    // Clear area first
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(modal, area);
}

/// Render the stash management modal
fn render_stash_modal(frame: &mut Frame, app: &App) {
    let area = centered_rect(60, 60, frame.area());

    let Some(project) = app.model.active_project() else {
        return;
    };

    let stashes = &project.tracked_stashes;
    let selected_idx = app.model.ui_state.stash_modal_selected_idx;

    let mut lines = vec![
        Line::from(Span::styled(
            "Tracked Stashes",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    if stashes.is_empty() {
        lines.push(Line::from(Span::styled(
            "No tracked stashes",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let label_style = Style::default().fg(Color::DarkGray);
        let value_style = Style::default().fg(Color::White);
        let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

        for (idx, stash) in stashes.iter().enumerate() {
            let is_selected = idx == selected_idx;
            let prefix = if is_selected { "► " } else { "  " };
            let style = if is_selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            // Stash header: icon + short SHA + description
            lines.push(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled("📦 ", style),
                Span::styled(&stash.stash_sha[..8.min(stash.stash_sha.len())], Style::default().fg(Color::Magenta)),
                Span::styled(" ", style),
                Span::styled(&stash.description, style),
            ]));

            // If selected, show details
            if is_selected {
                // Time since created
                let elapsed = chrono::Utc::now().signed_duration_since(stash.created_at);
                let time_ago = if elapsed.num_minutes() < 1 {
                    "just now".to_string()
                } else if elapsed.num_hours() < 1 {
                    format!("{}m ago", elapsed.num_minutes())
                } else if elapsed.num_hours() < 24 {
                    format!("{}h ago", elapsed.num_hours())
                } else {
                    format!("{}d ago", elapsed.num_days())
                };

                lines.push(Line::from(vec![
                    Span::raw("      "),
                    Span::styled("Created: ", label_style),
                    Span::styled(time_ago, value_style),
                    Span::styled("  │  ", label_style),
                    Span::styled(format!("{} files changed", stash.files_changed), value_style),
                ]));

                if !stash.files_summary.is_empty() {
                    // Show files summary, truncated if needed
                    let summary = if stash.files_summary.len() > 40 {
                        format!("{}...", &stash.files_summary[..37])
                    } else {
                        stash.files_summary.clone()
                    };
                    lines.push(Line::from(vec![
                        Span::raw("      "),
                        Span::styled("Files: ", label_style),
                        Span::styled(summary, Style::default().fg(Color::Gray)),
                    ]));
                }

                lines.push(Line::from(""));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("─".repeat(40), Style::default().fg(Color::DarkGray))));
    lines.push(Line::from(""));

    // Key hints
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let hint_style = Style::default().fg(Color::DarkGray);

    if !stashes.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("p", key_style),
            Span::styled(" pop  ", hint_style),
            Span::styled("d", key_style),
            Span::styled(" drop  ", hint_style),
            Span::styled("j/k", key_style),
            Span::styled(" navigate  ", hint_style),
            Span::styled("Esc/S/q", key_style),
            Span::styled(" close", hint_style),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Esc/S/q", key_style),
            Span::styled(" close", hint_style),
        ]));
    }

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Stash Manager ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .style(Style::default().fg(Color::White));

    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(modal, area);
}
