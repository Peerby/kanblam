//! Git worktree commands for task isolation

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use std::process::Command;
use uuid::Uuid;

/// Information about a worktree
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub head: String,
}

/// Get the worktree path for a specific project and task
/// Worktrees are stored in {project_dir}/worktrees/task-{task_id}/
pub fn get_worktree_path(project_dir: &PathBuf, task_id: Uuid) -> PathBuf {
    project_dir
        .join("worktrees")
        .join(format!("task-{}", task_id))
}

/// Create a new worktree for a task
///
/// Creates a worktree at `{project_dir}/.worktrees/task-{task-id}/`
/// on branch `claude/{task-id}` based on the current HEAD.
pub fn create_worktree(
    project_dir: &PathBuf,
    task_id: Uuid,
) -> Result<PathBuf> {
    let worktree_path = get_worktree_path(project_dir, task_id);
    let branch_name = format!("claude/{}", task_id);

    // Ensure parent directory exists
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Check if worktree already exists
    if worktree_path.exists() {
        // Verify it's a valid worktree
        let git_dir = worktree_path.join(".git");
        if git_dir.exists() {
            return Ok(worktree_path);
        }
        // Invalid state - remove and recreate
        std::fs::remove_dir_all(&worktree_path)?;
    }

    // Check if branch already exists (from a crashed session)
    let branch_exists = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "--verify", &branch_name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if branch_exists {
        // Branch exists, just add the worktree pointing to it
        let output = Command::new("git")
            .current_dir(project_dir)
            .args([
                "worktree",
                "add",
                &worktree_path.to_string_lossy(),
                &branch_name,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to add worktree: {}", stderr));
        }
    } else {
        // Create new branch and worktree
        let output = Command::new("git")
            .current_dir(project_dir)
            .args([
                "worktree",
                "add",
                "-b",
                &branch_name,
                &worktree_path.to_string_lossy(),
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to create worktree: {}", stderr));
        }
    }

    Ok(worktree_path)
}

/// Remove a worktree
pub fn remove_worktree(project_dir: &PathBuf, worktree_path: &PathBuf) -> Result<()> {
    // Use --force to remove even with uncommitted changes
    let output = Command::new("git")
        .current_dir(project_dir)
        .args([
            "worktree",
            "remove",
            "--force",
            &worktree_path.to_string_lossy(),
        ])
        .output()?;

    if !output.status.success() {
        // Try manual cleanup if git worktree remove fails
        if worktree_path.exists() {
            std::fs::remove_dir_all(worktree_path)?;
        }
        // Prune worktree list
        let _ = Command::new("git")
            .current_dir(project_dir)
            .args(["worktree", "prune"])
            .output();
    }

    Ok(())
}

/// Check if a worktree has any uncommitted changes (staged or unstaged)
/// Returns true if there are changes, false if clean
pub fn has_uncommitted_changes(worktree_path: &PathBuf) -> Result<bool> {
    let status_output = Command::new("git")
        .current_dir(worktree_path)
        .args(["status", "--porcelain"])
        .output()?;

    let status = String::from_utf8_lossy(&status_output.stdout);
    Ok(!status.trim().is_empty())
}

/// Commit any uncommitted changes in a worktree
/// Returns true if changes were committed, false if nothing to commit
pub fn commit_worktree_changes(worktree_path: &PathBuf, task_id: Uuid) -> Result<bool> {
    // Debug logging to file
    let log_path = std::path::PathBuf::from("/tmp/kanblam-apply.log");
    let log = |msg: &str| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = writeln!(f, "[{}] {}", chrono::Local::now().format("%H:%M:%S"), msg);
        }
    };

    log(&format!("=== commit_worktree_changes START: task={} ===", task_id));
    log(&format!("worktree_path={:?}", worktree_path));

    // Check if there are any changes (staged or unstaged)
    let status_output = Command::new("git")
        .current_dir(worktree_path)
        .args(["status", "--porcelain"])
        .output()?;

    let status = String::from_utf8_lossy(&status_output.stdout);
    log(&format!("git status output: '{}'", status.trim()));
    if status.trim().is_empty() {
        // Nothing to commit
        log("No changes to commit");
        return Ok(false);
    }

    log(&format!("Found changes, committing..."));

    // Stage all changes
    let add_output = Command::new("git")
        .current_dir(worktree_path)
        .args(["add", "-A"])
        .output()?;

    if !add_output.status.success() {
        let stderr = String::from_utf8_lossy(&add_output.stderr);
        return Err(anyhow!("Failed to stage changes: {}", stderr));
    }

    // Commit
    let commit_msg = format!("Task {} final changes", task_id);
    let commit_output = Command::new("git")
        .current_dir(worktree_path)
        .args(["commit", "-m", &commit_msg])
        .output()?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        // Check if it's just "nothing to commit"
        if stderr.contains("nothing to commit") ||
           String::from_utf8_lossy(&commit_output.stdout).contains("nothing to commit") {
            log("Nothing to commit (after staging)");
            return Ok(false);
        }
        log(&format!("Commit FAILED: {}", stderr));
        return Err(anyhow!("Failed to commit changes: {}", stderr));
    }

    log("Commit SUCCESS");
    Ok(true)
}

/// Check if a task branch has any changes compared to main
pub fn has_changes_to_merge(project_dir: &PathBuf, task_id: Uuid) -> Result<bool> {
    let branch_name = format!("claude/{}", task_id);

    // Get the merge base
    let merge_base_output = Command::new("git")
        .current_dir(project_dir)
        .args(["merge-base", "HEAD", &branch_name])
        .output()?;

    if !merge_base_output.status.success() {
        // Branch might not exist
        return Ok(false);
    }

    let merge_base = String::from_utf8_lossy(&merge_base_output.stdout).trim().to_string();

    // Check if branch has commits beyond merge base
    let log_output = Command::new("git")
        .current_dir(project_dir)
        .args(["log", "--oneline", &format!("{}..{}", merge_base, branch_name)])
        .output()?;

    let log = String::from_utf8_lossy(&log_output.stdout);
    Ok(!log.trim().is_empty())
}

/// Commit any uncommitted changes on main branch
/// Returns Ok(true) if changes were committed, Ok(false) if nothing to commit
/// This should be called before checking needs_rebase to ensure the worktree
/// properly detects it needs to integrate with main's latest state
pub fn commit_main_changes(project_dir: &PathBuf) -> Result<bool> {
    // Check if there are local changes
    let status_check = Command::new("git")
        .current_dir(project_dir)
        .args(["status", "--porcelain"])
        .output()?;

    let status_output = String::from_utf8_lossy(&status_check.stdout);
    if status_output.trim().is_empty() {
        return Ok(false); // Nothing to commit
    }

    // Stage all changes
    let add_output = Command::new("git")
        .current_dir(project_dir)
        .args(["add", "-A"])
        .output()?;

    if !add_output.status.success() {
        let stderr = String::from_utf8_lossy(&add_output.stderr);
        return Err(anyhow!("Failed to stage changes on main: {}", stderr));
    }

    // Commit with a WIP message
    let commit_output = Command::new("git")
        .current_dir(project_dir)
        .args(["commit", "-m", "WIP: uncommitted changes (auto-committed before task merge)"])
        .output()?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        // Check if it's just "nothing to commit"
        if stderr.contains("nothing to commit") ||
           String::from_utf8_lossy(&commit_output.stdout).contains("nothing to commit") {
            return Ok(false);
        }
        return Err(anyhow!("Failed to commit changes on main: {}", stderr));
    }

    Ok(true)
}

/// Commit applied changes from a task with a descriptive message
/// Returns Ok(true) if changes were committed, Ok(false) if nothing to commit
pub fn commit_applied_changes(project_dir: &PathBuf, task_title: &str, task_id: Uuid) -> Result<bool> {
    // Check if there are STAGED changes (applied task changes are staged via --3way)
    // Don't use git add -A as that would also commit user's unstaged edits
    let has_staged = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map(|s| !s.success())  // exit 1 means there ARE differences
        .unwrap_or(false);

    if !has_staged {
        return Ok(false); // Nothing staged to commit
    }

    // Commit only staged changes (task's applied changes)
    let commit_msg = format!("Merge task {} from Claude session\n\nTask: {}", task_id, task_title);
    let commit_output = Command::new("git")
        .current_dir(project_dir)
        .args(["commit", "-m", &commit_msg])
        .output()?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        // Check if it's just "nothing to commit"
        if stderr.contains("nothing to commit") ||
           String::from_utf8_lossy(&commit_output.stdout).contains("nothing to commit") {
            return Ok(false);
        }
        return Err(anyhow!("Failed to commit changes: {}", stderr));
    }

    Ok(true)
}

/// Merge a task branch into the base branch (squash merge)
/// Requires clean working directory - call commit_main_changes first if needed
pub fn merge_branch(project_dir: &PathBuf, task_id: Uuid) -> Result<()> {
    let branch_name = format!("claude/{}", task_id);

    // Verify working directory is clean
    // Caller should have called commit_main_changes() first
    let status_check = Command::new("git")
        .current_dir(project_dir)
        .args(["status", "--porcelain"])
        .output()?;

    if !String::from_utf8_lossy(&status_check.stdout).trim().is_empty() {
        return Err(anyhow!(
            "Working directory is not clean. Commit or stash changes before merging."
        ));
    }

    // Perform squash merge
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["merge", "--squash", &branch_name])
        .output()
        .context("Failed to run merge")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Abort the failed merge
        let _ = Command::new("git")
            .current_dir(project_dir)
            .args(["merge", "--abort"])
            .output();
        return Err(anyhow!(
            "Merge failed (conflicts?): {}. Resolve in {} and commit manually.",
            stderr,
            project_dir.display()
        ));
    }

    // Reset .kanblam/ and .claude/ to main's version - never merge these from worktrees
    // .kanblam/ = task state, .claude/ = hooks config (both are infrastructure, not code)
    let _ = Command::new("git")
        .current_dir(project_dir)
        .args(["checkout", "HEAD", "--", ".kanblam", ".claude"])
        .output();

    // Check if there are staged changes to commit
    let status_output = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", "--cached", "--quiet"])
        .output()?;

    if !status_output.status.success() {
        // There are staged changes, commit them
        let commit_msg = format!("Merge task {} from Claude session", task_id);
        let output = Command::new("git")
            .current_dir(project_dir)
            .args(["commit", "-m", &commit_msg])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to commit merge: {}", stderr));
        }
    }

    Ok(())
}

