#![allow(dead_code)]

use anyhow::{anyhow, Result};
use arboard::Clipboard;
use image::{imageops::FilterType, GenericImageView, ImageBuffer, Pixel, RgbaImage};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use std::path::PathBuf;

/// Get image directory for storing pasted images
pub fn get_image_dir() -> Result<PathBuf> {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kanblam")
        .join("images");

    std::fs::create_dir_all(&data_dir)?;
    Ok(data_dir)
}

/// Check if clipboard contains an image
pub fn clipboard_has_image() -> bool {
    if let Ok(mut clipboard) = Clipboard::new() {
        clipboard.get_image().is_ok()
    } else {
        false
    }
}

/// Get image from clipboard and save to disk
/// Returns the path where the image was saved
pub fn paste_image_from_clipboard() -> Result<PathBuf> {
    let mut clipboard = Clipboard::new()
        .map_err(|e| anyhow!("Failed to access clipboard: {}", e))?;

    let img_data = clipboard.get_image()
        .map_err(|e| anyhow!("No image in clipboard: {}", e))?;

    // Convert to image buffer
    let img: RgbaImage = ImageBuffer::from_raw(
        img_data.width as u32,
        img_data.height as u32,
        img_data.bytes.into_owned(),
    ).ok_or_else(|| anyhow!("Failed to create image buffer"))?;

    // Generate unique filename with timestamp
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
    let filename = format!("paste_{}.png", timestamp);

    let image_dir = get_image_dir()?;
    let image_path = image_dir.join(&filename);

    // Save as PNG
    img.save(&image_path)
        .map_err(|e| anyhow!("Failed to save image: {}", e))?;

    Ok(image_path)
}

/// Configuration for ANSI image rendering
pub struct AnsiRenderConfig {
    /// Maximum width in characters
    pub max_width: u32,
    /// Maximum height in terminal rows (each row = 2 pixels via half-block)
    pub max_height: u32,
}

impl Default for AnsiRenderConfig {
    fn default() -> Self {
        Self {
            max_width: 32,
            max_height: 16,
        }
    }
}

/// Render an image file as ANSI art using half-block unicode characters.
/// Uses the ▀ (upper half block) character with foreground for top pixel
/// and background for bottom pixel, allowing 2 vertical pixels per character.
pub fn render_image_to_ansi(path: &PathBuf, config: &AnsiRenderConfig) -> Result<Vec<Line<'static>>> {
    let img = image::open(path).map_err(|e| anyhow!("Failed to open image: {}", e))?;

    // Calculate target dimensions maintaining aspect ratio
    let (orig_width, orig_height) = img.dimensions();

    // Target height in pixels (each terminal row = 2 pixels)
    let max_pixel_height = config.max_height * 2;

    // Calculate scale factors
    let width_scale = config.max_width as f32 / orig_width as f32;
    let height_scale = max_pixel_height as f32 / orig_height as f32;
    let scale = width_scale.min(height_scale).min(1.0); // Don't upscale

    let new_width = (orig_width as f32 * scale).round() as u32;
    let new_height = (orig_height as f32 * scale).round() as u32;

    // Ensure even height for half-block rendering
    let new_height = if new_height % 2 == 1 {
        new_height + 1
    } else {
        new_height
    };

    // Resize image
    let resized = img.resize_exact(new_width, new_height, FilterType::Triangle);
    let rgba = resized.to_rgba8();

    let mut lines = Vec::new();

    // Process pairs of rows (top pixel = foreground, bottom pixel = background)
    for y in (0..new_height).step_by(2) {
        let mut spans = Vec::new();

        for x in 0..new_width {
            let top_pixel = rgba.get_pixel(x, y);
            let bottom_pixel = if y + 1 < new_height {
                rgba.get_pixel(x, y + 1)
            } else {
                top_pixel
            };

            let top_color = pixel_to_color(top_pixel);
            let bottom_color = pixel_to_color(bottom_pixel);

            // Use upper half block: ▀
            // Foreground = top pixel, Background = bottom pixel
            let style = Style::default().fg(top_color).bg(bottom_color);
            spans.push(Span::styled("▀", style));
        }

        lines.push(Line::from(spans));
    }

    Ok(lines)
}

/// Convert an RGBA pixel to a ratatui Color
fn pixel_to_color(pixel: &image::Rgba<u8>) -> Color {
    let channels = pixel.channels();
    let r = channels[0];
    let g = channels[1];
    let b = channels[2];
    let a = channels[3];

    // Handle transparency by blending with a dark background
    if a < 255 {
        let alpha = a as f32 / 255.0;
        let bg = 30u8; // Dark background
        let r = (r as f32 * alpha + bg as f32 * (1.0 - alpha)) as u8;
        let g = (g as f32 * alpha + bg as f32 * (1.0 - alpha)) as u8;
        let b = (b as f32 * alpha + bg as f32 * (1.0 - alpha)) as u8;
        Color::Rgb(r, g, b)
    } else {
        Color::Rgb(r, g, b)
    }
}

/// Try to render an image, returning None on any error instead of failing.
/// This is useful for UI code that should gracefully degrade.
pub fn try_render_image_to_ansi(path: &PathBuf, config: &AnsiRenderConfig) -> Option<Vec<Line<'static>>> {
    render_image_to_ansi(path, config).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_image_dir() {
        let dir = get_image_dir();
        assert!(dir.is_ok());
    }

    #[test]
    fn test_pixel_to_color() {
        let pixel = image::Rgba([255, 128, 64, 255]);
        let color = pixel_to_color(&pixel);
        assert!(matches!(color, Color::Rgb(255, 128, 64)));
    }

    #[test]
    fn test_pixel_to_color_transparent() {
        let pixel = image::Rgba([255, 255, 255, 0]);
        let color = pixel_to_color(&pixel);
        // Fully transparent should blend to background (30, 30, 30)
        assert!(matches!(color, Color::Rgb(30, 30, 30)));
    }
}
