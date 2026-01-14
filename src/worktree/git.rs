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

/// Get the base directory for worktrees
fn worktree_base_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kanclaude")
        .join("worktrees")
}

/// Get the worktree path for a specific project and task
pub fn get_worktree_path(project_slug: &str, task_id: Uuid) -> PathBuf {
    worktree_base_dir()
        .join(project_slug)
        .join(format!("task-{}", task_id))
}

/// Create a new worktree for a task
///
/// Creates a worktree at `~/.kanclaude/worktrees/{project}/{task-id}/`
/// on branch `claude/{task-id}` based on the current HEAD.
pub fn create_worktree(
    project_dir: &PathBuf,
    project_slug: &str,
    task_id: Uuid,
) -> Result<PathBuf> {
    let worktree_path = get_worktree_path(project_slug, task_id);
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

/// Merge a task branch into the base branch (squash merge)
/// Handles dirty working directory by stashing local changes first
pub fn merge_branch(project_dir: &PathBuf, task_id: Uuid) -> Result<()> {
    let branch_name = format!("claude/{}", task_id);

    // Check if there are local changes that need to be stashed
    let status_check = Command::new("git")
        .current_dir(project_dir)
        .args(["status", "--porcelain"])
        .output()?;

    let has_local_changes = !String::from_utf8_lossy(&status_check.stdout).trim().is_empty();

    // Stash local changes if any
    if has_local_changes {
        let stash_output = Command::new("git")
            .current_dir(project_dir)
            .args(["stash", "push", "-m", "kanclaude: auto-stash before merge"])
            .output()?;

        if !stash_output.status.success() {
            let stderr = String::from_utf8_lossy(&stash_output.stderr);
            return Err(anyhow!("Failed to stash local changes: {}", stderr));
        }
    }

    // Perform squash merge
    let merge_result = Command::new("git")
        .current_dir(project_dir)
        .args(["merge", "--squash", &branch_name])
        .output();

    // Helper to restore stash on error
    let restore_stash = |project_dir: &PathBuf, had_changes: bool| {
        if had_changes {
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["stash", "pop"])
                .output();
        }
    };

    let output = match merge_result {
        Ok(o) => o,
        Err(e) => {
            restore_stash(project_dir, has_local_changes);
            return Err(anyhow!("Failed to run merge: {}", e));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Abort the failed merge
        let _ = Command::new("git")
            .current_dir(project_dir)
            .args(["merge", "--abort"])
            .output();
        restore_stash(project_dir, has_local_changes);
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
            restore_stash(project_dir, has_local_changes);
            return Err(anyhow!("Failed to commit merge: {}", stderr));
        }
    }

    // Restore stashed changes
    if has_local_changes {
        let pop_output = Command::new("git")
            .current_dir(project_dir)
            .args(["stash", "pop"])
            .output()?;

        if !pop_output.status.success() {
            // Stash pop might fail due to conflicts with merged changes
            // This is expected - user will need to resolve manually
            let stderr = String::from_utf8_lossy(&pop_output.stderr);
            return Err(anyhow!(
                "Merge committed but stash pop had conflicts: {}. Resolve manually with 'git stash pop'.",
                stderr
            ));
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

    // Check if there are local changes that need to be stashed
    let status_check = Command::new("git")
        .current_dir(project_dir)
        .args(["status", "--porcelain"])
        .output()?;

    let has_local_changes = !String::from_utf8_lossy(&status_check.stdout).trim().is_empty();
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
    }

    // Get the diff from the task branch and apply it
    let diff_output = Command::new("git")
        .current_dir(project_dir)
        .args(["diff", "HEAD", &branch_name])
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

    // Apply the diff
    let mut apply_cmd = Command::new("git")
        .current_dir(project_dir)
        .args(["apply", "--3way"])
        .stdin(std::process::Stdio::piped())
        .spawn()?;

    if let Some(stdin) = apply_cmd.stdin.as_mut() {
        use std::io::Write;
        stdin.write_all(&diff_output.stdout)?;
    }

    let apply_result = apply_cmd.wait()?;

    if !apply_result.success() {
        // Restore stash if we made one
        if stash_ref.is_some() {
            let _ = Command::new("git")
                .current_dir(project_dir)
                .args(["stash", "pop"])
                .output();
        }
        return Err(anyhow!("Failed to apply changes. There may be conflicts."));
    }

    Ok(stash_ref)
}

/// Unapply task changes from the main worktree
/// This discards all unstaged changes and optionally restores a stash
pub fn unapply_task_changes(project_dir: &PathBuf, stash_ref: Option<&str>) -> Result<()> {
    // Discard all changes (staged and unstaged)
    let reset_output = Command::new("git")
        .current_dir(project_dir)
        .args(["checkout", "--", "."])
        .output()?;

    if !reset_output.status.success() {
        let stderr = String::from_utf8_lossy(&reset_output.stderr);
        return Err(anyhow!("Failed to discard changes: {}", stderr));
    }

    // Also unstage any staged changes
    let _ = Command::new("git")
        .current_dir(project_dir)
        .args(["reset", "HEAD"])
        .output();

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

/// Check if a directory is a git worktree (not the main repository)
pub fn is_worktree(dir: &PathBuf) -> bool {
    let git_path = dir.join(".git");
    // In worktrees, .git is a file containing "gitdir: /path/to/main/.git/worktrees/..."
    // In main repos, .git is a directory
    git_path.is_file()
}

/// Get the main repository path for a worktree
/// Returns the original path if it's not a worktree or detection fails
pub fn get_main_repo_path(dir: &PathBuf) -> PathBuf {
    // Only try git rev-parse if the directory exists
    if !dir.exists() {
        return dir.clone();
    }

    // Use git rev-parse --git-common-dir to get the common .git directory
    // This returns the main repo's .git dir for worktrees
    let output = Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let common_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
            // The common dir is the .git directory of the main repo
            // The main repo path is the parent of .git
            if let Some(main_repo) = PathBuf::from(&common_dir).parent() {
                // Verify this is actually a valid directory
                if main_repo.exists() && main_repo.is_dir() {
                    return main_repo.to_path_buf();
                }
            }
        }
    }

    // Fall back to original path
    dir.clone()
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

/// Generate a prompt for Claude to commit and rebase the current branch
pub fn generate_rebase_prompt(main_branch: &str) -> String {
    format!(r#"IMPORTANT: The main branch has advanced. You must rebase your changes before they can be merged.

Execute these steps IN ORDER:

1. COMMIT any uncommitted changes:
   git add -A && git commit -m "WIP: task changes" --allow-empty

2. REBASE onto main:
   git rebase {}

3. IF CONFLICTS occur during rebase:
   - Open each conflicted file and resolve the conflict (keep both changes merged intelligently)
   - Remove the conflict markers (<<<<<<<, =======, >>>>>>>)
   - Stage the resolved file: git add <filename>
   - Continue rebase: git rebase --continue
   - Repeat for each conflict

4. VERIFY the rebase succeeded by checking the log shows main's commits:
   git log --oneline -5

When rebase is complete, just say "Rebase complete" and stop.
If rebase fails and you cannot resolve it, say "Rebase failed" and stop.

Do not explain. Execute these commands now."#, main_branch)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree_path() {
        let task_id = Uuid::new_v4();
        let path = get_worktree_path("my-project", task_id);
        assert!(path.to_string_lossy().contains("my-project"));
        assert!(path.to_string_lossy().contains(&format!("task-{}", task_id)));
    }
}
