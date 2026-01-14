use crate::app::App;
use crate::model::{FocusArea, TaskStatus};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

/// Render the Kanban board with six columns in a 2x3 grid
pub fn render_kanban(frame: &mut Frame, area: Rect, app: &App) {
    let is_focused = app.model.ui_state.focus == FocusArea::KanbanBoard;

    let block = Block::default()
        .title(Span::styled(
            " Kanban Board ",
            if is_focused {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ))
        .borders(Borders::ALL)
        .border_style(if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        });

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split into 3 rows x 2 columns for the six statuses
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(inner);

    let row1_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);

    let row2_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);

    let row3_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[2]);

    // Render each column in 2x3 layout:
    // Row 1: Planned | Queued
    // Row 2: InProgress | NeedsInput
    // Row 3: Review | Done
    render_column(frame, row1_cols[0], app, TaskStatus::Planned);
    render_column(frame, row1_cols[1], app, TaskStatus::Queued);
    render_column(frame, row2_cols[0], app, TaskStatus::InProgress);
    render_column(frame, row2_cols[1], app, TaskStatus::NeedsInput);
    render_column(frame, row3_cols[0], app, TaskStatus::Review);
    render_column(frame, row3_cols[1], app, TaskStatus::Done);
}

/// Render a single column of the Kanban board
fn render_column(frame: &mut Frame, area: Rect, app: &App, status: TaskStatus) {
    let is_selected = app.model.ui_state.selected_column == status
        && app.model.ui_state.focus == FocusArea::KanbanBoard;

    // (title, background color, contrasting foreground for selected items)
    // Note: Accepting tasks appear in the Review column, so Accepting is styled like Review
    let (title, color, contrast_fg) = match status {
        TaskStatus::Planned => ("Planned", Color::Blue, Color::White),
        TaskStatus::Queued => ("Queued", Color::Cyan, Color::Black),
        TaskStatus::InProgress => ("In Progress", Color::Yellow, Color::Black),
        TaskStatus::NeedsInput => ("Needs Input", Color::Red, Color::White),
        TaskStatus::Review | TaskStatus::Accepting => ("Review", Color::Magenta, Color::White),
        TaskStatus::Done => ("Done", Color::Green, Color::Black),
    };

    let border_style = if is_selected {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    // Get task count for this column
    let task_count = app
        .model
        .active_project()
        .map(|p| p.tasks_by_status(status).len())
        .unwrap_or(0);

    let block = Block::default()
        .title(Line::from(vec![
            Span::styled(
                format!(" {} ", title),
                if is_selected {
                    Style::default().fg(color).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
            Span::styled(
                format!("({})", task_count),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);

    // Get tasks for this column (with dividers)
    let tasks: Vec<ListItem> = app
        .model
        .active_project()
        .map(|project| {
            project
                .tasks_by_status(status)
                .iter()
                .enumerate()
                .flat_map(|(idx, task)| {
                    let is_at_this_idx = is_selected
                        && app.model.ui_state.selected_task_idx == Some(idx);
                    // Task is selected only if at this idx AND not selecting any divider
                    let is_task_selected = is_at_this_idx
                        && !app.model.ui_state.selected_is_divider
                        && !app.model.ui_state.selected_is_divider_above;
                    // Divider below is selected if at this idx AND selecting divider
                    let is_divider_selected = is_at_this_idx
                        && app.model.ui_state.selected_is_divider;
                    // Divider above is selected if idx == 0 AND selecting divider above
                    let is_divider_above_selected = idx == 0
                        && is_selected
                        && app.model.ui_state.selected_is_divider_above;

                    let style = if is_task_selected {
                        Style::default()
                            .fg(contrast_fg)
                            .bg(color)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    // Handle long titles - marquee scroll for selected, truncate for others
                    let max_len = (inner.width as usize).saturating_sub(4);
                    let title_chars: Vec<char> = task.title.chars().collect();
                    let title_len = title_chars.len();

                    let title = if title_len > max_len {
                        if is_task_selected {
                            // Marquee scroll for selected task
                            let scroll_offset = app.model.ui_state.title_scroll_offset;
                            // Add padding at end for smooth wrap-around
                            let padded: String = title_chars.iter().collect::<String>() + "   •   ";
                            let padded_chars: Vec<char> = padded.chars().collect();
                            let padded_len = padded_chars.len();

                            // Get a window starting at scroll offset
                            let start = scroll_offset % padded_len;
                            let visible: String = padded_chars.iter()
                                .cycle()
                                .skip(start)
                                .take(max_len)
                                .collect();
                            visible
                        } else {
                            // Simple truncation for non-selected tasks
                            let truncated: String = title_chars.iter().take(max_len.saturating_sub(3)).collect();
                            format!("{}...", truncated)
                        }
                    } else {
                        task.title.clone()
                    };

                    // Add spinner for in-progress tasks, pulsing indicator for needs-input,
                    // merge animation for accepting tasks
                    let spinner_frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
                    let pulse_frames = ['◐', '◓', '◑', '◒'];
                    let merge_frames = ['⟳', '↻', '⟲', '↺'];
                    let prefix = match task.status {
                        TaskStatus::InProgress => {
                            let frame = app.model.ui_state.animation_frame % spinner_frames.len();
                            format!("{} ", spinner_frames[frame])
                        }
                        TaskStatus::NeedsInput => {
                            let frame = app.model.ui_state.animation_frame % pulse_frames.len();
                            format!("{} ", pulse_frames[frame])
                        }
                        TaskStatus::Accepting => {
                            let frame = app.model.ui_state.animation_frame % merge_frames.len();
                            format!("{} ", merge_frames[frame])
                        }
                        _ => String::new(),
                    };

                    let text = if !task.images.is_empty() {
                        format!("{}{} [img]", prefix, title)
                    } else {
                        format!("{}{}", prefix, title)
                    };

                    let task_item = ListItem::new(Line::from(Span::styled(text, style)));

                    let mut items = Vec::new();

                    // Add divider above first task if enabled
                    if idx == 0 && task.divider_above {
                        let divider_width = inner.width.saturating_sub(2) as usize;

                        // Build divider line with optional centered title
                        let divider_line = if let Some(title) = &task.divider_above_title {
                            let title_with_padding = format!(" {} ", title);
                            let title_len = title_with_padding.chars().count();
                            if title_len >= divider_width {
                                title_with_padding.chars().take(divider_width).collect()
                            } else {
                                let remaining = divider_width - title_len;
                                let left_dashes = remaining / 2;
                                let right_dashes = remaining - left_dashes;
                                format!(
                                    "{}{}{}",
                                    "─".repeat(left_dashes),
                                    title_with_padding,
                                    "─".repeat(right_dashes)
                                )
                            }
                        } else {
                            "─".repeat(divider_width)
                        };

                        let divider_style = if is_divider_above_selected {
                            Style::default()
                                .fg(contrast_fg)
                                .bg(color)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };
                        items.push(ListItem::new(Line::from(Span::styled(
                            divider_line,
                            divider_style,
                        ))));
                    }

                    items.push(task_item);

                    // Add divider after task if enabled
                    if task.divider_below {
                        let divider_width = inner.width.saturating_sub(2) as usize;

                        // Build divider line with optional centered title
                        let divider_line = if let Some(title) = &task.divider_title {
                            // Center the title in the divider line
                            let title_with_padding = format!(" {} ", title);
                            let title_len = title_with_padding.chars().count();
                            if title_len >= divider_width {
                                // Title too long, just show it
                                title_with_padding.chars().take(divider_width).collect()
                            } else {
                                let remaining = divider_width - title_len;
                                let left_dashes = remaining / 2;
                                let right_dashes = remaining - left_dashes;
                                format!(
                                    "{}{}{}",
                                    "─".repeat(left_dashes),
                                    title_with_padding,
                                    "─".repeat(right_dashes)
                                )
                            }
                        } else {
                            "─".repeat(divider_width)
                        };

                        // Highlight divider if selected
                        let divider_style = if is_divider_selected {
                            Style::default()
                                .fg(contrast_fg)
                                .bg(color)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };
                        items.push(ListItem::new(Line::from(Span::styled(
                            divider_line,
                            divider_style,
                        ))));
                    }

                    items
                })
                .collect()
        })
        .unwrap_or_default();

    frame.render_widget(block, area);

    // Tasks use the full inner area (hints are on the border now)
    let tasks_area = inner;

    if tasks.is_empty() {
        let empty_msg = Paragraph::new(Span::styled(
            "No tasks",
            Style::default().fg(Color::DarkGray),
        ));
        frame.render_widget(empty_msg, tasks_area);
    } else {
        let list = List::new(tasks);
        let mut list_state = ListState::default();

        // Calculate visual index accounting for dividers
        let visual_idx = if is_selected {
            if let Some(task_idx) = app.model.ui_state.selected_task_idx {
                if let Some(project) = app.model.active_project() {
                    let filtered_tasks = project.tasks_by_status(status);

                    // Check if first task has divider_above
                    let has_divider_above = filtered_tasks.first()
                        .map(|t| t.divider_above)
                        .unwrap_or(false);

                    // If selecting divider_above, visual index is 0
                    if app.model.ui_state.selected_is_divider_above && task_idx == 0 {
                        Some(0)
                    } else {
                        // Count dividers before selected task
                        let dividers_before: usize = filtered_tasks.iter()
                            .take(task_idx)
                            .filter(|t| t.divider_below)
                            .count();
                        // Start with task_idx + dividers before
                        let mut idx = task_idx + dividers_before;
                        // Add 1 if there's a divider_above (shifts everything down)
                        if has_divider_above {
                            idx += 1;
                        }
                        // If divider below is selected, add 1 to select the divider itself
                        if app.model.ui_state.selected_is_divider {
                            idx += 1;
                        }
                        Some(idx)
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            // Use saved scroll offset for unselected columns to preserve scroll position
            let saved_offset = app.model.ui_state.column_scroll_offsets[status.index()];
            Some(saved_offset)
        };

        list_state.select(visual_idx);
        frame.render_stateful_widget(list, tasks_area, &mut list_state);
    }

    // Show keyboard hints on the bottom border when column is selected
    if is_selected {
        let hints = get_column_hints(status);
        // Calculate the width of hints text
        let hints_text: String = hints.iter().map(|s| s.content.as_ref()).collect();
        let hints_width = hints_text.chars().count() as u16;

        // Position on bottom border, right-aligned (leave space for corner)
        if area.width > hints_width + 2 {
            let hints_area = Rect {
                x: area.x + area.width - hints_width - 2,
                y: area.y + area.height - 1,
                width: hints_width,
                height: 1,
            };
            let hints_widget = Paragraph::new(Line::from(hints));
            frame.render_widget(hints_widget, hints_area);
        }
    }

    // Show count badge for Review and NeedsInput columns
    let badge_count = match status {
        TaskStatus::Review => app.model.active_project().map(|p| p.review_count()),
        TaskStatus::NeedsInput => app.model.active_project().map(|p| p.needs_input_count()),
        _ => None,
    };

    if let Some(count) = badge_count {
        if count > 0 {
            let badge = format!(" {} ", count);
            let badge_area = Rect {
                x: area.x + area.width.saturating_sub(badge.len() as u16 + 2),
                y: area.y,
                width: badge.len() as u16 + 1,
                height: 1,
            };
            let badge_widget = Span::styled(
                badge,
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            );
            frame.render_widget(Paragraph::new(badge_widget), badge_area);
        }
    }

    // Render scrollbar if there are more items than visible area
    render_scrollbar(frame, area, inner, app, status, is_selected);
}

/// Render a subtle scrollbar on the right border when content overflows
fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &App,
    status: TaskStatus,
    is_selected: bool,
) {
    let visible_height = inner.height as usize;
    if visible_height == 0 {
        return;
    }

    // Calculate total visual items (tasks + dividers)
    let (total_items, scroll_offset) = if let Some(project) = app.model.active_project() {
        let tasks = project.tasks_by_status(status);
        let mut total = tasks.len();

        // Add dividers to count
        for task in &tasks {
            if task.divider_below {
                total += 1;
            }
            if task.divider_above {
                total += 1;
            }
        }

        // Calculate scroll offset based on selected item
        let offset = if is_selected && app.model.ui_state.selected_column == status {
            if let Some(task_idx) = app.model.ui_state.selected_task_idx {
                // Calculate visual index (same logic as list rendering)
                let has_divider_above = tasks.first().map(|t| t.divider_above).unwrap_or(false);
                let dividers_before: usize = tasks.iter()
                    .take(task_idx)
                    .filter(|t| t.divider_below)
                    .count();
                let mut visual_idx = task_idx + dividers_before;
                if has_divider_above {
                    visual_idx += 1;
                }
                if app.model.ui_state.selected_is_divider {
                    visual_idx += 1;
                }
                // Estimate scroll position - the list widget centers selected item
                visual_idx.saturating_sub(visible_height / 2)
            } else {
                0
            }
        } else {
            0
        };

        (total, offset)
    } else {
        (0, 0)
    };

    // Only show scrollbar if content overflows
    if total_items <= visible_height {
        return;
    }

    // Calculate scrollbar dimensions
    // The scrollbar track is on the right border, inside the corners
    let track_height = area.height.saturating_sub(2) as usize; // Exclude top and bottom borders
    if track_height == 0 {
        return;
    }

    // Calculate thumb size (minimum 1 character)
    let thumb_size = ((visible_height as f64 / total_items as f64) * track_height as f64)
        .ceil()
        .max(1.0) as usize;

    // Calculate thumb position
    let max_scroll = total_items.saturating_sub(visible_height);
    let scroll_ratio = if max_scroll > 0 {
        scroll_offset.min(max_scroll) as f64 / max_scroll as f64
    } else {
        0.0
    };
    let thumb_pos = ((track_height.saturating_sub(thumb_size)) as f64 * scroll_ratio) as usize;

    // Draw the scrollbar on the right border
    let scrollbar_x = area.x + area.width - 1;
    let track_start_y = area.y + 1; // Skip top border

    // Choose colors - subtle when not selected, slightly more visible when selected
    let track_char = '│';
    let thumb_char = '┃';
    let track_style = Style::default().fg(Color::DarkGray);
    let thumb_style = if is_selected {
        Style::default().fg(Color::Gray)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    for i in 0..track_height {
        let y = track_start_y + i as u16;
        let is_thumb = i >= thumb_pos && i < thumb_pos + thumb_size;

        let (ch, style) = if is_thumb {
            (thumb_char, thumb_style)
        } else {
            (track_char, track_style)
        };

        // Render single character
        let cell_area = Rect {
            x: scrollbar_x,
            y,
            width: 1,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(ch.to_string(), style)),
            cell_area,
        );
    }
}

/// Get keyboard hints for a column based on its status
fn get_column_hints(status: TaskStatus) -> Vec<Span<'static>> {
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::DarkGray);

    match status {
        TaskStatus::Planned => vec![
            Span::styled("⏎", key_style),
            Span::styled("start ", desc_style),
            Span::styled("e", key_style),
            Span::styled("dit ", desc_style),
            Span::styled("d", key_style),
            Span::styled("el ", desc_style),
            Span::styled("r", key_style),
            Span::styled("ev ", desc_style),
            Span::styled("x", key_style),
            Span::styled("-done", desc_style),
        ],
        TaskStatus::Queued => vec![
            Span::styled("⏎", key_style),
            Span::styled("start ", desc_style),
            Span::styled("p", key_style),
            Span::styled("lan ", desc_style),
            Span::styled("e", key_style),
            Span::styled("dit ", desc_style),
            Span::styled("d", key_style),
            Span::styled("el ", desc_style),
            Span::styled("x", key_style),
            Span::styled("-done", desc_style),
        ],
        TaskStatus::InProgress => vec![
            Span::styled("o", key_style),
            Span::styled("pen ", desc_style),
            Span::styled("t", key_style),
            Span::styled("est ", desc_style),
            Span::styled("r", key_style),
            Span::styled("estart ", desc_style),
            Span::styled("x", key_style),
            Span::styled("-done", desc_style),
        ],
        TaskStatus::NeedsInput => vec![
            Span::styled("o", key_style),
            Span::styled("pen ", desc_style),
            Span::styled("t", key_style),
            Span::styled("est ", desc_style),
            Span::styled("r", key_style),
            Span::styled("estart ", desc_style),
            Span::styled("x", key_style),
            Span::styled("-done", desc_style),
        ],
        TaskStatus::Review => vec![
            Span::styled("o", key_style),
            Span::styled("pen ", desc_style),
            Span::styled("t", key_style),
            Span::styled("est ", desc_style),
            Span::styled("a", key_style),
            Span::styled("pply ", desc_style),
            Span::styled("r", key_style),
            Span::styled("estart ", desc_style),
            Span::styled("y", key_style),
            Span::styled("/", desc_style),
            Span::styled("n", key_style),
            Span::styled(" accept/discard", desc_style),
        ],
        TaskStatus::Accepting => vec![
            Span::styled("rebasing...", desc_style),
        ],
        TaskStatus::Done => vec![
            Span::styled("e", key_style),
            Span::styled("dit ", desc_style),
            Span::styled("d", key_style),
            Span::styled("el ", desc_style),
            Span::styled("r", key_style),
            Span::styled("ev", desc_style),
        ],
    }
}
