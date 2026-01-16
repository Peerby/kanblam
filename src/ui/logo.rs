use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// The full ASCII art logo width (mascot + text)
pub const FULL_LOGO_WIDTH: u16 = 58;

/// The compact text-only width
pub const COMPACT_LOGO_WIDTH: u16 = 7;

/// Minimum width to show any branding
pub const MIN_BRANDING_WIDTH: u16 = 20;

/// Logo lines for the full version (mascot + KANBLAM text)
/// Uses block characters and green color scheme
const LOGO_MASCOT: [&str; 4] = [
    "  ▄▓▓▓▓▄  ",
    "  ▓ ▀▀ ▓▒▒",
    "  ▓▓▓▓▓▓▒ ",
    "   ▀▀ ▀▀  ",
];

/// Mascot color palette - warm gradient, toned down (normal state)
const MASCOT_YELLOW: Color = Color::Rgb(230, 190, 40);   // Slightly darker yellow
const MASCOT_ORANGE: Color = Color::Rgb(235, 130, 20);   // Warm orange
const MASCOT_RED: Color = Color::Rgb(200, 80, 70);       // Muted/desaturated red
const MASCOT_MAGENTA: Color = Color::Rgb(180, 70, 120);  // Softer magenta, less neon

/// Mascot color palette - saturated (shimmer/lit state)
const MASCOT_YELLOW_SAT: Color = Color::Rgb(255, 220, 50);   // Bright explosive yellow
const MASCOT_ORANGE_SAT: Color = Color::Rgb(255, 140, 0);    // Hot orange
const MASCOT_RED_SAT: Color = Color::Rgb(255, 60, 60);       // Vivid red
const MASCOT_MAGENTA_SAT: Color = Color::Rgb(255, 50, 150);  // Electric magenta

/// Feet colors - slightly dimmed (~8%) relative to torso
const FEET_MAGENTA: Color = Color::Rgb(166, 64, 110);        // Dimmed normal magenta
const FEET_MAGENTA_SAT: Color = Color::Rgb(235, 46, 138);    // Dimmed saturated magenta

/// Render the logo/branding in the given area
/// shimmer_frame: 0 = no animation, 1-4 = beam traveling up (row 4 to row 1), 5-7 = fade out
pub fn render_logo(frame: &mut Frame, area: Rect, shimmer_frame: u8) {
    let width = area.width;

    if width >= FULL_LOGO_WIDTH && area.height >= 3 {
        render_full_logo(frame, area, shimmer_frame);
    } else if width >= COMPACT_LOGO_WIDTH {
        render_compact_logo(frame, area);
    } else if width >= MIN_BRANDING_WIDTH {
        render_minimal_logo(frame, area);
    }
    // If width < MIN_BRANDING_WIDTH, render nothing
}

/// Get the mascot color for a row, considering highlight animation
/// row: 0-3 (top to bottom: head, face, body, feet)
/// shimmer_frame: 0 = no animation, 1 = lead-in (absorbs timing), 2-5 = highlight glides up
fn get_mascot_color(row: usize, shimmer_frame: u8) -> Color {
    // Feet (row 3) use slightly dimmed colors
    let normal_colors = [MASCOT_YELLOW, MASCOT_ORANGE, MASCOT_RED, FEET_MAGENTA];
    let saturated_colors = [MASCOT_YELLOW_SAT, MASCOT_ORANGE_SAT, MASCOT_RED_SAT, FEET_MAGENTA_SAT];

    // Simple gliding highlight: one row lit at a time, moving upward
    // Frame 1 = lead-in (normal colors, absorbs variable timing from trigger)
    // Frame 2 = row 3 (feet), Frame 3 = row 2 (body), Frame 4 = row 1 (face), Frame 5 = row 0 (head)
    let highlighted_row = match shimmer_frame {
        2 => Some(3),  // Feet row
        3 => Some(2),  // Body row
        4 => Some(1),  // Face row
        5 => Some(0),  // Head row
        _ => None,
    };

    if highlighted_row == Some(row) {
        saturated_colors[row]
    } else {
        normal_colors[row]
    }
}