/// Delete a task branch
pub fn delete_branch(project_dir: &PathBuf, task_id: Uuid) -> Result<()> {
    let branch_name = format!("claude/{}", task_id);

    // Use -D to force delete even if not merged
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["branch", "-D", &branch_name])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Don't fail if branch doesn't exist
        if !stderr.contains("not found") {
            return Err(anyhow!("Failed to delete branch: {}", stderr));
        }
    }

    Ok(())
}

/// Safely restore a stash by commit SHA - uses apply+drop instead of pop for reliability.
/// The SHA is stable even if other stashes are created, unlike stash@{N} indices.
/// Returns error if restore fails so we don't silently lose data.
fn safe_stash_restore(project_dir: &PathBuf, stash_sha: &Option<String>) -> Result<()> {
    if let Some(ref sha) = stash_sha {
        // First, verify the stash exists
        let verify = Command::new("git")
            .current_dir(project_dir)
            .args(["rev-parse", "--verify", &format!("{}^{{commit}}", sha)])
            .output();

        match verify {
            Ok(output) if !output.status.success() => {
                return Err(anyhow!(
                    "Stash {} no longer exists. Your uncommitted changes may have been lost.",
                    sha
                ));
            }
            Err(e) => {
                return Err(anyhow!(
                    "Could not verify stash {}: {}. Your changes may still be in the stash - run 'git stash list' to check.",
                    sha, e
                ));
            }
            Ok(_) => {} // Stash exists, continue
        }

        // Apply the stash (doesn't remove it)
        let apply_result = Command::new("git")
            .current_dir(project_dir)
            .args(["stash", "apply", sha])
            .output();

        match apply_result {
            Ok(output) if output.status.success() => {
                // Successfully applied, now drop it
                let drop_result = Command::new("git")
                    .current_dir(project_dir)
                    .args(["stash", "drop", sha])
                    .output();

                // Even if drop fails, the changes are restored - just warn
                if let Ok(drop_output) = drop_result {
                    if !drop_output.status.success() {
                        // Changes restored but stash not dropped - not critical
                        // User can manually drop it with 'git stash drop'
                        eprintln!("Warning: Stash applied but could not be dropped. Run 'git stash drop {}' manually.", sha);
                    }
                }
                Ok(())
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(anyhow!(
                    "Could not restore uncommitted changes from stash '{}': {}. Run 'git stash apply {}' manually to recover.",
                    sha, stderr.trim(), sha
                ))
            }
            Err(e) => {
                Err(anyhow!(
                    "Could not run stash apply for '{}': {}. Your changes may still be in the stash.",
                    sha, e
                ))
            }
        }
    } else {
        Ok(())
    }
}

/// Get the commit SHA for a stash (stable identifier unlike stash@{N} indices)
fn get_stash_sha(project_dir: &PathBuf) -> Result<String> {
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "stash@{0}"])
        .output()?;

    if !output.status.success() {
        return Err(anyhow!("No stash found"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get the path where we save the applied patch for surgical reversal
fn get_patch_file_path(task_id: Uuid) -> PathBuf {
    let kanclaude_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".kanclaude")
        .join("patches");
    kanclaude_dir.join(format!("{}.patch", task_id))
}

/// Parse file paths from unified diff patch content.
/// Extracts paths from "diff --git a/path b/path" header lines.
fn parse_patch_files(patch_content: &[u8]) -> Vec<String> {
    let content = String::from_utf8_lossy(patch_content);
    let mut files = Vec::new();

    for line in content.lines() {
        if line.starts_with("diff --git a/") {
            // Format: "diff --git a/path/to/file b/path/to/file"
            if let Some(path_part) = line.strip_prefix("diff --git a/") {
                if let Some(space_idx) = path_part.find(" b/") {
                    let file_path = &path_part[..space_idx];
                    files.push(file_path.to_string());
                }
            }
        }
    }

    files
}

/// Clean up after accepting an applied task.
/// Just removes the patch file - stash was already popped immediately after apply.
pub fn cleanup_applied_state(task_id: Uuid) {
    let patch_path = get_patch_file_path(task_id);
    let _ = std::fs::remove_file(&patch_path);
}

/// Apply a task's changes to the main worktree (for testing)
/// This stashes any existing changes, applies the diff, and tracks the stash for unapply
/// Returns the stash ref if there were local changes that were stashed
pub fn apply_task_changes(project_dir: &PathBuf, task_id: Uuid) -> Result<Option<String>> {
    let branch_name = format!("claude/{}", task_id);

    // Debug logging to file (TUI covers stderr)
    let log_path = std::path::PathBuf::from("/tmp/kanblam-apply.log");
    let mut log = |msg: &str| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = writeln!(f, "[{}] {}", chrono::Local::now().format("%H:%M:%S"), msg);
        }
    };

    log(&format!("=== apply_task_changes START: task={} ===", task_id));
    log(&format!("project_dir={:?}, branch={}", project_dir, branch_name));

    // Check for corrupted state (unmerged files from previous failed operation)
    let unmerged_check = Command::new("git")
        .current_dir(project_dir)
        .args(["ls-files", "-u"])
        .output()?;

    if !unmerged_check.stdout.is_empty() {
        let unmerged_files = String::from_utf8_lossy(&unmerged_check.stdout);
        log(&format!("ERROR: Unmerged files detected: {}", unmerged_files));
        return Err(anyhow!(
            "Repository has unmerged files from a previous conflict. \
             Please resolve manually or run: git reset --hard HEAD"
        ));
    }

    // Check for conflict markers in tracked files (another sign of corrupted state)
    let conflict_check = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", "--check"])
        .output();

    if let Ok(output) = conflict_check {
        if !output.status.success() {
            let conflicts = String::from_utf8_lossy(&output.stdout);
            if conflicts.contains("conflict") || conflicts.contains("<<<<<<") {
                log(&format!("ERROR: Conflict markers detected: {}", conflicts));
                return Err(anyhow!(
                    "Repository has conflict markers from a previous failed merge. \
                     Please resolve manually before applying."
                ));
            }
        }
    }

    // Check if there are local changes that need to be stashed
    // Only check for TRACKED file changes - untracked files don't need stashing
    // (git stash doesn't stash untracked files by default anyway)
    let status_check = Command::new("git")
        .current_dir(project_dir)
        .args(["status", "--porcelain"])
        .output()?;

    // Filter out untracked files (??) - only stash tracked changes
    let status_output = String::from_utf8_lossy(&status_check.stdout);
    let has_tracked_changes = status_output
        .lines()
        .any(|line| !line.starts_with("??"));
    log(&format!("has_tracked_changes={}", has_tracked_changes));
    let mut stash_ref = None;

    // Stash tracked changes if any (untracked files don't need stashing)
    if has_tracked_changes {
        let stash_output = Command::new("git")
            .current_dir(project_dir)
            .args(["stash", "push", "-m", &format!("kanclaude: before applying task {}", task_id)])
            .output()?;

        if !stash_output.status.success() {
            let stderr = String::from_utf8_lossy(&stash_output.stderr);
            return Err(anyhow!("Failed to stash local changes: {}", stderr));
        }

        // Get the stash SHA (stable identifier unlike stash@{N} indices)
        // This ensures we can restore the right stash even if other stashes are created later
        match get_stash_sha(project_dir) {
            Ok(sha) => {
                stash_ref = Some(sha);
                log(&format!("stashed changes, stash_sha={:?}", stash_ref));
            }
            Err(e) => {
                log(&format!("WARNING: Could not get stash SHA: {}", e));
                // Fall back to stash@{0} but this is less reliable
                stash_ref = Some("stash@{0}".to_string());
            }
        }
    }

    // Find the merge-base (common ancestor) between HEAD and the task branch
    // This ensures we only apply the task's changes, not revert changes made to main
    let merge_base_output = Command::new("git")
        .current_dir(project_dir)
        .args(["merge-base", "HEAD", &branch_name])
        .output()?;

    if !merge_base_output.status.success() {
        // Restore stash if we made one - fail if we can't restore
        safe_stash_restore(project_dir, &stash_ref)?;
        let stderr = String::from_utf8_lossy(&merge_base_output.stderr);
        return Err(anyhow!("Failed to find merge-base: {}", stderr));
    }

    let merge_base = String::from_utf8_lossy(&merge_base_output.stdout).trim().to_string();
    log(&format!("merge_base={}", merge_base));

    // Get the diff from merge-base to the task branch (only the task's changes)
    // Exclude .kanblam/ (task state) and .claude/ (hooks config) to avoid conflicts
    let diff_output = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", &merge_base, &branch_name, "--", ".", ":!.kanblam", ":!.claude"])
        .output()?;

    if !diff_output.status.success() {
        // Restore stash if we made one - fail if we can't restore
        safe_stash_restore(project_dir, &stash_ref)?;
        let stderr = String::from_utf8_lossy(&diff_output.stderr);
        return Err(anyhow!("Failed to get diff: {}", stderr));
    }

    log(&format!("diff size={} bytes", diff_output.stdout.len()));
    if diff_output.stdout.is_empty() {
        log("WARNING: diff is empty! Nothing to apply.");
        // Restore stash if we made one - fail if we can't restore
        safe_stash_restore(project_dir, &stash_ref)?;
        return Err(anyhow!("Nothing to apply - task changes are already in main."));
    }

    // Save the patch file for surgical reversal later
    let patch_path = get_patch_file_path(task_id);
    if let Some(parent) = patch_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&patch_path, &diff_output.stdout)?;
    log(&format!("saved patch to {:?}", patch_path));

    // Apply the diff (capture stderr so we can log it)
    let mut apply_cmd = Command::new("git")
        .current_dir(project_dir)
        .args(["apply", "--3way"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    // Write diff to stdin and explicitly close it (drop) so git knows we're done
    {
        use std::io::Write;
        let stdin = apply_cmd.stdin.take().expect("stdin was piped");
        let mut stdin = std::io::BufWriter::new(stdin);
        stdin.write_all(&diff_output.stdout)?;
        stdin.flush()?;
        // stdin is dropped here, closing the pipe
    }

    let apply_output = apply_cmd.wait_with_output()?;
    log(&format!("git apply exit code: {:?}", apply_output.status.code()));

    let stdout = String::from_utf8_lossy(&apply_output.stdout);
    let stderr = String::from_utf8_lossy(&apply_output.stderr);
    if !stdout.is_empty() {
        log(&format!("git apply stdout: {}", stdout));
    }
    if !stderr.is_empty() {
        log(&format!("git apply stderr: {}", stderr));
    }

    if !apply_output.status.success() {
        log("FAILED to apply changes - resetting working tree");
        // Reset working tree to clean state (removes conflict markers)
        let _ = Command::new("git")
            .current_dir(project_dir)
            .args(["reset", "--hard", "HEAD"])
            .output();

        // Restore stash if we made one - use SHA-based restore for reliability
        if let Err(e) = safe_stash_restore(project_dir, &stash_ref) {
            log(&format!("CRITICAL: Failed to restore stash: {}", e));
            return Err(anyhow!(
                "Apply failed AND could not restore your uncommitted changes! {}",
                e
            ));
        }
        if stash_ref.is_some() {
            log("Restored stashed changes successfully");
        }
        // Include conflict details in error for display in modal
        // Format: APPLY_CONFLICT:<conflict_output>
        let conflict_output = if !stderr.is_empty() {
            stderr.to_string()
        } else if !stdout.is_empty() {
            stdout.to_string()
        } else {
            "Failed to apply changes. There may be conflicts.".to_string()
        };
        return Err(anyhow!("APPLY_CONFLICT:{}", conflict_output));
    }

    // After --3way, files may be in "unmerged" state even if resolved
    // Stage them to complete the 3-way merge and allow clean unapply later
    let unmerged_check = Command::new("git")
        .current_dir(project_dir)
        .args(["ls-files", "-u"])
        .output();

    if let Ok(output) = unmerged_check {
        if !output.stdout.is_empty() {
            log("Found unmerged files after --3way, staging them...");
            // Stage all files to resolve the unmerged state
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["add", "-u"])  // Only stage modified tracked files
                .output();
        }
    }

    // Immediately restore stashed changes - no deferred tracking needed
    // If this conflicts, user deals with it now (better than later)
    if let Some(ref sha) = stash_ref {
        let pop_result = Command::new("git")
            .current_dir(project_dir)
            .args(["stash", "pop"])
            .output();

        match pop_result {
            Ok(output) if output.status.success() => {
                log("Restored stashed changes on top of applied patch");
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                log(&format!("Stash pop had conflicts: {}", stderr));
                // Return error with stash SHA so caller can offer conflict resolution options
                // The stash is still there (not dropped on conflict), user's changes are safe
                return Err(anyhow!("STASH_CONFLICT:{}", sha));
            }
            Err(e) => {
                log(&format!("Stash pop command failed: {}", e));
                return Ok(Some(format!("STASH_ERROR: {}", e)));
            }
        }
    }

    log("SUCCESS - changes applied");
    Ok(None) // No stash tracking needed - already popped
}

