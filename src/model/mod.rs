use chrono::{DateTime, Utc};
use edtui::{
    EditorEventHandler, EditorMode, EditorState, Lines,
    actions::{Composed, SelectInnerWord, DeleteSelection},
    events::{KeyEvent, KeyEventHandler, KeyEventRegister},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Application state following The Elm Architecture
#[derive(Serialize, Deserialize)]
pub struct AppModel {
    pub projects: Vec<Project>,
    pub active_project_idx: usize,
    #[serde(skip)]
    pub ui_state: UiState,
}

impl Default for AppModel {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            active_project_idx: 0,
            ui_state: UiState::default(),
        }
    }
}

impl AppModel {
    pub fn active_project(&self) -> Option<&Project> {
        self.projects.get(self.active_project_idx)
    }

    pub fn active_project_mut(&mut self) -> Option<&mut Project> {
        self.projects.get_mut(self.active_project_idx)
    }

    pub fn projects_needing_attention(&self) -> usize {
        self.projects.iter().filter(|p| p.needs_attention).count()
    }
}

/// A project represents a working directory with Claude Code sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub working_dir: PathBuf,
    pub tasks: Vec<Task>,
    pub needs_attention: bool,
    pub created_at: DateTime<Utc>,
    #[serde(skip)]
    pub captured_output: String,
    #[serde(skip)]
    pub hooks_installed: bool,
}

impl Project {
    pub fn new(name: String, working_dir: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            working_dir,
            tasks: Vec::new(),
            needs_attention: false,
            created_at: Utc::now(),
            captured_output: String::new(),
            hooks_installed: false,
        }
    }

    /// Get a URL-safe slug for the project name
    pub fn slug(&self) -> String {
        self.name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    }

    /// Check if project directory is a git repository
    pub fn is_git_repo(&self) -> bool {
        crate::worktree::git::is_git_repo(&self.working_dir)
    }

    pub fn tasks_by_status(&self, status: TaskStatus) -> Vec<&Task> {
        // Return tasks in Vec order - allows manual reordering with +/-
        // Accepting tasks appear in the Review column
        self.tasks.iter().filter(|t| {
            t.status == status ||
            (status == TaskStatus::Review && t.status == TaskStatus::Accepting)
        }).collect()
    }

    pub fn in_progress_task(&self) -> Option<&Task> {
        self.tasks.iter().find(|t| t.status == TaskStatus::InProgress)
    }

    /// Check if any task is currently active (InProgress or NeedsInput)
    pub fn has_active_task(&self) -> bool {
        self.tasks.iter().any(|t| {
            t.status == TaskStatus::InProgress || t.status == TaskStatus::NeedsInput
        })
    }

    /// Get all tasks that have an active Claude session (for queue dialog)
    pub fn tasks_with_active_sessions(&self) -> Vec<&Task> {
        self.tasks.iter().filter(|t| t.has_active_session()).collect()
    }

    /// Find the next task queued for a given session/task
    pub fn next_queued_for(&self, task_id: Uuid) -> Option<&Task> {
        self.tasks.iter().find(|t| t.queued_for_session == Some(task_id))
    }

    /// Find the next task queued for a given session/task (mutable)
    pub fn next_queued_for_mut(&mut self, task_id: Uuid) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.queued_for_session == Some(task_id))
    }

    /// Get the next queued task (first one in queue order)
    pub fn next_queued_task(&self) -> Option<&Task> {
        self.tasks.iter().find(|t| t.status == TaskStatus::Queued)
    }

    pub fn review_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.status == TaskStatus::Review).count()
    }

    pub fn needs_input_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.status == TaskStatus::NeedsInput).count()
    }
}

/// Claude session state within a worktree
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ClaudeSessionState {
    /// Task not started yet, no worktree
    #[default]
    NotStarted,
    /// Creating worktree and starting Claude
    Creating,
    /// Claude started, waiting for it to be ready
    Starting,
    /// Claude ready, task prompt being sent
    Ready,
    /// Claude actively working on the task
    Working,
    /// Claude finished, waiting for user review
    Paused,
    /// User interacting with Claude directly
    Continuing,
    /// Session ended, ready for cleanup
    Ended,
}

