use rodio::{Decoder, OutputStream, Sink};
use std::io::Cursor;
use std::thread;

/// Built-in notification sound (embedded as bytes)
/// Using a simple system beep if no custom sound is available
pub enum NotificationSound {
    Attention,
}

/// Play the attention notification sound
/// Plays asynchronously so it doesn't block the UI
pub fn play_attention_sound() {
    thread::spawn(|| {
        if let Err(e) = play_sound_internal() {
            // Silently ignore audio errors - notification is best-effort
            eprintln!("Audio notification failed: {}", e);
        }
    });
}

fn play_sound_internal() -> anyhow::Result<()> {
    // Try to get audio output stream
    let (_stream, stream_handle) = OutputStream::try_default()?;
    let sink = Sink::try_new(&stream_handle)?;

    // Try custom sound file first
    let sound_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("kanclaude")
        .join("sounds")
        .join("attention.mp3");

    if sound_path.exists() {
        let file = std::fs::File::open(&sound_path)?;
        let source = Decoder::new(std::io::BufReader::new(file))?;
        sink.append(source);
        sink.sleep_until_end();
    } else {
        // Fall back to system bell via terminal
        print!("\x07"); // ASCII BEL character
    }

    Ok(())
}

/// Play a test sound (for settings/configuration)
pub fn play_test_sound() {
    play_attention_sound();
}