/// Result of attempting to unapply task changes
#[derive(Debug)]
pub enum UnapplyResult {
    /// Surgical reversal succeeded
    Success,
    /// Surgical reversal failed, user confirmation needed for destructive reset
    NeedsConfirmation(String),
}

/// Unapply task changes from the main worktree using surgical patch reversal.
/// No stash handling needed - stash was already popped immediately after apply.
/// Returns Success if the patch was cleanly reversed, NeedsConfirmation if destructive reset is needed.
pub fn unapply_task_changes(project_dir: &PathBuf, task_id: Uuid) -> Result<UnapplyResult> {
    let patch_path = get_patch_file_path(task_id);

    // If we have a saved patch, try surgical reversal
    if patch_path.exists() {
        let patch_content = std::fs::read(&patch_path)?;

        // Check for unstaged changes - these would interfere with patch reversal
        let has_unstaged = Command::new("git")
            .current_dir(project_dir)
            .args(["diff", "--quiet"])
            .status()
            .map(|s| !s.success())
            .unwrap_or(false);

        // If there are unstaged changes, stash them while keeping staged (applied) changes
        let did_stash = if has_unstaged {
            let stash_result = Command::new("git")
                .current_dir(project_dir)
                .args(["stash", "push", "--keep-index", "-m", "kanclaude: unapply temp stash"])
                .output()?;
            stash_result.status.success()
        } else {
            false
        };

        // Try to reverse the patch
        let mut apply_cmd = Command::new("git")
            .current_dir(project_dir)
            .args(["apply", "-R", "--3way"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        {
            use std::io::Write;
            let stdin = apply_cmd.stdin.take().expect("stdin was piped");
            let mut stdin = std::io::BufWriter::new(stdin);
            stdin.write_all(&patch_content)?;
            stdin.flush()?;
        }

        let output = apply_cmd.wait_with_output()?;

        if output.status.success() {
            // Surgically unstage only the task's files.
            // git apply -R only modifies the working directory, but apply_task_changes
            // may have staged changes with git add -u. We need to clear those stale
            // index entries, but only for the files the task touched.
            let task_files = parse_patch_files(&patch_content);
            if !task_files.is_empty() {
                let mut reset_cmd = Command::new("git");
                reset_cmd
                    .current_dir(project_dir)
                    .arg("reset")
                    .arg("HEAD")
                    .arg("--");
                for file in &task_files {
                    reset_cmd.arg(file);
                }
                let _ = reset_cmd.output();
            }

            // Clean up the patch file
            let _ = std::fs::remove_file(&patch_path);

            // Restore user's unstaged changes if we stashed them
            if did_stash {
                let _ = Command::new("git")
                    .current_dir(project_dir)
                    .args(["stash", "pop"])
                    .output();
            }

            return Ok(UnapplyResult::Success);
        }

        // Surgical reversal failed - restore stash before returning
        if did_stash {
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["stash", "pop"])
                .output();
        }

        // Need user confirmation for destructive reset
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Ok(UnapplyResult::NeedsConfirmation(format!(
            "Surgical patch reversal failed: {}",
            stderr.trim()
        )));
    }

    // No patch file - need user confirmation for destructive reset
    Ok(UnapplyResult::NeedsConfirmation(
        "No saved patch found. Use destructive reset to unapply?".to_string()
    ))
}

/// Surgical unapply for stash conflict recovery.
/// Only resets the specific files that the task patch modified, leaving other changes intact.
/// After this, the stash can be popped cleanly because task changes are gone.
/// Returns Ok(files_reset) on success, Err if patch file not found or reset failed.
pub fn surgical_unapply_for_stash_conflict(project_dir: &PathBuf, task_id: Uuid) -> Result<Vec<String>> {
    let patch_path = get_patch_file_path(task_id);

    if !patch_path.exists() {
        return Err(anyhow!("No patch file found for task {}. Cannot perform surgical unapply.", task_id));
    }

    let patch_content = std::fs::read(&patch_path)?;
    let files_to_reset = parse_patch_files(&patch_content);

    if files_to_reset.is_empty() {
        return Err(anyhow!("Could not parse any files from patch"));
    }

    // Reset each file to HEAD (removes task changes and clears any conflict state)
    for file_path in &files_to_reset {
        let checkout_result = Command::new("git")
            .current_dir(project_dir)
            .args(["checkout", "HEAD", "--", file_path])
            .output()?;

        if !checkout_result.status.success() {
            let stderr = String::from_utf8_lossy(&checkout_result.stderr);
            return Err(anyhow!("Failed to reset file '{}': {}", file_path, stderr));
        }
    }

    // Clean up the patch file
    let _ = std::fs::remove_file(&patch_path);

    Ok(files_to_reset)
}

/// Force unapply using destructive reset (only call after user confirmation!).
/// No stash handling needed - stash was already popped immediately after apply.
pub fn force_unapply_task_changes(project_dir: &PathBuf, task_id: Uuid) -> Result<()> {
    // Discard all changes (staged and unstaged) by resetting to HEAD
    // Use reset --hard instead of checkout -- . because checkout fails on empty repos
    let reset_output = Command::new("git")
        .current_dir(project_dir)
        .args(["reset", "--hard", "HEAD"])
        .output()?;

    if !reset_output.status.success() {
        let stderr = String::from_utf8_lossy(&reset_output.stderr);
        return Err(anyhow!("Failed to discard changes: {}", stderr));
    }

    // Clean up the patch file if it exists
    let patch_path = get_patch_file_path(task_id);
    let _ = std::fs::remove_file(&patch_path);

    Ok(())
}

/// List all worktrees for a project
pub fn list_worktrees(project_dir: &PathBuf) -> Result<Vec<WorktreeInfo>> {
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["worktree", "list", "--porcelain"])
        .output()?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current_worktree: Option<WorktreeInfo> = None;

    for line in stdout.lines() {
        if line.starts_with("worktree ") {
            // Start of a new worktree entry
            if let Some(wt) = current_worktree.take() {
                worktrees.push(wt);
            }
            current_worktree = Some(WorktreeInfo {
                path: PathBuf::from(line.strip_prefix("worktree ").unwrap_or("")),
                branch: String::new(),
                head: String::new(),
            });
        } else if line.starts_with("HEAD ") {
            if let Some(ref mut wt) = current_worktree {
                wt.head = line.strip_prefix("HEAD ").unwrap_or("").to_string();
            }
        } else if line.starts_with("branch ") {
            if let Some(ref mut wt) = current_worktree {
                wt.branch = line
                    .strip_prefix("branch refs/heads/")
                    .unwrap_or("")
                    .to_string();
            }
        }
    }

    if let Some(wt) = current_worktree {
        worktrees.push(wt);
    }

    Ok(worktrees)
}

