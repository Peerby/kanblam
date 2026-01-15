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
    get_task_diff, apply_task_changes, unapply_task_changes,
    needs_rebase, verify_rebase_success, generate_rebase_prompt,
    is_rebase_in_progress, abort_rebase,
    commit_worktree_changes, has_changes_to_merge,
};
pub use settings::{setup_claude_settings, merge_with_project_settings, pre_trust_worktree, remove_worktree_trust};
