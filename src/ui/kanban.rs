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
    // Middle row (InProgress/NeedsWork) is smaller since those columns typically have fewer tasks
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(42),
            Constraint::Percentage(17),
            Constraint::Percentage(41),
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
    // Row 1: Planned | InProgress
    // Row 2: Testing | NeedsWork
    // Row 3: Review | Done
    render_column(frame, row1_cols[0], app, TaskStatus::Planned);
    render_column(frame, row1_cols[1], app, TaskStatus::InProgress);
    render_column(frame, row2_cols[0], app, TaskStatus::Testing);
    render_column(frame, row2_cols[1], app, TaskStatus::NeedsWork);
    render_column(frame, row3_cols[0], app, TaskStatus::Review);
    render_column(frame, row3_cols[1], app, TaskStatus::Done);
}

/// Render a single column of the Kanban board
fn render_column(frame: &mut Frame, area: Rect, app: &App, status: TaskStatus) {
    let is_selected = app.model.ui_state.selected_column == status
        && app.model.ui_state.focus == FocusArea::KanbanBoard;

    // (number, title, background color, contrasting foreground for selected items)
    // Note: Accepting/Updating tasks appear in the Review column, so they're styled like Review
    let (num, title, color, contrast_fg) = match status {
        TaskStatus::Planned => ("1", "Planned", Color::Blue, Color::White),
        TaskStatus::InProgress => ("2", "In Progress", Color::Yellow, Color::Black),
        TaskStatus::Testing => ("3", "Testing", Color::Cyan, Color::Black),
        TaskStatus::NeedsWork => ("4", "Needs Work", Color::Red, Color::White),
        TaskStatus::Review | TaskStatus::Accepting | TaskStatus::Updating | TaskStatus::Applying => ("5", "Review", Color::Magenta, Color::White),
        TaskStatus::Done => ("6", "Done", Color::Green, Color::Black),
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
                format!(" {}", num),
                Style::default().fg(Color::DarkGray),
            ),
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

    // Get tasks for this column
    let tasks: Vec<ListItem> = app
        .model
        .active_project()
        .map(|project| {
            project
                .tasks_by_status(status)
                .iter()
                .enumerate()
                .map(|(idx, task)| {
                    let is_task_selected = is_selected
                        && app.model.ui_state.selected_task_idx == Some(idx);

                    // Check if this task is the one being feedbacked
                    let is_feedback_task = app.model.ui_state.feedback_task_id == Some(task.id);

                    // Check if this task is blocked (in Review but another task has lock/applied)
                    let is_blocked = if status == TaskStatus::Review {
                        // Check if another task's changes are applied
                        let blocked_by_applied = project.applied_task_id
                            .map(|id| id != task.id)
                            .unwrap_or(false);
                        // Check if another task has the worktree lock
                        let blocked_by_lock = project.main_worktree_lock
                            .as_ref()
                            .map(|lock| lock.task_id != task.id)
                            .unwrap_or(false);
                        blocked_by_applied || blocked_by_lock
                    } else {
                        false
                    };

                    // Styles for different parts of the task line
                    // Title gets the main style, brackets are very dim, code is dim
                    let (title_style, bracket_style, code_style) = if is_task_selected {
                        let base = Style::default().bg(color);
                        (
                            base.fg(contrast_fg).add_modifier(Modifier::BOLD),
                            base.fg(contrast_fg).add_modifier(Modifier::DIM),
                            base.fg(contrast_fg),
                        )
                    } else if is_feedback_task {
                        (
                            Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                            Style::default().fg(Color::DarkGray),
                            Style::default().fg(Color::DarkGray),
                        )
                    } else if is_blocked {
                        // Dimmed style for blocked tasks
                        (
                            Style::default().fg(Color::DarkGray),
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                            Style::default().fg(Color::DarkGray),
                        )
                    } else {
                        (
                            Style::default().fg(Color::White),
                            Style::default().fg(Color::DarkGray),
                            Style::default().fg(Color::Gray),
                        )
                    };

                    // Get 4-character task identifier (matches tmux session naming)
                    let task_id_short = &task.id.to_string()[..4];
                    let id_prefix_len = 7; // "[xxxx] " = 7 chars

                    // Handle long titles - marquee scroll for selected, truncate for others
                    // Reserve space for id prefix + some margin
                    let max_title_len = (inner.width as usize).saturating_sub(4 + id_prefix_len);
                    // Use short_title if available, otherwise use full title
                    let display_source = task.short_title.as_ref().unwrap_or(&task.title);
                    let title_chars: Vec<char> = display_source.chars().collect();
                    let title_len = title_chars.len();

                    let display_title = if title_len > max_title_len {
                        if is_task_selected {
                            // Marquee scroll for selected task - only scroll the title part
                            let scroll_offset = app.model.ui_state.title_scroll_offset;
                            // Add padding at end for smooth wrap-around
                            let padded: String = title_chars.iter().collect::<String>() + "   •   ";
                            let padded_chars: Vec<char> = padded.chars().collect();
                            let padded_len = padded_chars.len();

                            // Get a window starting at scroll offset
                            let start = scroll_offset % padded_len;
                            padded_chars.iter()
                                .cycle()
                                .skip(start)
                                .take(max_title_len)
                                .collect()
                        } else {
                            // Simple truncation for non-selected tasks
                            let truncated: String = title_chars.iter().take(max_title_len.saturating_sub(3)).collect();
                            format!("{}...", truncated)
                        }
                    } else {
                        display_source.clone()
                    };

                    // Add spinner for in-progress tasks, prompt indicator for needs-work (when Claude waiting),
                    // merge animation for accepting tasks, apply animation for applying tasks,
                    // and building animation for queued tasks that are preparing (creating worktree)
                    // InProgress uses the same spinner as Claude Code CLI: ·✢✳✶✻✽
                    let spinner_frames = ['·', '✢', '✳', '✶', '✻', '✽'];
                    // Blinking prompt: ~500ms on, ~500ms off (classic cursor blink at ~1Hz)
                    // At 100ms/tick: 5 frames on, 5 frames off = 1 second cycle
                    let prompt_frames = ['›', '›', '›', '›', '›', ' ', ' ', ' ', ' ', ' '];
                    let merge_frames = ['\u{E727}', '\u{E725}', '\u{E728}', '\u{E726}'];
                    let rebase_frames = ['↑', '⇧', '⇈', '⇪', '⇈', '⇧', '↑']; // Upward arrows for rebase
                    // Saved patterns: ['░', '▒', '▓', '█', '▓', '▒'] (fill), ['⠁', '⠂', '⠄', '⡀', '⢀', '⠠', '⠐', '⠈'] (rotating dot)
                    let apply_frames = ['⠁', '⠂', '⠄', '⡀', '⢀', '⠠', '⠐', '⠈']; // Rotating dot for applying
                    // Building blocks animation - foundation being laid (worktree preparation)
                    let building_frames = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█', '▇', '▆', '▅', '▄', '▃', '▂'];
                    let prefix = match task.status {
                        TaskStatus::InProgress if matches!(
                            task.session_state,
                            crate::model::ClaudeSessionState::Creating | crate::model::ClaudeSessionState::Starting
                        ) => {
                            // Building animation while worktree is being prepared
                            let frame = app.model.ui_state.animation_frame % building_frames.len();
                            format!("{} ", building_frames[frame])
                        }
                        TaskStatus::InProgress => {
                            // Spinner when Claude is actively working
                            // Slow down spinner: change every 2 ticks (200ms per frame)
                            let frame = (app.model.ui_state.animation_frame / 2) % spinner_frames.len();
                            format!("{} ", spinner_frames[frame])
                        }
                        TaskStatus::NeedsWork if task.session_state == crate::model::ClaudeSessionState::Paused => {
                            // Only show blinking prompt when Claude is actively waiting for input
                            let frame = app.model.ui_state.animation_frame % prompt_frames.len();
                            format!("{} ", prompt_frames[frame])
                        }
                        TaskStatus::Accepting => {
                            let frame = app.model.ui_state.animation_frame % merge_frames.len();
                            format!("{} ", merge_frames[frame])
                        }
                        TaskStatus::Updating => {
                            let frame = app.model.ui_state.animation_frame % rebase_frames.len();
                            format!("{} ", rebase_frames[frame])
                        }
                        TaskStatus::Applying => {
                            let frame = app.model.ui_state.animation_frame % apply_frames.len();
                            format!("{} ", apply_frames[frame])
                        }
                        _ => String::new(),
                    };

                    // Check if this task is being celebrated with the gold dust sweep animation
                    let is_celebrating = app.model.ui_state.merge_celebration
                        .as_ref()
                        .map(|c| c.task_id == task.id && c.column_status == status && c.task_index == idx)
                        .unwrap_or(false);

                    // Build spans with different styles: brackets very dim, code dim, title prominent
                    let mut spans = Vec::new();

                    if is_celebrating {
                        // Render the celebratory sparkle sweep animation
                        let celebration = app.model.ui_state.merge_celebration.as_ref().unwrap();
                        let phase = celebration.phase();
                        let frame = celebration.frame;

                        // Vibrant yellow/gold palette with sparkle shimmer effect
                        let shimmer = ((frame % 4) as i16 - 1) * 20; // -20, 0, +20, +40 brightness pulse
                        // Bright celebratory yellow - THE STAR (★)
                        let star_yellow = Color::Rgb(
                            255,
                            (230_i16 + shimmer).clamp(200, 255) as u8,
                            (50_i16 + shimmer / 2).clamp(20, 100) as u8,
                        );
                        // Bright gold sparkle (✦)
                        let bright_sparkle = Color::Rgb(255, 215, 60);
                        // Warm yellow for fading sparkle (✧)
                        let warm_yellow = Color::Rgb(255, 200, 100);
                        // Soft amber for dots
                        let soft_amber = Color::Rgb(220, 180, 80);

                        // Phase 1: Confirmation pulse - show full text in bright celebratory yellow
                        if phase == 1 {
                            // Build the full display text: prefix + "[id] " + title
                            let full_text = format!("{}[{}] {}", prefix, task_id_short, display_title);
                            // Pulse with bright celebratory yellow
                            let pulse_style = Style::default()
                                .fg(Color::Rgb(255, 235, 60)) // Bright celebratory yellow!
                                .add_modifier(Modifier::BOLD);
                            spans.push(Span::styled(full_text, pulse_style));
                        } else {
                            // Phase 2+3: Sparkle substitution from right to left
                            // Build the full display text to match what render_chars was created from
                            let full_text = format!("{}[{}] {}", prefix, task_id_short, display_title);
                            let full_chars: Vec<char> = full_text.chars().collect();
                            let sparkle_count = celebration.sparkle_chars_count();
                            let text_len = full_chars.len();

                            // Golden glow for text as sparkles approach
                            let glow_gold = Color::Rgb(255, 220, 120);

                            for (i, &ch) in full_chars.iter().enumerate() {
                                let pos_from_right = text_len.saturating_sub(i + 1);

                                if pos_from_right < sparkle_count {
                                    // This character has been replaced by a sparkle
                                    let sparkle_age = sparkle_count - pos_from_right - 1;
                                    // Pick sparkle character and color based on age - more celebratory!
                                    let (sparkle_char, sparkle_color) = match sparkle_age {
                                        0 => ('★', star_yellow),        // Fresh star - brightest yellow!
                                        1 => ('✦', bright_sparkle),     // Bright sparkle - gold
                                        2 => ('✧', warm_yellow),        // Fading sparkle - warm yellow
                                        3 => ('·', soft_amber),         // Tiny dot - soft amber
                                        _ => (' ', Color::Reset),       // Evaporated
                                    };
                                    spans.push(Span::styled(
                                        sparkle_char.to_string(),
                                        Style::default().fg(sparkle_color),
                                    ));
                                } else {
                                    // Original character - glow golden as sparkles approach
                                    let distance_to_sparkle = pos_from_right - sparkle_count;
                                    let char_style = if distance_to_sparkle <= 3 {
                                        // Close to being sparkled - golden glow anticipation
                                        Style::default().fg(glow_gold)
                                    } else if distance_to_sparkle <= 6 {
                                        // Further out - subtle warm tint
                                        Style::default().fg(Color::Rgb(255, 245, 200))
                                    } else {
                                        // Normal white
                                        Style::default().fg(Color::White)
                                    };
                                    spans.push(Span::styled(ch.to_string(), char_style));
                                }
                            }
                        }
                    } else {
                        // Normal rendering
                        if !prefix.is_empty() {
                            spans.push(Span::styled(prefix.clone(), title_style));
                        }
                        spans.push(Span::styled("[", bracket_style));
                        spans.push(Span::styled(task_id_short.to_string(), code_style));
                        spans.push(Span::styled("] ", bracket_style));
                        spans.push(Span::styled(display_title.clone(), title_style));
                        if !task.images.is_empty() {
                            spans.push(Span::styled(" [img]", bracket_style));
                        }

                        // Show sync status indicator for tasks with worktrees, right-aligned
                        if task.worktree_path.is_some() {
                            let (indicator_text, indicator_style) = if task.git_commits_behind > 0 {
                                // Behind main - show how many commits behind
                                let style = if is_task_selected {
                                    Style::default().fg(contrast_fg).bg(color)
                                } else {
                                    Style::default().fg(Color::DarkGray)
                                };
                                (format!("↓{}", task.git_commits_behind), style)
                            } else {
                                // Synced with main - show neutral indicator (ready to apply)
                                let style = if is_task_selected {
                                    Style::default().fg(contrast_fg).bg(color)
                                } else {
                                    Style::default().fg(Color::DarkGray)
                                };
                                ("=".to_string(), style)
                            };
                            let indicator_len = indicator_text.chars().count();

                            // Calculate current content width to determine padding needed
                            let prefix_len = prefix.chars().count();
                            let img_len = if !task.images.is_empty() { 6 } else { 0 }; // " [img]"
                            let current_width = prefix_len + id_prefix_len + display_title.chars().count() + img_len;
                            let available_width = inner.width as usize;

                            // Add padding to push indicator to the right (with 1 space before it)
                            let padding_needed = available_width.saturating_sub(current_width + indicator_len + 1);
                            if padding_needed > 0 {
                                spans.push(Span::styled(" ".repeat(padding_needed), title_style));
                            }
                            spans.push(Span::styled(indicator_text, indicator_style));
                        }
                    }

                    ListItem::new(Line::from(spans))
                })
                .collect()
        })
        .unwrap_or_default();

    // Check if there's an active merge celebration animation for this column
    // that should be rendered as a "phantom" entry (task has moved to Done but animation plays)
    let mut tasks = tasks;
    if let Some(ref celebration) = app.model.ui_state.merge_celebration {
        if celebration.column_status == status {
            // Check if this celebration task is NOT in the current task list
            // (meaning it was moved away and we need to insert a phantom)
            let task_present = app.model.active_project()
                .map(|p| p.tasks_by_status(status).iter().any(|t| t.id == celebration.task_id))
                .unwrap_or(false);

            if !task_present {
                // Render the phantom celebration entry
                let phase = celebration.phase();
                let frame = celebration.frame;

                // Vibrant yellow/gold palette with sparkle shimmer effect
                let shimmer = ((frame % 4) as i16 - 1) * 20; // -20, 0, +20, +40 brightness pulse
                // Bright celebratory yellow - THE STAR (★)
                let star_yellow = Color::Rgb(
                    255,
                    (230_i16 + shimmer).clamp(200, 255) as u8,
                    (50_i16 + shimmer / 2).clamp(20, 100) as u8,
                );
                // Bright gold sparkle (✦)
                let bright_sparkle = Color::Rgb(255, 215, 60);
                // Warm yellow for fading sparkle (✧)
                let warm_yellow = Color::Rgb(255, 200, 100);
                // Soft amber for dots
                let soft_amber = Color::Rgb(220, 180, 80);

                let mut spans = Vec::new();

                if phase == 1 {
                    // Phase 1: Confirmation pulse in bright celebratory yellow
                    let pulse_style = Style::default()
                        .fg(Color::Rgb(255, 235, 60)) // Bright celebratory yellow!
                        .add_modifier(Modifier::BOLD);
                    spans.push(Span::styled(celebration.original_text.clone(), pulse_style));
                } else {
                    // Phase 2+3: Sparkle substitution
                    let full_chars: Vec<char> = celebration.original_text.chars().collect();
                    let sparkle_count = celebration.sparkle_chars_count();
                    let text_len = full_chars.len();
                    // Golden glow for text as sparkles approach
                    let glow_gold = Color::Rgb(255, 220, 120);

                    for (i, &ch) in full_chars.iter().enumerate() {
                        let pos_from_right = text_len.saturating_sub(i + 1);

                        if pos_from_right < sparkle_count {
                            let sparkle_age = sparkle_count - pos_from_right - 1;
                            // Pick sparkle character and color based on age - more celebratory!
                            let (sparkle_char, sparkle_color) = match sparkle_age {
                                0 => ('★', star_yellow),        // Fresh star - brightest yellow!
                                1 => ('✦', bright_sparkle),     // Bright sparkle - gold
                                2 => ('✧', warm_yellow),        // Fading sparkle - warm yellow
                                3 => ('·', soft_amber),         // Tiny dot - soft amber
                                _ => (' ', Color::Reset),       // Evaporated
                            };
                            spans.push(Span::styled(
                                sparkle_char.to_string(),
                                Style::default().fg(sparkle_color),
                            ));
                        } else {
                            // Original character - glow golden as sparkles approach
                            let distance_to_sparkle = pos_from_right - sparkle_count;
                            let char_style = if distance_to_sparkle <= 3 {
                                // Close to being sparkled - golden glow anticipation
                                Style::default().fg(glow_gold)
                            } else if distance_to_sparkle <= 6 {
                                // Further out - subtle warm tint
                                Style::default().fg(Color::Rgb(255, 245, 200))
                            } else {
                                Style::default().fg(Color::White)
                            };
                            spans.push(Span::styled(ch.to_string(), char_style));
                        }
                    }
                }

                let phantom_item = ListItem::new(Line::from(spans));

                // Insert at the original index (or append if index is beyond current length)
                let insert_idx = celebration.task_index.min(tasks.len());
                tasks.insert(insert_idx, phantom_item);
            }
        }
    }

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

        // Calculate visual index
        let visual_idx = if is_selected {
            app.model.ui_state.selected_task_idx
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
        // Check if selected task is actually in Accepting state (for merge feedback)
        let selected_task = app.model.ui_state.selected_task_idx.and_then(|idx| {
            app.model.active_project().and_then(|p| {
                p.tasks_by_status(status).get(idx).cloned()
            })
        });

        let hints = if let Some(ref task) = selected_task {
            if task.status == TaskStatus::Accepting {
                get_accepting_hints(task)
            } else if status == TaskStatus::Review {
                // Get context for review hints
                let project = app.model.active_project();
                let has_applied = project.and_then(|p| p.applied_task_id).is_some();
                let is_this_task_applied = project
                    .and_then(|p| p.applied_task_id)
                    .map(|id| id == task.id)
                    .unwrap_or(false);
                let is_blocked = project
                    .and_then(|p| p.applied_task_id)
                    .map(|id| id != task.id)
                    .unwrap_or(false)
                    || project
                        .and_then(|p| p.main_worktree_lock.as_ref())
                        .map(|lock| lock.task_id != task.id)
                        .unwrap_or(false);
                get_review_hints(has_applied, is_this_task_applied, is_blocked)
            } else {
                get_column_hints(status)
            }
        } else {
            get_column_hints(status)
        };

        // Try to fit hints in available width, using progressively shorter versions
        let available_width = area.width.saturating_sub(2); // Leave space for corners
        let animation_frame = app.model.ui_state.animation_frame;
        let final_hints = fit_hints_to_width(hints, available_width, status, animation_frame);

        if !final_hints.is_empty() {
            let hints_text: String = final_hints.iter().map(|s| s.content.as_ref()).collect();
            let hints_width = hints_text.chars().count() as u16;

            if hints_width > 0 && area.width > hints_width + 2 {
                let hints_area = Rect {
                    x: area.x + area.width - hints_width - 2,
                    y: area.y + area.height - 1,
                    width: hints_width,
                    height: 1,
                };
                let hints_widget = Paragraph::new(Line::from(final_hints));
                frame.render_widget(hints_widget, hints_area);
            }
        }
    }

    // Show count badge for Review and NeedsWork columns
    let badge_count = match status {
        TaskStatus::Review => app.model.active_project().map(|p| p.review_count()),
        TaskStatus::NeedsWork => app.model.active_project().map(|p| p.needs_work_count()),
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

    // Calculate total items
    let (total_items, scroll_offset) = if let Some(project) = app.model.active_project() {
        let tasks = project.tasks_by_status(status);
        let total = tasks.len();

        // Calculate scroll offset based on selected item
        let offset = if is_selected && app.model.ui_state.selected_column == status {
            if let Some(task_idx) = app.model.ui_state.selected_task_idx {
                // Estimate scroll position - the list widget centers selected item
                task_idx.saturating_sub(visible_height / 2)
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
            Span::styled("s", key_style),
            Span::styled("tart ", desc_style),
            Span::styled("e", key_style),
            Span::styled("dit ", desc_style),
            Span::styled("d", key_style),
            Span::styled("el", desc_style),
        ],
        TaskStatus::InProgress => vec![
            Span::styled("o", key_style),
            Span::styled("pen ", desc_style),
            Span::styled("r", key_style),
            Span::styled("eview ", desc_style),
            Span::styled("x", key_style),
            Span::styled("-reset", desc_style),
        ],
        TaskStatus::Testing => vec![
            // Empty for now - no tasks will be in Testing state yet
        ],
        TaskStatus::NeedsWork => vec![
            Span::styled("o", key_style),
            Span::styled("pen ", desc_style),
            Span::styled("r", key_style),
            Span::styled("eview ", desc_style),
            Span::styled("x", key_style),
            Span::styled("-reset", desc_style),
        ],
        TaskStatus::Review => vec![
            Span::styled("a", key_style),
            Span::styled("pply ", desc_style),
            Span::styled("u", key_style),
            Span::styled("napply ", desc_style),
            Span::styled("r", key_style),
            Span::styled("ebase ", desc_style),
            Span::styled("m", key_style),
            Span::styled("erge ", desc_style),
            Span::styled("d", key_style),
            Span::styled("iscard ", desc_style),
            Span::styled("c", key_style),
            Span::styled("heck ", desc_style),
            Span::styled("f", key_style),
            Span::styled("eedback ", desc_style),
            Span::styled("n", key_style),
            Span::styled("eeds-work ", desc_style),
            Span::styled("o", key_style),
            Span::styled("pen ", desc_style),
            Span::styled("x", key_style),
            Span::styled("-reset", desc_style),
        ],
        TaskStatus::Accepting => vec![
            // This case is handled by get_accepting_hints when a task is selected
            // Fallback text if no task is selected
            Span::styled("merging...", desc_style),
        ],
        TaskStatus::Updating => vec![
            // Shows when task is updating (rebasing worktree)
            Span::styled("updating...", desc_style),
        ],
        TaskStatus::Applying => vec![
            // Shows when task is being applied to main worktree
            Span::styled("applying...", desc_style),
        ],
        TaskStatus::Done => vec![
            Span::styled("e", key_style),
            Span::styled("dit ", desc_style),
            Span::styled("d", key_style),
            Span::styled("el ", desc_style),
            Span::styled("r", key_style),
            Span::styled("eview ", desc_style),
            Span::styled("x", key_style),
            Span::styled("-reset", desc_style),
        ],
    }
}

/// Get context-aware hints for the Review column
/// - has_applied: true if any task has changes applied to main
/// - is_this_task_applied: true if the selected task is the one with applied changes
/// - is_blocked: true if this task can't be merged (another task has lock/applied)
fn get_review_hints(has_applied: bool, is_this_task_applied: bool, is_blocked: bool) -> Vec<Span<'static>> {
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::DarkGray);

    let mut hints = Vec::new();

    // Show apply only if nothing is applied
    if !has_applied {
        hints.push(Span::styled("a", key_style));
        hints.push(Span::styled("pply ", desc_style));
    }

    // Show unapply only if this task is applied
    if is_this_task_applied {
        hints.push(Span::styled("u", key_style));
        hints.push(Span::styled("napply ", desc_style));
    }

    // Always show rebase
    hints.push(Span::styled("r", key_style));
    hints.push(Span::styled("ebase ", desc_style));

    // Show merge only if not blocked
    if !is_blocked {
        hints.push(Span::styled("m", key_style));
        hints.push(Span::styled("erge ", desc_style));
    }

    // Always show discard, check, feedback, needs-work, open, reset
    hints.push(Span::styled("d", key_style));
    hints.push(Span::styled("iscard ", desc_style));
    hints.push(Span::styled("c", key_style));
    hints.push(Span::styled("heck ", desc_style));
    hints.push(Span::styled("f", key_style));
    hints.push(Span::styled("eedback ", desc_style));
    hints.push(Span::styled("n", key_style));
    hints.push(Span::styled("eeds-work ", desc_style));
    hints.push(Span::styled("o", key_style));
    hints.push(Span::styled("pen ", desc_style));
    hints.push(Span::styled("x", key_style));
    hints.push(Span::styled("-reset", desc_style));

    hints
}