impl ClaudeSessionState {
    pub fn is_active(&self) -> bool {
        matches!(self,
            ClaudeSessionState::Creating |
            ClaudeSessionState::Starting |
            ClaudeSessionState::Ready |
            ClaudeSessionState::Working |
            ClaudeSessionState::Continuing
        )
    }

    pub fn label(&self) -> &'static str {
        match self {
            ClaudeSessionState::NotStarted => "Not Started",
            ClaudeSessionState::Creating => "Creating...",
            ClaudeSessionState::Starting => "Starting...",
            ClaudeSessionState::Ready => "Ready",
            ClaudeSessionState::Working => "Working",
            ClaudeSessionState::Paused => "Paused",
            ClaudeSessionState::Continuing => "Continuing",
            ClaudeSessionState::Ended => "Ended",
        }
    }
}

/// Mode of Claude session management
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SessionMode {
    /// Session managed by the SDK sidecar
    #[default]
    SdkManaged,
    /// User has taken over via interactive CLI (`claude --resume`)
    CliInteractive,
    /// Modal closed, waiting for CLI to exit before resuming SDK
    WaitingForCliExit,
}

/// A task to be executed by Claude Code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub images: Vec<PathBuf>,
    pub claude_session_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Visual divider below this task
    #[serde(default)]
    pub divider_below: bool,
    /// Visual divider above this task (for top-of-column dividers)
    #[serde(default)]
    pub divider_above: bool,
    /// Optional title for the divider below
    #[serde(default)]
    pub divider_title: Option<String>,
    /// Optional title for the divider above
    #[serde(default)]
    pub divider_above_title: Option<String>,

    // === Worktree isolation fields ===

    /// Path to the git worktree for this task
    #[serde(default)]
    pub worktree_path: Option<PathBuf>,
    /// Git branch name for this task (claude/{task-id})
    #[serde(default)]
    pub git_branch: Option<String>,
    /// Tmux window name for this task's Claude session
    #[serde(default)]
    pub tmux_window: Option<String>,
    /// Current state of the Claude session
    #[serde(default)]
    pub session_state: ClaudeSessionState,
    /// Whether session is SDK-managed or CLI-interactive
    #[serde(default)]
    pub session_mode: SessionMode,

    // === Task queueing ===

    /// If set, this task is queued to run after the specified task finishes
    /// (in the same Claude session/worktree)
    #[serde(default)]
    pub queued_for_session: Option<Uuid>,

    // === Activity tracking (for merge/rebase feedback) ===

    /// When the task entered Accepting state (for elapsed time display)
    #[serde(default)]
    pub accepting_started_at: Option<DateTime<Utc>>,
    /// Last time we received activity (Working/ToolUse event)
    #[serde(default)]
    pub last_activity_at: Option<DateTime<Utc>>,
    /// Name of the last tool used (for activity display)
    #[serde(default)]
    pub last_tool_name: Option<String>,
}

impl Task {
    pub fn new(title: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            title,
            description: String::new(),
            status: TaskStatus::Planned,
            images: Vec::new(),
            claude_session_id: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            divider_below: false,
            divider_above: false,
            divider_title: None,
            divider_above_title: None,
            // Worktree fields
            worktree_path: None,
            git_branch: None,
            tmux_window: None,
            session_state: ClaudeSessionState::NotStarted,
            session_mode: SessionMode::SdkManaged,
            // Queueing
            queued_for_session: None,
            // Activity tracking
            accepting_started_at: None,
            last_activity_at: None,
            last_tool_name: None,
        }
    }

    /// Check if this task has an active worktree session
    pub fn has_active_session(&self) -> bool {
        self.worktree_path.is_some() && self.session_state.is_active()
    }

    /// Check if this task can be started (not already active)
    pub fn can_start(&self) -> bool {
        matches!(self.status, TaskStatus::Planned | TaskStatus::Queued)
            && !self.has_active_session()
    }

    /// Check if this task can be continued (in review with a session)
    pub fn can_continue(&self) -> bool {
        self.status == TaskStatus::Review
            && self.worktree_path.is_some()
            && matches!(self.session_state, ClaudeSessionState::Paused | ClaudeSessionState::Ended)
    }

    pub fn with_description(mut self, description: String) -> Self {
        self.description = description;
        self
    }

    pub fn with_image(mut self, image_path: PathBuf) -> Self {
        self.images.push(image_path);
        self
    }
}

