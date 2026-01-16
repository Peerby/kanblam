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

    // Check if there are local changes that need to be stashed
    let status_check = Command::new("git")
        .current_dir(project_dir)
        .args(["status", "--porcelain"])
        .output()?;

    let has_local_changes = !String::from_utf8_lossy(&status_check.stdout).trim().is_empty();
    log(&format!("has_local_changes={}", has_local_changes));
    let mut stash_ref = None;

    // Stash local changes if any
    if has_local_changes {
        let stash_output = Command::new("git")
            .current_dir(project_dir)
            .args(["stash", "push", "-m", &format!("kanclaude: before applying task {}", task_id)])
            .output()?;

        if !stash_output.status.success() {
            let stderr = String::from_utf8_lossy(&stash_output.stderr);
            return Err(anyhow!("Failed to stash local changes: {}", stderr));
        }

        // Get the stash ref
        let stash_list = Command::new("git")
            .current_dir(project_dir)
            .args(["stash", "list", "-1"])
            .output()?;

        if stash_list.status.success() {
            let output = String::from_utf8_lossy(&stash_list.stdout);
            if let Some(ref_part) = output.split(':').next() {
                stash_ref = Some(ref_part.trim().to_string());
            }
        }
        log(&format!("stashed changes, stash_ref={:?}", stash_ref));
    }

    // Find the merge-base (common ancestor) between HEAD and the task branch
    // This ensures we only apply the task's changes, not revert changes made to main
    let merge_base_output = Command::new("git")
        .current_dir(project_dir)
        .args(["merge-base", "HEAD", &branch_name])
        .output()?;

    if !merge_base_output.status.success() {
        // Restore stash if we made one
        if stash_ref.is_some() {
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["stash", "pop"])
                .output();
        }
        let stderr = String::from_utf8_lossy(&merge_base_output.stderr);
        return Err(anyhow!("Failed to find merge-base: {}", stderr));
    }

    let merge_base = String::from_utf8_lossy(&merge_base_output.stdout).trim().to_string();
    log(&format!("merge_base={}", merge_base));

    // Get the diff from merge-base to the task branch (only the task's changes)
    let diff_output = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", &merge_base, &branch_name])
        .output()?;

    if !diff_output.status.success() {
        // Restore stash if we made one
        if stash_ref.is_some() {
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["stash", "pop"])
                .output();
        }
        let stderr = String::from_utf8_lossy(&diff_output.stderr);
        return Err(anyhow!("Failed to get diff: {}", stderr));
    }

    log(&format!("diff size={} bytes", diff_output.stdout.len()));
    if diff_output.stdout.is_empty() {
        log("WARNING: diff is empty! Nothing to apply.");
    }

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
        log("FAILED to apply changes");
        // Restore stash if we made one
        if stash_ref.is_some() {
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["stash", "pop"])
                .output();
        }
        return Err(anyhow!("Failed to apply changes. There may be conflicts."));
    }

    log("SUCCESS - changes applied");
    Ok(stash_ref)
}

/// Unapply task changes from the main worktree
/// This discards all changes (staged and unstaged) and optionally restores a stash
pub fn unapply_task_changes(project_dir: &PathBuf, stash_ref: Option<&str>) -> Result<()> {
    // Discard all changes (staged and unstaged) by restoring from HEAD
    // Using "checkout HEAD -- ." bypasses the index and restores directly from HEAD
    let reset_output = Command::new("git")
        .current_dir(project_dir)
        .args(["checkout", "HEAD", "--", "."])
        .output()?;

    if !reset_output.status.success() {
        let stderr = String::from_utf8_lossy(&reset_output.stderr);
        return Err(anyhow!("Failed to discard changes: {}", stderr));
    }

    // Restore the stash if we have one
    if let Some(_ref) = stash_ref {
        let pop_output = Command::new("git")
            .current_dir(project_dir)
            .args(["stash", "pop"])
            .output()?;

        if !pop_output.status.success() {
            let stderr = String::from_utf8_lossy(&pop_output.stderr);
            return Err(anyhow!("Failed to restore stashed changes: {}", stderr));
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree_path() {
        let task_id = Uuid::new_v4();
        let project_dir = PathBuf::from("/Users/test/my-project");
        let path = get_worktree_path(&project_dir, task_id);
        assert!(path.to_string_lossy().contains(".worktrees"));
        assert!(path.to_string_lossy().contains(&format!("task-{}", task_id)));
        assert!(path.starts_with(&project_dir));
    }
}
