mod capture;
mod session;

pub use capture::capture_pane_output;
pub use session::{
    switch_to_session,
    // Worktree-based task session management
    get_or_create_project_session, create_task_window, start_claude_in_window,
    wait_for_claude_ready, send_task_to_window, focus_task_window, switch_to_task_window,
    kill_task_window, kill_task_sessions, task_window_exists, capture_task_output, create_test_shell,
    // Detached session creation
    DetachedSessionResult, create_test_shell_detached, open_popup_detached,
    // SDK/CLI handoff support
    send_resume_command, send_start_command, send_key_to_pane, capture_pane_with_escapes,
    resize_pane, send_sigwinch, get_pane_size, open_popup,
};
