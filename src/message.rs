#![allow(dead_code)]

use crate::model::{FocusArea, HookSignal, PendingAction, TaskStatus};
use crate::sidecar::protocol::{WatcherComment, WatcherObserving};
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
    /// Merge only - merge changes to main but keep worktree and task in Review
    MergeOnlyTask(Uuid),
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
    /// Start a Claude session to resolve stash conflicts in main worktree
    StartStashConflictSession { task_id: Uuid, stash_sha: String },
    /// Complete stash conflict resolution (called when Claude session ends)
    CompleteStashConflictResolution(Uuid),
    /// Keep stash conflict markers for manual resolution (user pressed 'k')
    KeepStashConflictMarkers(Uuid),
    /// Stash user's changes and apply task cleanly (user pressed 's' on conflict)
    StashUserChangesAndApply(Uuid),

    // Stash management
    /// Toggle the stash management modal
    ToggleStashModal,
    /// Navigate in stash modal
    StashModalNavigate(i32),
    /// Pop the selected stash
    PopSelectedStash,
    /// Drop the selected stash (with confirmation)
    DropSelectedStash,
    /// Confirm dropping a stash
    ConfirmDropStash { stash_sha: String },
    /// Offer to pop a tracked stash (shows confirmation dialog)
    OfferPopStash { stash_sha: String, context: String },
    /// Pop a specific tracked stash by SHA
    PopTrackedStash { stash_sha: String },
    /// Handle stash pop conflict (from popping a tracked stash)
    HandleStashPopConflict { stash_sha: String },
    /// Stash changes before merge, then proceed with merge
    StashThenMerge { task_id: Uuid },

    /// Unapply/revert previously applied task changes
    UnapplyTaskChanges,
    /// Force unapply using destructive reset (after user confirms)
    ForceUnapplyTaskChanges(Uuid),
    /// Force unapply and then restore the user's stashed changes
    ForceUnapplyWithStashRestore { task_id: Uuid, stash_sha: String },
    /// Update worktree to latest main (rebase without merging)
    UpdateWorktreeToMain(Uuid),
    /// Start SDK update rebase session (internal - smart update with conflict resolution)
    StartUpdateRebaseSession { task_id: Uuid },
    /// Complete update after rebase verification (internal - no merge, just refresh status)
    CompleteUpdateTask(Uuid),
    /// Refresh git status (additions/deletions/behind) for all tasks with worktrees
    RefreshGitStatus,

    // Git remote operations (pull/push)
    /// Start git fetch to check remote status (background)
    StartGitFetch,
    /// Git fetch completed - update remote status
    GitFetchCompleted { ahead: usize, behind: usize },
    /// Git fetch failed
    GitFetchFailed { error: String },
    /// Start git pull from remote (background)
    StartGitPull,
    /// Git pull completed successfully
    GitPullCompleted { summary: String },
    /// Git pull failed
    GitPullFailed { error: String },
    /// Start git push to remote (background)
    StartGitPush,
    /// Git push completed successfully
    GitPushCompleted,
    /// Git push failed
    GitPushFailed { error: String },

    // Task queueing
    /// Show the queue dialog to select a session to queue the task for
    ShowQueueDialog(Uuid),
    /// Queue a task to run after another task finishes (in the same session)
    QueueTaskForSession { task_id: Uuid, after_task_id: Uuid },
    /// Navigate up/down in the queue dialog
    QueueDialogNavigate(i32),
    /// Jump to first item in queue dialog (Home)
    QueueDialogNavigateToStart,
    /// Jump to last item in queue dialog (End)
    QueueDialogNavigateToEnd,
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
    /// Confirm the open project dialog and open the selected directory
    ConfirmOpenProject,
    /// Confirm opening a specific path as project (from Miller columns [New Project Here])
    ConfirmOpenProjectPath(PathBuf),
    /// Close a project (with confirmation if it has active tasks)
    CloseProject(usize),
    /// Enter create folder mode in the open project dialog
    EnterCreateFolderMode,
    /// Cancel create folder mode
    CancelCreateFolderMode,
    /// Create a new folder with the given name and initialize git
    CreateFolder { name: String },

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

    // Async fast rebase
    /// Start fast rebase in background (deferred to allow UI render first)
    StartFastRebase { task_id: Uuid, worktree_path: PathBuf, project_dir: PathBuf },
    /// Fast rebase completed successfully (from background task)
    FastRebaseCompleted { task_id: Uuid },
    /// Fast rebase failed - conflicts detected, needs smart rebase (from background task)
    FastRebaseNeedsSmartRebase { task_id: Uuid },
    /// Fast rebase failed with error (from background task)
    FastRebaseFailed { task_id: Uuid, error: String },

    // Async rebase-for-apply (when pressing 'a' on task that's behind main)
    /// Start rebase-for-apply in background
    StartRebaseForApply { task_id: Uuid, worktree_path: PathBuf, project_dir: PathBuf },
    /// Rebase-for-apply completed successfully
    RebaseForApplyCompleted { task_id: Uuid },
    /// Rebase-for-apply needs Claude for conflicts
    RebaseForApplyNeedsClaude { task_id: Uuid },
    /// Rebase-for-apply failed
    RebaseForApplyFailed { task_id: Uuid, error: String },

    // Async smart accept (merge task)
    /// Start smart accept git operations in background
    StartSmartAcceptGitOps { task_id: Uuid, worktree_path: PathBuf, project_dir: PathBuf, has_branch: bool },
    /// Smart accept git ops done - ready to merge (no rebase needed or fast rebase succeeded)
    SmartAcceptReadyToMerge { task_id: Uuid },
    /// Smart accept needs Claude for conflict resolution
    SmartAcceptNeedsClaude { task_id: Uuid },
    /// Smart accept git ops failed
    SmartAcceptFailed { task_id: Uuid, error: String },

    // Async merge-only (M command)
    /// Start merge-only git operations in background
    StartMergeOnlyGitOps { task_id: Uuid, worktree_path: PathBuf, project_dir: PathBuf },
    /// Merge-only git ops done - ready to merge
    MergeOnlyReadyToMerge { task_id: Uuid },
    /// Merge-only failed (conflicts - needs full 'm' for Claude resolution)
    MergeOnlyConflicts { task_id: Uuid },
    /// Merge-only git ops failed
    MergeOnlyFailed { task_id: Uuid, error: String },

    // Async build before restart
    /// Start build in background before restarting
    StartBuildForRestart,
    /// Build completed successfully - proceed with restart
    BuildCompleted,
    /// Build failed with error
    BuildFailed { error: String },

    // Title summarization
    /// Request a short title summary for a task (sent to sidecar)
    RequestTitleSummary { task_id: Uuid },
    /// Short title summary and spec received from sidecar
    TitleSummaryReceived { task_id: Uuid, short_title: String, spec: Option<String> },

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
    /// Actually open interactive modal (after confirmation if SDK was working)
    DoOpenInteractiveModal(Uuid),
    /// Actually send feedback (after confirmation if CLI was working)
    DoSendFeedback { task_id: Uuid, feedback: String },
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
    /// Queue feedback to be sent when Claude finishes current work
    QueueFeedback { task_id: Uuid, feedback: String },

    // QA validation
    /// Start QA validation for a task (run tests, AI review)
    StartQaValidation(Uuid),
    /// QA validation passed - move task to Review
    QaValidationPassed(Uuid),
    /// QA validation found issues - provide feedback and retry
    QaValidationNeedsWork { task_id: Uuid, feedback: String },
    /// QA validation exceeded max attempts - move to NeedsWork with warning
    QaMaxAttemptsExceeded(Uuid),

    // Image handling
    PasteImage,
    AttachImage { task_id: Uuid, path: PathBuf },
    /// Clear all images (from pending or active edit/feedback task)
    ClearImages,
    /// Remove the last image (from pending or active edit/feedback task)
    RemoveLastImage,

    // UI events
    InputSubmit,
    /// Submit input and immediately start the task (Ctrl+S)
    InputSubmitAndStart,
    /// Open current input in external editor (vim), submit on save
    OpenExternalEditor,
    /// External editor finished - set the input text and submit
    ExternalEditorFinished(String),
    FocusChanged(FocusArea),
    NavigateUp,
    NavigateDown,
    NavigateLeft,
    NavigateRight,
    NavigateToStart, // Home key - jump to first item in list
    NavigateToEnd,   // End key - jump to last item in list
    ToggleHelp,
    ToggleStats,           // Show/hide project statistics modal (/)
    ScrollHelpUp(usize),   // Scroll help modal up by N lines
    ScrollHelpDown(usize), // Scroll help modal down by N lines
    ToggleTaskPreview,     // Show/hide task preview modal (v/space)
    TaskDetailNextTab,     // Move to next tab in task detail modal
    TaskDetailPrevTab,     // Move to previous tab in task detail modal
    ScrollGitDiffUp(usize),   // Scroll git diff up by N lines
    ScrollGitDiffDown(usize), // Scroll git diff down by N lines
    LoadGitDiff(Uuid),        // Load/refresh git diff for a task
    ScrollSpecUp(usize),      // Scroll spec tab up by N lines
    ScrollSpecDown(usize),    // Scroll spec tab down by N lines
    /// Open spec in external editor (Ctrl+G in spec tab)
    OpenSpecEditor(Uuid),
    /// External spec editor finished - update spec content
    SpecEditorFinished { task_id: Uuid, spec: String },
    ScrollActivityUp(usize),  // Scroll activity tab up by N entries
    ScrollActivityDown(usize), // Scroll activity tab down by N entries
    ToggleActivityExpand,     // Toggle expansion of selected activity entry

    // Confirmation dialogs
    ShowConfirmation { message: String, action: PendingAction },
    ConfirmAction,  // User pressed 'y'
    CancelAction,   // User pressed 'n' or Esc
    RestartConfirmationAnimation, // User pressed an unrecognized key - highlight the prompt
    ScrollConfirmationUp,   // Scroll multiline confirmation modal up
    ScrollConfirmationDown, // Scroll multiline confirmation modal down
    SetStatusMessage(Option<String>),

    // System
    Tick,
    /// Trigger the logo shimmer animation with star eyes (called on successful merge/commit)
    TriggerLogoShimmer,
    /// Trigger the merge celebration "gold dust sweep" animation for a task
    /// The task will be visually swept away with sparkles from right to left
    /// If pending_completion is true, complete_task() will be called when animation finishes
    TriggerMergeCelebration { task_id: Uuid, display_text: String, column_status: TaskStatus, task_index: usize, pending_completion: bool },
    /// Complete task after merge celebration animation finishes (called from Tick handler)
    FinishMergeCelebration(Uuid),
    /// Trigger a blink animation (called when clicking the mascot)
    TriggerMascotBlink,
    /// Trigger an immediate watcher observation (called when clicking mascot with watcher enabled)
    TriggerWatcher,
    /// Show the startup hints bar again (triggered by pressing ESC multiple times)
    ShowStartupHints,
    /// Focus the welcome speech bubble (triggered by pressing down on welcome screen)
    WelcomeBubbleFocus,
    /// Unfocus the welcome speech bubble (triggered by pressing up or Esc when focused)
    WelcomeBubbleUnfocus,
    /// Navigate to previous welcome message
    WelcomeMessagePrev,
    /// Navigate to next welcome message
    WelcomeMessageNext,
    /// Trigger app restart (for hot reload after apply)
    TriggerRestart,
    Quit,
    QuitAndSwitchPane(String), // Quit and switch to this pane ID
    Error(String),

    // Quick Claude CLI pane
    /// Open a fresh Claude CLI session in a pane to the right (Ctrl-T)
    OpenClaudeCliPane,

    // Watcher
    /// Start the watcher for the current project
    StartWatcher,
    /// Stop the watcher for the current project
    StopWatcher,
    /// Watcher comment received from sidecar
    WatcherCommentReceived(WatcherComment),
    /// Watcher observation status changed (Claude SDK started/stopped running)
    WatcherObservingChanged(WatcherObserving),
    /// Dismiss the current watcher comment
    DismissWatcherComment,
    /// Open the watcher insight modal (Ctrl+I when comment is visible)
    OpenWatcherInsightModal,
    /// Close the watcher insight modal
    CloseWatcherInsightModal,
    /// Create a task from the watcher insight (p key in modal)
    CreateTaskFromWatcherInsight,
    /// Start a task immediately from the watcher insight (Ctrl+S in modal)
    StartTaskFromWatcherInsight,
    /// Scroll watcher insight modal up
    ScrollWatcherInsightUp,
    /// Scroll watcher insight modal down
    ScrollWatcherInsightDown,

    // Configuration modal
    /// Open the configuration modal
    ShowConfigModal,
    /// Close the configuration modal without saving
    CloseConfigModal,
    /// Navigate to next field in config modal
    ConfigNavigateDown,
    /// Navigate to previous field in config modal
    ConfigNavigateUp,
    /// Enter edit mode for current field / cycle editor choice forward
    ConfigEditField,
    /// Cycle editor choice backward (for h/Left key)
    ConfigEditFieldPrev,
    /// Update the edit buffer while typing
    ConfigUpdateBuffer(String),
    /// Confirm the current edit and move to next field
    ConfigConfirmEdit,
    /// Cancel the current edit
    ConfigCancelEdit,
    /// Save all config changes and close modal
    ConfigSave,
    /// Reset project commands to auto-detected defaults
    ConfigResetToDefaults,
}
