mod capture;
mod session;

pub use capture::capture_pane_output;
pub use session::{
    detect_claude_sessions, reload_claude_session, spawn_claude_session, start_claude_task,
    switch_to_session, ClaudeSession,
    // Worktree-based task session management
    get_or_create_project_session, create_task_window, start_claude_in_window,
    wait_for_claude_ready, send_task_to_window, focus_task_window, switch_to_task_window,
    kill_task_window, task_window_exists, capture_task_output,
};
