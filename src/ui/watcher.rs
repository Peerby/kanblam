//! Watcher mascot balloon - displays periodic observations from the watcher Claude session
//!
//! The inline balloon rendering is in mod.rs (render_watcher_balloon_inline).
//! This module contains the insight modal for showing full watcher observations.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

use crate::model::WatcherCommentDisplay;

/// KanBlam green (matching logo.rs)
const KANBLAM_GREEN: Color = Color::Rgb(80, 200, 120);

/// Render the watcher insight modal
/// Shows full insight: remark (title), description, and task instructions
/// Hints at bottom: p(lan), ^s(tart), esc dismiss
/// Returns the total content line count for scroll bounds
pub fn render_watcher_insight_modal(
    frame: &mut Frame,
    area: Rect,
    comment: &WatcherCommentDisplay,
    scroll_offset: usize,
) -> usize {
    use ratatui::widgets::{Block, Borders, Clear, Wrap, Scrollbar, ScrollbarOrientation, ScrollbarState, Paragraph};
    use ratatui::layout::Alignment;

    // Get insight data - if not available, show simple message
    let insight = match &comment.insight {
        Some(i) => i,
        None => {
            // No insight data - show simple modal
            let block = Block::default()
                .title(" Watcher Comment ")
                .title_style(Style::default().fg(KANBLAM_GREEN).add_modifier(Modifier::BOLD))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan));

            let content = Paragraph::new(comment.comment.clone())
                .block(block)
                .wrap(Wrap { trim: true });

            // Center the modal
            let modal_width = area.width.min(60);
            let modal_height = area.height.min(10);
            let x = area.x + (area.width.saturating_sub(modal_width)) / 2;
            let y = area.y + (area.height.saturating_sub(modal_height)) / 2;
            let modal_area = Rect { x, y, width: modal_width, height: modal_height };

            frame.render_widget(Clear, modal_area);
            frame.render_widget(content, modal_area);
            return 0;
        }
    };

    // Calculate content: description lines + divider + task header + task lines
    let modal_width = area.width.min(70).max(50);
    let content_inner_width = modal_width.saturating_sub(4) as usize; // 2 border + 2 padding

    // Word-wrap description and task to get actual line count
    let desc_lines: Vec<String> = wrap_text_simple(&insight.description, content_inner_width);
    let task_lines: Vec<String> = wrap_text_simple(&insight.task, content_inner_width);

    // Total content: description + 1 blank + "Task:" header + task lines
    let total_content_lines = desc_lines.len() + 2 + task_lines.len();

    // Calculate modal height based on content, with min/max bounds
    // Add 4 for border (2) + padding (2)
    let ideal_height = total_content_lines + 4;
    let modal_height = (ideal_height as u16).min(area.height.saturating_sub(4)).max(8);

    let x = area.x + (area.width.saturating_sub(modal_width)) / 2;
    let y = area.y + (area.height.saturating_sub(modal_height)) / 2;
    let modal_area = Rect { x, y, width: modal_width, height: modal_height };

    // Clear the area behind the modal
    frame.render_widget(Clear, modal_area);

    // Build the title (remark)
    let title = format!(" {} ", insight.remark);

    // Build the bottom hints
    let hints = " j/k scroll  p(lan) ^s(tart) esc  ^w toggle ";

    // Create the block with title
    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(KANBLAM_GREEN).add_modifier(Modifier::BOLD))
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    // Render the block first to get inner area
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    // Render hints in bottom border
    let hints_x = modal_area.x + modal_area.width.saturating_sub(hints.len() as u16 + 1);
    let hints_y = modal_area.y + modal_area.height - 1;
    if hints_x > modal_area.x {
        let hints_area = Rect { x: hints_x, y: hints_y, width: hints.len() as u16, height: 1 };
        let hints_widget = Paragraph::new(hints)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(hints_widget, hints_area);
    }

    // Add 1 char padding on all sides for content area
    let content_area = Rect {
        x: inner.x + 1,
        y: inner.y + 1,
        width: inner.width.saturating_sub(3), // 1 left padding + 1 right padding + 1 scrollbar
        height: inner.height.saturating_sub(2), // 1 top padding + 1 bottom padding
    };

    // Build all content lines
    let mut all_lines: Vec<Line> = Vec::new();

    // Description lines (white)
    for line in &desc_lines {
        all_lines.push(Line::from(Span::styled(line.clone(), Style::default().fg(Color::White))));
    }

    // Blank line separator
    all_lines.push(Line::from(""));

    // Task header (yellow, bold)
    all_lines.push(Line::from(Span::styled("Task:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))));

    // Task lines (gray)
    for line in &task_lines {
        all_lines.push(Line::from(Span::styled(line.clone(), Style::default().fg(Color::Gray))));
    }

    // Clamp scroll offset
    let visible_height = content_area.height as usize;
    let max_scroll = total_content_lines.saturating_sub(visible_height);
    let clamped_scroll = scroll_offset.min(max_scroll);

    // Render content with scroll
    let visible_lines: Vec<Line> = all_lines
        .into_iter()
        .skip(clamped_scroll)
        .take(visible_height)
        .collect();

    let content_widget = Paragraph::new(visible_lines);
    frame.render_widget(content_widget, content_area);

    // Render scrollbar if content is scrollable
    if total_content_lines > visible_height {
        let scrollbar_area = Rect {
            x: inner.x + inner.width - 1,
            y: inner.y + 1, // Match content padding
            width: 1,
            height: inner.height.saturating_sub(2), // Match content height
        };

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("█")
            .track_style(Style::default().fg(Color::DarkGray))
            .thumb_style(Style::default().fg(Color::Cyan));

        let mut scrollbar_state = ScrollbarState::new(total_content_lines)
            .position(clamped_scroll)
            .viewport_content_length(visible_height);

        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }

    total_content_lines
}

/// Simple word-wrap helper that respects word boundaries
fn wrap_text_simple(text: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }

        let words: Vec<&str> = paragraph.split_whitespace().collect();
        let mut current_line = String::new();

        for word in words {
            if current_line.is_empty() {
                if word.len() > max_width {
                    // Word is longer than line - split it
                    let mut remaining = word;
                    while remaining.len() > max_width {
                        lines.push(remaining[..max_width].to_string());
                        remaining = &remaining[max_width..];
                    }
                    current_line = remaining.to_string();
                } else {
                    current_line = word.to_string();
                }
            } else if current_line.len() + 1 + word.len() <= max_width {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                lines.push(current_line);
                current_line = word.to_string();
            }
        }

        if !current_line.is_empty() {
            lines.push(current_line);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}