/// Check if project directory is a git repository
pub fn is_git_repo(project_dir: &PathBuf) -> bool {
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "--git-dir"])
        .output();

    output.map(|o| o.status.success()).unwrap_or(false)
}

/// Check if a git repository has at least one commit
pub fn has_commits(project_dir: &PathBuf) -> bool {
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "HEAD"])
        .output();

    output.map(|o| o.status.success()).unwrap_or(false)
}

/// Initialize a git repository in the given directory
pub fn init_repo(project_dir: &PathBuf) -> Result<()> {
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["init"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to initialize git: {}", stderr));
    }

    Ok(())
}

/// Create an initial commit in a git repository
pub fn create_initial_commit(project_dir: &PathBuf) -> Result<()> {
    // Ensure .gitignore has KanBlam entries before the initial commit
    ensure_gitignore_has_kanblam_entries(project_dir)?;

    // Add all files
    let add_output = Command::new("git")
        .current_dir(project_dir)
        .args(["add", "-A"])
        .output()?;

    if !add_output.status.success() {
        let stderr = String::from_utf8_lossy(&add_output.stderr);
        return Err(anyhow!("Failed to stage files: {}", stderr));
    }

    // Create initial commit (use --allow-empty in case there are no files)
    let commit_output = Command::new("git")
        .current_dir(project_dir)
        .args(["commit", "--allow-empty", "-m", "Initial commit"])
        .output()?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        return Err(anyhow!("Failed to create initial commit: {}", stderr));
    }

    Ok(())
}

/// Required entries for KanBlam to work properly with git
const KANBLAM_GITIGNORE_ENTRIES: &[&str] = &[".claude/", "worktrees/"];

/// Check if .gitignore is missing any KanBlam-required entries
pub fn gitignore_missing_kanblam_entries(project_dir: &PathBuf) -> Vec<String> {
    let gitignore_path = project_dir.join(".gitignore");
    let existing_content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();

    let mut missing = Vec::new();
    for entry in KANBLAM_GITIGNORE_ENTRIES {
        // Check if the entry exists (with or without trailing newline, commented out doesn't count)
        let has_entry = existing_content.lines().any(|line| {
            let trimmed = line.trim();
            trimmed == *entry || trimmed == entry.trim_end_matches('/')
        });
        if !has_entry {
            missing.push(entry.to_string());
        }
    }
    missing
}

/// Ensure .gitignore contains KanBlam-required entries (creates file if missing)
pub fn ensure_gitignore_has_kanblam_entries(project_dir: &PathBuf) -> Result<()> {
    let gitignore_path = project_dir.join(".gitignore");
    let missing = gitignore_missing_kanblam_entries(project_dir);

    if missing.is_empty() {
        return Ok(());
    }

    // Read existing content or start fresh
    let existing_content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();

    // Check if there's already a KanBlam section we can append to
    const KANBLAM_HEADER: &str = "# KanBlam";
    let lines: Vec<&str> = existing_content.lines().collect();

    // Find the KanBlam section if it exists
    let kanblam_section_idx = lines.iter().position(|line| line.starts_with(KANBLAM_HEADER));

    let new_content = if let Some(header_idx) = kanblam_section_idx {
        // Find where to insert: after the header and any existing KanBlam entries
        // We insert after consecutive non-empty, non-comment lines following the header
        let mut insert_idx = header_idx + 1;
        while insert_idx < lines.len() {
            let line = lines[insert_idx].trim();
            // Stop if we hit an empty line or another comment (new section)
            if line.is_empty() || line.starts_with('#') {
                break;
            }
            insert_idx += 1;
        }

        // Build new content by inserting missing entries at the right position
        let mut result = String::new();
        for (i, line) in lines.iter().enumerate() {
            result.push_str(line);
            result.push('\n');
            if i + 1 == insert_idx {
                // Insert missing entries here
                for entry in &missing {
                    result.push_str(entry);
                    result.push('\n');
                }
            }
        }
        // Handle edge case: insert at the very end
        if insert_idx >= lines.len() {
            for entry in &missing {
                result.push_str(entry);
                result.push('\n');
            }
        }
        result
    } else {
        // No existing KanBlam section - add a new one at the end
        let mut new_content = existing_content.clone();

        // Ensure there's a newline before our section if file has content
        if !new_content.is_empty() && !new_content.ends_with('\n') {
            new_content.push('\n');
        }
        if !new_content.is_empty() {
            new_content.push('\n');
        }
        new_content.push_str("# KanBlam (Claude Code task manager)\n");
        for entry in &missing {
            new_content.push_str(entry);
            new_content.push('\n');
        }
        new_content
    };

    std::fs::write(&gitignore_path, new_content)?;
    Ok(())
}

/// Get the diff between main/master and a task branch
pub fn get_task_diff(project_dir: &PathBuf, task_id: Uuid) -> Result<String> {
    let branch_name = format!("claude/{}", task_id);

    // Try to find the base branch (main or master)
    let base_branch = find_base_branch(project_dir)?;

    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", &format!("{}..{}", base_branch, branch_name)])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to get diff: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Find the base branch (main or master)
fn find_base_branch(project_dir: &PathBuf) -> Result<String> {
    // Check for main first
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "--verify", "main"])
        .output()?;

    if output.status.success() {
        return Ok("main".to_string());
    }

    // Try master
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "--verify", "master"])
        .output()?;

    if output.status.success() {
        return Ok("master".to_string());
    }

    // Fall back to HEAD
    Ok("HEAD".to_string())
}

/// Check if a task branch has already been squash-merged to main.
///
/// SAFETY: This function is EXTREMELY conservative. It only returns true when
/// we can PROVE the branch was squash-merged:
/// 1. The branch EXISTS (we can verify its state)
/// 2. The branch HAS commits beyond the merge-base (work was done)
/// 3. There is ZERO diff between branch and main (content is in main)
///
/// This combination means: work was done on the branch, and that work's content
/// is now in main (via squash merge). Without commits, we can't prove anything
/// was merged - it might just be a fresh branch.
///
/// If ANY check fails or errors, returns false to be safe.
pub fn is_branch_merged(project_dir: &PathBuf, task_id: Uuid) -> Result<bool> {
    let branch_name = format!("claude/{}", task_id);

    // SAFETY CHECK 1: Branch MUST exist - if not, we can't verify anything
    let branch_exists = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "--verify", &branch_name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !branch_exists {
        // Branch doesn't exist - return false (safe default)
        return Ok(false);
    }

    // SAFETY CHECK 2: Branch MUST have commits (work was done)
    // If there are no commits on the branch, we can't prove anything was merged
    // git log HEAD..branch shows commits in branch but not in HEAD (main)
    let branch_commits = Command::new("git")
        .current_dir(project_dir)
        .args(["log", "--oneline", &format!("HEAD..{}", branch_name)])
        .output()
        .context("Failed to check for branch commits")?;

    if !branch_commits.status.success() {
        return Ok(false);
    }

    let commits_output = String::from_utf8_lossy(&branch_commits.stdout);
    if commits_output.trim().is_empty() {
        // No commits on branch - can't prove anything was merged
        // This could be a fresh branch that never had work done
        return Ok(false);
    }

    // SAFETY CHECK 3: Content must match main (squash merge completed)
    // If the branch has commits BUT the diff is empty, the content was squash-merged
    let diff_check = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", "--quiet", "HEAD", &branch_name])
        .status()
        .context("Failed to check diff")?;

    if !diff_check.success() {
        // There's a diff - content not yet in main, NOT merged
        return Ok(false);
    }

    // All checks passed:
    // - Branch exists
    // - Branch has commits (work was done)
    // - No diff with main (content is in main via squash merge)
    Ok(true)
}

/// Check if task branch is behind main (needs rebase before merge)
pub fn needs_rebase(project_dir: &PathBuf, task_id: Uuid) -> Result<bool> {
    let branch_name = format!("claude/{}", task_id);

    // Get merge base between main and task branch
    let merge_base = Command::new("git")
        .current_dir(project_dir)
        .args(["merge-base", "HEAD", &branch_name])
        .output()
        .context("Failed to get merge base")?;

    if !merge_base.status.success() {
        // Branch might not exist or no common ancestor
        return Ok(false);
    }

    // Get current main HEAD
    let main_head = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("Failed to get HEAD")?;

    let merge_base_hash = String::from_utf8_lossy(&merge_base.stdout).trim().to_string();
    let main_head_hash = String::from_utf8_lossy(&main_head.stdout).trim().to_string();

    // If merge-base != main HEAD, branch is behind
    Ok(merge_base_hash != main_head_hash)
}

