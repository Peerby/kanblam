use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Eye animation states for the mascot
/// These create brief, playful eye animations
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum EyeAnimation {
    /// Normal eyes (▪ ▪)
    #[default]
    Normal,
    /// Blink - both eyes closed (─ ─)
    Blink,
    /// Wink left - left eye closed (─ ▪)
    WinkLeft,
    /// Wink right - right eye closed (▪ ─)
    WinkRight,
    /// Look down - eyes looking downward (. .)
    LookDown,
    /// Look up - eyes looking upward (' ')
    LookUp,
    /// Star eyes - celebratory (★ ★) - used for commits/merges
    StarEyes,
    /// Squint - both eyes squinted (- -)
    Squint,
    /// Wide - wide eyes (● ●)
    Wide,
    /// Heart - heart eyes (♥ ♥)
    Heart,
    /// Dizzy - dizzy eyes (@ @)
    Dizzy,
    /// Sleepy - half-closed eyes (˘ ˘)
    Sleepy,
}

/// Animated star eye frames for celebratory animations
/// Cycles through: ✦ → ✧ → ★ → ✧ → ✦ → · (sparkle effect)
pub const STAR_EYE_FRAMES: [&str; 6] = ["✦", "✧", "★", "✧", "✦", "·"];

impl EyeAnimation {
    /// Get the left and right eye characters for this animation state
    pub fn eye_chars(&self) -> (&'static str, &'static str) {
        match self {
            EyeAnimation::Normal => ("▪", "▪"),
            EyeAnimation::Blink => ("─", "─"),
            EyeAnimation::WinkLeft => ("─", "▪"),
            EyeAnimation::WinkRight => ("▪", "─"),
            EyeAnimation::LookDown => (".", "."),
            EyeAnimation::LookUp => ("'", "'"),
            EyeAnimation::StarEyes => ("★", "★"),
            EyeAnimation::Squint => ("-", "-"),
            EyeAnimation::Wide => ("●", "●"),
            EyeAnimation::Heart => ("♥", "♥"),
            EyeAnimation::Dizzy => ("@", "@"),
            EyeAnimation::Sleepy => ("˘", "˘"),
        }
    }

    /// Get animated star eye characters based on animation frame
    /// Returns (left_eye, right_eye) cycling through sparkle frames
    pub fn star_eyes_animated(animation_frame: usize) -> (&'static str, &'static str) {
        let frame = animation_frame % STAR_EYE_FRAMES.len();
        let char = STAR_EYE_FRAMES[frame];
        (char, char)
    }

    /// Get a random animation (excluding Normal and StarEyes)
    /// Uses a simple hash to ensure good distribution
    pub fn random() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        // Simple hash function for better distribution
        // Mix the bits to avoid patterns from regular time intervals
        let hash = nanos
            .wrapping_mul(0x517cc1b727220a95)
            .wrapping_add(0x9e3779b97f4a7c15);
        let index = (hash >> 32) as usize % 10;

        match index {
            0 => EyeAnimation::Blink,
            1 => EyeAnimation::WinkLeft,
            2 => EyeAnimation::WinkRight,
            3 => EyeAnimation::LookDown,
            4 => EyeAnimation::LookUp,
            5 => EyeAnimation::Squint,
            6 => EyeAnimation::Wide,
            7 => EyeAnimation::Heart,
            8 => EyeAnimation::Dizzy,
            _ => EyeAnimation::Sleepy,
        }
    }
}

/// The full ASCII art logo width (mascot + KANBLAM text)
pub const FULL_LOGO_WIDTH: u16 = 58;

/// The medium ASCII art logo width (mascot + KB text)
/// Mascot (10) + gap (2) + "KB" text (7) + trailing space (1) = 20
pub const MEDIUM_LOGO_WIDTH: u16 = 20;

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

/// Logo size variants for responsive design
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogoSize {
    /// Full mascot + KANBLAM wordmark (58 chars wide)
    Full,
    /// Medium mascot + KB wordmark (24 chars wide)
    Medium,
    /// Compact text-only "KANBLAM"
    Compact,
    /// Minimal branding
    Minimal,
    /// No branding (too small)
    None,
}