/// Get hints for a task in Accepting state (merge/rebase in progress)
/// Shows elapsed time and last activity for better feedback
fn get_accepting_hints(task: &crate::model::Task) -> Vec<Span<'static>> {
    use chrono::Utc;

    let desc_style = Style::default().fg(Color::DarkGray);
    let activity_style = Style::default().fg(Color::Yellow);
    let warning_style = Style::default().fg(Color::Red);

    let mut parts = vec![Span::styled("rebasing", desc_style)];

    // Show last tool used if available
    if let Some(ref tool_name) = task.last_tool_name {
        parts.push(Span::styled(" (", desc_style));
        parts.push(Span::styled(tool_name.clone(), activity_style));
        parts.push(Span::styled(")", desc_style));
    }

    // Calculate and show elapsed time
    if let Some(started_at) = task.accepting_started_at {
        let elapsed = Utc::now().signed_duration_since(started_at);
        let secs = elapsed.num_seconds();

        // Check for staleness (no activity in 30+ seconds)
        let is_stalled = task.last_activity_at
            .map(|last| Utc::now().signed_duration_since(last).num_seconds() > 30)
            .unwrap_or(false);

        if is_stalled && task.last_tool_name.is_none() {
            parts.push(Span::styled(" (stalled?)", warning_style));
        }

        parts.push(Span::styled(format!(" {}s", secs), desc_style));
    } else {
        parts.push(Span::styled("...", desc_style));
    }

    parts
}

