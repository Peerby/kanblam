//! Welcome panel for first-time users and empty state
//!
//! Replaces the empty kanban board when no projects are loaded,
//! featuring the mascot as a guide with speech bubbles and quick start hints.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::ui::logo::{EyeAnimation, STAR_EYE_FRAMES};

/// KanBlam green color (matches logo)
const KANBLAM_GREEN: Color = Color::Rgb(80, 200, 120);

/// Mascot color palette
const MASCOT_YELLOW: Color = Color::Rgb(230, 190, 40);
const MASCOT_ORANGE: Color = Color::Rgb(235, 130, 20);
const MASCOT_RED: Color = Color::Rgb(200, 80, 70);
const MASCOT_MAGENTA: Color = Color::Rgb(180, 70, 120);

/// Welcome messages that rotate in the speech bubble
const WELCOME_MESSAGES: &[(&str, &[&str])] = &[
    (
        "Welcome to KanBlam!",
        &[
            "Orchestrate parallel Claude",
            "sessions with kanban flow.",
        ],
    ),
    (
        "Isolated Worktrees",
        &[
            "Each task gets its own git",
            "branch - no conflicts!",
        ],
    ),
    (
        "Parallel Power",
        &[
            "Run multiple Claude sessions",
            "at once for faster iteration.",
        ],
    ),
    (
        "Ready to start?",
        &[
            "Press ! to open a project",
            "and start vibe coding.",
        ],
    ),
];

/// Get the total number of welcome messages
pub fn welcome_message_count() -> usize {
    WELCOME_MESSAGES.len()
}

/// Render the welcome panel when no projects are loaded
pub fn render_welcome_panel(
    frame: &mut Frame,
    area: Rect,
    eye_animation: EyeAnimation,
    animation_frame: usize,
    welcome_message_idx: usize,
    bubble_focused: bool,
    project_dialog_open: bool,
) {
    // Choose layout based on available space
    if area.width >= 70 && area.height >= 20 {
        render_full_welcome(frame, area, eye_animation, animation_frame, welcome_message_idx, bubble_focused, project_dialog_open);
    } else if area.width >= 50 && area.height >= 15 {
        render_medium_welcome(frame, area, eye_animation, animation_frame, welcome_message_idx, bubble_focused, project_dialog_open);
    } else {
        render_compact_welcome(frame, area, eye_animation, animation_frame);
    }
}

/// Full welcome layout with mascot, speech bubble, and quick start guide
fn render_full_welcome(
    frame: &mut Frame,
    area: Rect,
    eye_animation: EyeAnimation,
    animation_frame: usize,
    message_idx: usize,
    bubble_focused: bool,
    project_dialog_open: bool,
) {
    // Create a block for the welcome area (replaces kanban board)
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Render the CTA hint at top-left, pointing up at the +project button
    // Hide when project dialog is open
    if !project_dialog_open {
        render_cta_hint(frame, inner);
    }

    // Calculate total content height: mascot(6) + spacing(2) + quickstart(11) = 19
    let content_height = 6 + 2 + 11;
    let available_height = inner.height.saturating_sub(4); // Subtract CTA height
    let top_padding = available_height.saturating_sub(content_height) / 2;

    // Vertical layout: CTA space, top padding to center, mascot+bubble, spacing, quick start, bottom padding
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),            // Space for CTA hint at top
            Constraint::Length(top_padding),  // Top padding to center content
            Constraint::Length(6),            // Mascot + speech bubble
            Constraint::Length(2),            // Spacing
            Constraint::Length(11),           // Quick start guide (7 steps)
            Constraint::Min(1),               // Bottom padding
        ])
        .split(inner);

    // Render mascot with speech bubble (centered horizontally)
    render_mascot_with_bubble(frame, chunks[2], eye_animation, animation_frame, message_idx, bubble_focused);

    // Render quick start guide (centered horizontally)
    render_quick_start(frame, chunks[4]);
}

