//! Ultrathink styling - rainbow colors for "ultrathink" keyword
//! Based on Claude Code's visual treatment of extended thinking triggers

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

/// Rainbow colors for ultrathink display (RGB values from Claude Code)
/// These cycle through the spectrum for each character
pub const RAINBOW_COLORS: &[Color] = &[
    Color::Rgb(235, 95, 87),   // red
    Color::Rgb(245, 139, 87),  // orange
    Color::Rgb(250, 195, 95),  // yellow
    Color::Rgb(145, 200, 130), // green
    Color::Rgb(130, 170, 220), // blue
    Color::Rgb(155, 130, 200), // indigo
    Color::Rgb(200, 130, 180), // violet
];

/// ANSI fallback colors for terminals without true color support
#[allow(dead_code)]
pub const RAINBOW_COLORS_ANSI: &[Color] = &[
    Color::Red,
    Color::LightRed,
    Color::Yellow,
    Color::Green,
    Color::Cyan,
    Color::Blue,
    Color::Magenta,
];

/// Check if text contains an ultrathink trigger
/// Matches: "ultrathink", "think ultra hard", "think ultrahard" (case-insensitive)
pub fn contains_ultrathink(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("ultrathink")
        || lower.contains("think ultra hard")
        || lower.contains("think ultrahard")
}

/// Create rainbow-colored spans for "ultrathink" text
/// Each character gets a different color from the rainbow spectrum
pub fn rainbow_spans(text: &str) -> Vec<Span<'static>> {
    text.chars()
        .enumerate()
        .map(|(i, c)| {
            let color = RAINBOW_COLORS[i % RAINBOW_COLORS.len()];
            Span::styled(c.to_string(), Style::default().fg(color))
        })
        .collect()
}

/// Create a rainbow-styled "ULTRATHINK" indicator
#[allow(dead_code)]
pub fn ultrathink_indicator() -> Line<'static> {
    Line::from(rainbow_spans("ULTRATHINK"))
}

/// Style a line of text, highlighting any "ultrathink" occurrences with rainbow colors
/// Returns styled spans for the entire line
pub fn style_line_with_ultrathink(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let lower = text.to_lowercase();
    let mut spans = Vec::new();
    let mut last_end = 0;

    // Find all "ultrathink" occurrences (case-insensitive)
    let mut search_start = 0;
    while let Some(start) = lower[search_start..].find("ultrathink") {
        let abs_start = search_start + start;
        let abs_end = abs_start + "ultrathink".len();

        // Add text before the match
        if abs_start > last_end {
            spans.push(Span::styled(text[last_end..abs_start].to_string(), base_style));
        }

        // Add rainbow-styled ultrathink
        let matched_text = &text[abs_start..abs_end];
        spans.extend(rainbow_spans(matched_text));

        last_end = abs_end;
        search_start = abs_end;
    }

    // Add remaining text after last match
    if last_end < text.len() {
        spans.push(Span::styled(text[last_end..].to_string(), base_style));
    }

    // If no matches found, return the whole text with base style
    if spans.is_empty() {
        spans.push(Span::styled(text.to_string(), base_style));
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains_ultrathink() {
        assert!(contains_ultrathink("ultrathink"));
        assert!(contains_ultrathink("ULTRATHINK"));
        assert!(contains_ultrathink("UltraThink"));
        assert!(contains_ultrathink("think ultra hard"));
        assert!(contains_ultrathink("think ultrahard"));
        assert!(contains_ultrathink("Please ultrathink about this"));
        assert!(!contains_ultrathink("think hard"));
        assert!(!contains_ultrathink("ultra"));
    }

    #[test]
    fn test_rainbow_spans_length() {
        let spans = rainbow_spans("ultrathink");
        assert_eq!(spans.len(), 10); // One span per character
    }

    #[test]
    fn test_style_line_with_ultrathink() {
        let spans = style_line_with_ultrathink("Please ultrathink about this", Style::default());
        // Should have: "Please " + 10 rainbow chars + " about this"
        assert_eq!(spans.len(), 12);
    }
}
