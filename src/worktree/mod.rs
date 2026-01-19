//! Git worktree management for isolated Claude sessions
//!
//! Each task runs in its own git worktree, providing complete filesystem isolation
//! between concurrent tasks. Changes are tracked on separate branches for easy
//! review, accept, or discard.

pub mod git;
mod settings;

pub use git::{
    create_worktree, remove_worktree, merge_branch, delete_branch,
    get_task_diff, apply_task_changes, unapply_task_changes, force_unapply_task_changes,
    surgical_unapply_for_stash_conflict, UnapplyResult, cleanup_applied_state,
    needs_rebase, verify_rebase_success, generate_rebase_prompt,
    generate_apply_prompt, generate_stash_conflict_prompt, save_current_changes_as_patch,
    is_rebase_in_progress, try_fast_rebase,
    commit_worktree_changes, has_changes_to_merge, commit_main_changes, commit_applied_changes,
    get_worktree_git_status, update_worktree_to_main,
    has_uncommitted_changes,
    // Git remote operations
    git_fetch, git_push, smart_git_pull, get_remote_status,
    // Stash tracking
    create_tracked_stash, pop_tracked_stash, drop_tracked_stash,
    abort_stash_pop_keep_task_changes, get_stash_details,
};
pub use settings::{merge_with_project_settings, pre_trust_worktree, remove_worktree_trust};
