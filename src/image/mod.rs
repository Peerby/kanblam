use anyhow::{anyhow, Result};
use arboard::Clipboard;
use image::{ImageBuffer, RgbaImage};
use std::path::PathBuf;

/// Get image directory for storing pasted images
pub fn get_image_dir() -> Result<PathBuf> {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kanclaude")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_image_dir() {
        let dir = get_image_dir();
        assert!(dir.is_ok());
    }
}
