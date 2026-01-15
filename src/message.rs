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
    /// Smart accept - rebase onto main if needed, then accept
    SmartAcceptTask(Uuid),
    /// Complete accept after rebase verification (internal)
    CompleteAcceptTask(Uuid),
    /// Discard a task - delete worktree and branch without merging
    DiscardTask(Uuid),
    /// Reset a task - discard all changes and start fresh (moved to top of Planned)
    ResetTask(Uuid),
    /// Switch to the task's tmux window (focuses the Claude session)
    SwitchToTaskWindow(Uuid),
    /// Open a test shell in the task's worktree
    OpenTestShell(Uuid),
    /// Apply task's changes to main worktree (for testing)
    ApplyTaskChanges(Uuid),
    /// Unapply/revert previously applied task changes
    UnapplyTaskChanges,

    // Task queueing
    /// Show the queue dialog to select a session to queue the task for
    ShowQueueDialog(Uuid),
    /// Queue a task to run after another task finishes (in the same session)
    QueueTaskForSession { task_id: Uuid, after_task_id: Uuid },
    /// Navigate up/down in the queue dialog
    QueueDialogNavigate(i32),
    /// Confirm selection in queue dialog
    QueueDialogConfirm,
    /// Close the queue dialog
    CloseQueueDialog,
    /// Send the next queued task to a session (internal, called when a task stops)
    SendQueuedTask { finished_task_id: Uuid },

    // Project operations
    SwitchProject(usize),
    AddProject { name: String, working_dir: PathBuf },
    ReloadClaudeHooks,

    // Claude/Hook events
    HookSignalReceived(HookSignal),
    ClaudeOutputUpdated { project_id: Uuid, output: String },

    // Sidecar/SDK events
    /// Event received from the SDK sidecar
    SidecarEvent(crate::sidecar::SidecarEvent),
    /// SDK session started successfully
    SdkSessionStarted { task_id: Uuid, session_id: String },
    /// SDK session output received
    SdkSessionOutput { task_id: Uuid, output: String },
    /// Open interactive modal for a task (hand off to CLI)
    OpenInteractiveModal(Uuid),
    /// Close interactive modal (return control to app)
    CloseInteractiveModal,
    /// CLI session ended, hand back to SDK
    CliSessionEnded { task_id: Uuid },
    /// Resume SDK session after CLI handoff
    ResumeSdkSession { task_id: Uuid },

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
    ToggleTaskPreview, // Show/hide task preview modal (v/space)

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