/// Fit hints to available width by trying progressively shorter versions
/// When space is limited, we use cycling hints that show one full hint at a time
fn fit_hints_to_width(hints: Vec<Span<'static>>, available_width: u16, status: TaskStatus, animation_frame: usize) -> Vec<Span<'static>> {
    let hints_text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    let hints_width = hints_text.chars().count() as u16;

    // If full hints fit, use them (no cycling needed)
    if hints_width <= available_width {
        return hints;
    }

    // Check if medium abbreviated version would fit
    let medium = get_medium_hints(status);
    let medium_text: String = medium.iter().map(|s| s.content.as_ref()).collect();
    let medium_width = medium_text.chars().count() as u16;

    // Check if short version would fit
    let short = get_short_hints(status);
    let short_text: String = short.iter().map(|s| s.content.as_ref()).collect();
    let short_width = short_text.chars().count() as u16;

    // If medium fits, use cycling full-in-medium (one full, others abbreviated)
    if medium_width <= available_width {
        return get_cycling_full_in_medium_hints(status, animation_frame);
    }

    // If short fits, use cycling full-in-short (one full, others just key)
    if short_width <= available_width {
        return get_cycling_full_in_short_hints(status, animation_frame);
    }

    // Nothing fits
    Vec::new()
}

/// Get medium-length hints (abbreviated descriptions)
fn get_medium_hints(status: TaskStatus) -> Vec<Span<'static>> {
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::DarkGray);

    match status {
        TaskStatus::Planned => vec![
            Span::styled("s", key_style),
            Span::styled("tart ", desc_style),
            Span::styled("e", key_style),
            Span::styled("dit ", desc_style),
            Span::styled("d", key_style),
            Span::styled("el", desc_style),
        ],
        TaskStatus::Testing => vec![],
        TaskStatus::InProgress | TaskStatus::NeedsWork => vec![
            Span::styled("o", key_style),
            Span::styled("pen ", desc_style),
            Span::styled("t", key_style),
            Span::styled("est ", desc_style),
            Span::styled("r", key_style),
            Span::styled("ev ", desc_style),
            Span::styled("x", key_style),
            Span::styled("-rst", desc_style),
        ],
        TaskStatus::Review => vec![
            Span::styled("a", key_style),
            Span::styled("pp ", desc_style),
            Span::styled("u", key_style),
            Span::styled("nap ", desc_style),
            Span::styled("r", key_style),
            Span::styled("eb ", desc_style),
            Span::styled("m", key_style),
            Span::styled("rg ", desc_style),
            Span::styled("d", key_style),
            Span::styled("is ", desc_style),
            Span::styled("c", key_style),
            Span::styled("hk ", desc_style),
            Span::styled("f", key_style),
            Span::styled("b ", desc_style),
            Span::styled("n", key_style),
            Span::styled("w ", desc_style),
            Span::styled("o", key_style),
            Span::styled("pen ", desc_style),
            Span::styled("x", key_style),
            Span::styled("-rst", desc_style),
        ],
        TaskStatus::Accepting | TaskStatus::Updating | TaskStatus::Applying => vec![
            Span::styled("...", desc_style),
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

/// Get cycling full-in-medium hints: one hint shows full form, others show abbreviated
/// animation_frame is used to determine which hint to expand (cycles every ~1.5s)
fn get_cycling_full_in_medium_hints(status: TaskStatus, animation_frame: usize) -> Vec<Span<'static>> {
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::DarkGray);

    // Define hint tuples (key, abbreviated, full) for each status
    let hint_tuples: Vec<(&str, &str, &str)> = match status {
        TaskStatus::Planned => vec![
            ("s", "tart", "tart"),
            ("e", "dit", "dit"),
            ("d", "el", "elete"),
        ],
        TaskStatus::Testing => vec![],
        TaskStatus::InProgress | TaskStatus::NeedsWork => vec![
            ("o", "pen", "pen"),
            ("t", "est", "est"),
            ("r", "ev", "eview"),
            ("x", "-rst", "-reset"),
        ],
        TaskStatus::Review => vec![
            ("a", "pp", "pply"),
            ("u", "nap", "napply"),
            ("r", "eb", "ebase"),
            ("m", "rg", "erge"),
            ("d", "is", "iscard"),
            ("c", "hk", "heck"),
            ("f", "b", "eedback"),
            ("n", "w", "eeds-work"),
            ("o", "pen", "pen"),
            ("x", "-rst", "-reset"),
        ],
        TaskStatus::Accepting | TaskStatus::Updating | TaskStatus::Applying => {
            return vec![Span::styled("...", desc_style)];
        }
        TaskStatus::Done => vec![
            ("e", "dit", "dit"),
            ("d", "el", "elete"),
            ("r", "ev", "eview"),
        ],
    };

    if hint_tuples.is_empty() {
        return vec![];
    }

    let num_hints = hint_tuples.len();
    // Cycle every ~1.5 seconds (15 ticks at 100ms each)
    let expanded_idx = (animation_frame / 15) % num_hints;

    let mut hints = Vec::new();
    for (i, (key, abbrev, full)) in hint_tuples.iter().enumerate() {
        if i > 0 {
            hints.push(Span::styled(" ", desc_style));
        }
        hints.push(Span::styled(*key, key_style));
        if i == expanded_idx {
            // This is the expanded hint - show full description
            hints.push(Span::styled(*full, desc_style));
        } else {
            // Show abbreviated description
            hints.push(Span::styled(*abbrev, desc_style));
        }
    }

    hints
}

