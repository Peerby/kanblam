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


/// Render the logo/branding in the given area
/// Automatically chooses between full logo, compact text, or nothing based on available width
pub fn render_logo(frame: &mut Frame, area: Rect) {
    let width = area.width;

    if width >= FULL_LOGO_WIDTH && area.height >= 4 {
        render_full_logo(frame, area);
    } else if width >= COMPACT_LOGO_WIDTH {
        render_compact_logo(frame, area);
    } else if width >= MIN_BRANDING_WIDTH {
        render_minimal_logo(frame, area);
    }
    // If width < MIN_BRANDING_WIDTH, render nothing
}

/// BLAM-inspired color palette - comic book explosion colors!
/// Fiery gradient from yellow through orange to red/magenta
const BLAM_YELLOW: Color = Color::Rgb(255, 220, 50);   // Bright explosive yellow
const BLAM_ORANGE: Color = Color::Rgb(255, 140, 0);    // Hot orange
const BLAM_RED: Color = Color::Rgb(255, 60, 60);       // Action red
const BLAM_MAGENTA: Color = Color::Rgb(255, 50, 150);  // Electric magenta

/// Render the full ASCII art logo with mascot
fn render_full_logo(frame: &mut Frame, area: Rect) {
    // Mascot gets a gradient from yellow (top) to orange (bottom)
    let mascot_styles = [
        Style::default().fg(BLAM_YELLOW),
        Style::default().fg(BLAM_ORANGE),
        Style::default().fg(BLAM_RED),
        Style::default().fg(BLAM_MAGENTA),
    ];

    // KANBLAM text - each letter gets its own explosive color
    // K=yellow, A=orange, N=red, B=magenta, L=red, A=orange, M=yellow
    let lines = vec![
        Line::from(vec![
            Span::styled(LOGO_MASCOT[0], mascot_styles[0]),
            Span::styled("    ", Style::default()),
            Span::styled("█ █ ", Style::default().fg(BLAM_YELLOW)),   // K
            Span::styled("▄▀█ ", Style::default().fg(BLAM_ORANGE)),   // A
            Span::styled("█▄ █ ", Style::default().fg(BLAM_RED)),     // N
            Span::styled("██▄ ", Style::default().fg(BLAM_MAGENTA)),  // B
            Span::styled("█   ", Style::default().fg(BLAM_RED)),      // L
            Span::styled("▄▀█ ", Style::default().fg(BLAM_ORANGE)),   // A
            Span::styled("█▀▄▀█", Style::default().fg(BLAM_YELLOW)),  // M
        ]),
        Line::from(vec![
            Span::styled(LOGO_MASCOT[1], mascot_styles[1]),
            Span::styled("    ", Style::default()),
            Span::styled("█▀▄ ", Style::default().fg(BLAM_YELLOW)),   // K
            Span::styled("█▀█ ", Style::default().fg(BLAM_ORANGE)),   // A
            Span::styled("█ ▀█ ", Style::default().fg(BLAM_RED)),     // N
            Span::styled("█▀▄ ", Style::default().fg(BLAM_MAGENTA)),  // B
            Span::styled("█   ", Style::default().fg(BLAM_RED)),      // L
            Span::styled("█▀█ ", Style::default().fg(BLAM_ORANGE)),   // A
            Span::styled("█ ▀ █", Style::default().fg(BLAM_YELLOW)),  // M
        ]),
        Line::from(vec![
            Span::styled(LOGO_MASCOT[2], mascot_styles[2]),
            Span::styled("    ", Style::default()),
            Span::styled("█ █ ", Style::default().fg(BLAM_YELLOW)),   // K
            Span::styled("█ █ ", Style::default().fg(BLAM_ORANGE)),   // A
            Span::styled("█  █ ", Style::default().fg(BLAM_RED)),     // N
            Span::styled("██▀ ", Style::default().fg(BLAM_MAGENTA)),  // B
            Span::styled("█▄▄ ", Style::default().fg(BLAM_RED)),      // L
            Span::styled("█ █ ", Style::default().fg(BLAM_ORANGE)),   // A
            Span::styled("█   █", Style::default().fg(BLAM_YELLOW)),  // M
        ]),
        Line::from(vec![
            Span::styled(LOGO_MASCOT[3], mascot_styles[3]),
            Span::styled("                                   ", Style::default()),
        ]),
    ];

    let paragraph = Paragraph::new(lines).alignment(Alignment::Right);
    frame.render_widget(paragraph, area);
}

/// Render compact text-only version (single line) with BLAM gradient
fn render_compact_logo(frame: &mut Frame, area: Rect) {
    // Each letter gets its BLAM color
    let line = Line::from(vec![
        Span::styled("K", Style::default().fg(BLAM_YELLOW)),
        Span::styled("A", Style::default().fg(BLAM_ORANGE)),
        Span::styled("N", Style::default().fg(BLAM_RED)),
        Span::styled("B", Style::default().fg(BLAM_MAGENTA)),
        Span::styled("L", Style::default().fg(BLAM_RED)),
        Span::styled("A", Style::default().fg(BLAM_ORANGE)),
        Span::styled("M", Style::default().fg(BLAM_YELLOW)),
    ]);

    let paragraph = Paragraph::new(line).alignment(Alignment::Right);
    frame.render_widget(paragraph, area);
}

/// Render minimal version (just the name) with BLAM gradient
fn render_minimal_logo(frame: &mut Frame, area: Rect) {
    // Each letter gets its BLAM color
    let line = Line::from(vec![
        Span::styled("K", Style::default().fg(BLAM_YELLOW)),
        Span::styled("A", Style::default().fg(BLAM_ORANGE)),
        Span::styled("N", Style::default().fg(BLAM_RED)),
        Span::styled("B", Style::default().fg(BLAM_MAGENTA)),
        Span::styled("L", Style::default().fg(BLAM_RED)),
        Span::styled("A", Style::default().fg(BLAM_ORANGE)),
        Span::styled("M", Style::default().fg(BLAM_YELLOW)),
    ]);
    let paragraph = Paragraph::new(line).alignment(Alignment::Right);
    frame.render_widget(paragraph, area);
}

/// Calculate how much width the logo needs based on available space
pub fn logo_width_needed(available_width: u16, available_height: u16) -> u16 {
    if available_width >= FULL_LOGO_WIDTH && available_height >= 4 {
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