/// Try to perform an automatic rebase without Claude.
/// Returns Ok(true) if rebase succeeded (no conflicts).
/// Returns Ok(false) if rebase failed due to conflicts (aborted automatically).
/// Returns Err if something unexpected went wrong.
pub fn try_fast_rebase(worktree_path: &PathBuf, project_dir: &PathBuf) -> Result<bool> {
    // SAFETY: Check if a rebase is already in progress (from a previous failed attempt)
    if is_rebase_in_progress(worktree_path) {
        // Abort any existing rebase first
        let _ = Command::new("git")
            .current_dir(worktree_path)
            .args(["rebase", "--abort"])
            .output();
    }

    // First, fetch to make sure we have latest main
    // (ignore errors - might not have remote configured)
    let _ = Command::new("git")
        .current_dir(project_dir)
        .args(["fetch", "origin", "main"])
        .output();

    // Get the main branch HEAD to rebase onto
    let main_head = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("Failed to get main HEAD")?;

    if !main_head.status.success() {
        return Err(anyhow!("Failed to get main HEAD"));
    }

    let main_ref = String::from_utf8_lossy(&main_head.stdout).trim().to_string();

    // SAFETY: Record original HEAD so we can restore if something goes wrong
    let original_head = Command::new("git")
        .current_dir(worktree_path)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    // Try to rebase the worktree branch onto main
    let rebase_result = Command::new("git")
        .current_dir(worktree_path)
        .args(["rebase", &main_ref])
        .output()
        .context("Failed to run rebase")?;

    if rebase_result.status.success() {
        // Rebase succeeded without git conflicts, but we must verify the build
        // Git rebase only catches line-level conflicts, not semantic conflicts
        // (e.g., code using old APIs/structures that have since changed)

        let build_result = Command::new("cargo")
            .current_dir(worktree_path)
            .args(["build"])
            .output();

        match build_result {
            Ok(output) if output.status.success() => {
                // Build succeeded - safe to proceed with merge
                return Ok(true);
            }
            Ok(output) => {
                // Build failed - semantic conflicts exist
                // Restore to pre-rebase state and let Claude handle it
                if let Some(ref orig) = original_head {
                    let _ = Command::new("git")
                        .current_dir(worktree_path)
                        .args(["reset", "--hard", orig])
                        .output();
                }
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("Fast rebase succeeded but build failed - falling back to Claude: {}",
                    stderr.lines().take(5).collect::<Vec<_>>().join("\n"));
                return Ok(false); // Fall back to Claude
            }
            Err(_) => {
                // Couldn't run cargo build - proceed anyway but warn
                // This could happen in non-Rust projects
                return Ok(true);
            }
        }
    }

    // Rebase failed - check if it's due to conflicts
    let stderr = String::from_utf8_lossy(&rebase_result.stderr);

    // Abort the failed rebase to restore clean state
    let abort_result = Command::new("git")
        .current_dir(worktree_path)
        .args(["rebase", "--abort"])
        .output();

    // Check if abort succeeded (both execution and exit code)
    let abort_ok = match &abort_result {
        Ok(output) => output.status.success(),
        Err(_) => false,
    };

    if !abort_ok {
        // Abort failed - try to restore to original state
        if let Some(ref orig) = original_head {
            let reset_result = Command::new("git")
                .current_dir(worktree_path)
                .args(["reset", "--hard", orig])
                .output();

            if reset_result.is_err() || !reset_result.unwrap().status.success() {
                // CRITICAL: Could not restore worktree state
                return Err(anyhow!(
                    "Rebase failed and could not restore worktree. Manual intervention needed in: {}",
                    worktree_path.display()
                ));
            }
        } else {
            return Err(anyhow!(
                "Rebase failed and abort failed. Manual intervention needed in: {}",
                worktree_path.display()
            ));
        }
    }

    // Check if it was a conflict (expected) vs other error
    if stderr.contains("CONFLICT") || stderr.contains("could not apply") ||
       stderr.contains("Resolve all conflicts") || stderr.contains("merge conflict") {
        // Conflicts detected - need Claude to resolve
        Ok(false)
    } else if stderr.contains("nothing to do") || stderr.contains("up to date") ||
              stderr.contains("is up to date") {
        // Already up to date
        Ok(true)
    } else {
        // Some other error - but we've cleaned up, so let Claude try
        // This is safer than returning an error that might block the merge
        Ok(false)
    }
}

/// Verify that the task branch has been rebased onto main
/// Returns true if the branch is now on top of main (or equal)
pub fn verify_rebase_success(project_dir: &PathBuf, task_id: Uuid) -> Result<bool> {
    let branch_name = format!("claude/{}", task_id);

    // Get task branch HEAD
    let branch_head = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", &branch_name])
        .output()
        .context("Failed to get branch HEAD")?;

    if !branch_head.status.success() {
        return Ok(false);
    }

    // Check if main is an ancestor of the task branch
    // (means task branch is on top of main)
    let is_ancestor = Command::new("git")
        .current_dir(project_dir)
        .args(["merge-base", "--is-ancestor", "HEAD", &branch_name])
        .status()
        .context("Failed to check ancestry")?;

    Ok(is_ancestor.success())
}