/// Task status in the Kanban workflow
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TaskStatus {
    #[default]
    Planned,
    Queued,
    InProgress,
    NeedsInput,
    Review,
    Accepting, // Rebasing onto main before accepting
    Done,
}

impl TaskStatus {
    pub fn label(&self) -> &'static str {
        match self {
            TaskStatus::Planned => "Planned",
            TaskStatus::Queued => "Queued",
            TaskStatus::InProgress => "In Progress",
            TaskStatus::NeedsInput => "Needs Input",
            TaskStatus::Review => "Review",
            TaskStatus::Accepting => "Accepting",
            TaskStatus::Done => "Done",
        }
    }

    /// Get all status values that have their own columns (Accepting is shown in Review column)
    pub fn all() -> [TaskStatus; 6] {
        [
            TaskStatus::Planned,
            TaskStatus::Queued,
            TaskStatus::InProgress,
            TaskStatus::NeedsInput,
            TaskStatus::Review,
            TaskStatus::Done,
        ]
    }

    /// Get array index for this status (for column_scroll_offsets)
    /// Accepting tasks appear in the Review column
    pub fn index(&self) -> usize {
        match self {
            TaskStatus::Planned => 0,
            TaskStatus::Queued => 1,
            TaskStatus::InProgress => 2,
            TaskStatus::NeedsInput => 3,
            TaskStatus::Review | TaskStatus::Accepting => 4,
            TaskStatus::Done => 5,
        }
    }
}

/// UI state (not persisted)
pub struct UiState {
    pub focus: FocusArea,
    pub editor_state: EditorState,
    pub editor_event_handler: EditorEventHandler,
    pub selected_task_idx: Option<usize>,
    /// The ID of the currently selected task (source of truth for selection)
    pub selected_task_id: Option<Uuid>,
    pub selected_column: TaskStatus,
    pub show_help: bool,
    pub pending_confirmation: Option<PendingConfirmation>,
    pub status_message: Option<String>,
    /// If set, we're editing an existing task instead of creating a new one
    pub editing_task_id: Option<Uuid>,
    /// If set, we're editing a divider's title (task_id that has the divider)
    pub editing_divider_id: Option<Uuid>,
    /// If true, we're editing a divider_above (vs divider_below)
    pub editing_divider_is_above: bool,
    /// Scroll offset for long task titles (marquee effect)
    pub title_scroll_offset: usize,
    /// Delay counter before scrolling starts (ticks to wait)
    pub title_scroll_delay: usize,
    /// Pending images to attach to next created task
    pub pending_images: Vec<PathBuf>,
    /// If true, a divider below is selected (below the task at selected_task_idx)
    pub selected_is_divider: bool,
    /// If true, a divider above is selected (above the first task, only valid when selected_task_idx == 0)
    pub selected_is_divider_above: bool,
    /// Animation frame counter for spinners
    pub animation_frame: usize,
    /// Last scroll position (visual index) for each column, preserved when leaving
    /// Order: Planned, Queued, InProgress, NeedsInput, Review, Done
    pub column_scroll_offsets: [usize; 6],

    // Queue dialog state
    /// Task ID being queued (None if queue dialog is closed)
    pub queue_dialog_task_id: Option<Uuid>,
    /// Selected index in the queue dialog session list
    pub queue_dialog_selected_idx: usize,

    // Applied changes state
    /// Task ID whose changes are currently applied to main worktree (for testing)
    /// When set, user can press 'u' to unapply the changes
    pub applied_task_id: Option<Uuid>,
    /// Stash ref created when applying task changes (to restore original work on unapply)
    pub applied_stash_ref: Option<String>,

    // Task preview modal
    /// If true, show the task preview modal for the selected task
    pub show_task_preview: bool,

    // Interactive terminal modal
    /// If set, the interactive modal is open for this task
    pub interactive_modal: Option<InteractiveModal>,
}

