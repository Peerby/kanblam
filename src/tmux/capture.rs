use anyhow::Result;
use std::process::Command;

/// Capture the visible output from a tmux pane
pub fn capture_pane_output(pane_id: &str, lines: Option<i32>) -> Result<String> {
    let lines_arg = lines.map(|n| format!("-{}", n)).unwrap_or_default();

    let mut args = vec!["capture-pane", "-t", pane_id, "-p"];

    if !lines_arg.is_empty() {
        args.push("-S");
        args.push(&lines_arg);
    }

    let output = Command::new("tmux").args(&args).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("Failed to capture pane: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Capture only the recent output (last N lines)
pub fn capture_recent_output(pane_id: &str, num_lines: u32) -> Result<String> {
    capture_pane_output(pane_id, Some(num_lines as i32))
}

/// Get a hash of the pane content for change detection
pub fn get_content_hash(pane_id: &str) -> Result<u64> {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;

    let content = capture_recent_output(pane_id, 20)?;

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    Ok(hasher.finish())
}
