//! Interactive terminal modal for Claude CLI sessions
//!
//! This modal renders a tmux pane output with vt100 parsing and allows
//! users to interact with Claude directly. Ctrl-Esc closes the modal.

use crate::model::InteractiveModal;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Render the interactive terminal modal
pub fn render_interactive_modal(frame: &mut Frame, modal: &InteractiveModal) {
    // Use full screen for the terminal
    let area = frame.area();

    // Capture current pane content (with escape codes for styling)
    let terminal_content = match crate::tmux::capture_pane_with_escapes(&modal.tmux_target) {
        Ok(content) => content,
        Err(e) => {
            // Window is gone - show helpful message with error details
            format!(
                "\n\n  Session window not found.\n\n  Target: {}\n  Error: {}\n\n  Press Ctrl-Esc to close this modal.\n",
                modal.tmux_target,
                e
            )
        }
    };

    // Parse terminal content using vt100 for proper ANSI handling
    let lines = parse_terminal_output(&terminal_content, area.width.saturating_sub(2) as usize, modal.scroll_offset);

    // Create the terminal block with info bar
    let title = format!(
        " Claude Interactive - {} [Ctrl-Esc to close] ",
        modal.tmux_target
    );

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let terminal_view = Paragraph::new(lines)
        .block(block)
        .style(Style::default().fg(Color::White).bg(Color::Black));

    // Clear area and render
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(terminal_view, area);

    // Render status bar at bottom with hints
    render_status_bar(frame, area, modal);
}

/// Parse terminal output using vt100 for proper ANSI escape sequence handling
fn parse_terminal_output(content: &str, width: usize, scroll_offset: usize) -> Vec<Line<'static>> {
    // Create a vt100 parser with appropriate size
    let height = 500; // Large enough to capture all content
    let mut parser = vt100::Parser::new(height as u16, width as u16, 0);

    // Process the content through the parser
    parser.process(content.as_bytes());

    // Get the screen from the parser
    let screen = parser.screen();

    // Convert each row to a ratatui Line with styles
    let mut lines = Vec::new();

    for row in scroll_offset..screen.size().0 as usize {
        let mut spans = Vec::new();
        let mut current_style = Style::default();
        let mut current_text = String::new();

        for col in 0..screen.size().1 as usize {
            let cell = screen.cell(row as u16, col as u16);
            if let Some(cell) = cell {
                let cell_style = convert_vt100_style(&cell);

                if cell_style != current_style {
                    // Push previous span if not empty
                    if !current_text.is_empty() {
                        spans.push(Span::styled(current_text.clone(), current_style));
                        current_text.clear();
                    }
                    current_style = cell_style;
                }

                current_text.push(cell.contents().chars().next().unwrap_or(' '));
            } else {
                current_text.push(' ');
            }
        }

        // Push final span
        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }

        lines.push(Line::from(spans));
    }

    // Remove trailing empty lines
    while lines.last().map(|l| l.spans.iter().all(|s| s.content.trim().is_empty())).unwrap_or(false) {
        lines.pop();
    }

    lines
}

/// Convert vt100 cell attributes to ratatui Style
fn convert_vt100_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    // Convert foreground color
    style = style.fg(convert_vt100_color(cell.fgcolor()));

    // Convert background color
    if let vt100::Color::Default = cell.bgcolor() {
        // Keep default (transparent) background
    } else {
        style = style.bg(convert_vt100_color(cell.bgcolor()));
    }

    // Apply modifiers
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }

    style
}