/// Render the full ASCII art logo with mascot (head, face, body in header area)
fn render_full_logo(frame: &mut Frame, area: Rect, shimmer_frame: u8) {
    // Get colors for each row based on shimmer state
    let mascot_styles = [
        Style::default().fg(get_mascot_color(0, shimmer_frame)),
        Style::default().fg(get_mascot_color(1, shimmer_frame)),
        Style::default().fg(get_mascot_color(2, shimmer_frame)),
    ];

    // KANBLAM text stays green
    let green = Color::Rgb(80, 200, 120);
    let text_style = Style::default().fg(green);

    // Eye style - same green as KANBLAM text
    let eye_style = Style::default().fg(green);

    let lines = vec![
        Line::from(vec![
            Span::styled(LOGO_MASCOT[0], mascot_styles[0]),
            Span::styled("    ", Style::default()),
            Span::styled("█ █ ▄▀█ █▄ █ ██▄ █   ▄▀█ █▀▄▀█", text_style),
            Span::styled(" ", Style::default()),       // Shift left by 1
        ]),
        // Face row with green eyes in the negative space
        Line::from(vec![
            Span::styled("  ▓", mascot_styles[1]),      // Left edge
            Span::styled("▪", eye_style),               // Left eye (small square)
            Span::styled("▀▀", mascot_styles[1]),       // Nose/brow
            Span::styled("▪", eye_style),               // Right eye (small square)
            Span::styled("▓▒▒", mascot_styles[1]),      // Right edge + shadow
            Span::styled("    ", Style::default()),
            Span::styled("█▀▄ █▀█ █ ▀█ █▀▄ █   █▀█ █ ▀ █", text_style),
            Span::styled(" ", Style::default()),       // Shift left by 1
        ]),
        Line::from(vec![
            Span::styled(LOGO_MASCOT[2], mascot_styles[2]),
            Span::styled("    ", Style::default()),
            Span::styled("█ █ █ █ █  █ ██▀ █▄▄ █ █ █   █", text_style),
            Span::styled(" ", Style::default()),       // Shift left by 1
        ]),
    ];

    let paragraph = Paragraph::new(lines).alignment(Alignment::Right);
    frame.render_widget(paragraph, area);
}

/// Render just the mascot feet - call this AFTER rendering kanban to overlap the border
pub fn render_mascot_feet(frame: &mut Frame, area: Rect, shimmer_frame: u8) {
    let feet_style = Style::default().fg(get_mascot_color(3, shimmer_frame));
    let border_style = Style::default().fg(Color::Cyan);

    // Feet characters with border line filling the gaps for seamless appearance
    // Original feet: "   ▀▀ ▀▀  " - replace spaces with ─ in border color
    // Then add 35 chars of border line + corner to align with mascot body above (shifted left by 1)
    let feet_line = Line::from(vec![
        Span::styled("───", border_style),        // Leading border (was spaces)
        Span::styled("▀▀", feet_style),           // Left foot
        Span::styled("─", border_style),          // Gap between feet
        Span::styled("▀▀", feet_style),           // Right foot
        Span::styled("──", border_style),         // Trailing border (was spaces)
        Span::styled("──────────────────────────────────", border_style),  // 34 chars of line
        Span::styled("┐", border_style),          // Top-right corner
    ]);

    let paragraph = Paragraph::new(feet_line).alignment(Alignment::Right);
    frame.render_widget(paragraph, area);
}

/// Render compact text-only version (single line)
fn render_compact_logo(frame: &mut Frame, area: Rect) {
    let green = Color::Rgb(80, 200, 120);
    let line = Line::from(Span::styled("KANBLAM", Style::default().fg(green)));
    let paragraph = Paragraph::new(line).alignment(Alignment::Right);
    frame.render_widget(paragraph, area);
}

/// Render minimal version (just the name)
fn render_minimal_logo(frame: &mut Frame, area: Rect) {
    let green = Color::Rgb(80, 200, 120);
    let line = Line::from(Span::styled("KANBLAM", Style::default().fg(green)));
    let paragraph = Paragraph::new(line).alignment(Alignment::Right);
    frame.render_widget(paragraph, area);
}

/// Calculate how much width the logo needs based on available space
pub fn logo_width_needed(available_width: u16, available_height: u16) -> u16 {
    if available_width >= FULL_LOGO_WIDTH && available_height >= 3 {
        FULL_LOGO_WIDTH
    } else if available_width >= COMPACT_LOGO_WIDTH {
        COMPACT_LOGO_WIDTH
    } else if available_width >= MIN_BRANDING_WIDTH {
        MIN_BRANDING_WIDTH
    } else {
        0
    }
}

/// Minimum terminal height to show the full 4-line logo
/// Below this, we use single-line branding to preserve space for the kanban board
pub const MIN_HEIGHT_FOR_FULL_LOGO: u16 = 40;

/// Minimum terminal width to show the full logo (needs extra space beyond logo itself)
pub const MIN_WIDTH_FOR_FULL_LOGO: u16 = 120;

/// Check if we should show the full 4-line logo
/// Only shows when terminal is generously sized (120+ cols, 40+ rows)
pub fn should_show_full_logo(terminal_width: u16, terminal_height: u16) -> bool {
    terminal_width >= MIN_WIDTH_FOR_FULL_LOGO && terminal_height >= MIN_HEIGHT_FOR_FULL_LOGO
}
