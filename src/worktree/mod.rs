//! Git worktree management for isolated Claude sessions
//!
//! Each task runs in its own git worktree, providing complete filesystem isolation
//! between concurrent tasks. Changes are tracked on separate branches for easy
//! review, accept, or discard.

pub mod git;
mod settings;

pub use git::{
    create_worktree, remove_worktree, merge_branch, delete_branch,
    get_worktree_path, list_worktrees, WorktreeInfo, is_git_repo,
    get_task_diff, apply_task_changes, unapply_task_changes, force_unapply_task_changes,
    UnapplyResult,
    needs_rebase, verify_rebase_success, generate_rebase_prompt,
    generate_apply_prompt, is_rebase_in_progress, abort_rebase, try_fast_rebase,
    commit_worktree_changes, has_changes_to_merge, commit_main_changes, commit_applied_changes,
    get_worktree_git_status, update_worktree_to_main, WorktreeGitStatus,
    get_worktree_changed_files, ChangedFile, is_branch_merged,
    has_uncommitted_changes,
};
pub use settings::{setup_claude_settings, merge_with_project_settings, pre_trust_worktree, remove_worktree_trust};
