#[allow(dead_code)]
mod capture;
mod session;

pub use session::{
    // Worktree-based task session management
    send_task_to_window, switch_to_task_window,
    kill_task_window, kill_task_sessions, task_window_exists,
    // Detached session creation
    open_popup_detached,
    // SDK/CLI handoff support
    send_key_to_pane, capture_pane_with_escapes,
    get_pane_size, open_popup,
    // CLI state detection
    kill_claude_cli_session,
    // Question detection for idle_prompt handling
    claude_output_contains_question,
    // Quick pane split for Ctrl-T
    split_pane_with_claude,
};