/// Render the logo/branding in the given area
/// shimmer_frame: 0 = no animation, 1-4 = beam traveling up (row 4 to row 1), 5-7 = fade out
/// animation_frame: global animation frame counter for animated star eyes
pub fn render_logo(frame: &mut Frame, area: Rect, shimmer_frame: u8, eye_animation: EyeAnimation, animation_frame: usize) {
    render_logo_size(frame, area, shimmer_frame, LogoSize::Full, eye_animation, animation_frame)
}

/// Render the logo at a specific size
pub fn render_logo_size(frame: &mut Frame, area: Rect, shimmer_frame: u8, size: LogoSize, eye_animation: EyeAnimation, animation_frame: usize) {
    match size {
        LogoSize::Full if area.width >= FULL_LOGO_WIDTH && area.height >= 3 => {
            render_full_logo(frame, area, shimmer_frame, eye_animation, animation_frame);
        }
        LogoSize::Medium if area.width >= MEDIUM_LOGO_WIDTH && area.height >= 3 => {
            render_medium_logo(frame, area, shimmer_frame, eye_animation, animation_frame);
        }
        LogoSize::Compact | LogoSize::Full | LogoSize::Medium if area.width >= COMPACT_LOGO_WIDTH => {
            render_compact_logo(frame, area);
        }
        LogoSize::Minimal if area.width >= MIN_BRANDING_WIDTH => {
            render_minimal_logo(frame, area);
        }
        _ => {
            // Nothing to render
        }
    }
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
fn render_full_logo(frame: &mut Frame, area: Rect, shimmer_frame: u8, eye_animation: EyeAnimation, animation_frame: usize) {
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

    // Get eye characters based on animation state
    // StarEyes uses animated frames, others use static chars
    let (left_eye, right_eye) = if eye_animation == EyeAnimation::StarEyes {
        EyeAnimation::star_eyes_animated(animation_frame)
    } else {
        eye_animation.eye_chars()
    };

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
            Span::styled(left_eye, eye_style),          // Left eye (animated)
            Span::styled("▀▀", mascot_styles[1]),       // Nose/brow
            Span::styled(right_eye, eye_style),         // Right eye (animated)
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

/// Render the medium ASCII art logo with mascot + abbreviated KB text
fn render_medium_logo(frame: &mut Frame, area: Rect, shimmer_frame: u8, eye_animation: EyeAnimation, animation_frame: usize) {
    // Get colors for each row based on shimmer state
    let mascot_styles = [
        Style::default().fg(get_mascot_color(0, shimmer_frame)),
        Style::default().fg(get_mascot_color(1, shimmer_frame)),
        Style::default().fg(get_mascot_color(2, shimmer_frame)),
    ];

    // KB text stays green (same as full KANBLAM)
    let green = Color::Rgb(80, 200, 120);
    let text_style = Style::default().fg(green);

    // Eye style - same green as KB text
    let eye_style = Style::default().fg(green);

    // Get eye characters based on animation state
    // StarEyes uses animated frames, others use static chars
    let (left_eye, right_eye) = if eye_animation == EyeAnimation::StarEyes {
        EyeAnimation::star_eyes_animated(animation_frame)
    } else {
        eye_animation.eye_chars()
    };

    // KB wordmark (just K and B from KANBLAM)
    // K: █ █ / █▀▄ / █ █
    // B: ██▄ / █▀▄ / ██▀
    let lines = vec![
        Line::from(vec![
            Span::styled(LOGO_MASCOT[0], mascot_styles[0]),
            Span::styled("  ", Style::default()),
            Span::styled("█ █ ██▄", text_style),
            Span::styled(" ", Style::default()),
        ]),
        // Face row with green eyes in the negative space
        Line::from(vec![
            Span::styled("  ▓", mascot_styles[1]),      // Left edge
            Span::styled(left_eye, eye_style),          // Left eye (animated)
            Span::styled("▀▀", mascot_styles[1]),       // Nose/brow
            Span::styled(right_eye, eye_style),         // Right eye (animated)
            Span::styled("▓▒▒", mascot_styles[1]),      // Right edge + shadow
            Span::styled("  ", Style::default()),
            Span::styled("█▀▄ █▀▄", text_style),
            Span::styled(" ", Style::default()),
        ]),
        Line::from(vec![
            Span::styled(LOGO_MASCOT[2], mascot_styles[2]),
            Span::styled("  ", Style::default()),
            Span::styled("█ █ ██▀", text_style),
            Span::styled(" ", Style::default()),
        ]),
    ];

    let paragraph = Paragraph::new(lines).alignment(Alignment::Right);
    frame.render_widget(paragraph, area);
}

/// Render just the mascot feet - call this AFTER rendering kanban to overlap the border
pub fn render_mascot_feet(frame: &mut Frame, area: Rect, shimmer_frame: u8, logo_size: LogoSize) {
    let feet_style = Style::default().fg(get_mascot_color(3, shimmer_frame));
    let border_style = Style::default().fg(Color::Cyan);

    // Feet characters with border line filling the gaps for seamless appearance
    // Original feet: "   ▀▀ ▀▀  " - replace spaces with ─ in border color
    let feet_line = match logo_size {
        LogoSize::Full => {
            // Full logo: 35 chars of border line + corner to align with mascot body above
            Line::from(vec![
                Span::styled("───", border_style),        // Leading border (was spaces)
                Span::styled("▀▀", feet_style),           // Left foot
                Span::styled("─", border_style),          // Gap between feet
                Span::styled("▀▀", feet_style),           // Right foot
                Span::styled("──", border_style),         // Trailing border (was spaces)
                Span::styled("──────────────────────────────────", border_style),  // 34 chars of line
                Span::styled("┐", border_style),          // Top-right corner
            ])
        }
        LogoSize::Medium => {
            // Medium logo: shorter border line to match KB text width
            // Feet pattern: "───▀▀─▀▀───" = 11 chars (one extra dash shifts feet left to align)
            // Then gap (2) + KB text width (7) + trailing (1) - 1 = 9 more
            // Total: 11 + 9 = 20 chars to match medium logo
            Line::from(vec![
                Span::styled("───", border_style),        // Leading border (was spaces)
                Span::styled("▀▀", feet_style),           // Left foot
                Span::styled("─", border_style),          // Gap between feet
                Span::styled("▀▀", feet_style),           // Right foot
                Span::styled("───", border_style),        // Trailing border (one extra dash to shift feet left)
                Span::styled("────────", border_style),   // 8 chars of line
                Span::styled("┐", border_style),          // Top-right corner
            ])
        }
        _ => return, // No feet for compact/minimal/none
    };

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

/// Maximum percentage of screen width the full logo (mascot + KANBLAM) should occupy
/// When exceeded, fall back to medium logo (mascot + KB)
const MAX_FULL_LOGO_WIDTH_PERCENT: f32 = 0.60;

/// Maximum percentage of screen width the medium logo (mascot + KB) should occupy
/// When exceeded, fall back to text-only "KANBLAM"
const MAX_MEDIUM_LOGO_WIDTH_PERCENT: f32 = 0.70;

/// Minimum terminal height (in lines) to show the full mascot logo
/// Below this threshold, show text-only "KANBLAM" to preserve vertical space
pub const MIN_HEIGHT_FOR_FULL_LOGO: u16 = 30;

/// Determine which logo size to show based on terminal dimensions
/// Returns:
/// - Full: when terminal is tall enough (>=30 lines) and full logo takes <= 60% width
/// - Medium: when terminal is tall enough and full logo would exceed 60% but medium is <= 70%
/// - Compact: otherwise (text-only "KANBLAM")
pub fn get_logo_size(terminal_width: u16, terminal_height: u16) -> LogoSize {
    // Height check: need at least 30 lines for mascot
    if terminal_height < MIN_HEIGHT_FOR_FULL_LOGO {
        return LogoSize::Compact;
    }

    // Width check for full logo (mascot + KANBLAM): should not exceed 60% of terminal width
    let full_logo_percent = FULL_LOGO_WIDTH as f32 / terminal_width as f32;
    if full_logo_percent <= MAX_FULL_LOGO_WIDTH_PERCENT {
        return LogoSize::Full;
    }

    // Width check for medium logo (mascot + KB): should not exceed 70% of terminal width
    let medium_logo_percent = MEDIUM_LOGO_WIDTH as f32 / terminal_width as f32;
    if medium_logo_percent <= MAX_MEDIUM_LOGO_WIDTH_PERCENT {
        return LogoSize::Medium;
    }

    // Fall back to compact text-only
    LogoSize::Compact
}

/// Check if we should show the full 4-line logo with mascot (any size - full or medium)
/// This is used to determine header height
pub fn should_show_full_logo(terminal_width: u16, terminal_height: u16) -> bool {
    matches!(get_logo_size(terminal_width, terminal_height), LogoSize::Full | LogoSize::Medium)
}
