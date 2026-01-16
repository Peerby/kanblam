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
    /// Check if task was already merged, and if so cleanup and move to Done
    CheckAlreadyMerged(Uuid),
    /// Switch to the task's tmux window (focuses the Claude session)
    SwitchToTaskWindow(Uuid),
    /// Open combined session in detached mode (don't switch to it)
    OpenInteractiveDetached(Uuid),
    /// Apply task's changes to main worktree (for testing) - tries fast apply, falls back to Claude
    SmartApplyTask(Uuid),
    /// Start SDK apply session for conflict resolution (internal)
    StartApplySession { task_id: Uuid },
    /// Complete apply after Claude generates clean patch (internal)
    CompleteApplyTask(Uuid),
    /// Unapply/revert previously applied task changes
    UnapplyTaskChanges,
    /// Update worktree to latest main (rebase without merging)
    UpdateWorktreeToMain(Uuid),
    /// Start SDK update rebase session (internal - smart update with conflict resolution)
    StartUpdateRebaseSession { task_id: Uuid },
    /// Complete update after rebase verification (internal - no merge, just refresh status)
    CompleteUpdateTask(Uuid),
    /// Refresh git status (additions/deletions/behind) for all tasks with worktrees
    RefreshGitStatus,

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
    /// Show the open project dialog (triggered by pressing an unused project number)
    ShowOpenProjectDialog { slot: usize },
    /// Close the open project dialog without opening
    CloseOpenProjectDialog,
    /// Confirm the open project dialog and open the directory
    ConfirmOpenProject,
    /// Close a project (with confirmation if it has active tasks)
    CloseProject(usize),
    ReloadClaudeHooks,

    // Claude/Hook events
    HookSignalReceived(HookSignal),
    ClaudeOutputUpdated { project_id: Uuid, output: String },

    // Async background task results
    /// Create worktree for a task (deferred to allow UI render first)
    CreateWorktree { task_id: Uuid, project_dir: PathBuf },
    /// Worktree creation completed successfully (from background task)
    WorktreeCreated { task_id: Uuid, worktree_path: PathBuf, project_dir: PathBuf },
    /// Worktree creation failed (from background task)
    WorktreeCreationFailed { task_id: Uuid, error: String },

    // Sidecar/SDK events
    /// Event received from the SDK sidecar
    SidecarEvent(crate::sidecar::SidecarEvent),
    /// Start SDK session for a task (called after worktree is ready)
    StartSdkSession { task_id: Uuid },
    /// SDK session started successfully
    SdkSessionStarted { task_id: Uuid, session_id: String },
    /// SDK session start failed
    SdkSessionFailed { task_id: Uuid, error: String, project_dir: PathBuf, worktree_path: PathBuf },
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
    /// Start SDK rebase session for smart merge (internal)
    StartRebaseSession { task_id: Uuid },
    /// Enter feedback mode for a task in Review (focus input for feedback text)
    EnterFeedbackMode(Uuid),
    /// Cancel feedback mode
    CancelFeedbackMode,
    /// Send feedback to a task in Review and resume the SDK session
    SendFeedback { task_id: Uuid, feedback: String },

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
    /// Trigger the logo shimmer animation (called on successful merge)
    TriggerLogoShimmer,
    Quit,
    QuitAndSwitchPane(String), // Quit and switch to this pane ID
    Error(String),
}