/// Get cycling full-in-short hints: one hint shows full form, others show just the key
/// animation_frame is used to determine which hint to expand (cycles every ~1.5s)
fn get_cycling_full_in_short_hints(status: TaskStatus, animation_frame: usize) -> Vec<Span<'static>> {
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::DarkGray);

    // Define hint pairs (key, description) for each status
    let hint_pairs: Vec<(&str, &str)> = match status {
        TaskStatus::Planned => vec![
            ("s", "tart"),
            ("e", "dit"),
            ("d", "elete"),
        ],
        TaskStatus::Testing => vec![],
        TaskStatus::InProgress | TaskStatus::NeedsWork => vec![
            ("o", "pen"),
            ("t", "est"),
            ("r", "eview"),
            ("x", "-reset"),
        ],
        TaskStatus::Review => vec![
            ("a", "pply"),
            ("u", "napply"),
            ("r", "ebase"),
            ("m", "erge"),
            ("d", "iscard"),
            ("c", "heck"),
            ("f", "eedback"),
            ("n", "eeds-work"),
            ("o", "pen"),
            ("x", "-reset"),
        ],
        TaskStatus::Accepting | TaskStatus::Updating | TaskStatus::Applying => {
            return vec![Span::styled("...", desc_style)];
        }
        TaskStatus::Done => vec![
            ("e", "dit"),
            ("d", "elete"),
            ("r", "eview"),
        ],
    };

    if hint_pairs.is_empty() {
        return vec![];
    }

    let num_hints = hint_pairs.len();
    // Cycle every ~1.5 seconds (15 ticks at 100ms each)
    let expanded_idx = (animation_frame / 15) % num_hints;

    let mut hints = Vec::new();
    for (i, (key, desc)) in hint_pairs.iter().enumerate() {
        if i > 0 {
            hints.push(Span::styled(" ", desc_style));
        }
        hints.push(Span::styled(*key, key_style));
        if i == expanded_idx {
            // This is the expanded hint - show full description
            hints.push(Span::styled(*desc, desc_style));
        }
    }

    hints
}