/// Convert vt100 color to ratatui Color
fn convert_vt100_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(idx) => {
            // Standard 16 colors
            match idx {
                0 => Color::Black,
                1 => Color::Red,
                2 => Color::Green,
                3 => Color::Yellow,
                4 => Color::Blue,
                5 => Color::Magenta,
                6 => Color::Cyan,
                7 => Color::White,
                8 => Color::DarkGray,
                9 => Color::LightRed,
                10 => Color::LightGreen,
                11 => Color::LightYellow,
                12 => Color::LightBlue,
                13 => Color::LightMagenta,
                14 => Color::LightCyan,
                15 => Color::White,
                // Extended 256 colors
                _ => Color::Indexed(idx),
            }
        }
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Render the status bar with keybindings
fn render_status_bar(frame: &mut Frame, area: Rect, _modal: &InteractiveModal) {
    let hints = Line::from(vec![
        Span::styled(" Ctrl-Esc", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(" close  ", Style::default().fg(Color::DarkGray)),
        Span::styled("PgUp/PgDn", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(" scroll  ", Style::default().fg(Color::DarkGray)),
        Span::styled("All other keys", Style::default().fg(Color::Yellow)),
        Span::styled(" → Claude ", Style::default().fg(Color::DarkGray)),
    ]);

    let status_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };

    let status = Paragraph::new(hints)
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));

    frame.render_widget(status, status_area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_vt100_color_default() {
        let color = convert_vt100_color(vt100::Color::Default);
        assert_eq!(color, Color::Reset);
    }

    #[test]
    fn test_convert_vt100_color_standard_16() {
        // Standard 16 colors
        let test_cases = vec![
            (0, Color::Black),
            (1, Color::Red),
            (2, Color::Green),
            (3, Color::Yellow),
            (4, Color::Blue),
            (5, Color::Magenta),
            (6, Color::Cyan),
            (7, Color::White),
            (8, Color::DarkGray),
            (9, Color::LightRed),
            (10, Color::LightGreen),
            (11, Color::LightYellow),
            (12, Color::LightBlue),
            (13, Color::LightMagenta),
            (14, Color::LightCyan),
            (15, Color::White),
        ];

        for (idx, expected) in test_cases {
            let color = convert_vt100_color(vt100::Color::Idx(idx));
            assert_eq!(color, expected, "Failed for index {}", idx);
        }
    }

    #[test]
    fn test_convert_vt100_color_extended_256() {
        // Extended 256 colors (index >= 16)
        let color = convert_vt100_color(vt100::Color::Idx(196));
        assert_eq!(color, Color::Indexed(196));

        let color = convert_vt100_color(vt100::Color::Idx(255));
        assert_eq!(color, Color::Indexed(255));
    }

    #[test]
    fn test_convert_vt100_color_rgb() {
        let color = convert_vt100_color(vt100::Color::Rgb(128, 64, 255));
        assert_eq!(color, Color::Rgb(128, 64, 255));
    }

    #[test]
    fn test_parse_terminal_output_plain_text() {
        let content = "Hello, World!\nSecond line\n";
        let lines = parse_terminal_output(content, 80, 0);

        // Should have at least 2 non-empty lines
        assert!(lines.len() >= 2);
    }

    #[test]
    fn test_parse_terminal_output_with_scroll_offset() {
        let content = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\n";
        let lines_no_offset = parse_terminal_output(content, 80, 0);
        let lines_with_offset = parse_terminal_output(content, 80, 2);

        // With offset 2, we should have fewer lines
        assert!(lines_with_offset.len() <= lines_no_offset.len());
    }

    #[test]
    fn test_parse_terminal_output_ansi_colors() {
        // Red text: ESC[31m
        let content = "\x1b[31mRed Text\x1b[0m Normal\n";
        let lines = parse_terminal_output(content, 80, 0);

        // Should parse without panicking
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_parse_terminal_output_bold() {
        // Bold text: ESC[1m
        let content = "\x1b[1mBold Text\x1b[0m\n";
        let lines = parse_terminal_output(content, 80, 0);

        assert!(!lines.is_empty());
    }

    #[test]
    fn test_parse_terminal_output_256_color() {
        // 256 color: ESC[38;5;196m (foreground red)
        let content = "\x1b[38;5;196mColored\x1b[0m\n";
        let lines = parse_terminal_output(content, 80, 0);

        assert!(!lines.is_empty());
    }

    #[test]
    fn test_parse_terminal_output_rgb_color() {
        // RGB color: ESC[38;2;128;64;255m
        let content = "\x1b[38;2;128;64;255mRGB Color\x1b[0m\n";
        let lines = parse_terminal_output(content, 80, 0);

        assert!(!lines.is_empty());
    }

    #[test]
    fn test_parse_terminal_output_empty() {
        let content = "";
        let lines = parse_terminal_output(content, 80, 0);

        // Should handle empty content gracefully
        assert!(lines.is_empty() || lines.iter().all(|l| l.spans.iter().all(|s| s.content.trim().is_empty())));
    }

    #[test]
    fn test_parse_terminal_output_wide_width() {
        let content = "Short\n";
        let lines = parse_terminal_output(content, 200, 0);

        // Should work with wide terminal
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_parse_terminal_output_narrow_width() {
        let content = "This is a longer line that might wrap\n";
        let lines = parse_terminal_output(content, 10, 0);

        // Should work with narrow terminal (content wraps)
        assert!(!lines.is_empty());
    }
}
