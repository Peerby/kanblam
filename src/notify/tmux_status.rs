#![allow(dead_code)]

use std::process::Command;

const TMUX_OPTION_NAME: &str = "@kanblam_attention";

/// Set the attention indicator in tmux status bar
/// This sets a user option that can be used in the status-right config:
/// set -g status-right '#{?#{!=:#{@kanblam_attention},},#[fg=green]● ,}'
pub fn set_attention_indicator(project_name: &str) {
    let _ = Command::new("tmux")
        .args(["set-option", "-g", TMUX_OPTION_NAME, project_name])
        .output();
}

/// Clear the attention indicator from tmux status bar
pub fn clear_attention_indicator() {
    let _ = Command::new("tmux")
        .args(["set-option", "-g", TMUX_OPTION_NAME, ""])
        .output();
}

/// Check if attention indicator is set
pub fn has_attention_indicator() -> bool {
    if let Ok(output) = Command::new("tmux")
        .args(["show-option", "-gv", TMUX_OPTION_NAME])
        .output()
    {
        let value = String::from_utf8_lossy(&output.stdout);
        !value.trim().is_empty()
    } else {
        false
    }
}

/// Get the current attention project name (if any)
pub fn get_attention_project() -> Option<String> {
    if let Ok(output) = Command::new("tmux")
        .args(["show-option", "-gv", TMUX_OPTION_NAME])
        .output()
    {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    } else {
        None
    }
}

/// Setup hint - returns the tmux config line to add to .tmux.conf
pub fn get_tmux_config_hint() -> &'static str {
    r#"# Kanblam attention indicator - add to status-right:
# set -g status-right '#{?#{!=:#{@kanblam_attention},},#[fg=green]● #{@kanblam_attention} ,}'"#
}