/// Get short hints (just the key letters)
fn get_short_hints(status: TaskStatus) -> Vec<Span<'static>> {
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::DarkGray);
    let sep = Span::styled(" ", desc_style);

    match status {
        TaskStatus::Planned => vec![
            Span::styled("s", key_style), sep.clone(),
            Span::styled("e", key_style), sep.clone(),
            Span::styled("d", key_style),
        ],
        TaskStatus::Testing => vec![],
        TaskStatus::InProgress | TaskStatus::NeedsWork => vec![
            Span::styled("o", key_style), sep.clone(),
            Span::styled("t", key_style), sep.clone(),
            Span::styled("r", key_style), sep.clone(),
            Span::styled("x", key_style),
        ],
        TaskStatus::Review => vec![
            Span::styled("a", key_style), sep.clone(),
            Span::styled("u", key_style), sep.clone(),
            Span::styled("r", key_style), sep.clone(),
            Span::styled("m", key_style), sep.clone(),
            Span::styled("d", key_style), sep.clone(),
            Span::styled("c", key_style), sep.clone(),
            Span::styled("f", key_style), sep.clone(),
            Span::styled("n", key_style), sep.clone(),
            Span::styled("o", key_style), sep.clone(),
            Span::styled("x", key_style),
        ],
        TaskStatus::Accepting | TaskStatus::Updating | TaskStatus::Applying => vec![
            Span::styled("...", desc_style),
        ],
        TaskStatus::Done => vec![
            Span::styled("e", key_style), sep.clone(),
            Span::styled("d", key_style), sep.clone(),
            Span::styled("r", key_style),
        ],
    }
}