/// Generate a prompt for Claude to integrate task changes with latest main
pub fn generate_rebase_prompt(main_branch: &str) -> String {
    format!(r#"CRITICAL INTEGRATION TASK: Your task branch is behind the main branch. You must integrate your changes with the latest main WITHOUT losing any work.

CONTEXT:
- Your branch contains work that was done on an OLDER version of the codebase
- The main branch has progressed with new features, refactors, and bug fixes
- You must preserve YOUR work while also keeping ALL changes from main
- This is NOT just a mechanical rebase - you may need to adapt your changes to work with the new codebase

STEP 1 - UNDERSTAND THE DIVERGENCE:
First, see what has changed on main since your branch diverged:
```
git log --oneline HEAD..{0}
git diff HEAD...{0} --stat
```

STEP 2 - ATTEMPT REBASE:
```
git fetch origin {0} 2>/dev/null || true
git rebase {0}
```

STEP 3 - IF CONFLICTS OCCUR:
For each conflict:
1. Read BOTH versions carefully - understand what each side was trying to do
2. The goal is to KEEP BOTH: your task's changes AND main's changes
3. If main added new fields/functions your code doesn't know about, ADD them
4. If main refactored something your code uses, ADAPT your code to the new structure
5. Resolve the conflict, then: `git add <file>` and `git rebase --continue`

STEP 4 - VERIFY BUILD:
After rebase completes, verify the code compiles:
```
cargo build 2>&1 | head -50
```

If build fails:
- Read the errors carefully
- The errors likely indicate semantic conflicts (your code uses old APIs/structures)
- Fix each error by adapting your code to work with main's new structure
- Commit fixes: `git add -A && git commit -m "Fix integration with latest main"`

STEP 5 - FINAL VERIFICATION:
```
git log --oneline -5
cargo build
```

IMPORTANT PRINCIPLES:
- NEVER discard work from either branch - integrate both
- If unsure how to merge conflicting approaches, prefer main's structure but keep your functionality
- The goal is: main's latest code + your task's feature/fix working together

When complete, say "Integration complete - build verified".
If you cannot resolve issues, explain what's blocking you."#, main_branch)
}

/// Generate a prompt for Claude to prepare task changes for applying to main worktree
/// This is similar to rebase prompt but emphasizes the goal is to test changes in main
pub fn generate_apply_prompt(main_branch: &str) -> String {
    format!(r#"PREPARE FOR APPLY: Your task branch has diverged from main. To apply your changes to the main worktree for testing, we first need to integrate your branch with the latest main.

CONTEXT:
- The user wants to TEST your task's changes in the main worktree before accepting
- Your branch is based on an older version of main, so a direct apply would fail with conflicts
- You must rebase your branch onto main to make the changes compatible
- After this integration, your changes will be applied to main for testing

STEP 1 - UNDERSTAND THE DIVERGENCE:
```
git log --oneline HEAD..{0}
git diff HEAD...{0} --stat
```

STEP 2 - REBASE ONTO MAIN:
```
git fetch origin {0} 2>/dev/null || true
git rebase {0}
```

STEP 3 - IF CONFLICTS OCCUR:
For each conflict:
1. Read BOTH versions carefully
2. Keep YOUR task's changes while adapting to main's new structure
3. If main added new fields/functions, ADD them
4. If main refactored something, ADAPT your code
5. Resolve: `git add <file>` and `git rebase --continue`

STEP 4 - VERIFY BUILD:
```
cargo build 2>&1 | head -50
```

Fix any build errors by adapting your code to work with main's structure.

STEP 5 - FINAL CHECK:
```
git log --oneline -5
cargo build
```

When complete, say "Ready for apply - build verified".
After you finish, your changes will be applied to the main worktree for the user to test."#, main_branch)
}

/// Generate a prompt for Claude to resolve stash conflicts in the main worktree
pub fn generate_stash_conflict_prompt(stash_sha: &str) -> String {
    format!(r#"STASH CONFLICT RESOLUTION: There are merge conflict markers in the working directory that need to be resolved.

WHAT HAPPENED:
1. A task's changes were successfully applied to this main worktree
2. The user had uncommitted changes that were stashed before the apply
3. When restoring the user's stash, git found conflicts between the user's changes and the task's changes
4. The conflict markers (<<<<<<, =======, >>>>>>>) are now in the working directory

YOUR JOB:
1. Find all files with conflict markers
2. Resolve each conflict by combining BOTH sets of changes appropriately:
   - "Updated upstream" or "HEAD" = the task's changes (keep these - they're the feature being applied)
   - "Stashed changes" = the user's uncommitted work (preserve this intent too if possible)
3. After resolving, verify the code compiles

STEP 1 - FIND CONFLICTS:
```
git diff --check 2>/dev/null || grep -r "<<<<<<< " . --include="*.rs" --include="*.ts" --include="*.js" --include="*.tsx" --include="*.jsx" 2>/dev/null | head -20
```

STEP 2 - FOR EACH CONFLICTING FILE:
1. Read the file to understand both sides of the conflict
2. Edit to resolve - keep functionality from BOTH sides where possible
3. Remove ALL conflict markers (the <<<<<<, =======, >>>>>>> lines)
4. Run `git add <file>` to mark it as resolved

STEP 3 - VERIFY BUILD:
```
cargo build 2>&1 | head -50
```

STEP 4 - CONFIRM RESOLUTION:
```
git diff --check  # Should show no conflict markers
git status        # Should show NO "Unmerged paths" - all conflicts must be git-added
cargo build       # Should succeed
```

IMPORTANT:
- Do NOT commit the changes - leave them as uncommitted modifications
- Do NOT run git stash commands - the stash ({}) is preserved for the user
- If you cannot resolve a conflict sensibly, prefer the task's changes (Updated upstream/HEAD side)

When complete, say "Conflicts resolved - build verified"."#, stash_sha)
}

/// Save all current uncommitted changes as a patch file for surgical reversal
/// This captures the combined state: task changes + any conflict resolution edits
pub fn save_current_changes_as_patch(project_dir: &PathBuf, task_id: Uuid) -> Result<()> {
    let patch_path = get_patch_file_path(task_id);

    // Get diff of all uncommitted changes relative to HEAD
    let diff_output = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", "HEAD"])
        .output()?;

    if !diff_output.status.success() {
        let stderr = String::from_utf8_lossy(&diff_output.stderr);
        return Err(anyhow!("Failed to get diff: {}", stderr));
    }

    // Ensure parent directory exists
    if let Some(parent) = patch_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Save the combined patch (overwrites any existing patch)
    std::fs::write(&patch_path, &diff_output.stdout)?;

    Ok(())
}

/// Check if a rebase is currently in progress in the worktree
pub fn is_rebase_in_progress(worktree_path: &PathBuf) -> bool {
    let rebase_merge = worktree_path.join(".git/rebase-merge");
    let rebase_apply = worktree_path.join(".git/rebase-apply");
    // In worktrees, .git is a file pointing to the actual git dir
    let git_file = worktree_path.join(".git");

    if git_file.is_file() {
        // Read the gitdir path from the .git file
        if let Ok(content) = std::fs::read_to_string(&git_file) {
            if let Some(gitdir) = content.strip_prefix("gitdir: ") {
                let gitdir = PathBuf::from(gitdir.trim());
                return gitdir.join("rebase-merge").exists() || gitdir.join("rebase-apply").exists();
            }
        }
    }

    rebase_merge.exists() || rebase_apply.exists()
}

/// Abort a rebase in progress
pub fn abort_rebase(worktree_path: &PathBuf) -> Result<()> {
    let output = Command::new("git")
        .current_dir(worktree_path)
        .args(["rebase", "--abort"])
        .output()
        .context("Failed to abort rebase")?;

    if !output.status.success() {
        anyhow::bail!("Failed to abort rebase: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(())
}

/// Git status information for a worktree
#[derive(Debug, Clone, Default)]
pub struct WorktreeGitStatus {
    /// Number of lines added (insertions)
    pub additions: usize,
    /// Number of lines deleted
    pub deletions: usize,
    /// Number of files changed
    pub files_changed: usize,
    /// Number of commits ahead of main
    pub commits_ahead: usize,
    /// Number of commits behind main
    pub commits_behind: usize,
}

/// Get git status (additions, deletions, commits ahead/behind) for a worktree
pub fn get_worktree_git_status(project_dir: &PathBuf, task_id: Uuid) -> Result<WorktreeGitStatus> {
    let branch_name = format!("claude/{}", task_id);
    let mut status = WorktreeGitStatus::default();

    // Get merge base between main and task branch
    let merge_base_output = Command::new("git")
        .current_dir(project_dir)
        .args(["merge-base", "HEAD", &branch_name])
        .output()
        .context("Failed to get merge base")?;

    if !merge_base_output.status.success() {
        // Branch might not exist or no common ancestor
        return Ok(status);
    }

    let merge_base = String::from_utf8_lossy(&merge_base_output.stdout).trim().to_string();

    // Get diff stats (additions/deletions) from merge base to branch
    let diff_stat_output = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", "--shortstat", &format!("{}..{}", merge_base, branch_name)])
        .output()
        .context("Failed to get diff stats")?;

    if diff_stat_output.status.success() {
        let stat_line = String::from_utf8_lossy(&diff_stat_output.stdout);
        // Parse output like: " 3 files changed, 45 insertions(+), 12 deletions(-)"
        for part in stat_line.split(',') {
            let part = part.trim();
            if part.contains("file") {
                if let Some(num) = part.split_whitespace().next() {
                    status.files_changed = num.parse().unwrap_or(0);
                }
            } else if part.contains("insertion") {
                if let Some(num) = part.split_whitespace().next() {
                    status.additions = num.parse().unwrap_or(0);
                }
            } else if part.contains("deletion") {
                if let Some(num) = part.split_whitespace().next() {
                    status.deletions = num.parse().unwrap_or(0);
                }
            }
        }
    }

    // Get commits ahead (branch commits not in main)
    let ahead_output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-list", "--count", &format!("HEAD..{}", branch_name)])
        .output()
        .context("Failed to count commits ahead")?;

    if ahead_output.status.success() {
        let count = String::from_utf8_lossy(&ahead_output.stdout).trim().to_string();
        status.commits_ahead = count.parse().unwrap_or(0);
    }

    // Get commits behind (main commits not in branch)
    let behind_output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-list", "--count", &format!("{}..HEAD", branch_name)])
        .output()
        .context("Failed to count commits behind")?;

    if behind_output.status.success() {
        let count = String::from_utf8_lossy(&behind_output.stdout).trim().to_string();
        status.commits_behind = count.parse().unwrap_or(0);
    }

    Ok(status)
}

/// Update worktree by rebasing onto latest main (without merging back to main).
/// This preserves the task's changes while getting updates from main.
/// Returns Ok(true) if rebase succeeded, Ok(false) if conflicts need resolution.
pub fn update_worktree_to_main(worktree_path: &PathBuf, project_dir: &PathBuf) -> Result<bool> {
    // Use the same fast rebase logic, but don't proceed to merge
    try_fast_rebase(worktree_path, project_dir)
}

/// Information about a changed file in a worktree
#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    pub additions: usize,
    pub deletions: usize,
    pub is_new: bool,
    pub is_deleted: bool,
    pub is_renamed: bool,
}

/// Get list of changed files with their stats for a worktree
pub fn get_worktree_changed_files(project_dir: &PathBuf, task_id: Uuid) -> Result<Vec<ChangedFile>> {
    let branch_name = format!("claude/{}", task_id);
    let mut files = Vec::new();

    // Get merge base between main and task branch
    let merge_base_output = Command::new("git")
        .current_dir(project_dir)
        .args(["merge-base", "HEAD", &branch_name])
        .output()
        .context("Failed to get merge base")?;

    if !merge_base_output.status.success() {
        return Ok(files);
    }

    let merge_base = String::from_utf8_lossy(&merge_base_output.stdout).trim().to_string();

    // Get numstat for detailed file changes
    let numstat_output = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", "--numstat", &format!("{}..{}", merge_base, branch_name)])
        .output()
        .context("Failed to get diff numstat")?;

    if !numstat_output.status.success() {
        return Ok(files);
    }

    // Also get name-status for detecting new/deleted/renamed files
    let status_output = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", "--name-status", &format!("{}..{}", merge_base, branch_name)])
        .output()
        .context("Failed to get diff name-status")?;

    // Build a map of file statuses
    let mut file_statuses: std::collections::HashMap<String, char> = std::collections::HashMap::new();
    if status_output.status.success() {
        for line in String::from_utf8_lossy(&status_output.stdout).lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 2 {
                let status = parts[0].chars().next().unwrap_or('M');
                let path = parts.last().unwrap_or(&"").to_string();
                file_statuses.insert(path, status);
            }
        }
    }

    // Parse numstat output: "additions\tdeletions\tfilename"
    for line in String::from_utf8_lossy(&numstat_output.stdout).lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let additions = parts[0].parse().unwrap_or(0);
            let deletions = parts[1].parse().unwrap_or(0);
            let path = parts[2].to_string();

            let status = file_statuses.get(&path).copied().unwrap_or('M');

            files.push(ChangedFile {
                path: path.clone(),
                additions,
                deletions,
                is_new: status == 'A',
                is_deleted: status == 'D',
                is_renamed: status == 'R',
            });
        }
    }

    // Sort by most changes first
    files.sort_by(|a, b| {
        let total_a = a.additions + a.deletions;
        let total_b = b.additions + b.deletions;
        total_b.cmp(&total_a)
    });

    Ok(files)
}

/// Remote tracking status for the main branch
#[derive(Debug, Clone, Default)]
pub struct RemoteStatus {
    /// Commits ahead of remote (local commits not pushed)
    pub ahead: usize,
    /// Commits behind remote (remote commits not pulled)
    pub behind: usize,
    /// Whether there's a configured remote
    pub has_remote: bool,
    /// Name of the remote (usually "origin")
    pub remote_name: Option<String>,
    /// Name of the tracked branch
    pub remote_branch: Option<String>,
}

/// Fetch from remote to update refs (does not modify working directory)
/// This allows us to check ahead/behind status
pub fn git_fetch(project_dir: &PathBuf) -> Result<()> {
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["fetch", "--quiet"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Ignore "no remote" errors - just means there's nothing to fetch
        if !stderr.contains("No remote repository") && !stderr.contains("does not appear to be a git repository") {
            return Err(anyhow!("Failed to fetch: {}", stderr));
        }
    }

    Ok(())
}

