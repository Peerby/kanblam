use crate::model::{FocusArea, HookSignal, PendingAction, TaskStatus};
use std::path::PathBuf;
use uuid::Uuid;

/// Messages that can be dispatched to update application state (TEA pattern)
#[derive(Debug, Clone)]
pub enum Message {
    // Task operations
    CreateTask(String),
    EditTask(Uuid),
    UpdateTask { task_id: Uuid, title: String },
    CancelEdit,
    DeleteTask(Uuid),
    MoveTask { task_id: Uuid, to_status: TaskStatus },
    MoveTaskUp,      // Move selected task up in list (+)
    MoveTaskDown,    // Move selected task down in list (-)
    ToggleDivider,   // Toggle divider below selected task (|)
    DeleteDivider,   // Delete the currently selected divider
    EditDivider,     // Edit the currently selected divider's title
    UpdateDividerTitle { task_id: Uuid, title: Option<String> },
    StartTask(Uuid),
    SelectTask(Option<usize>),
    SelectColumn(TaskStatus),
    ClickedTask { status: TaskStatus, task_idx: usize },

    // Worktree-based task lifecycle
    /// Start a task with worktree isolation (creates worktree, tmux window, starts Claude)
    StartTaskWithWorktree(Uuid),
    /// Update the session state of a task (internal, from async operations)
    UpdateTaskSessionState { task_id: Uuid, state: crate::model::ClaudeSessionState },
    /// Continue a task from Review (focus the tmux window)
    ContinueTask(Uuid),
    /// Accept a task - merge changes to main, cleanup worktree
    AcceptTask(Uuid),
    /// Discard a task - delete worktree and branch without merging
    DiscardTask(Uuid),
    /// Switch to the task's tmux window (focuses the Claude session)
    SwitchToTaskWindow(Uuid),

    // Project operations
    SwitchProject(usize),
    AddProject { name: String, working_dir: PathBuf },
    RefreshProjects,
    ReloadClaudeHooks,

    // Session operations (within a project)
    NextSession,
    PrevSession,
    SpawnNewSession,

    // Claude/Hook events
    HookSignalReceived(HookSignal),
    ClaudeOutputUpdated { project_id: Uuid, output: String },

    // Image handling
    PasteImage,
    AttachImage { task_id: Uuid, path: PathBuf },

    // UI events
    InputSubmit,
    FocusChanged(FocusArea),
    NavigateUp,
    NavigateDown,
    NavigateLeft,
    NavigateRight,
    ToggleHelp,

    // Confirmation dialogs
    ShowConfirmation { message: String, action: PendingAction },
    ConfirmAction,  // User pressed 'y'
    CancelAction,   // User pressed 'n' or Esc
    SetStatusMessage(Option<String>),

    // System
    Tick,
    Quit,
    QuitAndSwitchPane(String), // Quit and switch to this pane ID
    Error(String),
}
