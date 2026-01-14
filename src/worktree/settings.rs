//! Claude Code settings generation for isolated worktrees

use anyhow::Result;
use serde_json::{json, Value};
use std::path::PathBuf;
use uuid::Uuid;

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
                    "command": format!("kanclaude signal stop {}", task_id)
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
                    "command": format!("kanclaude signal stop {}", task_id)
                }]
            }]
        }
    });

    // If project has settings, merge them (project settings take precedence for most fields)
    if project_settings_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&project_settings_path) {
            if let Ok(project_settings) = serde_json::from_str::<Value>(&content) {
                // Merge specific fields from project settings
                if let Some(obj) = project_settings.as_object() {
                    for (key, value) in obj {
                        match key.as_str() {
                            // Keep our hooks, but merge with project hooks
                            "hooks" => {
                                if let (Some(our_hooks), Some(project_hooks)) = (
                                    settings["hooks"].as_object_mut(),
                                    value.as_object(),
                                ) {
                                    for (hook_name, hook_value) in project_hooks {
                                        // Append project hooks to our hooks (ours come first)
                                        if let Some(existing) = our_hooks.get_mut(hook_name) {
                                            if let (Some(existing_arr), Some(new_arr)) = (
                                                existing.as_array_mut(),
                                                hook_value.as_array(),
                                            ) {
                                                existing_arr.extend(new_arr.clone());
                                            }
                                        } else {
                                            our_hooks.insert(hook_name.clone(), hook_value.clone());
                                        }
                                    }
                                }
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