/// Medium welcome layout - more compact
fn render_medium_welcome(
    frame: &mut Frame,
    area: Rect,
    eye_animation: EyeAnimation,
    animation_frame: usize,
    message_idx: usize,
    bubble_focused: bool,
    project_dialog_open: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Render the CTA hint at top-left
    // Hide when project dialog is open
    if !project_dialog_open {
        render_cta_hint(frame, inner);
    }

    // Calculate total content height: mascot(5) + spacing(1) + quickstart(4) = 10
    let content_height = 5 + 1 + 4;
    let available_height = inner.height.saturating_sub(3); // Subtract CTA height
    let top_padding = available_height.saturating_sub(content_height) / 2;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),            // Space for CTA hint
            Constraint::Length(top_padding),  // Top padding to center content
            Constraint::Length(5),            // Mascot + text
            Constraint::Length(1),            // Spacing
            Constraint::Length(4),            // Compact quick start
            Constraint::Min(1),               // Bottom padding
        ])
        .split(inner);

    render_mascot_inline(frame, chunks[2], eye_animation, animation_frame, message_idx, bubble_focused);
    render_quick_start_compact(frame, chunks[4]);
}

/// Compact welcome layout - minimal
fn render_compact_welcome(
    frame: &mut Frame,
    area: Rect,
    eye_animation: EyeAnimation,
    animation_frame: usize,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(4),   // Mascot
            Constraint::Length(1),
            Constraint::Length(3),   // Message
            Constraint::Length(1),
            Constraint::Length(2),   // Shortcuts
            Constraint::Min(1),
        ])
        .split(inner);

    render_mascot_small(frame, chunks[1], eye_animation, animation_frame);

    // Simple welcome message
    let msg = Paragraph::new(vec![
        Line::from(Span::styled("Welcome to KanBlam!", Style::default().fg(KANBLAM_GREEN).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("Press ! to get started", Style::default().fg(Color::Yellow))),
    ])
    .alignment(Alignment::Center);
    frame.render_widget(msg, chunks[3]);

    // Minimal shortcuts
    let shortcuts = Paragraph::new(Line::from(vec![
        Span::styled("[!]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled(" Open  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[?]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled(" Help", Style::default().fg(Color::DarkGray)),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(shortcuts, chunks[5]);
}

/// Render the mascot with a speech bubble pointing to it
fn render_mascot_with_bubble(
    frame: &mut Frame,
    area: Rect,
    eye_animation: EyeAnimation,
    animation_frame: usize,
    message_idx: usize,
    bubble_focused: bool,
) {
    // Center the mascot + bubble horizontally
    // Mascot is ~10 chars, gap ~2, bubble ~32 chars = ~44 total
    let total_width = 46u16;
    let start_x = area.x + area.width.saturating_sub(total_width) / 2;

    // Mascot area (left side)
    let mascot_area = Rect {
        x: start_x,
        y: area.y,
        width: 12,
        height: 4,
    };

    // Speech bubble area (right side)
    let bubble_area = Rect {
        x: start_x + 12,
        y: area.y,
        width: 34,
        height: 6,
    };

    render_mascot_large(frame, mascot_area, eye_animation, animation_frame);
    render_speech_bubble(frame, bubble_area, message_idx, bubble_focused);
}

/// Render a larger mascot for the welcome screen
fn render_mascot_large(
    frame: &mut Frame,
    area: Rect,
    eye_animation: EyeAnimation,
    animation_frame: usize,
) {
    let (left_eye, right_eye) = if eye_animation == EyeAnimation::StarEyes {
        let frame_idx = animation_frame % STAR_EYE_FRAMES.len();
        (STAR_EYE_FRAMES[frame_idx], STAR_EYE_FRAMES[frame_idx])
    } else {
        eye_animation.eye_chars()
    };

    let eye_style = Style::default().fg(KANBLAM_GREEN);

    let lines = vec![
        Line::from(Span::styled("  ▄▓▓▓▓▄  ", Style::default().fg(MASCOT_YELLOW))),
        Line::from(vec![
            Span::styled("  ▓", Style::default().fg(MASCOT_ORANGE)),
            Span::styled(left_eye, eye_style),
            Span::styled("▀▀", Style::default().fg(MASCOT_ORANGE)),
            Span::styled(right_eye, eye_style),
            Span::styled("▓▒▒", Style::default().fg(MASCOT_ORANGE)),
        ]),
        Line::from(Span::styled("  ▓▓▓▓▓▓▒ ", Style::default().fg(MASCOT_RED))),
        Line::from(Span::styled("   ▀▀ ▀▀  ", Style::default().fg(MASCOT_MAGENTA))),
    ];

    let mascot = Paragraph::new(lines);
    frame.render_widget(mascot, area);
}

/// Render the speech bubble with rotating messages
fn render_speech_bubble(frame: &mut Frame, area: Rect, message_idx: usize, is_focused: bool) {
    let (title, body) = WELCOME_MESSAGES[message_idx % WELCOME_MESSAGES.len()];
    let total_messages = WELCOME_MESSAGES.len();

    // Border color: cyan normally, white when focused
    let border_color = if is_focused { Color::White } else { Color::Cyan };
    let border_style = Style::default().fg(border_color);

    // Format hint number: "1/4" at top right
    let hint_num = format!("{}/{}", message_idx + 1, total_messages);

    // Top border with hint number at right
    // Box inner width is 29 chars, plus 2 for padding = 31 chars between borders
    // "╭" + dashes + hint + dashes + "╮"
    let dashes_before_hint = 31 - hint_num.len() - 1; // -1 for spacing
    let top_border = format!(
        " ╭{}{}─╮",
        "─".repeat(dashes_before_hint),
        hint_num
    );

    let mut lines = vec![
        Line::from(Span::styled(top_border, border_style)),
        Line::from(vec![
            Span::styled(" │ ", border_style),
            Span::styled(
                format!("{:<29}", title),
                Style::default().fg(KANBLAM_GREEN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" │", border_style),
        ]),
        Line::from(vec![
            Span::styled(" │ ", border_style),
            Span::styled(format!("{:<29}", ""), Style::default()),
            Span::styled(" │", border_style),
        ]),
    ];

    // Add body lines
    for line in body {
        lines.push(Line::from(vec![
            Span::styled(" │ ", border_style),
            Span::styled(format!("{:<29}", line), Style::default().fg(Color::White)),
            Span::styled(" │", border_style),
        ]));
    }

    // Pad if needed to maintain consistent height
    while lines.len() < 5 {
        lines.push(Line::from(vec![
            Span::styled(" │ ", border_style),
            Span::styled(format!("{:<29}", ""), Style::default()),
            Span::styled(" │", border_style),
        ]));
    }

    // Bottom border with navigation arrows on the right when focused
    if is_focused {
        let arrow_style = Style::default().fg(Color::White);
        // " ╰───────────────────────────← →─╯"
        lines.push(Line::from(vec![
            Span::styled(" ╰───────────────────────────", border_style),
            Span::styled("←", arrow_style),
            Span::styled(" ", border_style),
            Span::styled("→", arrow_style),
            Span::styled("─╯", border_style),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            " ╰───────────────────────────────╯",
            border_style,
        )));
    }

    // Add pointer on the left side (connects to mascot)
    // Use simple < instead of ◄
    // Modify line index 1 (title line) to add pointer
    if lines.len() > 1 {
        lines[1] = Line::from(vec![
            Span::styled("<  ", border_style),
            Span::styled(
                format!("{:<29}", title),
                Style::default().fg(KANBLAM_GREEN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" │", border_style),
        ]);
    }

    let bubble = Paragraph::new(lines);
    frame.render_widget(bubble, area);
}

/// Render CTA hint at top-left, pointing up at the +project button
fn render_cta_hint(frame: &mut Frame, area: Rect) {
    // Position at top-left corner of the welcome area
    // This should align under the [!] +project button in the header
    let cta_area = Rect {
        x: area.x + 1,
        y: area.y,
        width: 34,
        height: 4,
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("       ↑", Style::default().fg(Color::Yellow)),
        ]),
        Line::from(Span::styled(
            "┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(vec![
            Span::styled("┃  Press ", Style::default().fg(Color::Yellow)),
            Span::styled(" ! ", Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(" to open a project   ┃", Style::default().fg(Color::Yellow)),
        ]),
        Line::from(Span::styled(
            "┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛",
            Style::default().fg(Color::Yellow),
        )),
    ];

    let cta = Paragraph::new(lines);
    frame.render_widget(cta, cta_area);
}

/// Render the quick start guide
fn render_quick_start(frame: &mut Frame, area: Rect) {
    let box_width = 48u16;
    let start_x = area.x + area.width.saturating_sub(box_width) / 2;

    let guide_area = Rect {
        x: start_x,
        y: area.y,
        width: box_width,
        height: 11,
    };

    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let border_style = Style::default().fg(Color::DarkGray);

    let lines = vec![
        Line::from(Span::styled(
            "┌──────────────────────────────────────────────┐",
            border_style,
        )),
        Line::from(vec![
            Span::styled("│  ", border_style),
            Span::styled("Quick Start", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("                        Exit ", border_style),
            Span::styled("[q]", key_style),
            Span::styled(" ││", border_style),
        ]),
        Line::from(Span::styled(
            "├──────────────────────────────────────────────┤",
            border_style,
        )),
        Line::from(vec![
            Span::styled("│   1. Open/start a new git project  ", border_style),
            Span::styled("[!]", key_style),
            Span::styled("       │", border_style),
        ]),
        Line::from(vec![
            Span::styled("│   2. Create your first task        ", border_style),
            Span::styled("[i]", key_style),
            Span::styled("       │", border_style),
        ]),
        Line::from(vec![
            Span::styled("│   3. Kick it off                   ", border_style),
            Span::styled("[s]", key_style),
            Span::styled("       │", border_style),
        ]),
        Line::from(vec![
            Span::styled("│   4. Open isolated Claude+terminal ", border_style),
            Span::styled("[o]", key_style),
            Span::styled("       │", border_style),
        ]),
        Line::from(vec![
            Span::styled("│   5. Apply to main for testing     ", border_style),
            Span::styled("[a]", key_style),
            Span::styled("       │", border_style),
        ]),
        Line::from(vec![
            Span::styled("│   6. Merge if all's well           ", border_style),
            Span::styled("[m]", key_style),
            Span::styled("       │", border_style),
        ]),
        Line::from(Span::styled(
            "│   7. Rinse and repeat!                       │",
            border_style,
        )),
        Line::from(Span::styled(
            "└──────────────────────────────────────────────┘",
            border_style,
        )),
    ];

    let guide = Paragraph::new(lines);
    frame.render_widget(guide, guide_area);
}

/// Render mascot with inline text (medium layout)
fn render_mascot_inline(
    frame: &mut Frame,
    area: Rect,
    eye_animation: EyeAnimation,
    animation_frame: usize,
    message_idx: usize,
    bubble_focused: bool,
) {
    let (title, body) = WELCOME_MESSAGES[message_idx % WELCOME_MESSAGES.len()];
    let total_messages = WELCOME_MESSAGES.len();
    let (left_eye, right_eye) = if eye_animation == EyeAnimation::StarEyes {
        let frame_idx = animation_frame % STAR_EYE_FRAMES.len();
        (STAR_EYE_FRAMES[frame_idx], STAR_EYE_FRAMES[frame_idx])
    } else {
        eye_animation.eye_chars()
    };

    let eye_style = Style::default().fg(KANBLAM_GREEN);

    // Center everything
    let total_width = 50u16;
    let start_x = area.x + area.width.saturating_sub(total_width) / 2;

    let content_area = Rect {
        x: start_x,
        y: area.y,
        width: total_width,
        height: 5,
    };

    // Format title with hint number
    let title_with_hint = format!("{} ({}/{})", title, message_idx + 1, total_messages);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("  ▄▓▓▓▓▄     ", Style::default().fg(MASCOT_YELLOW)),
            Span::styled(title_with_hint, Style::default().fg(KANBLAM_GREEN).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("  ▓", Style::default().fg(MASCOT_ORANGE)),
            Span::styled(left_eye, eye_style),
            Span::styled("▀▀", Style::default().fg(MASCOT_ORANGE)),
            Span::styled(right_eye, eye_style),
            Span::styled("▓▒▒    ", Style::default().fg(MASCOT_ORANGE)),
            Span::styled(*body.get(0).unwrap_or(&""), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  ▓▓▓▓▓▓▒    ", Style::default().fg(MASCOT_RED)),
            Span::styled(*body.get(1).unwrap_or(&""), Style::default().fg(Color::White)),
        ]),
        Line::from(Span::styled("   ▀▀ ▀▀", Style::default().fg(MASCOT_MAGENTA))),
    ];

    // Add navigation arrows on the right when focused
    if bubble_focused {
        let arrow_style = Style::default().fg(Color::White);
        let dim_style = Style::default().fg(Color::DarkGray);
        lines.push(Line::from(vec![
            Span::styled("                                           ", dim_style),
            Span::styled("←", arrow_style),
            Span::styled(" ", dim_style),
            Span::styled("→", arrow_style),
        ]));
    }

    let content = Paragraph::new(lines);
    frame.render_widget(content, content_area);
}

/// Render small mascot (compact layout)
fn render_mascot_small(
    frame: &mut Frame,
    area: Rect,
    eye_animation: EyeAnimation,
    animation_frame: usize,
) {
    let (left_eye, right_eye) = if eye_animation == EyeAnimation::StarEyes {
        let frame_idx = animation_frame % STAR_EYE_FRAMES.len();
        (STAR_EYE_FRAMES[frame_idx], STAR_EYE_FRAMES[frame_idx])
    } else {
        eye_animation.eye_chars()
    };

    let eye_style = Style::default().fg(KANBLAM_GREEN);

    let lines = vec![
        Line::from(Span::styled("  ▄▓▓▓▓▄  ", Style::default().fg(MASCOT_YELLOW))),
        Line::from(vec![
            Span::styled("  ▓", Style::default().fg(MASCOT_ORANGE)),
            Span::styled(left_eye, eye_style),
            Span::styled("▀▀", Style::default().fg(MASCOT_ORANGE)),
            Span::styled(right_eye, eye_style),
            Span::styled("▓▒▒", Style::default().fg(MASCOT_ORANGE)),
        ]),
        Line::from(Span::styled("  ▓▓▓▓▓▓▒ ", Style::default().fg(MASCOT_RED))),
        Line::from(Span::styled("   ▀▀ ▀▀  ", Style::default().fg(MASCOT_MAGENTA))),
    ];

    let mascot = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(mascot, area);
}

/// Render compact quick start (medium/compact layout)
fn render_quick_start_compact(frame: &mut Frame, area: Rect) {
    let key_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(Color::DarkGray);

    let lines = vec![
        Line::from(Span::styled("Quick Start:", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(vec![
            Span::styled(" [!]", key_style),
            Span::styled(" Open project  ", text_style),
            Span::styled("[i]", key_style),
            Span::styled(" New task  ", text_style),
            Span::styled("[s]", key_style),
            Span::styled(" Start  ", text_style),
            Span::styled("[?]", key_style),
            Span::styled(" Help", text_style),
        ]),
    ];

    let guide = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(guide, area);
}