/// State for the interactive Claude terminal modal
#[derive(Debug, Clone)]
pub struct InteractiveModal {
    /// Task being interacted with
    pub task_id: Uuid,
    /// Tmux target for this session (e.g., "kc-project:task-abc123")
    pub tmux_target: String,
    /// Captured terminal output (parsed vt100)
    pub terminal_buffer: String,
    /// Scroll offset in the terminal output
    pub scroll_offset: usize,
}

/// Create vim mode handler with custom keybindings
fn create_vim_handler() -> EditorEventHandler {
    let mut key_handler = KeyEventHandler::vim_mode();

    // Add dw (delete word) in normal mode - selects inner word and deletes it
    key_handler.insert(
        KeyEventRegister::n(vec![KeyEvent::Char('d'), KeyEvent::Char('w')]),
        Composed::new(SelectInnerWord).chain(DeleteSelection),
    );

    // Add diw (delete inner word) explicitly too
    key_handler.insert(
        KeyEventRegister::n(vec![KeyEvent::Char('d'), KeyEvent::Char('i'), KeyEvent::Char('w')]),
        Composed::new(SelectInnerWord).chain(DeleteSelection),
    );

    EditorEventHandler::new(key_handler)
}

impl Default for UiState {
    fn default() -> Self {
        let mut editor_state = EditorState::default();
        // Ensure we're in insert mode for text input
        editor_state.mode = EditorMode::Insert;

        Self {
            focus: FocusArea::default(),
            editor_state,
            // Use vim mode with custom keybindings
            editor_event_handler: create_vim_handler(),
            selected_task_idx: None,
            selected_task_id: None,
            selected_column: TaskStatus::default(),
            show_help: false,
            pending_confirmation: None,
            status_message: None,
            editing_task_id: None,
            editing_divider_id: None,
            editing_divider_is_above: false,
            title_scroll_offset: 0,
            title_scroll_delay: 0,
            pending_images: Vec::new(),
            selected_is_divider: false,
            selected_is_divider_above: false,
            animation_frame: 0,
            column_scroll_offsets: [0; 6],
            queue_dialog_task_id: None,
            queue_dialog_selected_idx: 0,
            applied_task_id: None,
            applied_stash_ref: None,
            show_task_preview: false,
            interactive_modal: None,
        }
    }
}

impl UiState {
    /// Check if the interactive modal is open
    pub fn is_interactive_modal_open(&self) -> bool {
        self.interactive_modal.is_some()
    }
}

impl UiState {
    /// Get the current text content from the editor
    pub fn get_input_text(&self) -> String {
        self.editor_state.lines.to_string()
    }

    /// Set the editor text content (starts in Insert mode)
    pub fn set_input_text(&mut self, text: &str) {
        self.editor_state = EditorState::new(Lines::from(text));
        // Ensure we're in insert mode
        self.editor_state.mode = EditorMode::Insert;
    }

    /// Set the editor text content for editing (starts in Normal mode)
    pub fn set_input_text_normal_mode(&mut self, text: &str) {
        self.editor_state = EditorState::new(Lines::from(text));
        self.editor_state.mode = EditorMode::Normal;
    }

    /// Clear the editor text
    pub fn clear_input(&mut self) {
        self.editor_state = EditorState::default();
        // Ensure we're in insert mode
        self.editor_state.mode = EditorMode::Insert;
    }

    /// Check if the queue dialog is open
    pub fn is_queue_dialog_open(&self) -> bool {
        self.queue_dialog_task_id.is_some()
    }
}

/// A pending confirmation dialog
#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    pub message: String,
    pub action: PendingAction,
}

/// Actions that require user confirmation
#[derive(Debug, Clone)]
pub enum PendingAction {
    InstallHooks,
    ReloadClaude,
    DeleteTask(Uuid),
    /// Mark task as done and clean up worktree (when nothing to merge)
    MarkDoneNoMerge(Uuid),
}

/// Which UI element has focus
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FocusArea {
    #[default]
    KanbanBoard,
    TaskInput,
    ProjectTabs,
    OutputViewer,
}

/// Signal received from Claude Code hooks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSignal {
    pub event: String,
    pub session_id: String,
    pub project_dir: PathBuf,
    pub timestamp: DateTime<Utc>,
    pub transcript_path: Option<PathBuf>,
}
