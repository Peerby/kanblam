use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

/// Current hook version - increment when hooks change
const HOOKS_VERSION: u32 = 3;

/// Install KanClaude hooks into a project's .claude/settings.json
pub fn install_hooks(project_dir: &Path) -> Result<()> {
    let claude_dir = project_dir.join(".claude");
    let settings_file = claude_dir.join("settings.json");

    // Create .claude directory if it doesn't exist
    std::fs::create_dir_all(&claude_dir)?;

    // Read existing settings or create new
    let mut settings: Value = if settings_file.exists() {
        let content = std::fs::read_to_string(&settings_file)?;
        serde_json::from_str(&content).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    // Get the kanclaude binary path
    let kanclaude_bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "kanclaude".to_string());

    // Define our hook commands (array of hook matchers, each with array of hooks)

    // Stop hook - fires when Claude completes a response
    let stop_hook = json!([{
        "hooks": [{
            "type": "command",
            "command": format!("{} hook-signal --event=stop", kanclaude_bin)
        }]
    }]);

    // SessionEnd hook - fires when session terminates
    let session_end_hook = json!([{
        "hooks": [{
            "type": "command",
            "command": format!("{} hook-signal --event=end", kanclaude_bin)
        }]
    }]);

    // Notification hook - fires when Claude needs input (permission prompts, idle)
    let notification_hook = json!([
        {
            "matcher": "permission_prompt",
            "hooks": [{
                "type": "command",
                "command": format!("{} hook-signal --event=needs-input --type=permission", kanclaude_bin)
            }]
        },
        {
            "matcher": "idle_prompt",
            "hooks": [{
                "type": "command",
                "command": format!("{} hook-signal --event=needs-input --type=idle", kanclaude_bin)
            }]
        }
    ]);

    // UserPromptSubmit hook - fires when user provides text input
    let user_prompt_hook = json!([{
        "hooks": [{
            "type": "command",
            "command": format!("{} hook-signal --event=input-provided", kanclaude_bin)
        }]
    }]);

    // PreToolUse hook - fires when Claude is about to use any tool
    // This indicates Claude is actively working (moved past any permission prompts)
    let pre_tool_hook = json!([{
        "hooks": [{
            "type": "command",
            "command": format!("{} hook-signal --event=working", kanclaude_bin)
        }]
    }]);

    // Merge hooks into settings
    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));

    let hooks_obj = hooks.as_object_mut().unwrap();

    // Always update all hooks (overwrites existing)
    hooks_obj.insert("Stop".to_string(), stop_hook);
    hooks_obj.insert("SessionEnd".to_string(), session_end_hook);
    hooks_obj.insert("Notification".to_string(), notification_hook);
    hooks_obj.insert("UserPromptSubmit".to_string(), user_prompt_hook);
    hooks_obj.insert("PreToolUse".to_string(), pre_tool_hook);

    // Store version in settings for future upgrade detection
    settings.as_object_mut().unwrap().insert(
        "_kanclaude_hooks_version".to_string(),
        json!(HOOKS_VERSION)
    );

    // Write updated settings
    let content = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_file, content)?;

    Ok(())
}

/// Check if hooks are installed AND up to date for a project
pub fn hooks_installed(project_dir: &Path) -> bool {
    let settings_file = project_dir.join(".claude").join("settings.json");

    if !settings_file.exists() {
        return false;
    }

    if let Ok(content) = std::fs::read_to_string(&settings_file) {
        if let Ok(settings) = serde_json::from_str::<Value>(&content) {
            // Check version - if missing or outdated, return false
            let installed_version = settings
                .get("_kanclaude_hooks_version")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;

            if installed_version < HOOKS_VERSION {
                return false;
            }

            // Also verify required hooks exist
            if let Some(hooks) = settings.get("hooks") {
                let has_stop = hooks.get("Stop").is_some();
                let has_notification = hooks.get("Notification").is_some();
                let has_user_prompt = hooks.get("UserPromptSubmit").is_some();
                return has_stop && has_notification && has_user_prompt;
            }
        }
    }

    false
}

/// Uninstall KanClaude hooks from a project
pub fn uninstall_hooks(project_dir: &Path) -> Result<()> {
    let settings_file = project_dir.join(".claude").join("settings.json");

    if !settings_file.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&settings_file)?;
    let mut settings: Value = serde_json::from_str(&content)?;

    if let Some(hooks) = settings.get_mut("hooks") {
        if let Some(hooks_obj) = hooks.as_object_mut() {
            hooks_obj.remove("Stop");
            hooks_obj.remove("SessionEnd");
        }
    }

    let content = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_file, content)?;

    Ok(())
}
