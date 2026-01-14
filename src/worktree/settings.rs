//! Claude Code settings generation for isolated worktrees

use anyhow::Result;
use serde_json::{json, Value};
use std::path::PathBuf;
use uuid::Uuid;

/// Get the path to Claude's global config file
fn get_claude_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude.json")
}

/// Pre-trust a worktree directory in Claude's global config
/// This prevents the "Do you trust this folder?" dialog
pub fn pre_trust_worktree(worktree_path: &PathBuf) -> Result<()> {
    let config_path = get_claude_config_path();

    // Read existing config or create new one
    let mut config: Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    // Ensure projects object exists
    if config.get("projects").is_none() {
        config["projects"] = json!({});
    }

    // Get the absolute path as the key
    let path_key = worktree_path
        .canonicalize()
        .unwrap_or_else(|_| worktree_path.clone())
        .to_string_lossy()
        .to_string();

    // Add or update the project entry with trust accepted
    if let Some(projects) = config["projects"].as_object_mut() {
        let project_entry = projects.entry(&path_key).or_insert_with(|| json!({}));
        if let Some(obj) = project_entry.as_object_mut() {
            obj.insert("hasTrustDialogAccepted".to_string(), json!(true));
            obj.insert("hasCompletedProjectOnboarding".to_string(), json!(true));
            // Initialize other required fields if not present
            if !obj.contains_key("allowedTools") {
                obj.insert("allowedTools".to_string(), json!([]));
            }
            if !obj.contains_key("ignorePatterns") {
                obj.insert("ignorePatterns".to_string(), json!([]));
            }
        }
    }

    // Write back the config
    let content = serde_json::to_string_pretty(&config)?;
    std::fs::write(&config_path, content)?;

    Ok(())
}

/// Remove trust entry for a worktree from Claude's global config
pub fn remove_worktree_trust(worktree_path: &PathBuf) -> Result<()> {
    let config_path = get_claude_config_path();

    if !config_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let mut config: Value = serde_json::from_str(&content)?;

    // Get the absolute path as the key
    let path_key = worktree_path
        .canonicalize()
        .unwrap_or_else(|_| worktree_path.clone())
        .to_string_lossy()
        .to_string();

    // Remove the project entry
    if let Some(projects) = config["projects"].as_object_mut() {
        projects.remove(&path_key);
    }

    // Write back the config
    let content = serde_json::to_string_pretty(&config)?;
    std::fs::write(&config_path, content)?;

    Ok(())
}

/// Set up Claude Code settings in a worktree
///
/// Creates `.claude/settings.json` with:
/// - Hooks to notify KanClaude of session events
/// - Auto-accept for common tools (Bash, Read, Edit, Write)
/// - Co-authored-by attribution
pub fn setup_claude_settings(
    worktree_path: &PathBuf,
    task_id: Uuid,
    _project_dir: &PathBuf, // For future: copy project settings
) -> Result<()> {
    let claude_dir = worktree_path.join(".claude");
    std::fs::create_dir_all(&claude_dir)?;

    // Get the absolute path to the kanclaude binary
    let kanclaude_bin = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("kanclaude"))
        .to_string_lossy()
        .to_string();

    // Build settings JSON with correct Claude Code format
    // Permissions: use tool names without parentheses for "allow all"
    // Hooks: use new matcher format
    let settings = json!({
        "permissions": {
            "allow": [
                "Bash",
                "Read",
                "Edit",
                "Write",
                "Grep",
                "Glob"
            ],
            "deny": []
        },
        "includeCoAuthoredBy": true,
        "hooks": {
            "Stop": [{
                "hooks": [{
                    "type": "command",
                    "command": format!("{} signal stop {}", kanclaude_bin, task_id)
                }]
            }]
        }
    });

    let settings_path = claude_dir.join("settings.json");
    let content = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, content)?;

    // Also create a .claudeignore if it doesn't exist, to prevent Claude
    // from wandering outside the worktree
    let claudeignore_path = worktree_path.join(".claudeignore");
    if !claudeignore_path.exists() {
        // Ignore parent directories that might be symlinked
        std::fs::write(&claudeignore_path, "# KanClaude worktree - stay within this directory\n")?;
    }

    Ok(())
}

/// Merge project's existing Claude settings with worktree settings
pub fn merge_with_project_settings(
    worktree_path: &PathBuf,
    project_dir: &PathBuf,
    task_id: Uuid,
) -> Result<()> {
    let project_settings_path = project_dir.join(".claude").join("settings.json");

    // Get the absolute path to the kanclaude binary
    let kanclaude_bin = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("kanclaude"))
        .to_string_lossy()
        .to_string();

    // Start with our base settings (correct Claude Code format)
    let mut settings = json!({
        "permissions": {
            "allow": [
                "Bash",
                "Read",
                "Edit",
                "Write",
                "Grep",
                "Glob"
            ],
            "deny": []
        },
        "includeCoAuthoredBy": true,
        "hooks": {
            "Stop": [{
                "hooks": [{
                    "type": "command",
                    "command": format!("{} signal stop {}", kanclaude_bin, task_id)
                }]
            }],
            "SessionEnd": [{
                "hooks": [{
                    "type": "command",
                    "command": format!("{} signal end {}", kanclaude_bin, task_id)
                }]
            }],
            "Notification": [
                {
                    "matcher": "permission_prompt",
                    "hooks": [{
                        "type": "command",
                        "command": format!("{} signal needs-input {}", kanclaude_bin, task_id)
                    }]
                },
                {
                    // idle_prompt fires after 60+ seconds of Claude being idle
                    // Could be: (1) waiting for user to answer a question, or (2) finished
                    // Send needs-input - the handler ignores it if already in Review (case 2)
                    "matcher": "idle_prompt",
                    "hooks": [{
                        "type": "command",
                        "command": format!("{} signal needs-input {}", kanclaude_bin, task_id)
                    }]
                }
            ],
            "PreToolUse": [{
                "hooks": [{
                    "type": "command",
                    "command": format!("{} signal working {}", kanclaude_bin, task_id)
                }]
            }],
            "UserPromptSubmit": [{
                "hooks": [{
                    "type": "command",
                    "command": format!("{} signal input-provided {}", kanclaude_bin, task_id)
                }]
            }]
        }
    });

    // If project has settings, merge ONLY non-hook settings
    // IMPORTANT: Do NOT merge hooks - worktrees have their own task-specific hooks.
    // Merging project hooks would add duplicate hooks with wrong task IDs.
    if project_settings_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&project_settings_path) {
            if let Ok(project_settings) = serde_json::from_str::<Value>(&content) {
                // Merge specific fields from project settings
                if let Some(obj) = project_settings.as_object() {
                    for (key, value) in obj {
                        match key.as_str() {
                            // NEVER merge hooks - worktree has its own task-specific hooks
                            "hooks" => {
                                // Skip - do not merge project hooks into worktree
                            }
                            // Merge permissions (union of allow, intersection of deny)
                            "permissions" => {
                                // For now, keep our permissive permissions
                                // Project can restrict via deny list
                                if let Some(perms) = value.get("deny") {
                                    if let Some(our_perms) = settings["permissions"].as_object_mut() {
                                        our_perms.insert("deny".to_string(), perms.clone());
                                    }
                                }
                            }
                            // Copy other settings directly
                            _ => {
                                if let Some(obj) = settings.as_object_mut() {
                                    obj.insert(key.clone(), value.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Write the merged settings
    let claude_dir = worktree_path.join(".claude");
    std::fs::create_dir_all(&claude_dir)?;
    let settings_path = claude_dir.join("settings.json");
    let content = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, content)?;

    Ok(())
}