/// Get the remote tracking status for the current branch
/// Returns ahead/behind counts relative to the remote tracking branch
pub fn get_remote_status(project_dir: &PathBuf) -> Result<RemoteStatus> {
    // Get the current branch name
    let branch_output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()?;

    if !branch_output.status.success() {
        return Ok(RemoteStatus::default());
    }

    let branch = String::from_utf8_lossy(&branch_output.stdout).trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        // Detached HEAD state
        return Ok(RemoteStatus::default());
    }

    // Get the remote tracking branch
    let tracking_output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "--abbrev-ref", &format!("{}@{{upstream}}", branch)])
        .output()?;

    if !tracking_output.status.success() {
        // No upstream configured
        return Ok(RemoteStatus {
            has_remote: false,
            ..Default::default()
        });
    }

    let upstream = String::from_utf8_lossy(&tracking_output.stdout).trim().to_string();

    // Parse remote name and branch (e.g., "origin/main" -> ("origin", "main"))
    let (remote_name, remote_branch) = if let Some(slash_pos) = upstream.find('/') {
        (
            Some(upstream[..slash_pos].to_string()),
            Some(upstream[slash_pos + 1..].to_string()),
        )
    } else {
        (None, None)
    };

    // Get ahead/behind counts
    let rev_list_output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-list", "--left-right", "--count", &format!("{}...{}", branch, upstream)])
        .output()?;

    if !rev_list_output.status.success() {
        return Ok(RemoteStatus {
            has_remote: true,
            remote_name,
            remote_branch,
            ..Default::default()
        });
    }

    let counts = String::from_utf8_lossy(&rev_list_output.stdout);
    let parts: Vec<&str> = counts.trim().split_whitespace().collect();

    let (ahead, behind) = if parts.len() == 2 {
        (
            parts[0].parse().unwrap_or(0),
            parts[1].parse().unwrap_or(0),
        )
    } else {
        (0, 0)
    };

    Ok(RemoteStatus {
        ahead,
        behind,
        has_remote: true,
        remote_name,
        remote_branch,
    })
}

/// Pull from remote (fetch + merge)
/// Only pulls on the main branch in the main worktree
pub fn git_pull(project_dir: &PathBuf) -> Result<()> {
    // First check if we're on the main branch
    let branch_output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()?;

    let branch = String::from_utf8_lossy(&branch_output.stdout).trim().to_string();

    // Only allow pull on main/master branches
    if branch != "main" && branch != "master" {
        return Err(anyhow!(
            "Git pull only allowed on main/master branch. Currently on '{}'",
            branch
        ));
    }

    // Check for uncommitted changes
    if has_uncommitted_changes(project_dir)? {
        return Err(anyhow!(
            "Cannot pull with uncommitted changes. Please commit or stash your changes first."
        ));
    }

    // Perform the pull with rebase to keep history clean
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["pull", "--rebase"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Check for common issues
        if stderr.contains("CONFLICT") {
            // Abort the rebase
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["rebase", "--abort"])
                .output();
            return Err(anyhow!(
                "Pull failed due to conflicts. The pull has been aborted. Please resolve conflicts manually."
            ));
        }

        return Err(anyhow!("Pull failed: {}", stderr));
    }

    Ok(())
}

/// Smart pull that handles .kanblam/tasks.json gracefully
/// Stashes tasks.json, pulls, then restores local tasks.json (ignoring remote's version)
pub fn smart_git_pull(project_dir: &PathBuf) -> Result<String> {
    // First check if we're on the main branch
    let branch_output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()?;

    let branch = String::from_utf8_lossy(&branch_output.stdout).trim().to_string();

    // Only allow pull on main/master branches
    if branch != "main" && branch != "master" {
        return Err(anyhow!(
            "Git pull only allowed on main/master branch. Currently on '{}'",
            branch
        ));
    }

    // Check what files are modified
    let status_output = Command::new("git")
        .current_dir(project_dir)
        .args(["status", "--porcelain"])
        .output()?;

    let status = String::from_utf8_lossy(&status_output.stdout);
    let modified_files: Vec<&str> = status.lines()
        .filter(|line| !line.trim().is_empty())
        .collect();

    // Check if tasks.json is the only modified file (or among modified files)
    let tasks_json_path = ".kanblam/tasks.json";
    let has_tasks_json_changes = modified_files.iter()
        .any(|line| line.contains(tasks_json_path));
    let has_other_changes = modified_files.iter()
        .any(|line| !line.contains(tasks_json_path));

    if has_other_changes {
        return Err(anyhow!(
            "Cannot pull with uncommitted changes (other than tasks.json). Please commit or stash first."
        ));
    }

    // Stash tasks.json if it has changes
    let did_stash = if has_tasks_json_changes {
        let stash_output = Command::new("git")
            .current_dir(project_dir)
            .args(["stash", "push", "-m", "kanblam: tasks.json before pull", "--", tasks_json_path])
            .output()?;
        stash_output.status.success()
    } else {
        false
    };

    // Perform the pull with rebase
    let pull_output = Command::new("git")
        .current_dir(project_dir)
        .args(["pull", "--rebase"])
        .output()?;

    let pull_success = pull_output.status.success();
    let pull_stdout = String::from_utf8_lossy(&pull_output.stdout).to_string();
    let pull_stderr = String::from_utf8_lossy(&pull_output.stderr).to_string();

    // Restore tasks.json from stash (always use our local version)
    if did_stash {
        // Use checkout to restore just tasks.json from stash, avoiding merge
        let restore_output = Command::new("git")
            .current_dir(project_dir)
            .args(["checkout", "stash@{0}", "--", tasks_json_path])
            .output()?;

        if restore_output.status.success() {
            // Unstage tasks.json (checkout stages it, we want it unstaged)
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["restore", "--staged", tasks_json_path])
                .output();

            // Drop the stash since we've restored what we need
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["stash", "drop", "stash@{0}"])
                .output();
        } else {
            // Try regular stash pop as fallback
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["stash", "pop"])
                .output();
        }
    }

    if !pull_success {
        // Check for conflicts
        if pull_stderr.contains("CONFLICT") {
            // Abort the rebase
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["rebase", "--abort"])
                .output();
            return Err(anyhow!(
                "Pull failed due to conflicts. The pull has been aborted."
            ));
        }
        return Err(anyhow!("Pull failed: {}", pull_stderr));
    }

    // Parse output to create a nice summary
    let summary = if pull_stdout.contains("Already up to date") || pull_stderr.contains("Already up to date") {
        "Already up to date.".to_string()
    } else {
        // Count files changed from the output
        format!("Pull successful. {}", pull_stdout.lines().last().unwrap_or(""))
    };

    Ok(summary)
}

/// Push to remote
/// Only pushes the main branch
pub fn git_push(project_dir: &PathBuf) -> Result<()> {
    // First check if we're on the main branch
    let branch_output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()?;

    let branch = String::from_utf8_lossy(&branch_output.stdout).trim().to_string();

    // Only allow push on main/master branches
    if branch != "main" && branch != "master" {
        return Err(anyhow!(
            "Git push only allowed on main/master branch. Currently on '{}'",
            branch
        ));
    }

    // Perform the push
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["push"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Check for common issues
        if stderr.contains("rejected") || stderr.contains("non-fast-forward") {
            return Err(anyhow!(
                "Push rejected. Remote has changes you don't have. Pull first with 'P'."
            ));
        }

        if stderr.contains("No configured push destination") || stderr.contains("no upstream") {
            return Err(anyhow!(
                "No remote configured for pushing. Set up a remote with 'git remote add origin <url>'"
            ));
        }

        return Err(anyhow!("Push failed: {}", stderr));
    }

    Ok(())
}

// ============================================================================
// Stash tracking functions
// ============================================================================

use crate::model::TrackedStash;
use chrono::Utc;

/// Create a stash with a description and return tracking info
/// Returns None if there's nothing to stash
pub fn create_tracked_stash(project_dir: &PathBuf, description: &str) -> Result<Option<TrackedStash>> {
    // Check if there are changes to stash
    if !has_uncommitted_changes(project_dir)? {
        return Ok(None);
    }

    // Get list of changed files before stashing (for summary)
    let status_output = Command::new("git")
        .current_dir(project_dir)
        .args(["status", "--porcelain"])
        .output()?;

    let status = String::from_utf8_lossy(&status_output.stdout);
    let files: Vec<&str> = status.lines()
        .filter_map(|line| {
            if line.len() > 3 {
                Some(line[3..].trim())
            } else {
                None
            }
        })
        .collect();

    let files_changed = files.len();
    let files_summary = if files.len() <= 3 {
        files.join(", ")
    } else {
        format!("{}, {} and {} more", files[0], files[1], files.len() - 2)
    };

    // Create the stash with a message
    let stash_msg = format!("kanblam: {}", description);
    let stash_output = Command::new("git")
        .current_dir(project_dir)
        .args(["stash", "push", "-m", &stash_msg])
        .output()?;

    if !stash_output.status.success() {
        let stderr = String::from_utf8_lossy(&stash_output.stderr);
        return Err(anyhow!("Failed to create stash: {}", stderr));
    }

    // Get the stash SHA (stash@{0} after we just created it)
    let sha_output = Command::new("git")
        .current_dir(project_dir)
        .args(["rev-parse", "stash@{0}"])
        .output()?;

    if !sha_output.status.success() {
        return Err(anyhow!("Failed to get stash SHA"));
    }

    let stash_sha = String::from_utf8_lossy(&sha_output.stdout).trim().to_string();

    Ok(Some(TrackedStash {
        stash_ref: "stash@{0}".to_string(),
        description: description.to_string(),
        created_at: Utc::now(),
        files_changed,
        files_summary,
        stash_sha,
    }))
}

/// Find a stash by its SHA and return the current ref (stash index can change)
fn find_stash_ref_by_sha(project_dir: &PathBuf, stash_sha: &str) -> Result<Option<String>> {
    // List all stashes with their SHAs
    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["stash", "list", "--format=%H %gd"])
        .output()?;

    if !output.status.success() {
        return Ok(None);
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    for line in output_str.lines() {
        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        if parts.len() == 2 && parts[0] == stash_sha {
            return Ok(Some(parts[1].to_string()));
        }
    }

    Ok(None)
}

/// Pop a tracked stash by its SHA
/// Returns Ok(true) if popped cleanly, Err with "STASH_CONFLICT" prefix if conflicts
pub fn pop_tracked_stash(project_dir: &PathBuf, stash_sha: &str) -> Result<bool> {
    // Find the current ref for this stash SHA
    let stash_ref = find_stash_ref_by_sha(project_dir, stash_sha)?
        .ok_or_else(|| anyhow!("Stash not found (may have been dropped)"))?;

    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["stash", "pop", &stash_ref])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);

        // Check for conflicts
        if stderr.contains("CONFLICT") || stdout.contains("CONFLICT") {
            return Err(anyhow!("STASH_CONFLICT:{}", stash_sha));
        }

        return Err(anyhow!("Failed to pop stash: {}", stderr));
    }

    Ok(true)
}

/// Drop a tracked stash by its SHA
pub fn drop_tracked_stash(project_dir: &PathBuf, stash_sha: &str) -> Result<()> {
    // Find the current ref for this stash SHA
    let stash_ref = find_stash_ref_by_sha(project_dir, stash_sha)?
        .ok_or_else(|| anyhow!("Stash not found (may have already been dropped)"))?;

    let output = Command::new("git")
        .current_dir(project_dir)
        .args(["stash", "drop", &stash_ref])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to drop stash: {}", stderr));
    }

    Ok(())
}

/// Abort a conflicted stash pop while surgically preserving task changes
/// This is called when user chooses "Stash my changes" during an apply conflict
pub fn abort_stash_pop_keep_task_changes(project_dir: &PathBuf, task_id: Uuid) -> Result<()> {
    // The task patch file should already exist from the apply operation
    let patch_path = get_patch_file_path(task_id);

    if !patch_path.exists() {
        return Err(anyhow!("Task patch file not found - cannot restore task changes"));
    }

    // Get list of files modified by the stash (the one we tried to pop, still at stash@{0})
    let stash_files_output = Command::new("git")
        .current_dir(project_dir)
        .args(["stash", "show", "--name-only", "stash@{0}"])
        .output()?;

    let stash_files: Vec<String> = String::from_utf8_lossy(&stash_files_output.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect();

    // Read the task patch to know which files it modifies
    let patch_content = std::fs::read_to_string(&patch_path)?;
    let task_files: std::collections::HashSet<String> = patch_content
        .lines()
        .filter(|line| line.starts_with("+++ b/") || line.starts_with("--- a/"))
        .filter_map(|line| {
            line.strip_prefix("+++ b/")
                .or_else(|| line.strip_prefix("--- a/"))
                .map(|s| s.to_string())
        })
        .collect();

    // First, resolve all conflicts in favor of "ours" (the task changes)
    // This handles files that have conflict markers
    let _ = Command::new("git")
        .current_dir(project_dir)
        .args(["checkout", "--ours", "."])
        .output();

    // Unstage everything (stash pop may have staged some changes)
    let _ = Command::new("git")
        .current_dir(project_dir)
        .args(["reset", "HEAD"])
        .output();

    // For files that the stash touched but aren't in the task patch,
    // restore them to HEAD (remove stash-only changes)
    for file in &stash_files {
        if !task_files.contains(file) {
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["checkout", "HEAD", "--", file])
                .output();
        }
    }

    // For files that are in the task patch, re-apply just the task's version
    // This ensures we have exactly the task changes, not the merged result
    for file in &task_files {
        // Extract just this file's patch and apply it
        // Reset the file to HEAD first
        let _ = Command::new("git")
            .current_dir(project_dir)
            .args(["checkout", "HEAD", "--", file])
            .output();
    }

    // Re-apply the entire task patch (now on a clean base)
    let apply_output = Command::new("git")
        .current_dir(project_dir)
        .args(["apply", "--3way", patch_path.to_str().unwrap()])
        .output()?;

    if !apply_output.status.success() {
        let stderr = String::from_utf8_lossy(&apply_output.stderr);
        return Err(anyhow!("Failed to re-apply task patch: {}", stderr));
    }

    Ok(())
}

/// Get info about a stash for display (without creating a TrackedStash)
pub fn get_stash_details(project_dir: &PathBuf, stash_sha: &str) -> Result<(usize, String)> {
    // Find the stash ref
    let stash_ref = find_stash_ref_by_sha(project_dir, stash_sha)?
        .ok_or_else(|| anyhow!("Stash not found"))?;

    // Get file count
    let show_output = Command::new("git")
        .current_dir(project_dir)
        .args(["stash", "show", "--stat", &stash_ref])
        .output()?;

    let show_str = String::from_utf8_lossy(&show_output.stdout);
    let lines: Vec<&str> = show_str.lines().collect();

    // Last line usually shows summary like "3 files changed, 10 insertions(+), 5 deletions(-)"
    let files_changed = lines.last()
        .and_then(|line| {
            line.split_whitespace().next()
                .and_then(|n| n.parse::<usize>().ok())
        })
        .unwrap_or(0);

    // Get file names (all but last line)
    let file_summary = if lines.len() > 1 {
        lines[..lines.len()-1]
            .iter()
            .take(3)
            .map(|l| l.split('|').next().unwrap_or("").trim())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        String::new()
    };

    Ok((files_changed, file_summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_worktree_path() {
        let task_id = Uuid::new_v4();
        let project_dir = PathBuf::from("/Users/test/my-project");
        let path = get_worktree_path(&project_dir, task_id);
        assert!(path.to_string_lossy().contains(".worktrees"));
        assert!(path.to_string_lossy().contains(&format!("task-{}", task_id)));
        assert!(path.starts_with(&project_dir));
    }

    #[test]
    fn test_gitignore_missing_entries_empty_file() {
        let dir = tempdir().unwrap();
        let project_dir = dir.path().to_path_buf();

        // No .gitignore file - both entries should be missing
        let missing = gitignore_missing_kanblam_entries(&project_dir);
        assert_eq!(missing.len(), 2);
        assert!(missing.contains(&".claude/".to_string()));
        assert!(missing.contains(&"worktrees/".to_string()));
    }

    #[test]
    fn test_gitignore_missing_entries_partial() {
        let dir = tempdir().unwrap();
        let project_dir = dir.path().to_path_buf();

        // Only .claude/ present
        fs::write(project_dir.join(".gitignore"), ".claude/\n").unwrap();
        let missing = gitignore_missing_kanblam_entries(&project_dir);
        assert_eq!(missing.len(), 1);
        assert!(missing.contains(&"worktrees/".to_string()));

        // Only worktrees/ present
        fs::write(project_dir.join(".gitignore"), "worktrees/\n").unwrap();
        let missing = gitignore_missing_kanblam_entries(&project_dir);
        assert_eq!(missing.len(), 1);
        assert!(missing.contains(&".claude/".to_string()));
    }

    #[test]
    fn test_gitignore_missing_entries_all_present() {
        let dir = tempdir().unwrap();
        let project_dir = dir.path().to_path_buf();

        fs::write(project_dir.join(".gitignore"), ".claude/\nworktrees/\n").unwrap();
        let missing = gitignore_missing_kanblam_entries(&project_dir);
        assert!(missing.is_empty());
    }

    #[test]
    fn test_ensure_gitignore_creates_new_section() {
        let dir = tempdir().unwrap();
        let project_dir = dir.path().to_path_buf();

        // Existing content without KanBlam section
        fs::write(project_dir.join(".gitignore"), "node_modules/\n.env\n").unwrap();

        ensure_gitignore_has_kanblam_entries(&project_dir).unwrap();

        let content = fs::read_to_string(project_dir.join(".gitignore")).unwrap();
        assert!(content.contains("# KanBlam"));
        assert!(content.contains(".claude/"));
        assert!(content.contains("worktrees/"));
        // Original content preserved
        assert!(content.contains("node_modules/"));
        assert!(content.contains(".env"));
    }

    #[test]
    fn test_ensure_gitignore_appends_to_existing_section() {
        let dir = tempdir().unwrap();
        let project_dir = dir.path().to_path_buf();

        // Existing KanBlam section with only .claude/
        let initial = "node_modules/\n\n# KanBlam (Claude Code task manager)\n.claude/\n\n# Other section\nfoo/\n";
        fs::write(project_dir.join(".gitignore"), initial).unwrap();

        ensure_gitignore_has_kanblam_entries(&project_dir).unwrap();

        let content = fs::read_to_string(project_dir.join(".gitignore")).unwrap();

        // Should have exactly one KanBlam header
        let header_count = content.matches("# KanBlam").count();
        assert_eq!(header_count, 1, "Should have exactly one KanBlam header, found: {}", header_count);

        // Both entries present
        assert!(content.contains(".claude/"));
        assert!(content.contains("worktrees/"));

        // Other content preserved
        assert!(content.contains("node_modules/"));
        assert!(content.contains("# Other section"));
        assert!(content.contains("foo/"));
    }

    #[test]
    fn test_ensure_gitignore_appends_at_end_of_section() {
        let dir = tempdir().unwrap();
        let project_dir = dir.path().to_path_buf();

        // KanBlam section at end of file
        let initial = "node_modules/\n\n# KanBlam\nworktrees/\n";
        fs::write(project_dir.join(".gitignore"), initial).unwrap();

        ensure_gitignore_has_kanblam_entries(&project_dir).unwrap();

        let content = fs::read_to_string(project_dir.join(".gitignore")).unwrap();

        // Should have exactly one KanBlam header
        let header_count = content.matches("# KanBlam").count();
        assert_eq!(header_count, 1);

        // Both entries present
        assert!(content.contains(".claude/"));
        assert!(content.contains("worktrees/"));
    }

    #[test]
    fn test_ensure_gitignore_noop_when_complete() {
        let dir = tempdir().unwrap();
        let project_dir = dir.path().to_path_buf();

        let initial = "# KanBlam\n.claude/\nworktrees/\n";
        fs::write(project_dir.join(".gitignore"), initial).unwrap();

        ensure_gitignore_has_kanblam_entries(&project_dir).unwrap();

        let content = fs::read_to_string(project_dir.join(".gitignore")).unwrap();
        assert_eq!(content, initial, "File should not be modified when entries already present");
    }
}
