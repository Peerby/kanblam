use crate::message::Message;
use crate::model::{AppModel, FocusArea, MainWorktreeOperation, PendingAction, PendingConfirmation, Project, Task, TaskStatus, SessionMode};
use crate::notify;
use crate::sidecar::SidecarClient;
use crate::ui::logo::EyeAnimation;
use anyhow::Result;
use chrono::Utc;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Channel sender for async task results
pub type AsyncTaskSender = mpsc::UnboundedSender<Message>;

/// Check if a project is the "bootstrap" project (i.e., we're developing KanBlam itself).
/// Returns true if the currently running executable lives within the project's directory.
fn is_bootstrap_project(project: &Project) -> bool {
    let Ok(exe_path) = std::env::current_exe() else {
        return false;
    };

    // Canonicalize both paths to handle symlinks and normalize
    let Ok(exe_canonical) = exe_path.canonicalize() else {
        return false;
    };
    let Ok(project_canonical) = project.working_dir.canonicalize() else {
        return false;
    };

    // Check if the running binary lives inside this project
    exe_canonical.starts_with(&project_canonical)
}

/// Application state and update logic (TEA pattern)
pub struct App {
    pub model: AppModel,
    pub should_quit: bool,
    /// Whether to restart the app (for hot reload after apply)
    pub should_restart: bool,
    /// Sidecar client for SDK session management (if available)
    pub sidecar_client: Option<SidecarClient>,
    /// Channel to send results from async background tasks back to the main loop
    pub async_sender: Option<AsyncTaskSender>,
}

impl App {
    pub fn new() -> Self {
        Self {
            model: AppModel::default(),
            should_quit: false,
            should_restart: false,
            sidecar_client: None,
            async_sender: None,
        }
    }

    pub fn with_model(model: AppModel) -> Self {
        Self {
            model,
            should_quit: false,
            should_restart: false,
            sidecar_client: None,
            async_sender: None,
        }
    }

    pub fn with_sidecar(mut self, client: Option<SidecarClient>) -> Self {
        self.sidecar_client = client;
        self
    }

    pub fn with_async_sender(mut self, sender: AsyncTaskSender) -> Self {
        self.async_sender = Some(sender);
        self
    }

    /// Sync selected_task_idx based on selected_task_id
    /// Call this after any operation that might change task order/status
    pub fn sync_selection(&mut self) {
        let task_id = self.model.ui_state.selected_task_id;
        let column = self.model.ui_state.selected_column;

        if let Some(task_id) = task_id {
            // Gather task info without holding borrow
            let (new_idx, fallback_id, is_empty) = if let Some(project) = self.model.active_project() {
                let tasks = project.tasks_by_status(column);
                let new_idx = tasks.iter().position(|t| t.id == task_id);
                let fallback_id = tasks.first().map(|t| t.id);
                (new_idx, fallback_id, tasks.is_empty())
            } else {
                (None, None, true)
            };

            // Now update the ui_state
            if let Some(idx) = new_idx {
                self.model.ui_state.selected_task_idx = Some(idx);
            } else {
                // Task no longer in this column - clear selection
                self.model.ui_state.selected_task_idx = if !is_empty { Some(0) } else { None };
                self.model.ui_state.selected_task_id = fallback_id;
            }
        }
    }

    /// Set selection to a specific task (sets both idx and id)
    pub fn select_task(&mut self, idx: Option<usize>) {
        self.model.ui_state.selected_task_idx = idx;
        if let Some(i) = idx {
            if let Some(project) = self.model.active_project() {
                let tasks = project.tasks_by_status(self.model.ui_state.selected_column);
                self.model.ui_state.selected_task_id = tasks.get(i).map(|t| t.id);
            } else {
                self.model.ui_state.selected_task_id = None;
            }
        } else {
            self.model.ui_state.selected_task_id = None;
        }
    }

    /// Calculate and save the current visual scroll position for the current column
    /// Call this before switching to a different column
    fn save_scroll_offset(&mut self) {
        let column = self.model.ui_state.selected_column;
        let task_idx = self.model.ui_state.selected_task_idx;

        let visual_idx = task_idx.unwrap_or(0);
        self.model.ui_state.column_scroll_offsets[column.index()] = visual_idx;
    }

    /// Restore scroll position when entering a column
    /// Returns the task index to select based on saved offset
    fn get_restored_task_idx(&self, column: TaskStatus) -> Option<usize> {
        let saved_offset = self.model.ui_state.column_scroll_offsets[column.index()];

        if let Some(project) = self.model.active_project() {
            let tasks = project.tasks_by_status(column);
            if tasks.is_empty() {
                return None;
            }
            // Clamp to valid range
            Some(saved_offset.min(tasks.len().saturating_sub(1)))
        } else {
            None
        }
    }

    /// Update application state based on message (TEA pattern)
    pub fn update(&mut self, msg: Message) -> Vec<Message> {
        let mut commands = Vec::new();

        match msg {
            Message::CreateTask(title) => {
                // Take pending images before borrowing project
                let pending_images = std::mem::take(&mut self.model.ui_state.pending_images);
                let task_id;
                let title_len = title.len();
                if let Some(project) = self.model.active_project_mut() {
                    let mut task = Task::new(title);
                    task_id = task.id;
                    // Attach pending images
                    task.images = pending_images;
                    // Insert at beginning so newest tasks appear first in Planned
                    project.tasks.insert(0, task);
                } else {
                    task_id = uuid::Uuid::nil();
                }
                // Clear editor after creating task
                self.model.ui_state.clear_input();
                // Focus on the kanban board and select the new task
                // (New tasks in Planned are sorted newest first, so index 0)
                self.model.ui_state.focus = FocusArea::KanbanBoard;
                self.model.ui_state.selected_column = TaskStatus::Planned;
                self.model.ui_state.selected_task_idx = Some(0);
                self.model.ui_state.title_scroll_offset = 0;
                self.model.ui_state.title_scroll_delay = 0;

                // Request title summarization if title is long (> 40 chars)
                if title_len > 40 && !task_id.is_nil() {
                    commands.push(Message::RequestTitleSummary { task_id });
                }
            }

            Message::EditTask(task_id) => {
                // Find the task and get its title (clone to avoid borrow issues)
                let task_title = self.model.active_project()
                    .and_then(|p| p.tasks.iter().find(|t| t.id == task_id))
                    .map(|t| t.title.clone());

                if let Some(title) = task_title {
                    // Set editor content (starts in Normal mode for editing)
                    self.model.ui_state.set_input_text_normal_mode(&title);
                    // Set editing mode
                    self.model.ui_state.editing_task_id = Some(task_id);
                    self.model.ui_state.focus = FocusArea::TaskInput;
                }
            }

            Message::UpdateTask { task_id, title } => {
                let title_len = title.len();
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.title = title;
                        // Clear short_title when title changes (will be regenerated if needed)
                        task.short_title = None;
                        task.log_activity("User edited task");
                    }
                }
                // Clear editing state and editor
                self.model.ui_state.editing_task_id = None;
                self.model.ui_state.clear_input();
                self.model.ui_state.focus = FocusArea::KanbanBoard;

                // Request title summarization if title is long (> 40 chars)
                if title_len > 40 {
                    commands.push(Message::RequestTitleSummary { task_id });
                }
            }

            Message::CancelEdit => {
                // Clear editing state and editor
                self.model.ui_state.editing_task_id = None;
                self.model.ui_state.clear_input();
                self.model.ui_state.focus = FocusArea::KanbanBoard;
            }

            Message::DeleteTask(task_id) => {
                // Stop SDK session first (if running)
                if let Some(ref client) = self.sidecar_client {
                    let _ = client.stop_session(task_id);
                }

                // Get all necessary info before mutating (for worktree cleanup)
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (
                            p.slug(),
                            p.working_dir.clone(),
                            t.tmux_window.clone(),
                            t.worktree_path.clone(),
                        ))
                });

                // Clean up worktree and associated resources if they exist
                if let Some((project_slug, project_dir, window_name, worktree_path)) = task_info {
                    // Kill tmux window if exists
                    if let Some(ref window) = window_name {
                        let _ = crate::tmux::kill_task_window(&project_slug, window);
                    }

                    // Kill any detached tmux sessions for this task
                    crate::tmux::kill_task_sessions(&task_id.to_string());

                    // Remove worktree
                    if let Some(ref wt_path) = worktree_path {
                        if let Err(e) = crate::worktree::remove_worktree(&project_dir, wt_path) {
                            commands.push(Message::SetStatusMessage(Some(
                                format!("Warning: Could not remove worktree: {}", e)
                            )));
                        }
                    }

                    // Delete branch
                    if let Err(e) = crate::worktree::delete_branch(&project_dir, task_id) {
                        // Don't warn if branch doesn't exist (task may never have been started)
                        let err_str = e.to_string();
                        if !err_str.contains("not found") && !err_str.contains("does not exist") {
                            commands.push(Message::SetStatusMessage(Some(
                                format!("Warning: Could not delete branch: {}", e)
                            )));
                        }
                    }
                }

                // Remove the task from the project
                if let Some(project) = self.model.active_project_mut() {
                    project.tasks.retain(|t| t.id != task_id);
                }
            }

            Message::MoveTask { task_id, to_status } => {
                let mut follow_to_planned = false;

                // Get task info for session cleanup before mutating (needed for Done)
                let cleanup_info = if to_status == TaskStatus::Done {
                    self.model.active_project().and_then(|p| {
                        p.tasks.iter()
                            .find(|t| t.id == task_id)
                            .map(|t| (p.slug(), t.tmux_window.clone()))
                    })
                } else {
                    None
                };

                if let Some(project) = self.model.active_project_mut() {
                    // Special handling for moving to Planned: move to top of list
                    if to_status == TaskStatus::Planned {
                        // Find and remove the task
                        if let Some(idx) = project.tasks.iter().position(|t| t.id == task_id) {
                            let mut task = project.tasks.remove(idx);
                            task.status = TaskStatus::Planned;
                            // Insert at the beginning (will be first in Planned column)
                            project.tasks.insert(0, task);
                            follow_to_planned = true;
                        }
                    } else if to_status == TaskStatus::Done {
                        // Special handling for moving to Done: move to end of list
                        if let Some(idx) = project.tasks.iter().position(|t| t.id == task_id) {
                            let mut task = project.tasks.remove(idx);
                            task.status = TaskStatus::Done;
                            task.completed_at = Some(Utc::now());
                            // Clear session-related fields
                            task.tmux_window = None;
                            task.session_state = crate::model::ClaudeSessionState::Ended;
                            // Push to end (will be last in Done column)
                            project.tasks.push(task);
                        }
                    } else if to_status == TaskStatus::Review {
                        // Special handling for moving to Review: move to end of Review tasks
                        // This ensures the first task to finish appears at the top
                        project.move_task_to_end_of_status(task_id, TaskStatus::Review);
                    } else if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.status = to_status;
                        if to_status == TaskStatus::InProgress {
                            task.started_at = Some(Utc::now());
                        }
                    }
                    // Clear attention flag if no more review tasks
                    if project.review_count() == 0 {
                        project.needs_attention = false;
                        // Check if any other projects need attention
                        let any_need_attention = self.model.projects.iter()
                            .any(|p| p.needs_attention);
                        if !any_need_attention {
                            notify::clear_attention_indicator();
                        }
                    }
                }

                // Kill any running sessions when moving to Done
                if let Some((project_slug, window_name)) = cleanup_info {
                    // Kill tmux window if exists
                    if let Some(ref window) = window_name {
                        let _ = crate::tmux::kill_task_window(&project_slug, window);
                    }
                    // Kill any detached Claude/test sessions for this task
                    crate::tmux::kill_task_sessions(&task_id.to_string());
                }

                // Move cursor to follow the task to Planned
                if follow_to_planned {
                    self.model.ui_state.selected_column = TaskStatus::Planned;
                    self.model.ui_state.selected_task_idx = Some(0);
                }
            }

            Message::MoveTaskUp => {
                // Move selected task up within its column
                if let Some(selected_idx) = self.model.ui_state.selected_task_idx {
                    if selected_idx > 0 {
                        let status = self.model.ui_state.selected_column;
                        // Get task IDs from the display view
                        let (task_id, above_task_id) = {
                            if let Some(project) = self.model.active_project() {
                                let tasks = project.tasks_by_status(status);
                                if selected_idx < tasks.len() {
                                    (Some(tasks[selected_idx].id), Some(tasks[selected_idx - 1].id))
                                } else {
                                    (None, None)
                                }
                            } else {
                                (None, None)
                            }
                        };

                        if let (Some(task_id), Some(above_id)) = (task_id, above_task_id) {
                            if let Some(project) = self.model.active_project_mut() {
                                // Find actual indices in the tasks Vec and swap
                                let idx_a = project.tasks.iter().position(|t| t.id == task_id);
                                let idx_b = project.tasks.iter().position(|t| t.id == above_id);
                                if let (Some(a), Some(b)) = (idx_a, idx_b) {
                                    project.tasks.swap(a, b);
                                    // Selection follows the task
                                    self.model.ui_state.selected_task_idx = Some(selected_idx - 1);
                                }
                            }
                        }
                    }
                }
            }

            Message::MoveTaskDown => {
                // Move selected task down within its column
                if let Some(selected_idx) = self.model.ui_state.selected_task_idx {
                    let status = self.model.ui_state.selected_column;
                    // Get task IDs from the display view
                    let (task_id, below_task_id) = {
                        if let Some(project) = self.model.active_project() {
                            let tasks = project.tasks_by_status(status);
                            if selected_idx + 1 < tasks.len() {
                                (Some(tasks[selected_idx].id), Some(tasks[selected_idx + 1].id))
                            } else {
                                (None, None)
                            }
                        } else {
                            (None, None)
                        }
                    };

                    if let (Some(task_id), Some(below_id)) = (task_id, below_task_id) {
                        if let Some(project) = self.model.active_project_mut() {
                            // Find actual indices in the tasks Vec and swap
                            let idx_a = project.tasks.iter().position(|t| t.id == task_id);
                            let idx_b = project.tasks.iter().position(|t| t.id == below_id);
                            if let (Some(a), Some(b)) = (idx_a, idx_b) {
                                project.tasks.swap(a, b);
                                // Selection follows the task
                                self.model.ui_state.selected_task_idx = Some(selected_idx + 1);
                            }
                        }
                    }
                }
            }

            Message::StartTask(task_id) => {
                // Legacy StartTask handler for non-git repos
                // For git repos, use StartTaskWithWorktree instead
                if let Some(project) = self.model.active_project_mut() {
                    // Get task status first
                    let task_status = project.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| t.status);

                    // Handle reset tasks from Review or NeedsInput (legacy path)
                    if matches!(task_status, Some(TaskStatus::Review) | Some(TaskStatus::NeedsInput)) {
                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                            task.status = TaskStatus::InProgress;
                            project.needs_attention = false;
                            notify::clear_attention_indicator();
                            commands.push(Message::SetStatusMessage(Some(
                                "Task reset".to_string()
                            )));
                        }
                        return commands;
                    }

                    // Check if another task is active (InProgress or NeedsInput)
                    if project.has_active_task() {
                        // Queue the task instead of starting it (only if Planned)
                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                            if task.status == TaskStatus::Planned {
                                task.status = TaskStatus::Queued;
                                commands.push(Message::SetStatusMessage(Some(
                                    "Task queued - waiting for current task to complete".to_string()
                                )));
                            }
                        }
                    } else {
                        // For non-git repos, just show an error - worktree isolation required
                        commands.push(Message::Error(
                            "Cannot start task: project is not a git repository. Worktree isolation requires git.".to_string()
                        ));
                    }
                }
            }

            // === Worktree-based task lifecycle ===

            Message::StartTaskWithWorktree(task_id) => {
                // Get project info first to validate
                let project_info = self.model.active_project().map(|p| {
                    (p.working_dir.clone(), p.is_git_repo())
                });

                if let Some((project_dir, is_git)) = project_info {
                    if !is_git {
                        commands.push(Message::Error(
                            "Project is not a git repository. Worktree isolation requires git.".to_string()
                        ));
                        return commands;
                    }

                    // Update task state immediately for UI feedback
                    if let Some(project) = self.model.active_project_mut() {
                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                            task.session_state = crate::model::ClaudeSessionState::Creating;
                            task.status = TaskStatus::Queued;
                            task.started_at = Some(Utc::now());
                            task.log_activity("User started task");
                        }
                    }

                    // Defer the actual worktree creation to allow UI to render first
                    commands.push(Message::CreateWorktree { task_id, project_dir });
                }
            }

            Message::CreateWorktree { task_id, project_dir } => {
                // Spawn worktree creation in background to keep UI responsive
                if let Some(sender) = self.async_sender.clone() {
                    let project_dir_clone = project_dir.clone();
                    tokio::spawn(async move {
                        // Run blocking git operations in a separate thread
                        let result = tokio::task::spawn_blocking(move || {
                            crate::worktree::create_worktree(&project_dir_clone, task_id)
                        }).await;

                        let msg = match result {
                            Ok(Ok(worktree_path)) => {
                                Message::WorktreeCreated { task_id, worktree_path, project_dir }
                            }
                            Ok(Err(e)) => {
                                Message::WorktreeCreationFailed { task_id, error: e.to_string() }
                            }
                            Err(e) => {
                                Message::WorktreeCreationFailed { task_id, error: format!("Task panicked: {}", e) }
                            }
                        };

                        let _ = sender.send(msg);
                    });
                } else {
                    // Fallback to sync if no async sender (shouldn't happen in normal operation)
                    match crate::worktree::create_worktree(&project_dir, task_id) {
                        Ok(worktree_path) => {
                            commands.push(Message::WorktreeCreated { task_id, worktree_path, project_dir });
                        }
                        Err(e) => {
                            commands.push(Message::WorktreeCreationFailed { task_id, error: e.to_string() });
                        }
                    }
                }
            }

            Message::UpdateTaskSessionState { task_id, state } => {
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.session_state = state;
                    }
                }
            }

            Message::ContinueTask(task_id) => {
                // Get project slug and task window
                let switch_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .and_then(|t| t.tmux_window.as_ref().map(|w| (p.slug(), w.clone())))
                });

                if let Some((project_slug, window_name)) = switch_info {
                    // Check if window still exists
                    if crate::tmux::task_window_exists(&project_slug, &window_name) {
                        // Switch to the window
                        if let Err(e) = crate::tmux::switch_to_task_window(&project_slug, &window_name) {
                            commands.push(Message::Error(format!("Failed to switch to task window: {}", e)));
                        } else {
                            // Update state - only update session state, NOT task status
                            // Task status should only change when user actually provides input
                            // (via input-provided signal from hook), not just by viewing the window
                            if let Some(project) = self.model.active_project_mut() {
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                    task.session_state = crate::model::ClaudeSessionState::Continuing;
                                    task.log_activity("User continued task");
                                    // Don't change task.status - let the hook signals manage it
                                }
                                project.needs_attention = false;
                                notify::clear_attention_indicator();
                            }
                        }
                    } else {
                        commands.push(Message::SetStatusMessage(Some(
                            "Task window no longer exists. Reset with Enter.".to_string()
                        )));
                    }
                } else {
                    commands.push(Message::SetStatusMessage(Some(
                        "No active session for this task.".to_string()
                    )));
                }
            }

            Message::AcceptTask(task_id) => {
                // Get all necessary info before mutating
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (
                            p.slug(),
                            p.working_dir.clone(),
                            t.tmux_window.clone(),
                            t.worktree_path.clone(),
                        ))
                });

                if let Some((project_slug, project_dir, window_name, worktree_path)) = task_info {
                    // CRITICAL: Commit any uncommitted changes in the worktree FIRST
                    // This ensures we don't lose work that Claude did but didn't commit
                    if let Some(ref wt_path) = worktree_path {
                        match crate::worktree::commit_worktree_changes(wt_path, task_id) {
                            Ok(true) => {
                                // Changes were committed
                            }
                            Ok(false) => {
                                // Nothing to commit, that's fine
                            }
                            Err(e) => {
                                commands.push(Message::Error(format!(
                                    "Failed to commit worktree changes: {}. Changes preserved in worktree.",
                                    e
                                )));
                                return commands;
                            }
                        }
                    }

                    // Verify there are changes to merge before proceeding
                    match crate::worktree::has_changes_to_merge(&project_dir, task_id) {
                        Ok(true) => {
                            // Good, there are changes to merge
                        }
                        Ok(false) => {
                            // Nothing to merge - ask if user wants to mark done and clean up anyway
                            commands.push(Message::ShowConfirmation {
                                message: "Nothing to merge. Mark task as done and clean up worktree? (y/n)".to_string(),
                                action: PendingAction::MarkDoneNoMerge(task_id),
                            });
                            return commands;
                        }
                        Err(e) => {
                            commands.push(Message::Error(format!("Failed to check for changes: {}", e)));
                            return commands;
                        }
                    }

                    // Kill tmux window if exists
                    if let Some(ref window) = window_name {
                        let _ = crate::tmux::kill_task_window(&project_slug, window);
                    }

                    // Check for uncommitted changes on main - ask user what to do
                    match crate::worktree::has_uncommitted_changes(&project_dir) {
                        Ok(true) => {
                            // Main has uncommitted changes - ask user what to do
                            self.model.ui_state.confirmation_scroll_offset = 0;
                            self.model.ui_state.pending_confirmation = Some(PendingConfirmation {
                                message: "Main worktree has uncommitted changes.\n\n\
                                         c=commit changes, s=stash changes, n=cancel".to_string(),
                                action: PendingAction::DirtyMainBeforeMerge { task_id },
                                animation_tick: 20,
                            });
                            return commands;
                        }
                        Ok(false) => {
                            // No uncommitted changes, proceed
                        }
                        Err(e) => {
                            commands.push(Message::Error(format!(
                                "Failed to check main status: {}", e
                            )));
                            return commands;
                        }
                    }

                    // Kill any detached Claude/test sessions for this task
                    crate::tmux::kill_task_sessions(&task_id.to_string());

                    // Merge branch to main
                    if let Err(e) = crate::worktree::merge_branch(&project_dir, task_id) {
                        commands.push(Message::Error(format!(
                            "Merge failed: {}. Resolve manually in the worktree, then discard.",
                            e
                        )));
                        return commands;
                    }

                    // Remove worktree
                    if let Some(ref wt_path) = worktree_path {
                        if let Err(e) = crate::worktree::remove_worktree(&project_dir, wt_path) {
                            commands.push(Message::SetStatusMessage(Some(
                                format!("Warning: Could not remove worktree: {}", e)
                            )));
                        }
                        // Clean up trust entry from Claude's config
                        let _ = crate::worktree::remove_worktree_trust(wt_path);
                    }

                    // Delete branch
                    if let Err(e) = crate::worktree::delete_branch(&project_dir, task_id) {
                        commands.push(Message::SetStatusMessage(Some(
                            format!("Warning: Could not delete branch: {}", e)
                        )));
                    }

                    // Update task and move to end of list (bottom of Done column)
                    if let Some(project) = self.model.active_project_mut() {
                        if let Some(idx) = project.tasks.iter().position(|t| t.id == task_id) {
                            let mut task = project.tasks.remove(idx);
                            task.status = TaskStatus::Done;
                            task.completed_at = Some(Utc::now());
                            task.worktree_path = None;
                            task.tmux_window = None;
                            task.session_state = crate::model::ClaudeSessionState::Ended;
                            task.log_activity("User merged changes");
                            project.tasks.push(task);
                        }
                        project.needs_attention = project.review_count() > 0;
                        if !project.needs_attention {
                            notify::clear_attention_indicator();
                        }
                    }

                    commands.push(Message::SetStatusMessage(Some(
                        "Task accepted and merged to main.".to_string()
                    )));
                }
            }

            Message::SmartAcceptTask(task_id) => {
                // Check if this task's changes are already applied to main
                // If so, we can just commit them directly (skip merge)
                let is_already_applied = self.model.active_project()
                    .map(|p| p.applied_task_id == Some(task_id))
                    .unwrap_or(false);

                if is_already_applied {
                    // Changes already applied - show confirmation to commit them
                    self.model.ui_state.confirmation_scroll_offset = 0;
                    self.model.ui_state.pending_confirmation = Some(PendingConfirmation {
                        message: "Task changes are already applied. Commit them to main and complete the task?".to_string(),
                        action: PendingAction::CommitAppliedChanges(task_id),
                        animation_tick: 20,
                    });
                    return commands;
                }

                // Get task info to check if rebase is needed
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (
                            p.working_dir.clone(),
                            t.worktree_path.clone(),
                            t.git_branch.clone(),
                            t.status,
                        ))
                });

                if let Some((project_dir, worktree_path, git_branch, current_status)) = task_info {
                    // Don't process if already accepting
                    if current_status == TaskStatus::Accepting {
                        return commands;
                    }

                    // Try to acquire exclusive lock on main worktree
                    if let Some(project) = self.model.active_project_mut() {
                        if let Err(reason) = project.try_lock_main_worktree(task_id, MainWorktreeOperation::Accepting) {
                            commands.push(Message::Error(reason));
                            return commands;
                        }
                    }

                    // CRITICAL: Commit any uncommitted changes in the worktree FIRST
                    // This ensures we don't lose work that Claude did but didn't commit
                    if let Some(ref wt_path) = worktree_path {
                        match crate::worktree::commit_worktree_changes(wt_path, task_id) {
                            Ok(_) => {
                                // Changes committed (or nothing to commit)
                            }
                            Err(e) => {
                                commands.push(Message::Error(format!(
                                    "Failed to commit worktree changes: {}. Changes preserved in worktree.",
                                    e
                                )));
                                return commands;
                            }
                        }
                    }

                    // Commit any uncommitted changes on main BEFORE checking rebase
                    // This ensures the worktree properly detects it needs to integrate
                    // with main's latest state (including uncommitted work)
                    match crate::worktree::commit_main_changes(&project_dir) {
                        Ok(true) => {
                            // Main had uncommitted changes that are now committed
                            // The rebase check below will detect the worktree is behind
                        }
                        Ok(false) => {
                            // Nothing to commit on main, that's fine
                        }
                        Err(e) => {
                            commands.push(Message::Error(format!(
                                "Failed to commit main changes: {}",
                                e
                            )));
                            return commands;
                        }
                    }

                    // Check if rebase is needed
                    let needs_rebase = git_branch.is_some() &&
                        crate::worktree::needs_rebase(&project_dir, task_id).unwrap_or(false);

                    if needs_rebase {
                        // Try fast rebase first (no Claude needed)
                        if let Some(ref wt_path) = worktree_path {
                            match crate::worktree::try_fast_rebase(wt_path, &project_dir) {
                                Ok(true) => {
                                    // Fast rebase succeeded, proceed to merge
                                    commands.push(Message::SetStatusMessage(Some(
                                        "✓ Fast rebase succeeded, merging...".to_string()
                                    )));
                                    commands.push(Message::CompleteAcceptTask(task_id));
                                }
                                Ok(false) => {
                                    // Conflicts detected, need Claude to resolve
                                    commands.push(Message::SetStatusMessage(Some(
                                        "Conflicts detected, starting smart merge...".to_string()
                                    )));
                                    commands.push(Message::StartRebaseSession { task_id });
                                }
                                Err(e) => {
                                    // Error during rebase attempt, fallback to Claude
                                    commands.push(Message::SetStatusMessage(Some(
                                        format!("Fast rebase failed ({}), trying smart merge...", e)
                                    )));
                                    commands.push(Message::StartRebaseSession { task_id });
                                }
                            }
                        } else {
                            // No worktree path, fallback to Claude
                            commands.push(Message::StartRebaseSession { task_id });
                        }
                    } else {
                        // No rebase needed - go straight to accept
                        commands.push(Message::CompleteAcceptTask(task_id));
                    }
                }
            }

            Message::CompleteAcceptTask(task_id) => {
                // Verify rebase was successful before merging
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (
                            p.slug(),
                            p.working_dir.clone(),
                            t.tmux_window.clone(),
                            t.worktree_path.clone(),
                            t.status,
                        ))
                });

                if let Some((project_slug, project_dir, window_name, worktree_path, status)) = task_info {
                    // If was accepting, verify rebase succeeded
                    if status == TaskStatus::Accepting {
                        // Check if rebase is still in progress
                        if let Some(ref wt_path) = worktree_path {
                            if crate::worktree::is_rebase_in_progress(wt_path) {
                                commands.push(Message::Error(
                                    "Rebase still in progress. Wait for Claude to finish.".to_string()
                                ));
                                return commands;
                            }
                        }

                        // Verify branch is now on top of main
                        match crate::worktree::verify_rebase_success(&project_dir, task_id) {
                            Ok(true) => {
                                // Rebase successful, continue with merge
                            }
                            Ok(false) => {
                                // Rebase failed - return to Review status
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.status = TaskStatus::Review;
                                        task.session_state = crate::model::ClaudeSessionState::Paused;
                                    }
                                    project.release_main_worktree_lock(task_id);
                                }
                                commands.push(Message::Error(
                                    "Rebase failed. Check the Claude session for errors.".to_string()
                                ));
                                return commands;
                            }
                            Err(e) => {
                                // Error checking - return to Review
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.status = TaskStatus::Review;
                                    }
                                    project.release_main_worktree_lock(task_id);
                                }
                                commands.push(Message::Error(format!("Error verifying rebase: {}", e)));
                                return commands;
                            }
                        }
                    }

                    // CRITICAL: Commit any uncommitted changes in the worktree FIRST
                    if let Some(ref wt_path) = worktree_path {
                        match crate::worktree::commit_worktree_changes(wt_path, task_id) {
                            Ok(_) => {
                                // Changes committed (or nothing to commit)
                            }
                            Err(e) => {
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.status = TaskStatus::Review;
                                    }
                                    project.release_main_worktree_lock(task_id);
                                }
                                commands.push(Message::Error(format!(
                                    "Failed to commit worktree changes: {}. Changes preserved.",
                                    e
                                )));
                                return commands;
                            }
                        }
                    }

                    // Verify there are changes to merge
                    match crate::worktree::has_changes_to_merge(&project_dir, task_id) {
                        Ok(true) => {
                            // Good, there are changes
                        }
                        Ok(false) => {
                            if let Some(project) = self.model.active_project_mut() {
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                    task.status = TaskStatus::Review;
                                }
                                project.release_main_worktree_lock(task_id);
                            }
                            // Nothing to merge - ask if user wants to mark done and clean up anyway
                            commands.push(Message::ShowConfirmation {
                                message: "Nothing to merge. Mark task as done and clean up worktree? (y/n)".to_string(),
                                action: PendingAction::MarkDoneNoMerge(task_id),
                            });
                            return commands;
                        }
                        Err(e) => {
                            if let Some(project) = self.model.active_project_mut() {
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                    task.status = TaskStatus::Review;
                                }
                                project.release_main_worktree_lock(task_id);
                            }
                            commands.push(Message::Error(format!("Failed to check for changes: {}", e)));
                            return commands;
                        }
                    }

                    // Kill tmux window if exists
                    if let Some(ref window) = window_name {
                        let _ = crate::tmux::kill_task_window(&project_slug, window);
                    }

                    // Kill any detached Claude/test sessions for this task
                    crate::tmux::kill_task_sessions(&task_id.to_string());

                    // Merge branch to main (should be fast-forward now)
                    if let Err(e) = crate::worktree::merge_branch(&project_dir, task_id) {
                        // Return to Review status on error
                        if let Some(project) = self.model.active_project_mut() {
                            if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                task.status = TaskStatus::Review;
                            }
                            project.release_main_worktree_lock(task_id);
                        }
                        commands.push(Message::Error(format!(
                            "Merge failed: {}. Try accepting again or resolve manually.",
                            e
                        )));
                        return commands;
                    }

                    // Remove worktree
                    if let Some(ref wt_path) = worktree_path {
                        if let Err(e) = crate::worktree::remove_worktree(&project_dir, wt_path) {
                            commands.push(Message::SetStatusMessage(Some(
                                format!("Warning: Could not remove worktree: {}", e)
                            )));
                        }
                        let _ = crate::worktree::remove_worktree_trust(wt_path);
                    }

                    // Delete branch
                    if let Err(e) = crate::worktree::delete_branch(&project_dir, task_id) {
                        commands.push(Message::SetStatusMessage(Some(
                            format!("Warning: Could not delete branch: {}", e)
                        )));
                    }

                    // Update task and move to end of list (bottom of Done column)
                    if let Some(project) = self.model.active_project_mut() {
                        if let Some(idx) = project.tasks.iter().position(|t| t.id == task_id) {
                            let mut task = project.tasks.remove(idx);
                            task.status = TaskStatus::Done;
                            task.completed_at = Some(Utc::now());
                            task.worktree_path = None;
                            task.tmux_window = None;
                            task.git_branch = None;
                            task.session_state = crate::model::ClaudeSessionState::Ended;
                            project.tasks.push(task);
                        }
                        project.needs_attention = project.review_count() > 0;
                        if !project.needs_attention {
                            notify::clear_attention_indicator();
                        }
                        // Release the lock - merge completed successfully
                        project.release_main_worktree_lock(task_id);
                    }

                    // Trigger celebratory logo shimmer animation
                    commands.push(Message::TriggerLogoShimmer);

                    // Check if there are tracked stashes to offer popping
                    let offer_stash = self.model.active_project()
                        .and_then(|p| p.tracked_stashes.first().cloned());

                    if let Some(stash) = offer_stash {
                        commands.push(Message::OfferPopStash {
                            stash_sha: stash.stash_sha,
                            context: "merge".to_string(),
                        });
                    } else {
                        commands.push(Message::SetStatusMessage(Some(
                            "Task accepted and merged to main.".to_string()
                        )));
                    }
                }
            }

            Message::MergeOnlyTask(task_id) => {
                // Merge changes to main but keep worktree and task in Review
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (
                            p.working_dir.clone(),
                            t.worktree_path.clone(),
                            t.status,
                        ))
                });

                if let Some((project_dir, worktree_path, current_status)) = task_info {
                    // Don't process if already accepting
                    if current_status == TaskStatus::Accepting {
                        return commands;
                    }

                    // Try to acquire exclusive lock on main worktree
                    if let Some(project) = self.model.active_project_mut() {
                        if let Err(reason) = project.try_lock_main_worktree(task_id, MainWorktreeOperation::Accepting) {
                            commands.push(Message::Error(reason));
                            return commands;
                        }
                    }

                    // Commit any uncommitted changes in the worktree FIRST
                    if let Some(ref wt_path) = worktree_path {
                        match crate::worktree::commit_worktree_changes(wt_path, task_id) {
                            Ok(_) => {
                                // Changes committed (or nothing to commit)
                            }
                            Err(e) => {
                                if let Some(project) = self.model.active_project_mut() {
                                    project.release_main_worktree_lock(task_id);
                                }
                                commands.push(Message::Error(format!(
                                    "Failed to commit worktree changes: {}. Changes preserved in worktree.",
                                    e
                                )));
                                return commands;
                            }
                        }
                    }

                    // Commit any uncommitted changes on main BEFORE checking rebase
                    match crate::worktree::commit_main_changes(&project_dir) {
                        Ok(_) => {}
                        Err(e) => {
                            if let Some(project) = self.model.active_project_mut() {
                                project.release_main_worktree_lock(task_id);
                            }
                            commands.push(Message::Error(format!(
                                "Failed to commit main changes: {}",
                                e
                            )));
                            return commands;
                        }
                    }

                    // Check if rebase is needed
                    let needs_rebase = crate::worktree::needs_rebase(&project_dir, task_id).unwrap_or(false);

                    if needs_rebase {
                        // Try fast rebase first
                        if let Some(ref wt_path) = worktree_path {
                            match crate::worktree::try_fast_rebase(wt_path, &project_dir) {
                                Ok(true) => {
                                    // Fast rebase succeeded
                                }
                                Ok(false) | Err(_) => {
                                    // Conflicts or error - cannot auto-merge
                                    if let Some(project) = self.model.active_project_mut() {
                                        project.release_main_worktree_lock(task_id);
                                    }
                                    commands.push(Message::Error(
                                        "Rebase conflicts detected. Use 'm' to merge with conflict resolution.".to_string()
                                    ));
                                    return commands;
                                }
                            }
                        }
                    }

                    // Verify there are changes to merge
                    match crate::worktree::has_changes_to_merge(&project_dir, task_id) {
                        Ok(true) => {
                            // Good, there are changes
                        }
                        Ok(false) => {
                            if let Some(project) = self.model.active_project_mut() {
                                project.release_main_worktree_lock(task_id);
                            }
                            commands.push(Message::SetStatusMessage(Some(
                                "Nothing to merge - worktree is up to date with main.".to_string()
                            )));
                            return commands;
                        }
                        Err(e) => {
                            if let Some(project) = self.model.active_project_mut() {
                                project.release_main_worktree_lock(task_id);
                            }
                            commands.push(Message::Error(format!("Failed to check for changes: {}", e)));
                            return commands;
                        }
                    }

                    // Merge branch to main (should be fast-forward now)
                    if let Err(e) = crate::worktree::merge_branch(&project_dir, task_id) {
                        if let Some(project) = self.model.active_project_mut() {
                            project.release_main_worktree_lock(task_id);
                        }
                        commands.push(Message::Error(format!(
                            "Merge failed: {}. Try 'm' to merge with conflict resolution.",
                            e
                        )));
                        return commands;
                    }

                    // Release the lock - merge completed successfully
                    if let Some(project) = self.model.active_project_mut() {
                        project.release_main_worktree_lock(task_id);
                    }

                    // Check if there are tracked stashes to offer popping
                    let offer_stash = self.model.active_project()
                        .and_then(|p| p.tracked_stashes.first().cloned());

                    if let Some(stash) = offer_stash {
                        commands.push(Message::OfferPopStash {
                            stash_sha: stash.stash_sha,
                            context: "merge".to_string(),
                        });
                    } else {
                        commands.push(Message::SetStatusMessage(Some(
                            "Changes merged to main. Worktree preserved for continued work.".to_string()
                        )));
                    }
                }
            }

            Message::DiscardTask(task_id) => {
                // Stop SDK session first (if running)
                if let Some(ref client) = self.sidecar_client {
                    let _ = client.stop_session(task_id);
                }

                // Get all necessary info before mutating
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (
                            p.slug(),
                            p.working_dir.clone(),
                            t.tmux_window.clone(),
                            t.worktree_path.clone(),
                        ))
                });

                if let Some((project_slug, project_dir, window_name, worktree_path)) = task_info {
                    // Kill tmux window if exists
                    if let Some(ref window) = window_name {
                        let _ = crate::tmux::kill_task_window(&project_slug, window);
                    }

                    // Kill any detached Claude/test sessions for this task
                    crate::tmux::kill_task_sessions(&task_id.to_string());

                    // Remove worktree (don't merge)
                    if let Some(ref wt_path) = worktree_path {
                        if let Err(e) = crate::worktree::remove_worktree(&project_dir, wt_path) {
                            commands.push(Message::SetStatusMessage(Some(
                                format!("Warning: Could not remove worktree: {}", e)
                            )));
                        }
                        // Clean up trust entry from Claude's config
                        let _ = crate::worktree::remove_worktree_trust(wt_path);
                    }

                    // Delete branch
                    if let Err(e) = crate::worktree::delete_branch(&project_dir, task_id) {
                        commands.push(Message::SetStatusMessage(Some(
                            format!("Warning: Could not delete branch: {}", e)
                        )));
                    }

                    // Update task - move back to Planned (not deleted, just discarded changes)
                    if let Some(project) = self.model.active_project_mut() {
                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                            task.status = TaskStatus::Planned;
                            task.worktree_path = None;
                            task.git_branch = None;
                            task.tmux_window = None;
                            task.session_state = crate::model::ClaudeSessionState::NotStarted;
                            task.started_at = None;
                            task.log_activity("User discarded changes");
                        }
                        project.needs_attention = project.review_count() > 0;
                        if !project.needs_attention {
                            notify::clear_attention_indicator();
                        }
                    }

                    commands.push(Message::SetStatusMessage(Some(
                        "Task discarded - changes removed, task moved back to Planned.".to_string()
                    )));
                }
            }

            Message::ResetTask(task_id) => {
                // Stop SDK session first (if running)
                if let Some(ref client) = self.sidecar_client {
                    let _ = client.stop_session(task_id);
                }

                // Get all necessary info before mutating
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (
                            p.slug(),
                            p.working_dir.clone(),
                            p.is_git_repo(),
                            t.tmux_window.clone(),
                            t.worktree_path.clone(),
                            t.git_branch.clone(),
                        ))
                });

                if let Some((project_slug, project_dir, is_git, window_name, worktree_path, git_branch)) = task_info {
                    // Kill tmux window if exists
                    if let Some(ref window) = window_name {
                        let _ = crate::tmux::kill_task_window(&project_slug, window);
                    }

                    // Kill any detached tmux sessions for this task
                    crate::tmux::kill_task_sessions(&task_id.to_string());

                    // Remove worktree if exists
                    if let Some(ref wt_path) = worktree_path {
                        let _ = crate::worktree::remove_worktree(&project_dir, wt_path);
                        // Clean up trust entry
                        let _ = crate::worktree::remove_worktree_trust(wt_path);
                    }

                    // Delete branch if exists
                    if git_branch.is_some() {
                        let _ = crate::worktree::delete_branch(&project_dir, task_id);
                    }

                    // Reset task state to fresh Planned and move to top of Planned list
                    if let Some(project) = self.model.active_project_mut() {
                        // Find and remove the task from its current position
                        if let Some(task_idx) = project.tasks.iter().position(|t| t.id == task_id) {
                            let mut task = project.tasks.remove(task_idx);

                            // Reset task state
                            task.status = TaskStatus::Planned;
                            task.worktree_path = None;
                            task.git_branch = None;
                            task.tmux_window = None;
                            task.claude_session_id = None;
                            task.session_state = crate::model::ClaudeSessionState::NotStarted;
                            task.started_at = None;
                            task.completed_at = None;
                            task.queued_for_session = None;

                            // Find the position of the first Planned task to insert before it
                            let insert_pos = project.tasks.iter()
                                .position(|t| t.status == TaskStatus::Planned)
                                .unwrap_or(0);

                            // Insert at top of Planned list
                            project.tasks.insert(insert_pos, task);
                        }
                    }

                    // Select the Planned column and highlight the reset task
                    self.model.ui_state.selected_column = TaskStatus::Planned;
                    self.model.ui_state.selected_task_idx = Some(0);
                    self.model.ui_state.selected_task_id = Some(task_id);

                    commands.push(Message::SetStatusMessage(Some(
                        "Task reset to Planned (top). Press Enter to start fresh.".to_string()
                    )));
                }
            }

            Message::CheckAlreadyMerged(task_id) => {
                // Check if the task's branch was already merged to main
                // Shows a detailed report and asks user for confirmation before any cleanup
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (
                            p.working_dir.clone(),
                            t.worktree_path.clone(),
                        ))
                });

                let Some((project_dir, worktree_path)) = task_info else {
                    commands.push(Message::SetStatusMessage(Some(
                        "Task not found".to_string()
                    )));
                    return commands;
                };

                {
                    let branch_name = format!("claude/{}", task_id);
                    let mut report_lines: Vec<String> = vec![];

                    // Check 1: Does branch exist?
                    let branch_exists = std::process::Command::new("git")
                        .current_dir(&project_dir)
                        .args(["rev-parse", "--verify", &branch_name])
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false);

                    if !branch_exists {
                        report_lines.push(format!("Branch: {} does NOT exist", branch_name));
                        report_lines.push("".to_string());
                        report_lines.push("VERDICT: CANNOT VERIFY - branch missing".to_string());
                        report_lines.push("".to_string());
                        report_lines.push("Press any key to close.".to_string());
                        commands.push(Message::ShowConfirmation {
                            message: report_lines.join("\n"),
                            action: PendingAction::ViewMergeReport,
                        });
                        return commands;
                    }
                    report_lines.push(format!("Branch: {} exists", branch_name));

                    // Check 2: Does branch have commits?
                    let commits_output = std::process::Command::new("git")
                        .current_dir(&project_dir)
                        .args(["log", "--oneline", &format!("HEAD..{}", branch_name)])
                        .output();

                    let has_commits = match &commits_output {
                        Ok(o) if o.status.success() => {
                            let out = String::from_utf8_lossy(&o.stdout);
                            !out.trim().is_empty()
                        }
                        _ => false,
                    };

                    let commit_count = commits_output
                        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count())
                        .unwrap_or(0);

                    if has_commits {
                        report_lines.push(format!("Commits on branch: {} (work was done)", commit_count));
                    } else {
                        report_lines.push("Commits on branch: 0 (no work done)".to_string());
                    }

                    // Check 3: Is there a diff between branch and main?
                    let has_diff = std::process::Command::new("git")
                        .current_dir(&project_dir)
                        .args(["diff", "--quiet", "HEAD", &branch_name])
                        .status()
                        .map(|s| !s.success())
                        .unwrap_or(true);

                    if has_diff {
                        // Get diff stats
                        let diff_stat = std::process::Command::new("git")
                            .current_dir(&project_dir)
                            .args(["diff", "--shortstat", "HEAD", &branch_name])
                            .output()
                            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                            .unwrap_or_default();
                        report_lines.push(format!("Diff with main: YES - {}", diff_stat));
                    } else {
                        report_lines.push("Diff with main: NONE (content matches main)".to_string());
                    }

                    // Check 4: Uncommitted changes in worktree?
                    let has_uncommitted = if let Some(ref wt_path) = worktree_path {
                        if wt_path.exists() {
                            crate::worktree::has_uncommitted_changes(wt_path).unwrap_or(false)
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    let worktree_exists = worktree_path.as_ref().map(|p| p.exists()).unwrap_or(false);
                    if worktree_exists {
                        if has_uncommitted {
                            report_lines.push("Worktree: EXISTS with UNCOMMITTED CHANGES".to_string());
                        } else {
                            report_lines.push("Worktree: exists, clean".to_string());
                        }
                    } else {
                        report_lines.push("Worktree: not present".to_string());
                    }

                    // Determine verdict
                    let is_merged = has_commits && !has_diff;
                    let is_safe_to_cleanup = is_merged && !has_uncommitted;

                    report_lines.push("---".to_string());
                    if is_merged {
                        report_lines.push("VERDICT: MERGED (branch has commits, content is in main)".to_string());
                        if is_safe_to_cleanup {
                            report_lines.push("".to_string());
                            report_lines.push("Press 'y' to clean up, 'n'/Esc to cancel.".to_string());
                            // Show confirmation dialog with cleanup action
                            commands.push(Message::ShowConfirmation {
                                message: report_lines.join("\n"),
                                action: PendingAction::CleanupMergedTask(task_id),
                            });
                        } else {
                            report_lines.push("NOT safe: worktree has uncommitted changes!".to_string());
                            report_lines.push("".to_string());
                            report_lines.push("Press any key to close.".to_string());
                            // View-only modal
                            commands.push(Message::ShowConfirmation {
                                message: report_lines.join("\n"),
                                action: PendingAction::ViewMergeReport,
                            });
                        }
                    } else if !has_commits {
                        if has_uncommitted {
                            report_lines.push("VERDICT: HAS UNCOMMITTED WORK".to_string());
                            report_lines.push("Worktree has changes that haven't been committed yet.".to_string());
                        } else {
                            report_lines.push("VERDICT: NO WORK DONE".to_string());
                            report_lines.push("No commits on branch and no uncommitted changes.".to_string());
                        }
                        report_lines.push("".to_string());
                        report_lines.push("Press any key to close.".to_string());
                        // View-only modal
                        commands.push(Message::ShowConfirmation {
                            message: report_lines.join("\n"),
                            action: PendingAction::ViewMergeReport,
                        });
                    } else {
                        report_lines.push("VERDICT: NOT MERGED (branch has changes not in main)".to_string());
                        if has_uncommitted {
                            report_lines.push("Also has uncommitted changes in worktree.".to_string());
                        }
                        report_lines.push("Use 'a' to accept and merge.".to_string());
                        report_lines.push("".to_string());
                        report_lines.push("Press any key to close.".to_string());
                        // View-only modal
                        commands.push(Message::ShowConfirmation {
                            message: report_lines.join("\n"),
                            action: PendingAction::ViewMergeReport,
                        });
                    }
                }
            }

            Message::SwitchToTaskWindow(task_id) => {
                let switch_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .and_then(|t| t.tmux_window.as_ref().map(|w| (p.slug(), w.clone())))
                });

                if let Some((project_slug, window_name)) = switch_info {
                    if let Err(e) = crate::tmux::switch_to_task_window(&project_slug, &window_name) {
                        commands.push(Message::Error(format!("Failed to switch: {}", e)));
                    } else {
                        // Log activity
                        if let Some(project) = self.model.active_project_mut() {
                            if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                task.log_activity("User switched to session");
                            }
                        }
                    }
                }
            }

            Message::OpenInteractiveDetached(task_id) => {
                // Gather task info
                let task_info = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id).map(|task| {
                        (
                            task.worktree_path.clone(),
                            task.claude_session_id.clone(),
                        )
                    })
                });

                if let Some((worktree_path, session_id)) = task_info {
                    let Some(worktree_path) = worktree_path else {
                        commands.push(Message::Error(
                            "Cannot open interactive mode: no worktree path.".to_string()
                        ));
                        return commands;
                    };

                    // Stop SDK session first (if running) before CLI takeover
                    if let Some(ref client) = self.sidecar_client {
                        if let Err(e) = client.stop_session(task_id) {
                            eprintln!("Note: Could not stop SDK session: {}", e);
                        }
                    }

                    let resume_session_id = session_id.as_deref();

                    match crate::tmux::open_popup_detached(&worktree_path, resume_session_id) {
                        Ok(result) => {
                            let status = if result.was_created {
                                format!("Created session '{}'", result.session_name)
                            } else {
                                format!("Session '{}' already exists", result.session_name)
                            };
                            commands.push(Message::SetStatusMessage(Some(status)));

                            // Update session mode to CLI
                            if let Some(project) = self.model.active_project_mut() {
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                    task.session_mode = crate::model::SessionMode::CliInteractive;
                                }
                            }
                        }
                        Err(e) => {
                            commands.push(Message::Error(format!(
                                "Failed to create interactive session: {}", e
                            )));
                        }
                    }
                }
            }

            Message::SmartApplyTask(task_id) => {
                // Check if changes are already applied
                let already_applied = self.model.active_project()
                    .map(|p| p.applied_task_id.is_some())
                    .unwrap_or(false);
                if already_applied {
                    commands.push(Message::SetStatusMessage(Some(
                        "Changes already applied. Press 'u' to unapply first.".to_string()
                    )));
                    return commands;
                }

                // Try to acquire exclusive lock on main worktree
                if let Some(project) = self.model.active_project_mut() {
                    if let Err(reason) = project.try_lock_main_worktree(task_id, MainWorktreeOperation::Applying) {
                        commands.push(Message::Error(reason));
                        return commands;
                    }
                }

                // Get task info and project dir
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (
                            p.working_dir.clone(),
                            t.worktree_path.clone(),
                            t.git_branch.clone(),
                            t.status,
                            t.git_commits_behind,
                        ))
                });

                if let Some((project_dir, worktree_path, git_branch, current_status, commits_behind)) = task_info {
                    // Require rebase before apply - ensures clean patches
                    if commits_behind > 0 {
                        if let Some(project) = self.model.active_project_mut() {
                            project.release_main_worktree_lock(task_id);
                        }
                        commands.push(Message::SetStatusMessage(Some(
                            format!("Task is {} commits behind main. Press 'r' to rebase first.", commits_behind)
                        )));
                        return commands;
                    }
                    // Don't process if already applying
                    if current_status == TaskStatus::Applying {
                        return commands;
                    }

                    // Check if the task has a git branch reference
                    let branch_name = match git_branch {
                        Some(ref b) => b.clone(),
                        None => {
                            if let Some(project) = self.model.active_project_mut() {
                                project.release_main_worktree_lock(task_id);
                            }
                            commands.push(Message::Error(
                                "Task has no git branch. Was it started before worktree support?".to_string()
                            ));
                            return commands;
                        }
                    };

                    // Check if the branch actually exists
                    let branch_exists = std::process::Command::new("git")
                        .current_dir(&project_dir)
                        .args(["rev-parse", "--verify", &branch_name])
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false);

                    if !branch_exists {
                        if let Some(project) = self.model.active_project_mut() {
                            project.release_main_worktree_lock(task_id);
                        }
                        commands.push(Message::Error(format!(
                            "Branch '{}' no longer exists. Task data is stale.",
                            branch_name
                        )));
                        return commands;
                    }

                    // CRITICAL: Commit any uncommitted changes in the worktree FIRST
                    // This ensures we apply all work that Claude did, not just what was committed
                    if let Some(ref wt_path) = worktree_path {
                        match crate::worktree::commit_worktree_changes(wt_path, task_id) {
                            Ok(_) => {
                                // Changes committed (or nothing to commit)
                            }
                            Err(e) => {
                                if let Some(project) = self.model.active_project_mut() {
                                    project.release_main_worktree_lock(task_id);
                                }
                                commands.push(Message::Error(format!(
                                    "Failed to commit worktree changes: {}. Changes preserved in worktree.",
                                    e
                                )));
                                return commands;
                            }
                        }
                    }

                    // STEP 1: Try fast apply first
                    match crate::worktree::apply_task_changes(&project_dir, task_id) {
                        Ok(stash_warning) => {
                            // Fast apply succeeded - stash was immediately popped
                            // stash_warning contains message if there were stash conflicts
                            if let Some(ref warning) = stash_warning {
                                if warning.starts_with("STASH_") {
                                    commands.push(Message::SetStatusMessage(Some(warning.clone())));
                                }
                            }
                            if let Some(project) = self.model.active_project_mut() {
                                project.applied_task_id = Some(task_id);
                                project.applied_stash_ref = None; // No longer tracked - stash already popped
                            }

                            // Release lock and trigger async build + restart
                            // Build check happens async in TriggerRestart - if it fails,
                            // user is prompted to unapply
                            if let Some(project) = self.model.active_project_mut() {
                                project.release_main_worktree_lock(task_id);
                            }
                            commands.push(Message::SetStatusMessage(Some(
                                "✓ Changes applied. Building...".to_string()
                            )));
                            commands.push(Message::TriggerRestart);
                        }
                        Err(apply_err) => {
                            let err_msg = apply_err.to_string();

                            // Check for "empty diff" case - task is already merged
                            if err_msg.contains("No valid patches") || err_msg.contains("Nothing to apply") {
                                if let Some(project) = self.model.active_project_mut() {
                                    project.release_main_worktree_lock(task_id);
                                }
                                commands.push(Message::Error(
                                    "Nothing to apply - task changes are already in main. Mark as done with 'm'.".to_string()
                                ));
                                return commands;
                            }

                            // Check for stash conflict (user's uncommitted changes conflict with task)
                            if let Some(stash_sha) = err_msg.strip_prefix("STASH_CONFLICT:") {
                                // Show confirmation dialog with options
                                self.model.ui_state.confirmation_scroll_offset = 0;
                                self.model.ui_state.pending_confirmation = Some(PendingConfirmation {
                                    message: format!(
                                        "Stash conflict detected.\n\
                                        Your uncommitted changes conflict with the task's changes.\n\
                                        (Your original changes are safely stored in stash {})\n\n\
                                        [Y] Solve with Claude  [N] Unapply  [K] Keep markers",
                                        &stash_sha[..8.min(stash_sha.len())]
                                    ),
                                    action: PendingAction::StashConflict {
                                        task_id,
                                        stash_sha: stash_sha.to_string(),
                                    },
                                    animation_tick: 20,
                                });
                                return commands;
                            }

                            // Check for apply conflict (task changes conflict with main)
                            if let Some(conflict_output) = err_msg.strip_prefix("APPLY_CONFLICT:") {
                                // Show conflict details in scrollable modal
                                self.model.ui_state.confirmation_scroll_offset = 0;
                                self.model.ui_state.pending_confirmation = Some(PendingConfirmation {
                                    message: format!(
                                        "=== Apply Conflict ===\n\n\
                                        Task changes conflict with the main branch.\n\
                                        The worktree needs to be rebased first.\n\n\
                                        --- Conflict Details ---\n\
                                        {}\n\n\
                                        [Y] Smart apply with Claude  [N] Cancel",
                                        conflict_output.trim()
                                    ),
                                    action: PendingAction::ApplyConflict {
                                        task_id,
                                        conflict_output: conflict_output.to_string(),
                                    },
                                    animation_tick: 20,
                                });
                                if let Some(project) = self.model.active_project_mut() {
                                    project.release_main_worktree_lock(task_id);
                                }
                                return commands;
                            }

                            // Fast apply failed - check if we need to rebase first
                            let needs_rebase = crate::worktree::needs_rebase(&project_dir, task_id).unwrap_or(false);

                            if needs_rebase {
                                // Worktree diverged from main - need to rebase first
                                if let Some(ref wt_path) = worktree_path {
                                    // Try fast rebase first
                                    match crate::worktree::try_fast_rebase(wt_path, &project_dir) {
                                        Ok(true) => {
                                            // Fast rebase succeeded, now try apply again
                                            // Keep lock - CompleteApplyTask will release it
                                            commands.push(Message::SetStatusMessage(Some(
                                                "✓ Fast rebase succeeded, applying...".to_string()
                                            )));
                                            commands.push(Message::CompleteApplyTask(task_id));
                                        }
                                        Ok(false) => {
                                            // Conflicts - need Claude to resolve
                                            // Keep lock - will be released when apply session completes
                                            commands.push(Message::SetStatusMessage(Some(
                                                "Conflicts detected, starting smart apply...".to_string()
                                            )));
                                            commands.push(Message::StartApplySession { task_id });
                                        }
                                        Err(e) => {
                                            // Error during rebase - try Claude
                                            // Keep lock - will be released when apply session completes
                                            commands.push(Message::SetStatusMessage(Some(
                                                format!("Fast rebase failed ({}), trying smart apply...", e)
                                            )));
                                            commands.push(Message::StartApplySession { task_id });
                                        }
                                    }
                                } else {
                                    if let Some(project) = self.model.active_project_mut() {
                                        project.release_main_worktree_lock(task_id);
                                    }
                                    commands.push(Message::Error(
                                        "Cannot apply: worktree path not found.".to_string()
                                    ));
                                }
                            } else {
                                // No divergence but apply still failed - show actual error
                                if let Some(project) = self.model.active_project_mut() {
                                    project.release_main_worktree_lock(task_id);
                                }
                                commands.push(Message::Error(format!(
                                    "Failed to apply: {}", err_msg
                                )));
                            }
                        }
                    }
                }
            }

            Message::UnapplyTaskChanges => {
                let project_info = self.model.active_project()
                    .map(|p| (p.working_dir.clone(), p.applied_task_id, p.applied_stash_ref.clone()));

                match project_info {
                    Some((_, None, _)) | None => {
                        commands.push(Message::SetStatusMessage(Some(
                            "No changes applied to unapply.".to_string()
                        )));
                        return commands;
                    }
                    Some((project_dir, Some(task_id), _stash_ref)) => {
                        match crate::worktree::unapply_task_changes(&project_dir, task_id) {
                            Ok(crate::worktree::UnapplyResult::Success) => {
                                // Check for tracked stashes before clearing state
                                let offer_stash = self.model.active_project()
                                    .and_then(|p| p.tracked_stashes.first().cloned());

                                if let Some(project) = self.model.active_project_mut() {
                                    project.applied_task_id = None;
                                    project.applied_stash_ref = None;
                                    project.applied_with_conflict_resolution = false;
                                }

                                // If there are tracked stashes, offer to pop one
                                if let Some(stash) = offer_stash {
                                    commands.push(Message::OfferPopStash {
                                        stash_sha: stash.stash_sha,
                                        context: "unapply".to_string(),
                                    });
                                } else {
                                    commands.push(Message::SetStatusMessage(Some(
                                        "Changes unapplied. Original work restored.".to_string()
                                    )));
                                }
                            }
                            Ok(crate::worktree::UnapplyResult::NeedsConfirmation(reason)) => {
                                // Surgical reversal failed - ask user for confirmation before destructive reset
                                self.model.ui_state.confirmation_scroll_offset = 0;
                                self.model.ui_state.pending_confirmation = Some(PendingConfirmation {
                                    message: format!("{}\n\nThis will discard ALL uncommitted changes in main worktree.", reason),
                                    action: PendingAction::ForceUnapply(task_id),
                                    animation_tick: 20,
                                });
                            }
                            Err(e) => {
                                commands.push(Message::Error(format!("Failed to unapply changes: {}", e)));
                            }
                        }
                    }
                }
            }

            Message::ForceUnapplyTaskChanges(task_id) => {
                let project_dir = self.model.active_project()
                    .map(|p| p.working_dir.clone());

                if let Some(project_dir) = project_dir {
                    match crate::worktree::force_unapply_task_changes(&project_dir, task_id) {
                        Ok(()) => {
                            // Check for tracked stashes before clearing state
                            let offer_stash = self.model.active_project()
                                .and_then(|p| p.tracked_stashes.first().cloned());

                            if let Some(project) = self.model.active_project_mut() {
                                project.applied_task_id = None;
                                project.applied_stash_ref = None;
                                project.applied_with_conflict_resolution = false;
                            }

                            // If there are tracked stashes, offer to pop one
                            if let Some(stash) = offer_stash {
                                commands.push(Message::OfferPopStash {
                                    stash_sha: stash.stash_sha,
                                    context: "unapply".to_string(),
                                });
                            } else {
                                commands.push(Message::SetStatusMessage(Some(
                                    "Changes force-unapplied via destructive reset.".to_string()
                                )));
                            }
                        }
                        Err(e) => {
                            commands.push(Message::Error(format!("Failed to force unapply: {}", e)));
                        }
                    }
                }
            }

            Message::ForceUnapplyWithStashRestore { task_id, stash_sha } => {
                // Surgical unapply: only reset the files from the task patch, then restore stash
                let project_dir = self.model.active_project()
                    .map(|p| p.working_dir.clone());

                if let Some(project_dir) = project_dir {
                    // Step 1: Surgically reset only the files that the task modified
                    match crate::worktree::surgical_unapply_for_stash_conflict(&project_dir, task_id) {
                        Ok(files_reset) => {
                            // Clear applied state
                            if let Some(project) = self.model.active_project_mut() {
                                project.applied_task_id = None;
                                project.applied_stash_ref = None;
                                project.applied_with_conflict_resolution = false;
                            }

                            // Step 2: Pop the stash (not apply - pop removes it on success)
                            // Task changes are gone, so stash should apply cleanly now
                            let pop_result = std::process::Command::new("git")
                                .current_dir(&project_dir)
                                .args(["stash", "pop", &stash_sha])
                                .output();

                            match pop_result {
                                Ok(output) if output.status.success() => {
                                    // Success! Repo is exactly as before apply was attempted
                                    commands.push(Message::SetStatusMessage(Some(
                                        format!("Unapplied ({} files reset). Your changes restored.", files_reset.len())
                                    )));
                                }
                                Ok(output) => {
                                    // Stash pop failed - DON'T auto-drop! Stash is still safe.
                                    let stderr = String::from_utf8_lossy(&output.stderr);
                                    commands.push(Message::SetStatusMessage(Some(
                                        format!("Files reset but stash restore failed: {}. Stash preserved.", stderr.trim())
                                    )));
                                }
                                Err(e) => {
                                    // Git command failed - stash is still safe
                                    commands.push(Message::SetStatusMessage(Some(
                                        format!("Files reset but stash command failed: {}. Stash preserved.", e)
                                    )));
                                }
                            }
                        }
                        Err(e) => {
                            // Surgical unapply failed - don't touch the stash, ask user
                            commands.push(Message::Error(format!(
                                "Surgical unapply failed: {}. Stash '{}' preserved. Manual cleanup may be needed.",
                                e, &stash_sha[..8.min(stash_sha.len())]
                            )));
                        }
                    }
                }
            }

            Message::UpdateWorktreeToMain(task_id) => {
                // Get task info
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (
                            p.working_dir.clone(),
                            t.worktree_path.clone(),
                            t.status,
                        ))
                });

                if let Some((project_dir, worktree_path, status)) = task_info {
                    // Don't update tasks that are already being accepted or updated
                    if status == TaskStatus::Accepting || status == TaskStatus::Updating {
                        commands.push(Message::SetStatusMessage(Some(
                            "Cannot update while rebasing is in progress.".to_string()
                        )));
                        return commands;
                    }

                    if let Some(wt_path) = worktree_path {
                        // Set task to Updating status IMMEDIATELY for UI feedback (shows animation)
                        let task_display_name = if let Some(project) = self.model.active_project_mut() {
                            if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                task.status = TaskStatus::Updating;
                                task.last_activity_at = Some(chrono::Utc::now());
                                task.short_title.clone().unwrap_or_else(|| task.title.clone())
                            } else {
                                "task".to_string()
                            }
                        } else {
                            "task".to_string()
                        };

                        commands.push(Message::SetStatusMessage(Some(
                            format!("Rebasing {}...", task_display_name)
                        )));

                        // Defer ALL git operations (commit + rebase) to run async
                        commands.push(Message::StartFastRebase {
                            task_id,
                            worktree_path: wt_path,
                            project_dir
                        });
                    } else {
                        commands.push(Message::SetStatusMessage(Some(
                            "No worktree found for this task.".to_string()
                        )));
                    }
                }
            }

            Message::StartFastRebase { task_id, worktree_path, project_dir } => {
                // Require async sender - fail explicitly if missing
                let sender = match self.async_sender.clone() {
                    Some(s) => s,
                    None => {
                        if let Some(project) = self.model.active_project_mut() {
                            if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                task.status = TaskStatus::Review;
                            }
                        }
                        commands.push(Message::Error(
                            "Internal error: async_sender not configured.".to_string()
                        ));
                        return commands;
                    }
                };

                // Spawn ALL git operations in background to keep UI responsive
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        // First commit any uncommitted changes
                        if let Err(e) = crate::worktree::commit_worktree_changes(&worktree_path, task_id) {
                            return Err(e);
                        }
                        // Then do the rebase
                        crate::worktree::update_worktree_to_main(&worktree_path, &project_dir)
                    }).await;

                    let msg = match result {
                        Ok(Ok(true)) => Message::FastRebaseCompleted { task_id },
                        Ok(Ok(false)) => Message::FastRebaseNeedsSmartRebase { task_id },
                        Ok(Err(e)) => Message::FastRebaseFailed { task_id, error: e.to_string() },
                        Err(e) => Message::FastRebaseFailed { task_id, error: format!("Task panicked: {}", e) },
                    };

                    let _ = sender.send(msg);
                });
            }

            Message::FastRebaseCompleted { task_id } => {
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.status = TaskStatus::Review;
                    }
                }
                commands.push(Message::SetStatusMessage(Some(
                    "✓ Updated to latest main successfully.".to_string()
                )));
                commands.push(Message::RefreshGitStatus);
            }

            Message::FastRebaseNeedsSmartRebase { task_id } => {
                commands.push(Message::SetStatusMessage(Some(
                    "Conflicts detected, starting smart update...".to_string()
                )));
                commands.push(Message::StartUpdateRebaseSession { task_id });
            }

            Message::FastRebaseFailed { task_id, error } => {
                commands.push(Message::SetStatusMessage(Some(
                    format!("Fast rebase failed ({}), trying smart update...", error)
                )));
                commands.push(Message::StartUpdateRebaseSession { task_id });
            }

            Message::RefreshGitStatus => {
                // Refresh git status for all tasks with worktrees in the active project
                if let Some(project) = self.model.active_project_mut() {
                    let project_dir = project.working_dir.clone();

                    for task in project.tasks.iter_mut() {
                        // Only need worktree_path - branch name is derived from task ID
                        if task.worktree_path.is_some() {
                            // Update git status cache
                            if let Ok(status) = crate::worktree::get_worktree_git_status(&project_dir, task.id) {
                                task.git_additions = status.additions;
                                task.git_deletions = status.deletions;
                                task.git_files_changed = status.files_changed;
                                task.git_commits_ahead = status.commits_ahead;
                                task.git_commits_behind = status.commits_behind;
                                task.git_status_updated_at = Some(chrono::Utc::now());
                            }
                        }
                    }
                }
            }

            // === Git remote operations (fetch/pull/push) ===

            Message::StartGitFetch => {
                // Check if there's already an operation in progress
                if let Some(project) = self.model.active_project() {
                    if project.git_operation_in_progress.is_some() {
                        commands.push(Message::SetStatusMessage(Some(
                            "Git operation already in progress".to_string()
                        )));
                        return commands;
                    }
                }

                // Set operation in progress
                if let Some(project) = self.model.active_project_mut() {
                    project.git_operation_in_progress = Some(crate::model::GitOperation::Fetching);
                }

                // Get project dir for async operation
                let project_dir = self.model.active_project()
                    .map(|p| p.working_dir.clone());

                if let (Some(sender), Some(project_dir)) = (self.async_sender.clone(), project_dir) {
                    tokio::spawn(async move {
                        // First fetch from remote
                        let fetch_result = tokio::task::spawn_blocking({
                            let dir = project_dir.clone();
                            move || crate::worktree::git_fetch(&dir)
                        }).await;

                        // Then get remote status
                        let status_result = tokio::task::spawn_blocking({
                            let dir = project_dir.clone();
                            move || crate::worktree::get_remote_status(&dir)
                        }).await;

                        let msg = match (fetch_result, status_result) {
                            (Ok(Ok(())), Ok(Ok(status))) => {
                                Message::GitFetchCompleted {
                                    ahead: status.ahead,
                                    behind: status.behind,
                                }
                            }
                            (Ok(Err(e)), _) => {
                                Message::GitFetchFailed { error: e.to_string() }
                            }
                            (_, Ok(Err(e))) => {
                                Message::GitFetchFailed { error: e.to_string() }
                            }
                            (Err(e), _) | (_, Err(e)) => {
                                Message::GitFetchFailed { error: format!("Task panicked: {}", e) }
                            }
                        };

                        let _ = sender.send(msg);
                    });
                }
            }

            Message::GitFetchCompleted { ahead, behind } => {
                if let Some(project) = self.model.active_project_mut() {
                    project.remote_ahead = ahead;
                    project.remote_behind = behind;
                    project.has_remote = true;
                    project.git_operation_in_progress = None;
                }
                // Silent update - no status message for fetch
            }

            Message::GitFetchFailed { error } => {
                if let Some(project) = self.model.active_project_mut() {
                    project.git_operation_in_progress = None;
                    // Don't show error for "no remote" case - it's expected
                    if !error.contains("No remote") && !error.contains("no upstream") {
                        commands.push(Message::SetStatusMessage(Some(
                            format!("Fetch failed: {}", error)
                        )));
                    }
                }
            }

            Message::StartGitPull => {
                // Check if there's already an operation in progress
                if let Some(project) = self.model.active_project() {
                    if project.git_operation_in_progress.is_some() {
                        commands.push(Message::SetStatusMessage(Some(
                            "Git operation already in progress".to_string()
                        )));
                        return commands;
                    }
                    // Check if main worktree is locked (accept/apply in progress)
                    if project.main_worktree_lock.is_some() {
                        commands.push(Message::SetStatusMessage(Some(
                            "Cannot pull: main worktree is in use by another operation".to_string()
                        )));
                        return commands;
                    }
                }

                // Set operation in progress
                if let Some(project) = self.model.active_project_mut() {
                    project.git_operation_in_progress = Some(crate::model::GitOperation::Pulling);
                }

                commands.push(Message::SetStatusMessage(Some(
                    "Pulling from remote...".to_string()
                )));

                let project_dir = self.model.active_project()
                    .map(|p| p.working_dir.clone());

                if let (Some(sender), Some(project_dir)) = (self.async_sender.clone(), project_dir) {
                    tokio::spawn(async move {
                        let result = tokio::task::spawn_blocking(move || {
                            // Use smart_git_pull which handles .kanblam/tasks.json gracefully
                            crate::worktree::smart_git_pull(&project_dir)
                        }).await;

                        let msg = match result {
                            Ok(Ok(summary)) => Message::GitPullCompleted { summary },
                            Ok(Err(e)) => Message::GitPullFailed { error: e.to_string() },
                            Err(e) => Message::GitPullFailed { error: format!("Task panicked: {}", e) },
                        };

                        let _ = sender.send(msg);
                    });
                }
            }

            Message::GitPullCompleted { summary } => {
                if let Some(project) = self.model.active_project_mut() {
                    project.git_operation_in_progress = None;
                    project.remote_behind = 0; // We pulled, so we're up to date
                }
                commands.push(Message::SetStatusMessage(Some(
                    format!("✓ {}", summary)
                )));
                commands.push(Message::RefreshGitStatus);
                commands.push(Message::TriggerLogoShimmer);
            }

            Message::GitPullFailed { error } => {
                if let Some(project) = self.model.active_project_mut() {
                    project.git_operation_in_progress = None;
                }
                commands.push(Message::SetStatusMessage(Some(
                    format!("Pull failed: {}", error)
                )));
            }

            Message::StartGitPush => {
                // Check if there's already an operation in progress
                if let Some(project) = self.model.active_project() {
                    if project.git_operation_in_progress.is_some() {
                        commands.push(Message::SetStatusMessage(Some(
                            "Git operation already in progress".to_string()
                        )));
                        return commands;
                    }
                    // Only skip if we've confirmed with remote that there's nothing to push
                    // (has_remote means we've successfully fetched at least once)
                    if project.has_remote && project.remote_ahead == 0 {
                        commands.push(Message::SetStatusMessage(Some(
                            "Nothing to push - already up to date".to_string()
                        )));
                        return commands;
                    }
                }

                // Set operation in progress
                if let Some(project) = self.model.active_project_mut() {
                    project.git_operation_in_progress = Some(crate::model::GitOperation::Pushing);
                }

                commands.push(Message::SetStatusMessage(Some(
                    "Pushing to remote...".to_string()
                )));

                let project_dir = self.model.active_project()
                    .map(|p| p.working_dir.clone());

                if let (Some(sender), Some(project_dir)) = (self.async_sender.clone(), project_dir) {
                    tokio::spawn(async move {
                        let result = tokio::task::spawn_blocking(move || {
                            crate::worktree::git_push(&project_dir)
                        }).await;

                        let msg = match result {
                            Ok(Ok(())) => Message::GitPushCompleted,
                            Ok(Err(e)) => Message::GitPushFailed { error: e.to_string() },
                            Err(e) => Message::GitPushFailed { error: format!("Task panicked: {}", e) },
                        };

                        let _ = sender.send(msg);
                    });
                }
            }

            Message::GitPushCompleted => {
                if let Some(project) = self.model.active_project_mut() {
                    project.git_operation_in_progress = None;
                    project.remote_ahead = 0; // We pushed, so we're up to date
                }
                commands.push(Message::SetStatusMessage(Some(
                    "✓ Push completed successfully".to_string()
                )));
                commands.push(Message::TriggerLogoShimmer);
            }

            Message::GitPushFailed { error } => {
                if let Some(project) = self.model.active_project_mut() {
                    project.git_operation_in_progress = None;
                }
                commands.push(Message::SetStatusMessage(Some(
                    format!("Push failed: {}", error)
                )));
            }

            // === Task queueing ===

            Message::ShowQueueDialog(task_id) => {
                // Check if there are running sessions
                if let Some(project) = self.model.active_project() {
                    if project.tasks_with_active_sessions().is_empty() {
                        commands.push(Message::SetStatusMessage(Some(
                            "No running sessions to queue for".to_string()
                        )));
                    } else {
                        self.model.ui_state.queue_dialog_task_id = Some(task_id);
                        self.model.ui_state.queue_dialog_selected_idx = 0;
                    }
                }
            }

            Message::CloseQueueDialog => {
                self.model.ui_state.queue_dialog_task_id = None;
                self.model.ui_state.queue_dialog_selected_idx = 0;
            }

            Message::QueueDialogNavigate(delta) => {
                if let Some(project) = self.model.active_project() {
                    let sessions = project.tasks_with_active_sessions();
                    if !sessions.is_empty() {
                        let len = sessions.len();
                        let current = self.model.ui_state.queue_dialog_selected_idx;
                        let new_idx = if delta < 0 {
                            if current == 0 { len - 1 } else { current - 1 }
                        } else {
                            (current + 1) % len
                        };
                        self.model.ui_state.queue_dialog_selected_idx = new_idx;
                    }
                }
            }

            Message::QueueDialogNavigateToStart => {
                self.model.ui_state.queue_dialog_selected_idx = 0;
            }

            Message::QueueDialogNavigateToEnd => {
                if let Some(project) = self.model.active_project() {
                    let sessions = project.tasks_with_active_sessions();
                    if !sessions.is_empty() {
                        self.model.ui_state.queue_dialog_selected_idx = sessions.len() - 1;
                    }
                }
            }

            Message::QueueDialogConfirm => {
                // Get the task being queued and the session to queue it for
                if let Some(task_to_queue) = self.model.ui_state.queue_dialog_task_id {
                    let session_task_id = self.model.active_project()
                        .and_then(|p| {
                            let sessions = p.tasks_with_active_sessions();
                            sessions.get(self.model.ui_state.queue_dialog_selected_idx)
                                .map(|t| t.id)
                        });

                    if let Some(after_task_id) = session_task_id {
                        commands.push(Message::QueueTaskForSession {
                            task_id: task_to_queue,
                            after_task_id,
                        });
                    }
                }
                // Close dialog
                self.model.ui_state.queue_dialog_task_id = None;
                self.model.ui_state.queue_dialog_selected_idx = 0;
            }

            Message::QueueTaskForSession { task_id, after_task_id } => {
                // Find the end of the queue for this session
                // (there might already be tasks queued for it)
                let mut current_id = after_task_id;
                while let Some(next) = self.model.active_project()
                    .and_then(|p| p.next_queued_for(current_id).map(|t| t.id))
                {
                    current_id = next;
                }

                // Set the task to queue after the last task in the chain
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.queued_for_session = Some(current_id);
                        task.status = TaskStatus::Queued;
                    }
                }

                // Get task title for status message (prefer short_title if available)
                let task_title = self.model.active_project()
                    .and_then(|p| p.tasks.iter().find(|t| t.id == after_task_id))
                    .map(|t| t.short_title.clone().unwrap_or_else(|| t.title.clone()))
                    .unwrap_or_else(|| "unknown".to_string());

                commands.push(Message::SetStatusMessage(Some(
                    format!("Task queued after: {}", task_title)
                )));
            }

            Message::SendQueuedTask { finished_task_id } => {
                // Find the next task queued for this session
                let next_task_info = self.model.active_project().and_then(|p| {
                    p.next_queued_for(finished_task_id).map(|t| (
                        t.id,
                        t.title.clone(),
                        t.short_title.clone().unwrap_or_else(|| t.title.clone()), // For status display
                        t.images.clone(),
                        p.slug(),
                    ))
                });

                if let Some((next_task_id, title, display_title, images, project_slug)) = next_task_info {
                    // Get worktree info from the finished task
                    let worktree_info = self.model.active_project().and_then(|p| {
                        p.tasks.iter().find(|t| t.id == finished_task_id).map(|t| (
                            t.worktree_path.clone(),
                            t.git_branch.clone(),
                            t.tmux_window.clone(),
                        ))
                    });

                    if let Some((worktree_path, git_branch, tmux_window)) = worktree_info {
                        // Transfer session ownership to the next task
                        if let Some(project) = self.model.active_project_mut() {
                            // Update the next task with session info
                            if let Some(next_task) = project.tasks.iter_mut().find(|t| t.id == next_task_id) {
                                next_task.worktree_path = worktree_path.clone();
                                next_task.git_branch = git_branch;
                                next_task.tmux_window = tmux_window.clone();
                                next_task.status = TaskStatus::InProgress;
                                next_task.session_state = crate::model::ClaudeSessionState::Working;
                                next_task.started_at = Some(Utc::now());
                                next_task.queued_for_session = None; // Clear queue reference
                            }

                            // Clear session info from finished task (it's now in Review)
                            if let Some(finished_task) = project.tasks.iter_mut().find(|t| t.id == finished_task_id) {
                                finished_task.worktree_path = None;
                                finished_task.tmux_window = None;
                                // Keep git_branch so we know it was part of this chain
                            }
                        }

                        // Send the task to the Claude session
                        if let Some(ref window) = tmux_window {
                            if let Err(e) = crate::tmux::send_task_to_window(
                                &project_slug,
                                window,
                                &title,
                                &images,
                            ) {
                                commands.push(Message::Error(format!("Failed to send queued task: {}", e)));
                            } else {
                                commands.push(Message::SetStatusMessage(Some(
                                    format!("Continuing with queued task: {}", display_title)
                                )));
                            }
                        }
                    }
                }
            }

            // === End of worktree-based task lifecycle ===

            Message::SelectTask(idx) => {
                self.model.ui_state.selected_task_idx = idx;
                self.model.ui_state.title_scroll_offset = 0;
                self.model.ui_state.title_scroll_delay = 0;
            }

            Message::SelectColumn(status) => {
                self.model.ui_state.selected_column = status;
                self.model.ui_state.focus = FocusArea::KanbanBoard;
                // Select first task in column if any exist
                let has_tasks = self.model.active_project()
                    .map(|p| !p.tasks_by_status(status).is_empty())
                    .unwrap_or(false);
                self.model.ui_state.selected_task_idx = if has_tasks { Some(0) } else { None };
                self.model.ui_state.title_scroll_offset = 0;
                self.model.ui_state.title_scroll_delay = 0;
            }

            Message::ClickedTask { status, task_idx } => {
                self.model.ui_state.selected_column = status;
                self.model.ui_state.selected_task_idx = Some(task_idx);
                self.model.ui_state.focus = FocusArea::KanbanBoard;
                self.model.ui_state.title_scroll_offset = 0;
                self.model.ui_state.title_scroll_delay = 0;
            }

            Message::SwitchProject(idx) => {
                if idx < self.model.projects.len() {
                    self.model.active_project_idx = idx;
                    self.model.ui_state.selected_task_idx = None;
                    self.model.ui_state.focus = FocusArea::KanbanBoard;

                    // Refresh git status for the new project
                    commands.push(Message::RefreshGitStatus);
                    // Also fetch from remote to update ahead/behind indicators
                    commands.push(Message::StartGitFetch);
                }
            }

            Message::AddProject { name, working_dir } => {
                let project = Project::new(name, working_dir);
                self.model.projects.push(project);
            }

            Message::ShowOpenProjectDialog { slot } => {
                self.model.ui_state.open_project_dialog_slot = Some(slot);
                // Create a directory browser starting at home directory
                let start_dir = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
                if let Ok(browser) = crate::model::DirectoryBrowser::new(start_dir) {
                    self.model.ui_state.directory_browser = Some(browser);
                }
            }

            Message::CloseOpenProjectDialog => {
                self.model.ui_state.open_project_dialog_slot = None;
                self.model.ui_state.directory_browser = None;
                self.model.ui_state.create_folder_input = None;
            }

            Message::EnterCreateFolderMode => {
                // Enter create folder mode with empty input
                self.model.ui_state.create_folder_input = Some(String::new());
            }

            Message::CancelCreateFolderMode => {
                // Exit create folder mode
                self.model.ui_state.create_folder_input = None;
            }

            Message::CreateFolder { name } => {
                // Create a new folder with git init in the current directory
                if let Some(ref mut browser) = self.model.ui_state.directory_browser {
                    match browser.create_folder(&name) {
                        Ok(path) => {
                            // Clear create mode
                            self.model.ui_state.create_folder_input = None;

                            // Show success message
                            commands.push(Message::SetStatusMessage(Some(
                                format!("Created folder '{}' with git init", name)
                            )));

                            // Optionally, we could auto-open the new folder as a project
                            // For now, just leave it selected in the browser
                            let _ = path; // The browser is already updated with the new folder selected
                        }
                        Err(e) => {
                            // Show error message but stay in create mode for retry
                            commands.push(Message::SetStatusMessage(Some(
                                format!("Failed to create folder: {}", e)
                            )));
                        }
                    }
                }
            }

            Message::ConfirmOpenProject => {
                if let Some(slot) = self.model.ui_state.open_project_dialog_slot {
                    if let Some(ref browser) = self.model.ui_state.directory_browser {
                        // Use the selected (cursor) directory as the project path
                        if let Some(selected) = browser.selected() {
                            // Don't allow selecting ".." or special entries as project
                            if selected.special != crate::model::SpecialEntry::None {
                                commands.push(Message::SetStatusMessage(Some(
                                    "Cannot select this item - use [New Project Here] or navigate into a directory".to_string()
                                )));
                            } else {
                                let path = selected.path.clone();

                                // Check if this project is already open
                                if let Some(existing_project) = self.model.projects.iter().find(|p| p.working_dir == path) {
                                    commands.push(Message::SetStatusMessage(Some(
                                        format!("Project '{}' is already open", existing_project.name)
                                    )));
                                    // Close the dialog
                                    self.model.ui_state.open_project_dialog_slot = None;
                                    self.model.ui_state.directory_browser = None;
                                } else {
                                    // Use the directory name as the project name
                                    let name = path
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("project")
                                        .to_string();

                                    // Check git status and offer to initialize if needed
                                    let is_git = crate::worktree::git::is_git_repo(&path);
                                    let has_commits = is_git && crate::worktree::git::has_commits(&path);

                                    if !is_git {
                                        // Not a git repo - offer to initialize
                                        commands.push(Message::ShowConfirmation {
                                            message: format!(
                                                "'{}' is not a git repository.\n\nInitialize git? (y/n)",
                                                name
                                            ),
                                            action: PendingAction::InitGit {
                                                path: path.clone(),
                                                name: name.clone(),
                                                slot,
                                            },
                                        });
                                        // Close the browser dialog (confirmation will handle opening)
                                        self.model.ui_state.open_project_dialog_slot = None;
                                        self.model.ui_state.directory_browser = None;
                                    } else if !has_commits {
                                        // Git repo but no commits - offer to create initial commit
                                        commands.push(Message::ShowConfirmation {
                                            message: format!(
                                                "'{}' has no commits.\n\nCreate initial commit? (y/n)",
                                                name
                                            ),
                                            action: PendingAction::CreateInitialCommit {
                                                path: path.clone(),
                                                name: name.clone(),
                                                slot,
                                            },
                                        });
                                        // Close the browser dialog (confirmation will handle opening)
                                        self.model.ui_state.open_project_dialog_slot = None;
                                        self.model.ui_state.directory_browser = None;
                                    } else {
                                        // Valid git repo with commits - check .gitignore
                                        let missing_entries =
                                            crate::worktree::git::gitignore_missing_kanblam_entries(&path);
                                        if !missing_entries.is_empty() {
                                            // Ask permission to add missing entries
                                            commands.push(Message::ShowConfirmation {
                                                message: format!(
                                                    "'{}' .gitignore is missing KanBlam entries:\n  {}\n\nAdd them? (y/n)",
                                                    name,
                                                    missing_entries.join(", ")
                                                ),
                                                action: PendingAction::UpdateGitignore {
                                                    path: path.clone(),
                                                    name: name.clone(),
                                                    slot,
                                                    missing_entries,
                                                },
                                            });
                                            // Close the browser dialog (confirmation will handle opening)
                                            self.model.ui_state.open_project_dialog_slot = None;
                                            self.model.ui_state.directory_browser = None;
                                        } else {
                                            // All good - open directly
                                            let mut project = Project::new(name, path);
                                            // Load any existing tasks from the project's .kanblam/tasks.json
                                            project.load_tasks();
                                            let has_tasks = !project.tasks.is_empty();
                                            self.model.projects.push(project);
                                            self.model.active_project_idx = slot;
                                            self.model.ui_state.selected_task_idx = None;
                                            // Focus TaskInput if project has no tasks, otherwise KanbanBoard
                                            self.model.ui_state.focus = if has_tasks {
                                                FocusArea::KanbanBoard
                                            } else {
                                                FocusArea::TaskInput
                                            };

                                            // Close the dialog
                                            self.model.ui_state.open_project_dialog_slot = None;
                                            self.model.ui_state.directory_browser = None;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Message::ConfirmOpenProjectPath(path) => {
                if let Some(slot) = self.model.ui_state.open_project_dialog_slot {
                    // Check if this project is already open
                    if let Some(existing_project) = self.model.projects.iter().find(|p| p.working_dir == path) {
                        commands.push(Message::SetStatusMessage(Some(
                            format!("Project '{}' is already open", existing_project.name)
                        )));
                        // Close the dialog
                        self.model.ui_state.open_project_dialog_slot = None;
                        self.model.ui_state.directory_browser = None;
                    } else {
                        // Use the directory name as the project name
                        let name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("project")
                            .to_string();

                        // Check git status and offer to initialize if needed
                        let is_git = crate::worktree::git::is_git_repo(&path);
                        let has_commits = is_git && crate::worktree::git::has_commits(&path);

                        if !is_git {
                            // Not a git repo - offer to initialize
                            commands.push(Message::ShowConfirmation {
                                message: format!(
                                    "'{}' is not a git repository.\n\nInitialize git? (y/n)",
                                    name
                                ),
                                action: PendingAction::InitGit {
                                    path: path.clone(),
                                    name: name.clone(),
                                    slot,
                                },
                            });
                            // Close the browser dialog (confirmation will handle opening)
                            self.model.ui_state.open_project_dialog_slot = None;
                            self.model.ui_state.directory_browser = None;
                        } else if !has_commits {
                            // Git repo but no commits - offer to create initial commit
                            commands.push(Message::ShowConfirmation {
                                message: format!(
                                    "'{}' has no commits.\n\nCreate initial commit? (y/n)",
                                    name
                                ),
                                action: PendingAction::CreateInitialCommit {
                                    path: path.clone(),
                                    name: name.clone(),
                                    slot,
                                },
                            });
                            // Close the browser dialog (confirmation will handle opening)
                            self.model.ui_state.open_project_dialog_slot = None;
                            self.model.ui_state.directory_browser = None;
                        } else {
                            // Valid git repo with commits - check .gitignore
                            let missing_entries =
                                crate::worktree::git::gitignore_missing_kanblam_entries(&path);
                            if !missing_entries.is_empty() {
                                // Ask permission to add missing entries
                                commands.push(Message::ShowConfirmation {
                                    message: format!(
                                        "'{}' .gitignore is missing KanBlam entries:\n  {}\n\nAdd them? (y/n)",
                                        name,
                                        missing_entries.join(", ")
                                    ),
                                    action: PendingAction::UpdateGitignore {
                                        path: path.clone(),
                                        name: name.clone(),
                                        slot,
                                        missing_entries,
                                    },
                                });
                                // Close the browser dialog (confirmation will handle opening)
                                self.model.ui_state.open_project_dialog_slot = None;
                                self.model.ui_state.directory_browser = None;
                            } else {
                                // All good - open directly
                                let mut project = Project::new(name, path);
                                // Load any existing tasks from the project's .kanblam/tasks.json
                                project.load_tasks();
                                let has_tasks = !project.tasks.is_empty();
                                self.model.projects.push(project);
                                self.model.active_project_idx = slot;
                                self.model.ui_state.selected_task_idx = None;
                                // Focus TaskInput if project has no tasks, otherwise KanbanBoard
                                self.model.ui_state.focus = if has_tasks {
                                    FocusArea::KanbanBoard
                                } else {
                                    FocusArea::TaskInput
                                };

                                // Close the dialog
                                self.model.ui_state.open_project_dialog_slot = None;
                                self.model.ui_state.directory_browser = None;
                            }
                        }
                    }
                }
            }

            Message::CloseProject(idx) => {
                if idx < self.model.projects.len() {
                    let project = &self.model.projects[idx];
                    // Check if project has active tasks
                    if project.has_active_task() {
                        let name = project.name.clone();
                        commands.push(Message::ShowConfirmation {
                            message: format!(
                                "Project '{}' has active tasks. Close anyway? (y/n)",
                                name
                            ),
                            action: PendingAction::CloseProject(idx),
                        });
                    } else {
                        // No active tasks, close directly
                        // Save tasks before closing
                        if let Err(e) = self.model.projects[idx].save_tasks() {
                            eprintln!("Warning: Failed to save tasks before closing: {}", e);
                        }
                        self.model.projects.remove(idx);
                        // Adjust active project index
                        if self.model.projects.is_empty() {
                            self.model.active_project_idx = 0;
                        } else if self.model.active_project_idx >= self.model.projects.len() {
                            self.model.active_project_idx = self.model.projects.len() - 1;
                        }
                        // Reset selection
                        self.model.ui_state.selected_task_idx = None;
                        // Save global state so closed project doesn't reappear
                        if let Err(e) = save_state(&self.model) {
                            eprintln!("Warning: Failed to save state after closing project: {}", e);
                        }
                    }
                }
            }

            Message::ShowConfirmation { message, action } => {
                self.model.ui_state.pending_confirmation = Some(PendingConfirmation {
                    message,
                    action,
                    animation_tick: 20, // Start sweep animation (same duration as startup hints)
                });
                // Reset scroll offset for new confirmation
                self.model.ui_state.confirmation_scroll_offset = 0;
            }

            Message::ConfirmAction => {
                // Reset scroll offset when confirmation is dismissed
                self.model.ui_state.confirmation_scroll_offset = 0;
                if let Some(confirmation) = self.model.ui_state.pending_confirmation.take() {
                    match confirmation.action {
                        PendingAction::DeleteTask(task_id) => {
                            // Actually delete the task
                            commands.push(Message::DeleteTask(task_id));
                        }
                        PendingAction::MarkDoneNoMerge(task_id) => {
                            // Mark task as done and clean up worktree without merging
                            // Get task info needed for cleanup
                            let task_info = self.model.active_project().and_then(|p| {
                                p.tasks.iter()
                                    .find(|t| t.id == task_id)
                                    .map(|t| (
                                        p.slug(),
                                        p.working_dir.clone(),
                                        t.tmux_window.clone(),
                                        t.worktree_path.clone(),
                                    ))
                            });

                            if let Some((project_slug, project_dir, window_name, worktree_path)) = task_info {
                                // Kill tmux window if exists
                                if let Some(ref window) = window_name {
                                    let _ = crate::tmux::kill_task_window(&project_slug, window);
                                }

                                // Kill any detached Claude/test sessions for this task
                                crate::tmux::kill_task_sessions(&task_id.to_string());

                                // Remove worktree
                                if let Some(ref wt_path) = worktree_path {
                                    if let Err(e) = crate::worktree::remove_worktree(&project_dir, wt_path) {
                                        commands.push(Message::SetStatusMessage(Some(
                                            format!("Warning: Could not remove worktree: {}", e)
                                        )));
                                    }
                                    // Clean up trust entry from Claude's config
                                    let _ = crate::worktree::remove_worktree_trust(wt_path);
                                }

                                // Delete branch
                                if let Err(e) = crate::worktree::delete_branch(&project_dir, task_id) {
                                    commands.push(Message::SetStatusMessage(Some(
                                        format!("Warning: Could not delete branch: {}", e)
                                    )));
                                }

                                // Update task and move to end of list (bottom of Done column)
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(idx) = project.tasks.iter().position(|t| t.id == task_id) {
                                        let mut task = project.tasks.remove(idx);
                                        task.status = TaskStatus::Done;
                                        task.completed_at = Some(Utc::now());
                                        task.worktree_path = None;
                                        task.tmux_window = None;
                                        task.git_branch = None;
                                        task.session_state = crate::model::ClaudeSessionState::Ended;
                                        project.tasks.push(task);
                                    }
                                    project.needs_attention = project.review_count() > 0;
                                    if !project.needs_attention {
                                        notify::clear_attention_indicator();
                                    }
                                }

                                commands.push(Message::SetStatusMessage(Some(
                                    "Task marked as done. Worktree cleaned up.".to_string()
                                )));
                            }
                        }
                        PendingAction::CloseProject(idx) => {
                            // Close the project (user confirmed)
                            if idx < self.model.projects.len() {
                                // Save tasks before closing
                                if let Err(e) = self.model.projects[idx].save_tasks() {
                                    eprintln!("Warning: Failed to save tasks before closing: {}", e);
                                }
                                self.model.projects.remove(idx);
                                // Adjust active project index
                                if self.model.projects.is_empty() {
                                    self.model.active_project_idx = 0;
                                } else if self.model.active_project_idx >= self.model.projects.len() {
                                    self.model.active_project_idx = self.model.projects.len() - 1;
                                }
                                // Reset selection
                                self.model.ui_state.selected_task_idx = None;
                                // Save global state so closed project doesn't reappear
                                if let Err(e) = save_state(&self.model) {
                                    eprintln!("Warning: Failed to save state after closing project: {}", e);
                                }
                            }
                        }
                        PendingAction::AcceptTask(task_id) => {
                            // Accept task: merge changes and mark as done
                            // This reuses the SmartAcceptTask logic
                            commands.push(Message::SmartAcceptTask(task_id));
                        }
                        PendingAction::MergeOnlyTask(task_id) => {
                            // Merge only: merge changes but keep worktree and task in Review
                            commands.push(Message::MergeOnlyTask(task_id));
                        }
                        PendingAction::DeclineTask(task_id) => {
                            // Decline task: discard all changes and mark as done
                            // Get task info needed for cleanup
                            let task_info = self.model.active_project().and_then(|p| {
                                p.tasks.iter()
                                    .find(|t| t.id == task_id)
                                    .map(|t| (
                                        p.slug(),
                                        p.working_dir.clone(),
                                        t.tmux_window.clone(),
                                        t.worktree_path.clone(),
                                    ))
                            });

                            if let Some((project_slug, project_dir, window_name, worktree_path)) = task_info {
                                // Kill tmux window if exists
                                if let Some(ref window) = window_name {
                                    let _ = crate::tmux::kill_task_window(&project_slug, window);
                                }

                                // Remove worktree (discards all changes)
                                if let Some(ref wt_path) = worktree_path {
                                    if let Err(e) = crate::worktree::remove_worktree(&project_dir, wt_path) {
                                        commands.push(Message::SetStatusMessage(Some(
                                            format!("Warning: Could not remove worktree: {}", e)
                                        )));
                                    }
                                    // Clean up trust entry from Claude's config
                                    let _ = crate::worktree::remove_worktree_trust(wt_path);
                                }

                                // Delete branch (discards all commits)
                                if let Err(e) = crate::worktree::delete_branch(&project_dir, task_id) {
                                    commands.push(Message::SetStatusMessage(Some(
                                        format!("Warning: Could not delete branch: {}", e)
                                    )));
                                }

                                // Update task and move to end of list (bottom of Done column)
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(idx) = project.tasks.iter().position(|t| t.id == task_id) {
                                        let mut task = project.tasks.remove(idx);
                                        task.status = TaskStatus::Done;
                                        task.completed_at = Some(Utc::now());
                                        task.worktree_path = None;
                                        task.tmux_window = None;
                                        task.git_branch = None;
                                        task.session_state = crate::model::ClaudeSessionState::Ended;
                                        project.tasks.push(task);
                                    }
                                    project.needs_attention = project.review_count() > 0;
                                    if !project.needs_attention {
                                        notify::clear_attention_indicator();
                                    }
                                }

                                commands.push(Message::SetStatusMessage(Some(
                                    "Task declined. Changes discarded.".to_string()
                                )));
                            }
                        }
                        PendingAction::ViewMergeReport => {
                            // View-only modal - just dismiss, no action needed
                        }
                        PendingAction::CleanupMergedTask(task_id) => {
                            // User confirmed cleanup of an already-merged task
                            let task_info = self.model.active_project().and_then(|p| {
                                p.tasks.iter()
                                    .find(|t| t.id == task_id)
                                    .map(|t| (
                                        p.slug(),
                                        p.working_dir.clone(),
                                        t.tmux_window.clone(),
                                        t.worktree_path.clone(),
                                    ))
                            });

                            if let Some((project_slug, project_dir, window_name, worktree_path)) = task_info {
                                // Kill tmux window if exists
                                if let Some(ref window) = window_name {
                                    let _ = crate::tmux::kill_task_window(&project_slug, window);
                                }

                                // Kill any detached Claude/test sessions for this task
                                crate::tmux::kill_task_sessions(&task_id.to_string());

                                // Remove worktree if still around
                                if let Some(ref wt_path) = worktree_path {
                                    if wt_path.exists() {
                                        if let Err(e) = crate::worktree::remove_worktree(&project_dir, wt_path) {
                                            commands.push(Message::SetStatusMessage(Some(
                                                format!("Warning: Could not remove worktree: {}", e)
                                            )));
                                        }
                                        let _ = crate::worktree::remove_worktree_trust(wt_path);
                                    }
                                }

                                // Delete branch
                                let _ = crate::worktree::delete_branch(&project_dir, task_id);

                                // Move task to Done
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(idx) = project.tasks.iter().position(|t| t.id == task_id) {
                                        let mut task = project.tasks.remove(idx);
                                        task.status = TaskStatus::Done;
                                        task.completed_at = Some(Utc::now());
                                        task.worktree_path = None;
                                        task.tmux_window = None;
                                        task.git_branch = None;
                                        task.session_state = crate::model::ClaudeSessionState::Ended;
                                        project.tasks.push(task);
                                    }
                                    project.needs_attention = project.review_count() > 0;
                                    if !project.needs_attention {
                                        notify::clear_attention_indicator();
                                    }
                                }

                                commands.push(Message::SetStatusMessage(Some(
                                    "Merged task cleaned up and moved to Done.".to_string()
                                )));
                            }
                        }
                        PendingAction::CommitAppliedChanges(task_id) => {
                            // Commit applied changes to main and complete the task
                            let task_info = self.model.active_project().and_then(|p| {
                                p.tasks.iter()
                                    .find(|t| t.id == task_id)
                                    .map(|t| (
                                        p.slug(),
                                        p.working_dir.clone(),
                                        t.tmux_window.clone(),
                                        t.worktree_path.clone(),
                                        t.title.clone(),
                                    ))
                            });

                            if let Some((project_slug, project_dir, window_name, worktree_path, task_title)) = task_info {
                                // Commit the applied changes to main
                                match crate::worktree::commit_applied_changes(&project_dir, &task_title, task_id) {
                                    Ok(_) => {
                                        // Clean up patch file (stash was already popped during apply)
                                        crate::worktree::cleanup_applied_state(task_id);

                                        // Clear applied state
                                        if let Some(project) = self.model.active_project_mut() {
                                            project.applied_task_id = None;
                                            project.applied_stash_ref = None;
                                            project.applied_with_conflict_resolution = false;
                                        }

                                        // Kill tmux window if exists
                                        if let Some(ref window) = window_name {
                                            let _ = crate::tmux::kill_task_window(&project_slug, window);
                                        }

                                        // Stop SDK session
                                        if let Some(ref client) = self.sidecar_client {
                                            let _ = client.stop_session(task_id);
                                        }

                                        // Kill any detached sessions
                                        crate::tmux::kill_task_sessions(&task_id.to_string());

                                        // Remove worktree
                                        if let Some(ref wt_path) = worktree_path {
                                            let _ = crate::worktree::remove_worktree(&project_dir, wt_path);
                                            let _ = crate::worktree::remove_worktree_trust(wt_path);
                                        }

                                        // Delete branch
                                        let _ = crate::worktree::delete_branch(&project_dir, task_id);

                                        // Move task to Done
                                        if let Some(project) = self.model.active_project_mut() {
                                            if let Some(idx) = project.tasks.iter().position(|t| t.id == task_id) {
                                                let mut task = project.tasks.remove(idx);
                                                task.status = TaskStatus::Done;
                                                task.completed_at = Some(Utc::now());
                                                task.worktree_path = None;
                                                task.tmux_window = None;
                                                task.git_branch = None;
                                                task.session_state = crate::model::ClaudeSessionState::Ended;
                                                project.tasks.push(task);
                                            }
                                            project.needs_attention = project.review_count() > 0;
                                            if !project.needs_attention {
                                                notify::clear_attention_indicator();
                                            }
                                        }

                                        commands.push(Message::SetStatusMessage(Some(
                                            "✓ Changes committed to main. Task complete!".to_string()
                                        )));
                                        commands.push(Message::TriggerLogoShimmer);
                                    }
                                    Err(e) => {
                                        commands.push(Message::Error(format!(
                                            "Failed to commit changes: {}", e
                                        )));
                                    }
                                }
                            }
                        }
                        PendingAction::ResetTask(task_id) => {
                            // Reset the task (cleanup and move to Planned)
                            commands.push(Message::ResetTask(task_id));
                        }
                        PendingAction::ForceUnapply(task_id) => {
                            // User confirmed destructive unapply
                            commands.push(Message::ForceUnapplyTaskChanges(task_id));
                        }
                        PendingAction::StashConflict { task_id, stash_sha } => {
                            // User pressed 'y' - solve conflicts with Claude
                            commands.push(Message::StartStashConflictSession { task_id, stash_sha });
                        }
                        PendingAction::InterruptSdkForCli(task_id) => {
                            // User confirmed interrupting SDK to open CLI
                            commands.push(Message::DoOpenInteractiveModal(task_id));
                        }
                        PendingAction::InterruptSdkForFeedback { task_id, feedback } => {
                            // User confirmed interrupting SDK to send feedback (i=interrupt)
                            commands.push(Message::DoSendFeedback { task_id, feedback });
                        }
                        PendingAction::InterruptCliForFeedback { task_id, feedback } => {
                            // User confirmed interrupting CLI to send feedback via SDK (i=interrupt)
                            commands.push(Message::DoSendFeedback { task_id, feedback });
                        }
                        PendingAction::DirtyMainBeforeMerge { task_id } => {
                            // User chose to commit (y) - commit changes then proceed with merge
                            let project_dir = self.model.active_project()
                                .map(|p| p.working_dir.clone());
                            if let Some(project_dir) = project_dir {
                                if let Err(e) = crate::worktree::commit_main_changes(&project_dir) {
                                    commands.push(Message::Error(format!(
                                        "Failed to commit changes: {}", e
                                    )));
                                } else {
                                    // Now proceed with the merge
                                    commands.push(Message::AcceptTask(task_id));
                                }
                            }
                        }
                        PendingAction::PopTrackedStash { stash_sha } => {
                            // User confirmed popping the stash
                            commands.push(Message::PopTrackedStash { stash_sha });
                        }
                        PendingAction::InitGit { path, name, slot } => {
                            // Initialize git repository
                            match crate::worktree::git::init_repo(&path) {
                                Ok(()) => {
                                    // After git init, create initial commit
                                    match crate::worktree::git::create_initial_commit(&path) {
                                        Ok(()) => {
                                            // Now open the project
                                            let mut project = Project::new(name.clone(), path);
                                            project.load_tasks();
                                            let has_tasks = !project.tasks.is_empty();
                                            self.model.projects.push(project);
                                            self.model.active_project_idx = slot;
                                            self.model.ui_state.selected_task_idx = None;
                                            // Focus TaskInput if project has no tasks, otherwise KanbanBoard
                                            self.model.ui_state.focus = if has_tasks {
                                                FocusArea::KanbanBoard
                                            } else {
                                                FocusArea::TaskInput
                                            };
                                            commands.push(Message::SetStatusMessage(Some(
                                                format!("Initialized git and created initial commit for '{}'", name)
                                            )));
                                        }
                                        Err(e) => {
                                            commands.push(Message::Error(format!(
                                                "Git initialized but failed to create initial commit: {}", e
                                            )));
                                        }
                                    }
                                }
                                Err(e) => {
                                    commands.push(Message::Error(format!(
                                        "Failed to initialize git: {}", e
                                    )));
                                }
                            }
                        }
                        PendingAction::CreateInitialCommit { path, name, slot } => {
                            // Create initial commit in existing git repo
                            match crate::worktree::git::create_initial_commit(&path) {
                                Ok(()) => {
                                    // Now open the project
                                    let mut project = Project::new(name.clone(), path);
                                    project.load_tasks();
                                    let has_tasks = !project.tasks.is_empty();
                                    self.model.projects.push(project);
                                    self.model.active_project_idx = slot;
                                    self.model.ui_state.selected_task_idx = None;
                                    // Focus TaskInput if project has no tasks, otherwise KanbanBoard
                                    self.model.ui_state.focus = if has_tasks {
                                        FocusArea::KanbanBoard
                                    } else {
                                        FocusArea::TaskInput
                                    };
                                    commands.push(Message::SetStatusMessage(Some(
                                        format!("Created initial commit for '{}'", name)
                                    )));
                                }
                                Err(e) => {
                                    commands.push(Message::Error(format!(
                                        "Failed to create initial commit: {}", e
                                    )));
                                }
                            }
                        }
                        PendingAction::ApplyConflict { task_id, .. } => {
                            // User chose to use smart apply with Claude
                            commands.push(Message::StartApplySession { task_id });
                        }
                        PendingAction::UpdateGitignore { path, name, slot, .. } => {
                            // User confirmed adding KanBlam entries to .gitignore
                            match crate::worktree::git::ensure_gitignore_has_kanblam_entries(&path) {
                                Ok(()) => {
                                    // Now open the project
                                    let mut project = Project::new(name.clone(), path);
                                    project.load_tasks();
                                    self.model.projects.push(project);
                                    self.model.active_project_idx = slot;
                                    self.model.ui_state.selected_task_idx = None;
                                    self.model.ui_state.focus = FocusArea::KanbanBoard;
                                    commands.push(Message::SetStatusMessage(Some(
                                        format!("Updated .gitignore and opened '{}'", name)
                                    )));
                                }
                                Err(e) => {
                                    commands.push(Message::Error(format!(
                                        "Failed to update .gitignore: {}", e
                                    )));
                                }
                            }
                        }
                    }
                }
            }

            Message::CancelAction => {
                // Reset scroll offset when confirmation is dismissed
                self.model.ui_state.confirmation_scroll_offset = 0;
                if let Some(confirmation) = self.model.ui_state.pending_confirmation.take() {
                    // Show manual instructions when user cancels
                    match confirmation.action {
                        PendingAction::DeleteTask(_) => {
                            // Just clear the confirmation, no message needed
                        }
                        PendingAction::MarkDoneNoMerge(_) => {
                            // Just clear the confirmation, task stays in Review
                            commands.push(Message::SetStatusMessage(Some(
                                "Task left in Review.".to_string()
                            )));
                        }
                        PendingAction::CloseProject(_) => {
                            // User cancelled closing project, no message needed
                        }
                        PendingAction::AcceptTask(_) | PendingAction::DeclineTask(_) | PendingAction::CommitAppliedChanges(_) | PendingAction::MergeOnlyTask(_) => {
                            // User cancelled, task stays in Review
                            commands.push(Message::SetStatusMessage(Some(
                                "Cancelled. Task left in Review.".to_string()
                            )));
                        }
                        PendingAction::CleanupMergedTask(_) => {
                            // User cancelled cleanup, task stays in Review
                            commands.push(Message::SetStatusMessage(Some(
                                "Cleanup cancelled. Task left in Review.".to_string()
                            )));
                        }
                        PendingAction::ViewMergeReport => {
                            // View-only modal dismissed - no message needed
                        }
                        PendingAction::ResetTask(_) => {
                            // User cancelled reset - no message needed
                        }
                        PendingAction::ForceUnapply(_) => {
                            // User declined destructive unapply - changes remain applied
                            commands.push(Message::SetStatusMessage(Some(
                                "Changes still applied. Use 'u' to try surgical unapply again.".to_string()
                            )));
                        }
                        PendingAction::StashConflict { task_id, stash_sha } => {
                            // User pressed 'n' - unapply task changes and restore user's original work
                            commands.push(Message::ForceUnapplyWithStashRestore { task_id, stash_sha });
                        }
                        PendingAction::InterruptSdkForCli(_) => {
                            // User cancelled opening CLI - SDK continues running
                            commands.push(Message::SetStatusMessage(Some(
                                "Cancelled. Claude continues working via SDK.".to_string()
                            )));
                        }
                        PendingAction::InterruptSdkForFeedback { .. } => {
                            // User cancelled sending feedback - SDK continues working
                            commands.push(Message::SetStatusMessage(Some(
                                "Cancelled. Claude continues working.".to_string()
                            )));
                        }
                        PendingAction::InterruptCliForFeedback { .. } => {
                            // User cancelled sending feedback - CLI continues
                            commands.push(Message::SetStatusMessage(Some(
                                "Cancelled. Press 'o' to view CLI.".to_string()
                            )));
                        }
                        PendingAction::DirtyMainBeforeMerge { .. } => {
                            // User cancelled merge due to dirty worktree
                            commands.push(Message::SetStatusMessage(Some(
                                "Merge cancelled. Commit or stash your changes first.".to_string()
                            )));
                        }
                        PendingAction::PopTrackedStash { .. } => {
                            // User declined to pop stash - no action needed
                            commands.push(Message::SetStatusMessage(Some(
                                "Stash preserved. Press 'S' to manage stashes.".to_string()
                            )));
                        }
                        PendingAction::InitGit { .. } => {
                            // User declined to initialize git - project not opened
                            commands.push(Message::SetStatusMessage(Some(
                                "Project not opened. Initialize git manually to use with KanBlam.".to_string()
                            )));
                        }
                        PendingAction::CreateInitialCommit { .. } => {
                            // User declined to create initial commit - project not opened
                            commands.push(Message::SetStatusMessage(Some(
                                "Project not opened. Create an initial commit to use with KanBlam.".to_string()
                            )));
                        }
                        PendingAction::ApplyConflict { .. } => {
                            // User cancelled smart apply - nothing to do
                            commands.push(Message::SetStatusMessage(Some(
                                "Apply cancelled. Use 'p' to try again.".to_string()
                            )));
                        }
                        PendingAction::UpdateGitignore { path, name, slot, .. } => {
                            // User declined to update .gitignore - open anyway but warn
                            let mut project = Project::new(name.clone(), path);
                            project.load_tasks();
                            self.model.projects.push(project);
                            self.model.active_project_idx = slot;
                            self.model.ui_state.selected_task_idx = None;
                            self.model.ui_state.focus = FocusArea::KanbanBoard;
                            commands.push(Message::SetStatusMessage(Some(
                                format!("Opened '{}' (warning: .gitignore not updated)", name)
                            )));
                        }
                    }
                }
            }

            Message::RestartConfirmationAnimation => {
                // Restart the highlight sweep animation when user presses an unrecognized key
                // This signals that they need to respond to the prompt first
                if let Some(ref mut confirmation) = self.model.ui_state.pending_confirmation {
                    confirmation.animation_tick = 20;
                }
            }

            Message::ScrollConfirmationUp => {
                // Scroll up in multiline confirmation modal
                if self.model.ui_state.pending_confirmation.is_some() {
                    self.model.ui_state.confirmation_scroll_offset =
                        self.model.ui_state.confirmation_scroll_offset.saturating_sub(1);
                }
            }

            Message::ScrollConfirmationDown => {
                // Scroll down in multiline confirmation modal
                if let Some(ref confirmation) = self.model.ui_state.pending_confirmation {
                    let line_count = confirmation.message.lines().count();
                    // Allow scrolling up to line_count - 1 (so at least one line is visible)
                    let max_offset = line_count.saturating_sub(1);
                    if self.model.ui_state.confirmation_scroll_offset < max_offset {
                        self.model.ui_state.confirmation_scroll_offset += 1;
                    }
                }
            }

            Message::SetStatusMessage(msg) => {
                self.model.ui_state.status_message = msg.clone();
                // Set decay timer: ~5 seconds (50 ticks at 100ms each)
                // Longer messages get more time to read
                self.model.ui_state.status_message_decay = if msg.is_some() {
                    50 + (msg.as_ref().map(|m| m.len() as u16 / 3).unwrap_or(0))
                } else {
                    0
                };
            }

            Message::TriggerLogoShimmer => {
                // Start the shimmer animation (frame 1 = bottom row lit)
                self.model.ui_state.logo_shimmer_frame = 1;
                // Use animated star eyes for commit/merge celebrations
                // Longer duration (10 ticks = ~1 second) to show the sparkle animation
                self.model.ui_state.eye_animation = EyeAnimation::StarEyes;
                self.model.ui_state.eye_animation_ticks_remaining = 10;
            }

            Message::TriggerMascotBlink => {
                // Trigger a random eye animation when clicking the mascot
                self.model.ui_state.eye_animation = EyeAnimation::random();
                self.model.ui_state.eye_animation_ticks_remaining = 2;
            }

            Message::ShowStartupHints => {
                // Show the startup hints bar again (triggered by pressing ESC multiple times)
                // Reset to 100 ticks (10 seconds) to match initial display
                self.model.ui_state.startup_hint_until_tick = Some(100);
                // Reset the ESC counter so they need to press ESC twice again
                self.model.ui_state.consecutive_esc_count = 0;
            }

            Message::WelcomeBubbleFocus => {
                self.model.ui_state.welcome_bubble_focused = true;
            }

            Message::WelcomeBubbleUnfocus => {
                self.model.ui_state.welcome_bubble_focused = false;
            }

            Message::WelcomeMessagePrev => {
                let count = crate::ui::welcome_message_count();
                self.model.ui_state.welcome_message_idx =
                    (self.model.ui_state.welcome_message_idx + count - 1) % count;
                // Reset cooldown so it doesn't immediately rotate
                self.model.ui_state.welcome_message_cooldown = 80;
            }

            Message::WelcomeMessageNext => {
                let count = crate::ui::welcome_message_count();
                self.model.ui_state.welcome_message_idx =
                    (self.model.ui_state.welcome_message_idx + 1) % count;
                // Reset cooldown so it doesn't immediately rotate
                self.model.ui_state.welcome_message_cooldown = 80;
            }

            Message::HookSignalReceived(signal) => {
                // Try to find task by task_id first (worktree-based tasks use task UUID as session_id)
                let task_uuid = uuid::Uuid::parse_str(&signal.session_id).ok();

                // Find the task either by UUID or by worktree path
                let signal_dir = signal.project_dir.canonicalize().unwrap_or(signal.project_dir.clone());

                let mut found_task = false;

                for project in &mut self.model.projects {
                    // Find task by UUID or by worktree path
                    let task_idx = project.tasks.iter().position(|t| {
                        // Match by UUID (for worktree-based tasks)
                        if let Some(uuid) = task_uuid {
                            if t.id == uuid {
                                return true;
                            }
                        }
                        // Match by worktree path
                        if let Some(ref wt_path) = t.worktree_path {
                            let wt_canonical = wt_path.canonicalize().unwrap_or(wt_path.clone());
                            if wt_canonical == signal_dir {
                                return true;
                            }
                        }
                        false
                    });

                    if let Some(idx) = task_idx {
                        let task_id = project.tasks[idx].id;
                        // Check if there's a queued task before getting mutable ref
                        let has_queued = project.next_queued_for(task_id).is_some();
                        let was_accepting = project.tasks[idx].status == TaskStatus::Accepting;
                        let was_updating = project.tasks[idx].status == TaskStatus::Updating;
                        let was_applying = project.tasks[idx].status == TaskStatus::Applying;
                        // Terminal states - tasks that are Done should not be moved back
                        let is_terminal = project.tasks[idx].status == TaskStatus::Done;
                        let was_waiting_for_cli = project.tasks[idx].session_mode == crate::model::SessionMode::WaitingForCliExit;
                        let project_name = project.name.clone();
                        let project_slug = project.slug();

                        let was_cli_actively_working = project.tasks[idx].session_mode == crate::model::SessionMode::CliActivelyWorking;

                        let task = &mut project.tasks[idx];
                        found_task = true;

                        // Track CLI activity state for SDK/CLI handoff coordination
                        // When CLI is in CliInteractive or CliActivelyWorking mode, update state based on hooks
                        if matches!(task.session_mode, crate::model::SessionMode::CliInteractive | crate::model::SessionMode::CliActivelyWorking) {
                            match signal.event.as_str() {
                                "working" | "input-provided" => {
                                    // CLI is actively working (user submitted input or tool is running)
                                    task.session_mode = crate::model::SessionMode::CliActivelyWorking;
                                }
                                "stop" | "end" | "needs-input" => {
                                    // CLI finished its turn, back to waiting for input
                                    task.session_mode = crate::model::SessionMode::CliInteractive;
                                }
                                _ => {}
                            }
                        }

                        // Check if we're waiting for CLI to exit (SDK handoff case)
                        // Only trigger SDK resume if CLI is NOT actively working
                        if was_waiting_for_cli && matches!(signal.event.as_str(), "stop" | "end") {
                            // CLI exited - resume SDK session
                            // Note: Don't overwrite claude_session_id here - the signal uses task_id,
                            // but we want to keep the real SDK session_id that was set when session started
                            task.session_mode = crate::model::SessionMode::SdkManaged;
                            commands.push(Message::CliSessionEnded { task_id });
                            break;
                        }

                        // If we were actively working and got a stop/end, transition to WaitingForCliExit
                        // so the SDK can pick up (unless user closes the modal)
                        if was_cli_actively_working && matches!(signal.event.as_str(), "stop" | "end") {
                            task.session_mode = crate::model::SessionMode::CliInteractive;
                        }

                        match signal.event.as_str() {
                            "stop" => {
                                // Skip terminal tasks - these are stale signals from before task was completed
                                if is_terminal {
                                    // Don't modify Done/Discarded tasks
                                } else if was_accepting {
                                    // Task was rebasing for accept - try to complete the accept
                                    // Keep status as Accepting, CompleteAcceptTask will verify and update
                                    commands.push(Message::CompleteAcceptTask(task_id));
                                } else if was_updating {
                                    // Task was rebasing for update - complete update (no merge!)
                                    commands.push(Message::CompleteUpdateTask(task_id));
                                } else if was_applying {
                                    // Task was rebasing for apply - complete the apply
                                    commands.push(Message::CompleteApplyTask(task_id));
                                } else if has_queued {
                                    // Don't move to review - send the queued task instead
                                    // Move to end of Review tasks so first-finished appears at top
                                    // (only if not already in Review from a duplicate hook)
                                    task.session_state = crate::model::ClaudeSessionState::Paused;
                                    if task.status != TaskStatus::Review {
                                        project.move_task_to_end_of_status(task_id, TaskStatus::Review);
                                    }
                                    // Don't play attention sound - we're continuing automatically
                                    commands.push(Message::SendQueuedTask { finished_task_id: task_id });
                                } else {
                                    // Normal stop - move to review and notify
                                    // Move to end of Review tasks so first-finished appears at top
                                    // (only if not already in Review from a duplicate hook)
                                    task.session_state = crate::model::ClaudeSessionState::Paused;
                                    if task.status != TaskStatus::Review {
                                        project.move_task_to_end_of_status(task_id, TaskStatus::Review);
                                        project.needs_attention = true;
                                        notify::play_attention_sound();
                                        notify::set_attention_indicator(&project_name);
                                    }
                                }
                            }
                            "end" => {
                                // Skip terminal tasks - these are stale signals from before task was completed
                                if is_terminal {
                                    // Don't modify Done/Discarded tasks
                                } else {
                                    if was_accepting {
                                        task.log_activity("Accept cancelled");
                                        task.status = TaskStatus::Review;
                                        task.session_state = crate::model::ClaudeSessionState::Ended;
                                        commands.push(Message::SetStatusMessage(Some(
                                            "Accept cancelled: Claude session ended during rebase.".to_string()
                                        )));
                                    } else if was_updating {
                                        task.log_activity("Update cancelled");
                                        task.status = TaskStatus::Review;
                                        task.session_state = crate::model::ClaudeSessionState::Ended;
                                        commands.push(Message::SetStatusMessage(Some(
                                            "Update cancelled: Claude session ended during rebase.".to_string()
                                        )));
                                    } else if was_applying {
                                        task.log_activity("Apply cancelled");
                                        task.status = TaskStatus::Review;
                                        task.session_state = crate::model::ClaudeSessionState::Ended;
                                        commands.push(Message::SetStatusMessage(Some(
                                            "Apply cancelled: Claude session ended during rebase.".to_string()
                                        )));
                                    } else if task.status != TaskStatus::Review {
                                        // Normal end - move to end of Review tasks so first-finished appears at top
                                        // (only if not already in Review from a duplicate hook)
                                        task.session_state = crate::model::ClaudeSessionState::Ended;
                                        project.move_task_to_end_of_status(task_id, TaskStatus::Review);
                                    }
                                    project.needs_attention = true;
                                    notify::play_attention_sound();
                                    notify::set_attention_indicator(&project.name);
                                }
                            }
                            "needs-input" => {
                                // Don't change status if task is in a special state
                                if is_terminal {
                                    // Skip - task already completed, this is a replayed signal
                                } else if was_accepting || was_updating || was_applying {
                                    // Skip - we're in the middle of a rebase operation
                                } else if signal.input_type == "permission" {
                                    // permission_prompt means Claude is blocked waiting for tool approval.
                                    // Always move to NeedsInput, even from Review - this is unambiguous.
                                    task.log_activity("Waiting for permission...");
                                    task.status = TaskStatus::NeedsInput;
                                    task.session_state = crate::model::ClaudeSessionState::Paused;
                                    project.needs_attention = true;
                                    notify::play_attention_sound();
                                    notify::set_attention_indicator(&project.name);
                                } else if signal.input_type == "idle" && task.status == TaskStatus::Review {
                                    // idle_prompt fires after 60+ seconds of Claude being idle.
                                    // Task is already in Review (from Stop hook). Check if Claude
                                    // actually asked a question by examining tmux pane content.
                                    if let Some(ref window_name) = task.tmux_window {
                                        if crate::tmux::claude_output_contains_question(&project_slug, window_name) {
                                            task.log_activity("Waiting for answer...");
                                            task.status = TaskStatus::NeedsInput;
                                            task.session_state = crate::model::ClaudeSessionState::Paused;
                                            project.needs_attention = true;
                                            notify::play_attention_sound();
                                            notify::set_attention_indicator(&project.name);
                                        }
                                        // Otherwise, Claude is just idle after finishing - stay in Review
                                    }
                                } else if task.status != TaskStatus::Review {
                                    // idle_prompt or unknown type - only move to NeedsInput if NOT
                                    // already in Review. The idle_prompt fires both when Claude asks
                                    // a question AND when Claude is done but sitting at an idle prompt.
                                    // We can't distinguish these cases, so trust the Review state.
                                    task.log_activity("Waiting for input...");
                                    task.status = TaskStatus::NeedsInput;
                                    task.session_state = crate::model::ClaudeSessionState::Paused;
                                    project.needs_attention = true;
                                    notify::play_attention_sound();
                                    notify::set_attention_indicator(&project.name);
                                }
                            }
                            "input-provided" => {
                                task.log_activity("Input received, continuing...");
                                // Don't change status if task is in a special state
                                if !was_accepting && !was_updating && !was_applying && !is_terminal {
                                    task.status = TaskStatus::InProgress;
                                }
                                if !is_terminal {
                                    task.session_state = crate::model::ClaudeSessionState::Working;
                                    project.needs_attention = false;
                                    notify::clear_attention_indicator();
                                }
                            }
                            "working" => {
                                // PreToolUse signal - Claude is using a tool
                                // Don't process for terminal tasks (Done/Discarded) - these are stale signals
                                if is_terminal {
                                    // Skip - task already completed, this is a replayed signal
                                } else {
                                    task.log_activity("Working...");
                                    // Don't override Accepting/Updating/Applying status if mid-rebase
                                    if !was_accepting && !was_updating && !was_applying {
                                        task.status = TaskStatus::InProgress;
                                    }
                                    task.session_state = crate::model::ClaudeSessionState::Working;
                                    project.needs_attention = false;
                                    notify::clear_attention_indicator();
                                }
                            }
                            _ => {}
                        }

                        break;
                    }
                }

                // Handle pending feedback after the main loop (avoid borrow conflicts)
                // We need to re-find the task since the previous borrow ended
                if matches!(signal.event.as_str(), "stop" | "needs-input") {
                    if let Some(task_uuid) = task_uuid {
                        for project in &mut self.model.projects {
                            if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_uuid) {
                                if let Some(feedback) = task.pending_feedback.take() {
                                    // Claude finished - send the queued feedback
                                    task.log_activity(&format!("Sending queued feedback: {}...",
                                        if feedback.len() > 20 { &feedback[..20] } else { &feedback }));
                                    task.session_mode = crate::model::SessionMode::SdkManaged;
                                    commands.push(Message::DoSendFeedback { task_id: task_uuid, feedback });
                                }
                                break;
                            }
                        }
                    }
                }

                // Only process signals that match a specific task (by UUID or worktree path)
                // Signals from the main project's Claude are silently ignored - use worktree isolation
                if !found_task {
                    // Check if this signal is from the main project directory (not a worktree)
                    // This is expected when developing on the project itself with Claude
                    let is_main_project = self.model.projects.iter().any(|p| {
                        let main_dir = p.working_dir.canonicalize().unwrap_or(p.working_dir.clone());
                        main_dir == signal_dir
                    });

                    if !is_main_project {
                        // Only warn for unexpected signals (not from main project or known worktrees)
                        commands.push(Message::SetStatusMessage(Some(
                            format!("Hook '{}' received but no matching task for: {} (session: {})",
                                signal.event,
                                signal.project_dir.display(),
                                signal.session_id)
                        )));
                    }
                    // Silently ignore signals from main project - they're from the dev Claude session
                }

                // Sync selection after task status changes
                self.sync_selection();
            }

            Message::ClaudeOutputUpdated { project_id, output } => {
                // Store captured output for display
                if let Some(project) = self.model.projects.iter_mut().find(|p| p.id == project_id) {
                    project.captured_output = output;
                }
            }

            // === Async Background Task Results ===

            Message::WorktreeCreated { task_id, worktree_path, project_dir } => {
                // Update task with worktree info immediately for UI feedback
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.worktree_path = Some(worktree_path.clone());
                        task.git_branch = Some(format!("claude/{}", task_id));
                        task.session_state = crate::model::ClaudeSessionState::Starting;
                    }
                }

                // Spawn settings setup in background, then start SDK session
                if let Some(sender) = self.async_sender.clone() {
                    let wt_path = worktree_path.clone();
                    let proj_dir = project_dir.clone();
                    tokio::spawn(async move {
                        // Run settings setup in background thread
                        let setup_result = tokio::task::spawn_blocking(move || {
                            // Set up Claude settings (non-fatal if fails)
                            let settings_err = crate::worktree::merge_with_project_settings(
                                &wt_path,
                                &proj_dir,
                                task_id,
                            ).err();

                            // Pre-trust the worktree (non-fatal if fails)
                            let trust_err = crate::worktree::pre_trust_worktree(&wt_path).err();

                            (settings_err, trust_err)
                        }).await;

                        // Report warnings but continue to start SDK session
                        if let Ok((settings_err, trust_err)) = setup_result {
                            if let Some(e) = settings_err {
                                let _ = sender.send(Message::SetStatusMessage(Some(
                                    format!("Warning: Could not set up Claude settings: {}", e)
                                )));
                            }
                            if let Some(e) = trust_err {
                                let _ = sender.send(Message::SetStatusMessage(Some(
                                    format!("Warning: Could not pre-trust worktree: {}", e)
                                )));
                            }
                        }

                        // Start SDK session
                        let _ = sender.send(Message::StartSdkSession { task_id });
                    });
                } else {
                    // Fallback to sync if no async sender
                    if let Err(e) = crate::worktree::merge_with_project_settings(
                        &worktree_path,
                        &project_dir,
                        task_id,
                    ) {
                        commands.push(Message::SetStatusMessage(Some(
                            format!("Warning: Could not set up Claude settings: {}", e)
                        )));
                    }

                    if let Err(e) = crate::worktree::pre_trust_worktree(&worktree_path) {
                        commands.push(Message::SetStatusMessage(Some(
                            format!("Warning: Could not pre-trust worktree: {}", e)
                        )));
                    }

                    commands.push(Message::StartSdkSession { task_id });
                }
            }

            Message::WorktreeCreationFailed { task_id, error } => {
                // Reset task state on failure
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.session_state = crate::model::ClaudeSessionState::NotStarted;
                        task.status = TaskStatus::Planned;
                        task.started_at = None;
                    }
                }
                commands.push(Message::Error(format!("Failed to create worktree: {}", error)));
            }

            Message::SdkSessionFailed { task_id, error, project_dir, worktree_path } => {
                // Clean up worktree since SDK failed
                let _ = crate::worktree::remove_worktree(&project_dir, &worktree_path);
                // Reset task state
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.session_state = crate::model::ClaudeSessionState::NotStarted;
                        task.status = TaskStatus::Planned;
                        task.started_at = None;
                        task.worktree_path = None;
                        task.git_branch = None;
                    }
                }
                commands.push(Message::Error(format!("Failed to start SDK session: {}", error)));
            }

            // === Sidecar/SDK Events ===

            Message::StartSdkSession { task_id } => {
                // Get task info for SDK call
                let task_info = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id).map(|task| {
                        (
                            task.title.clone(),
                            task.images.clone(),
                            task.worktree_path.clone(),
                            project.working_dir.clone(),
                        )
                    })
                });

                if let Some((title, images, Some(worktree_path), project_dir)) = task_info {
                    // Check if sidecar is available before spawning background task
                    if self.sidecar_client.is_none() {
                        // No sidecar available - cannot start task
                        commands.push(Message::Error(
                            "Cannot start task: Sidecar not connected. Ensure sidecar is running.".to_string()
                        ));
                        // Reset task state
                        if let Some(project) = self.model.active_project_mut() {
                            if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                task.session_state = crate::model::ClaudeSessionState::NotStarted;
                                task.status = TaskStatus::Planned;
                                task.worktree_path = None;
                                task.git_branch = None;
                            }
                        }
                        // Clean up worktree since we can't start
                        let _ = crate::worktree::remove_worktree(&project_dir, &worktree_path);
                    } else if let Some(sender) = self.async_sender.clone() {
                        // Spawn SDK session start in background to keep UI responsive
                        let images_str: Option<Vec<String>> = if !images.is_empty() {
                            Some(images.iter().map(|p| p.to_string_lossy().to_string()).collect())
                        } else {
                            None
                        };

                        // Clone paths for use in closures
                        let worktree_path_for_call = worktree_path.clone();
                        let worktree_path_for_error = worktree_path.clone();

                        tokio::spawn(async move {
                            // Run blocking sidecar call in a separate thread
                            let result = tokio::task::spawn_blocking(move || {
                                crate::sidecar::SidecarClient::start_session_standalone(
                                    task_id,
                                    worktree_path_for_call,
                                    title,
                                    images_str,
                                )
                            }).await;

                            let msg = match result {
                                Ok(Ok(session_id)) => {
                                    Message::SdkSessionStarted { task_id, session_id }
                                }
                                Ok(Err(e)) => {
                                    Message::SdkSessionFailed { task_id, error: e.to_string(), project_dir, worktree_path: worktree_path_for_error }
                                }
                                Err(e) => {
                                    Message::SdkSessionFailed { task_id, error: format!("Task panicked: {}", e), project_dir, worktree_path: worktree_path_for_error }
                                }
                            };

                            let _ = sender.send(msg);
                        });
                    } else {
                        // Fallback to sync if no async sender (shouldn't happen in normal operation)
                        if let Some(ref client) = self.sidecar_client {
                            let images_str: Option<Vec<String>> = if !images.is_empty() {
                                Some(images.iter().map(|p| p.to_string_lossy().to_string()).collect())
                            } else {
                                None
                            };

                            match client.start_session(task_id, &worktree_path, &title, images_str) {
                                Ok(session_id) => {
                                    commands.push(Message::SdkSessionStarted { task_id, session_id });
                                }
                                Err(e) => {
                                    commands.push(Message::SdkSessionFailed { task_id, error: e.to_string(), project_dir, worktree_path });
                                }
                            }
                        }
                    }
                }
            }

            Message::SidecarEvent(event) => {
                // Handle events from the SDK sidecar
                use crate::sidecar::SessionEventType;

                let task_id = event.task_id;

                // Track if this was an Accepting/Updating/Applying task that stopped/ended (for completion)
                let mut was_accepting = false;
                let mut was_updating = false;
                let mut was_applying = false;

                for project in &mut self.model.projects {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        // Update session_id if provided
                        if let Some(ref session_id) = event.session_id {
                            task.claude_session_id = Some(session_id.clone());
                        }

                        // Check if task was in Accepting/Updating/Applying status (rebase in progress)
                        was_accepting = task.status == TaskStatus::Accepting;
                        was_updating = task.status == TaskStatus::Updating;
                        was_applying = task.status == TaskStatus::Applying;

                        match event.event_type {
                            SessionEventType::Started => {
                                // Don't override Accepting/Updating/Applying status if this is a rebase session
                                if task.status != TaskStatus::Accepting && task.status != TaskStatus::Updating && task.status != TaskStatus::Applying {
                                    task.status = TaskStatus::InProgress; // Move from Queued to InProgress
                                }
                                task.session_state = crate::model::ClaudeSessionState::Working;
                                task.session_mode = crate::model::SessionMode::SdkManaged;
                                task.log_activity("Session started");
                            }
                            SessionEventType::Stopped => {
                                task.log_activity("Session stopped");
                                // If task was accepting/updating/applying (rebase) OR is already done/review, don't change status
                                // Note: Both stopped and ended events may arrive - if stopped triggered
                                // CompleteAcceptTask/CompleteUpdateTask/CompleteApplyTask which moved task, we must not reset it to Review
                                if !was_accepting && !was_updating && !was_applying && task.status != TaskStatus::Done && task.status != TaskStatus::Review {
                                    // Move to end of Review tasks so first-finished appears at top
                                    task.session_state = crate::model::ClaudeSessionState::Paused;
                                    let task_id = task.id;
                                    project.move_task_to_end_of_status(task_id, TaskStatus::Review);
                                    project.needs_attention = true;
                                    notify::play_attention_sound();
                                    notify::set_attention_indicator(&project.name);
                                }
                            }
                            SessionEventType::Ended => {
                                task.log_activity("Session ended");
                                if !was_accepting && !was_updating && !was_applying && task.status != TaskStatus::Done && task.status != TaskStatus::Review {
                                    // Move to end of Review tasks so first-finished appears at top
                                    task.session_state = crate::model::ClaudeSessionState::Ended;
                                    let task_id = task.id;
                                    project.move_task_to_end_of_status(task_id, TaskStatus::Review);
                                    project.needs_attention = true;
                                    notify::play_attention_sound();
                                    notify::set_attention_indicator(&project.name);
                                }
                            }
                            SessionEventType::NeedsInput => {
                                task.log_activity("Waiting for input...");
                                // Don't change status if task is Accepting/Updating/Applying (mid-rebase)
                                if !was_accepting && !was_updating && !was_applying {
                                    task.status = TaskStatus::NeedsInput;
                                    task.session_state = crate::model::ClaudeSessionState::Paused;
                                    project.needs_attention = true;
                                    notify::play_attention_sound();
                                    notify::set_attention_indicator(&project.name);
                                }
                            }
                            SessionEventType::Working => {
                                task.log_activity("Working...");
                                // Don't override Accepting/Updating/Applying status if this is a rebase session
                                if task.status != TaskStatus::Accepting && task.status != TaskStatus::Updating && task.status != TaskStatus::Applying {
                                    task.status = TaskStatus::InProgress;
                                }
                                task.session_state = crate::model::ClaudeSessionState::Working;
                                project.needs_attention = false;
                                notify::clear_attention_indicator();
                                task.last_activity_at = Some(chrono::Utc::now());
                            }
                            SessionEventType::ToolUse => {
                                // Log the tool being used
                                let tool_msg = if let Some(ref tool_name) = event.tool_name {
                                    format!("Using {}", tool_name)
                                } else {
                                    "Using tool...".to_string()
                                };
                                task.log_activity(&tool_msg);
                                // Don't override Accepting/Updating/Applying status if this is a rebase session
                                if task.status != TaskStatus::Accepting && task.status != TaskStatus::Updating && task.status != TaskStatus::Applying {
                                    task.status = TaskStatus::InProgress;
                                }
                                task.session_state = crate::model::ClaudeSessionState::Working;
                                project.needs_attention = false;
                                notify::clear_attention_indicator();
                                // Track activity for merge feedback
                                task.last_activity_at = Some(chrono::Utc::now());
                                if let Some(ref tool_name) = event.tool_name {
                                    task.last_tool_name = Some(tool_name.clone());
                                }
                            }
                            SessionEventType::Output => {
                                // Store output for display (could be used by output panel)
                                if let Some(ref output) = event.output {
                                    project.captured_output.push_str(output);
                                    // Log first line of output if it's meaningful
                                    let first_line = output.lines().next().unwrap_or("").trim();
                                    if !first_line.is_empty() && first_line.len() <= 60 {
                                        task.log_activity(first_line);
                                    }
                                }
                            }
                        }
                        break;
                    }
                }
                self.sync_selection();

                // If an Accepting task's session stopped/ended, try to complete the smart merge
                if was_accepting && matches!(event.event_type, SessionEventType::Stopped | SessionEventType::Ended) {
                    commands.push(Message::CompleteAcceptTask(task_id));
                }
                // If an Updating task's session stopped/ended, complete the update (no merge!)
                if was_updating && matches!(event.event_type, SessionEventType::Stopped | SessionEventType::Ended) {
                    commands.push(Message::CompleteUpdateTask(task_id));
                }
                // If an Applying task's session stopped/ended, complete the apply
                if was_applying && matches!(event.event_type, SessionEventType::Stopped | SessionEventType::Ended) {
                    // Check if this was a stash conflict resolution (runs in main worktree)
                    let is_conflict_resolution = self.model.active_project()
                        .map(|p| p.applied_with_conflict_resolution)
                        .unwrap_or(false);

                    if is_conflict_resolution {
                        commands.push(Message::CompleteStashConflictResolution(task_id));
                    } else {
                        commands.push(Message::CompleteApplyTask(task_id));
                    }
                }
            }

            Message::SdkSessionStarted { task_id, session_id } => {
                // Update task with session ID from SDK
                let mut worktree_display = String::new();
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.claude_session_id = Some(session_id);
                        task.session_state = crate::model::ClaudeSessionState::Working;
                        task.session_mode = crate::model::SessionMode::SdkManaged;
                        // Increment SDK command count for CLI staleness detection
                        task.sdk_command_count = task.sdk_command_count.saturating_add(1);
                        if let Some(ref wt) = task.worktree_path {
                            worktree_display = wt.display().to_string();
                        }
                    }
                }
                if !worktree_display.is_empty() {
                    commands.push(Message::SetStatusMessage(Some(
                        format!("Task started via SDK in worktree: {}", worktree_display)
                    )));
                }
            }

            Message::SdkSessionOutput { task_id, output } => {
                // Store SDK output for display
                for project in &mut self.model.projects {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        // Append to captured output
                        project.captured_output.push_str(&output);
                        break;
                    }
                }
            }

            Message::RequestTitleSummary { task_id } => {
                // Get the task title for summarization
                let title = self.model.active_project()
                    .and_then(|p| p.tasks.iter().find(|t| t.id == task_id))
                    .map(|t| t.title.clone());

                if let Some(title) = title {
                    // Spawn the summarization request in background
                    if let Some(sender) = self.async_sender.clone() {
                        tokio::spawn(async move {
                            // Run blocking sidecar call in a separate thread
                            let result = tokio::task::spawn_blocking(move || {
                                crate::sidecar::SidecarClient::summarize_title_standalone(task_id, title)
                            }).await;

                            let msg = match result {
                                Ok(Ok(short_title)) => {
                                    Message::TitleSummaryReceived { task_id, short_title }
                                }
                                Ok(Err(e)) => {
                                    // Log error but don't show to user - summarization is optional
                                    eprintln!("[Summarization] Failed for task {}: {}", task_id, e);
                                    return; // Don't send a message
                                }
                                Err(e) => {
                                    eprintln!("[Summarization] Task panicked for {}: {}", task_id, e);
                                    return;
                                }
                            };

                            let _ = sender.send(msg);
                        });
                    }
                }
            }

            Message::TitleSummaryReceived { task_id, short_title } => {
                // Update the task with the short title
                for project in &mut self.model.projects {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.short_title = Some(short_title);
                        break;
                    }
                }
            }

            Message::OpenInteractiveModal(task_id) => {
                // Gather task info including SDK command count for staleness check
                let task_info = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id).map(|task| {
                        (
                            task.worktree_path.clone(),
                            task.claude_session_id.clone(),
                            task.session_state.clone(),
                            task.session_mode,
                            task.sdk_command_count,
                            task.cli_opened_at_command_count,
                        )
                    })
                });

                if let Some((worktree_path, session_id, session_state, session_mode, sdk_count, cli_opened_at)) = task_info {
                    // Require worktree path
                    let Some(worktree_path) = worktree_path else {
                        commands.push(Message::Error(
                            "Cannot open interactive mode: no worktree path.".to_string()
                        ));
                        return commands;
                    };

                    // Check if SDK is actively working - if so, ask for confirmation before interrupting
                    let sdk_is_working = session_mode == crate::model::SessionMode::SdkManaged
                        && session_state == crate::model::ClaudeSessionState::Working;

                    if sdk_is_working {
                        // Show confirmation dialog before interrupting SDK
                        commands.push(Message::ShowConfirmation {
                            message: "Claude is working via SDK. Interrupt to open terminal? (y/n)".to_string(),
                            action: PendingAction::InterruptSdkForCli(task_id),
                        });
                        return commands;
                    }

                    // SDK not actively working - proceed with opening CLI
                    commands.push(Message::DoOpenInteractiveModal(task_id));
                }
            }

            Message::DoOpenInteractiveModal(task_id) => {
                // Actually open the interactive modal (after confirmation or if SDK was idle)
                let task_info = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id).map(|task| {
                        (
                            task.worktree_path.clone(),
                            task.claude_session_id.clone(),
                            task.sdk_command_count,
                            task.cli_opened_at_command_count,
                            task.session_mode.clone(),
                        )
                    })
                });

                if let Some((worktree_path, session_id, sdk_count, cli_opened_at, session_mode)) = task_info {
                    let Some(worktree_path) = worktree_path else {
                        return commands;
                    };

                    // Stop SDK session first (if running) before CLI takeover
                    if let Some(ref client) = self.sidecar_client {
                        if let Err(e) = client.stop_session(task_id) {
                            eprintln!("Note: Could not stop SDK session: {}", e);
                        }
                    }

                    // Check if existing CLI terminal has stale state (SDK ran commands since it was opened)
                    let cli_is_stale = sdk_count > cli_opened_at;
                    let task_id_str = task_id.to_string();

                    if cli_is_stale {
                        // Check if CLI is currently working (using session_mode updated by hooks)
                        if session_mode == crate::model::SessionMode::CliActivelyWorking {
                            // CLI is actively working - don't interrupt, let user see it
                            // Just switch to the existing session
                        } else {
                            // CLI is idle or not running - safe to kill and restart
                            if let Err(e) = crate::tmux::kill_claude_cli_session(&task_id_str) {
                                eprintln!("Note: Could not kill stale CLI session: {}", e);
                            }
                        }
                    }

                    // Always try to resume if we have a session_id
                    // This shows conversation history even for completed sessions
                    let resume_session_id = session_id.as_deref();

                    // Open tmux popup with Claude (will create new if killed above, or switch to existing)
                    if let Err(e) = crate::tmux::open_popup(&worktree_path, resume_session_id) {
                        commands.push(Message::Error(format!(
                            "Failed to open interactive popup: {}", e
                        )));
                        return commands;
                    }

                    // Update session mode to CLI and record when CLI was opened
                    if let Some(project) = self.model.active_project_mut() {
                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                            task.session_mode = crate::model::SessionMode::CliInteractive;
                            // Record current SDK command count so we can detect future staleness
                            task.cli_opened_at_command_count = task.sdk_command_count;
                            task.log_activity("User opened terminal");
                        }
                    }
                }
            }

            Message::CloseInteractiveModal => {
                // Get the task_id before closing the modal
                if let Some(modal) = &self.model.ui_state.interactive_modal {
                    let task_id = modal.task_id;

                    // Mark task as waiting for CLI to exit
                    if let Some(project) = self.model.active_project_mut() {
                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                            task.session_mode = crate::model::SessionMode::WaitingForCliExit;
                        }
                    }
                }

                // Close the modal, keep Claude running in background
                self.model.ui_state.interactive_modal = None;
            }

            Message::CliSessionEnded { task_id } => {
                // CLI session ended, resume with SDK
                commands.push(Message::ResumeSdkSession { task_id });
            }

            Message::ResumeSdkSession { task_id } => {
                // Get the session_id and worktree_path from the task first (immutable borrow)
                let task_info = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id)
                        .and_then(|task| {
                            task.claude_session_id.clone().and_then(|sid| {
                                task.worktree_path.clone().map(|wt| (sid, wt))
                            })
                        })
                });

                // Resume the SDK session via sidecar
                if let Some((session_id, worktree_path)) = task_info {
                    if let Some(ref client) = self.sidecar_client {
                        match client.resume_session(task_id, &session_id, &worktree_path, None) {
                            Ok(new_session_id) => {
                                // Update task with new session ID and mode
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.claude_session_id = Some(new_session_id);
                                        task.session_mode = crate::model::SessionMode::SdkManaged;
                                        task.session_state = crate::model::ClaudeSessionState::Working;
                                        // Increment SDK command count for CLI staleness detection
                                        task.sdk_command_count = task.sdk_command_count.saturating_add(1);
                                    }
                                }
                                commands.push(Message::SetStatusMessage(Some(
                                    "SDK session resumed".to_string()
                                )));
                            }
                            Err(e) => {
                                commands.push(Message::Error(format!("Failed to resume SDK session: {}", e)));
                                // Fallback: just mark as SDK managed and hope events come in
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.session_mode = crate::model::SessionMode::SdkManaged;
                                    }
                                }
                            }
                        }
                    } else {
                        // No sidecar client available
                        commands.push(Message::Error("Cannot resume: sidecar not connected".to_string()));
                    }
                } else {
                    // No session or worktree path to resume
                    commands.push(Message::Error("Cannot resume: no session ID or worktree path found".to_string()));
                }
            }

            Message::StartRebaseSession { task_id } => {
                // Start an SDK session specifically for rebasing during smart merge
                let task_info = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id).map(|task| {
                        (
                            task.worktree_path.clone(),
                            project.working_dir.clone(),
                        )
                    })
                });

                if let Some((Some(worktree_path), project_dir)) = task_info {
                    // Detect main branch name (master or main)
                    let main_branch = std::process::Command::new("git")
                        .current_dir(&project_dir)
                        .args(["rev-parse", "--abbrev-ref", "HEAD"])
                        .output()
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                        .unwrap_or_else(|_| "master".to_string());

                    // Generate the rebase prompt
                    let prompt = crate::worktree::generate_rebase_prompt(&main_branch);

                    if let Some(ref client) = self.sidecar_client {
                        match client.start_session(task_id, &worktree_path, &prompt, None) {
                            Ok(session_id) => {
                                // Update task with session ID and Accepting status
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.claude_session_id = Some(session_id);
                                        task.status = TaskStatus::Accepting;
                                        task.session_state = crate::model::ClaudeSessionState::Working;
                                        task.session_mode = crate::model::SessionMode::SdkManaged;
                                        // Track when merge started for elapsed time display
                                        task.accepting_started_at = Some(chrono::Utc::now());
                                        task.last_activity_at = Some(chrono::Utc::now());
                                        task.last_tool_name = None;
                                        // Clear and start activity log
                                        task.clear_activity_log();
                                        task.log_activity("Starting smart merge...");
                                        task.log_activity("Rebasing onto main branch");
                                    }
                                }
                                commands.push(Message::SetStatusMessage(Some(
                                    "Rebasing onto main... Claude is resolving any conflicts.".to_string()
                                )));
                            }
                            Err(e) => {
                                commands.push(Message::Error(format!("Failed to start rebase session: {}", e)));
                                // Reset task to Review status
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.status = TaskStatus::Review;
                                    }
                                }
                            }
                        }
                    } else {
                        commands.push(Message::Error(
                            "Cannot start rebase: sidecar not connected.".to_string()
                        ));
                    }
                }
            }

            Message::StartApplySession { task_id } => {
                // Start an SDK session for rebasing before apply (different from accept)
                let task_info = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id).map(|task| {
                        (
                            task.worktree_path.clone(),
                            project.working_dir.clone(),
                        )
                    })
                });

                if let Some((Some(worktree_path), project_dir)) = task_info {
                    // Detect main branch name
                    let main_branch = std::process::Command::new("git")
                        .current_dir(&project_dir)
                        .args(["rev-parse", "--abbrev-ref", "HEAD"])
                        .output()
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                        .unwrap_or_else(|_| "master".to_string());

                    // Generate apply prompt (rebase with apply context)
                    let prompt = crate::worktree::generate_apply_prompt(&main_branch);

                    if let Some(ref client) = self.sidecar_client {
                        match client.start_session(task_id, &worktree_path, &prompt, None) {
                            Ok(session_id) => {
                                // Update task with session ID and Applying status
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.claude_session_id = Some(session_id);
                                        task.status = TaskStatus::Applying;
                                        task.session_state = crate::model::ClaudeSessionState::Working;
                                        task.session_mode = crate::model::SessionMode::SdkManaged;
                                        task.accepting_started_at = Some(chrono::Utc::now());
                                        task.last_activity_at = Some(chrono::Utc::now());
                                        task.last_tool_name = None;
                                        task.clear_activity_log();
                                        task.log_activity("Starting smart apply...");
                                        task.log_activity("Rebasing onto main for apply");
                                    }
                                }
                                commands.push(Message::SetStatusMessage(Some(
                                    "Rebasing for apply... Claude is resolving conflicts.".to_string()
                                )));
                            }
                            Err(e) => {
                                commands.push(Message::Error(format!("Failed to start apply session: {}", e)));
                                // Task stays in Review status
                            }
                        }
                    } else {
                        commands.push(Message::Error(
                            "Cannot start apply: sidecar not connected.".to_string()
                        ));
                    }
                }
            }

            Message::CompleteApplyTask(task_id) => {
                // Complete the apply after rebase - do the actual apply
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (
                            p.working_dir.clone(),
                            t.worktree_path.clone(),
                            t.status,
                        ))
                });

                if let Some((project_dir, worktree_path, status)) = task_info {
                    // Check if rebase is still in progress
                    if let Some(ref wt_path) = worktree_path {
                        if crate::worktree::is_rebase_in_progress(wt_path) {
                            commands.push(Message::Error(
                                "Rebase still in progress. Wait for Claude to finish.".to_string()
                            ));
                            return commands;
                        }
                    }

                    // Verify rebase succeeded
                    match crate::worktree::verify_rebase_success(&project_dir, task_id) {
                        Ok(true) => {
                            // Rebase successful, now do the apply
                            match crate::worktree::apply_task_changes(&project_dir, task_id) {
                                Ok(stash_warning) => {
                                    // Apply succeeded - stash was immediately popped
                                    if let Some(ref warning) = stash_warning {
                                        if warning.starts_with("STASH_") {
                                            commands.push(Message::SetStatusMessage(Some(warning.clone())));
                                        }
                                    }
                                    if let Some(project) = self.model.active_project_mut() {
                                        project.applied_task_id = Some(task_id);
                                        project.applied_stash_ref = None; // No longer tracked

                                        // Return task to Review status
                                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                            task.status = TaskStatus::Review;
                                            task.session_state = crate::model::ClaudeSessionState::Paused;
                                            task.accepting_started_at = None;
                                        }
                                    }

                                    // Release lock and trigger async build + restart
                                    // Build check happens async in TriggerRestart - if it fails,
                                    // user is prompted to unapply
                                    if let Some(project) = self.model.active_project_mut() {
                                        project.release_main_worktree_lock(task_id);
                                    }
                                    commands.push(Message::SetStatusMessage(Some(
                                        "✓ Changes applied. Building...".to_string()
                                    )));
                                    commands.push(Message::RefreshGitStatus);
                                    commands.push(Message::TriggerRestart);
                                }
                                Err(e) => {
                                    let err_msg = e.to_string();
                                    if let Some(project) = self.model.active_project_mut() {
                                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                            task.status = TaskStatus::Review;
                                            task.session_state = crate::model::ClaudeSessionState::Paused;
                                        }
                                        project.release_main_worktree_lock(task_id);
                                    }
                                    // Check for "already merged" case
                                    if err_msg.contains("Nothing to apply") || err_msg.contains("No valid patches") {
                                        commands.push(Message::Error(
                                            "Nothing to apply - task changes are already in main. Mark as done with 'm'.".to_string()
                                        ));
                                    } else {
                                        commands.push(Message::Error(format!(
                                            "Apply failed: {}",
                                            err_msg
                                        )));
                                    }
                                }
                            }
                        }
                        Ok(false) => {
                            // Rebase failed
                            if let Some(project) = self.model.active_project_mut() {
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                    // Only reset if currently Applying
                                    if status == TaskStatus::Applying {
                                        task.status = TaskStatus::Review;
                                        task.session_state = crate::model::ClaudeSessionState::Paused;
                                    }
                                }
                                project.release_main_worktree_lock(task_id);
                            }
                            commands.push(Message::Error(
                                "Rebase failed. Check the Claude session for errors.".to_string()
                            ));
                        }
                        Err(e) => {
                            if let Some(project) = self.model.active_project_mut() {
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                    if status == TaskStatus::Applying {
                                        task.status = TaskStatus::Review;
                                    }
                                }
                                project.release_main_worktree_lock(task_id);
                            }
                            commands.push(Message::Error(format!("Error verifying rebase: {}", e)));
                        }
                    }
                }
            }

            Message::StartStashConflictSession { task_id, stash_sha } => {
                // Start a Claude session to resolve stash conflicts in the main worktree
                let project_info = self.model.active_project()
                    .map(|p| p.working_dir.clone());

                if let Some(project_dir) = project_info {
                    // Generate the stash conflict prompt
                    let prompt = crate::worktree::generate_stash_conflict_prompt(&stash_sha);

                    // Start session in MAIN worktree (not task worktree)
                    if let Some(client) = &self.sidecar_client {
                        match client.start_session(task_id, &project_dir, &prompt, None) {
                            Ok(session_id) => {
                                if let Some(project) = self.model.active_project_mut() {
                                    // Track that we're in conflict resolution mode
                                    project.applied_task_id = Some(task_id);
                                    project.applied_with_conflict_resolution = true;

                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.status = TaskStatus::Applying;
                                        task.session_state = crate::model::ClaudeSessionState::Working;
                                        task.session_mode = crate::model::SessionMode::SdkManaged;
                                        task.claude_session_id = Some(session_id);
                                        task.log_activity("Resolving stash conflicts in main worktree...");
                                    }
                                }
                                commands.push(Message::SetStatusMessage(Some(
                                    "Claude is resolving conflicts...".to_string()
                                )));
                            }
                            Err(e) => {
                                // Release lock on error
                                if let Some(project) = self.model.active_project_mut() {
                                    project.release_main_worktree_lock(task_id);
                                    project.applied_task_id = None;
                                    project.applied_with_conflict_resolution = false;
                                }
                                commands.push(Message::Error(format!(
                                    "Failed to start conflict resolution session: {}", e
                                )));
                            }
                        }
                    } else {
                        commands.push(Message::Error("Sidecar not connected".to_string()));
                    }
                }
            }

            Message::CompleteStashConflictResolution(task_id) => {
                // Complete stash conflict resolution - check if conflicts are resolved and save combined patch
                let project_dir = self.model.active_project()
                    .map(|p| p.working_dir.clone());

                if let Some(project_dir) = project_dir {
                    // Check if conflicts are resolved (no conflict markers)
                    let conflict_check = std::process::Command::new("git")
                        .current_dir(&project_dir)
                        .args(["diff", "--check"])
                        .output();

                    let has_conflicts = match conflict_check {
                        Ok(output) => !output.status.success(),
                        Err(_) => true, // Assume conflicts if check fails
                    };

                    if has_conflicts {
                        // Conflicts not resolved - return to Review with error
                        if let Some(project) = self.model.active_project_mut() {
                            if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                task.status = TaskStatus::Review;
                                task.session_state = crate::model::ClaudeSessionState::Paused;
                            }
                            project.release_main_worktree_lock(task_id);
                        }
                        commands.push(Message::Error(
                            "Conflict resolution incomplete. Some conflict markers remain.".to_string()
                        ));
                    } else {
                        // Conflicts resolved!
                        // CRITICAL: Save combined patch for surgical unapply
                        if let Err(e) = crate::worktree::save_current_changes_as_patch(&project_dir, task_id) {
                            // Log warning but don't fail - surgical unapply just won't work
                            commands.push(Message::SetStatusMessage(Some(
                                format!("Warning: Could not save patch for surgical unapply: {}", e)
                            )));
                        }

                        // Proceed with build check
                        if let Some(project) = self.model.active_project_mut() {
                            if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                task.status = TaskStatus::Review;
                                task.session_state = crate::model::ClaudeSessionState::Paused;
                            }
                            project.release_main_worktree_lock(task_id);
                            // Keep applied_task_id and applied_with_conflict_resolution set
                        }
                        commands.push(Message::SetStatusMessage(Some(
                            "Conflicts resolved. Building...".to_string()
                        )));
                        commands.push(Message::RefreshGitStatus);
                        commands.push(Message::TriggerRestart);
                    }
                }
            }

            Message::KeepStashConflictMarkers(task_id) => {
                // User chose to keep conflict markers for manual resolution
                if let Some(project) = self.model.active_project_mut() {
                    project.applied_task_id = Some(task_id);
                    project.applied_with_conflict_resolution = true; // Track that conflicts exist
                    project.release_main_worktree_lock(task_id);
                }
                self.model.ui_state.pending_confirmation = None;
                commands.push(Message::SetStatusMessage(Some(
                    "Conflict markers kept. Resolve manually, then press 'a' to re-apply or 'u' to unapply.".to_string()
                )));
            }

            Message::StashUserChangesAndApply(task_id) => {
                // User chose to stash their changes and apply task cleanly
                let project_info = self.model.active_project()
                    .map(|p| (p.working_dir.clone(), p.applied_stash_ref.clone()));

                if let Some((project_dir, stash_ref)) = project_info {
                    // Abort the stash pop while keeping task changes
                    match crate::worktree::abort_stash_pop_keep_task_changes(&project_dir, task_id) {
                        Ok(()) => {
                            // The stash already exists from the failed pop - track it
                            if let Some(ref sha) = stash_ref {
                                if let Some(project) = self.model.active_project_mut() {
                                    // Add to tracked stashes with info from the apply stash
                                    if let Ok((files_changed, files_summary)) =
                                        crate::worktree::get_stash_details(&project_dir, sha)
                                    {
                                        project.tracked_stashes.push(crate::model::TrackedStash {
                                            stash_ref: "stash@{0}".to_string(),
                                            description: "Uncommitted changes before task apply".to_string(),
                                            created_at: chrono::Utc::now(),
                                            files_changed,
                                            files_summary,
                                            stash_sha: sha.clone(),
                                        });
                                    }
                                    project.applied_task_id = Some(task_id);
                                    project.applied_with_conflict_resolution = false;
                                    project.release_main_worktree_lock(task_id);
                                }
                            }
                            commands.push(Message::SetStatusMessage(Some(
                                "Changes stashed. Task applied cleanly. Press 'S' to manage stashes.".to_string()
                            )));
                            commands.push(Message::TriggerRestart);
                        }
                        Err(e) => {
                            commands.push(Message::Error(format!(
                                "Failed to abort stash pop: {}", e
                            )));
                        }
                    }
                }
            }

            Message::ToggleStashModal => {
                self.model.ui_state.show_stash_modal = !self.model.ui_state.show_stash_modal;
                if self.model.ui_state.show_stash_modal {
                    self.model.ui_state.stash_modal_selected_idx = 0;
                }
            }

            Message::StashModalNavigate(delta) => {
                if let Some(project) = self.model.active_project() {
                    let count = project.tracked_stashes.len();
                    if count > 0 {
                        let current = self.model.ui_state.stash_modal_selected_idx as i32;
                        let new_idx = (current + delta).rem_euclid(count as i32) as usize;
                        self.model.ui_state.stash_modal_selected_idx = new_idx;
                    }
                }
            }

            Message::PopSelectedStash => {
                let stash_sha = self.model.active_project()
                    .and_then(|p| p.tracked_stashes.get(self.model.ui_state.stash_modal_selected_idx))
                    .map(|s| s.stash_sha.clone());

                if let Some(sha) = stash_sha {
                    commands.push(Message::PopTrackedStash { stash_sha: sha });
                    self.model.ui_state.show_stash_modal = false;
                }
            }

            Message::DropSelectedStash => {
                let stash_info = self.model.active_project()
                    .and_then(|p| p.tracked_stashes.get(self.model.ui_state.stash_modal_selected_idx))
                    .map(|s| (s.stash_sha.clone(), s.description.clone()));

                if let Some((sha, desc)) = stash_info {
                    self.model.ui_state.confirmation_scroll_offset = 0;
                    self.model.ui_state.pending_confirmation = Some(PendingConfirmation {
                        message: format!("Delete stash '{}'?\nThis cannot be undone.", desc),
                        action: PendingAction::PopTrackedStash { stash_sha: sha },
                        animation_tick: 20,
                    });
                    self.model.ui_state.show_stash_modal = false;
                }
            }

            Message::ConfirmDropStash { stash_sha } => {
                let project_dir = self.model.active_project()
                    .map(|p| p.working_dir.clone());

                if let Some(project_dir) = project_dir {
                    match crate::worktree::drop_tracked_stash(&project_dir, &stash_sha) {
                        Ok(()) => {
                            // Remove from tracked stashes
                            if let Some(project) = self.model.active_project_mut() {
                                project.tracked_stashes.retain(|s| s.stash_sha != stash_sha);
                            }
                            commands.push(Message::SetStatusMessage(Some(
                                "Stash deleted.".to_string()
                            )));
                        }
                        Err(e) => {
                            commands.push(Message::Error(format!("Failed to drop stash: {}", e)));
                        }
                    }
                }
            }

            Message::OfferPopStash { stash_sha, context } => {
                // Show confirmation dialog to pop stash
                self.model.ui_state.confirmation_scroll_offset = 0;
                self.model.ui_state.pending_confirmation = Some(PendingConfirmation {
                    message: format!("{}\n\nRestore your stashed changes now? (y/n)", context),
                    action: PendingAction::PopTrackedStash { stash_sha },
                    animation_tick: 20,
                });
            }

            Message::PopTrackedStash { stash_sha } => {
                let project_dir = self.model.active_project()
                    .map(|p| p.working_dir.clone());

                if let Some(project_dir) = project_dir {
                    match crate::worktree::pop_tracked_stash(&project_dir, &stash_sha) {
                        Ok(_) => {
                            // Remove from tracked stashes
                            if let Some(project) = self.model.active_project_mut() {
                                project.tracked_stashes.retain(|s| s.stash_sha != stash_sha);
                            }
                            commands.push(Message::SetStatusMessage(Some(
                                "Stashed changes restored.".to_string()
                            )));
                        }
                        Err(e) => {
                            let err_msg = e.to_string();
                            if err_msg.starts_with("STASH_CONFLICT:") {
                                // Stash pop had conflicts
                                commands.push(Message::HandleStashPopConflict { stash_sha });
                            } else {
                                commands.push(Message::Error(format!("Failed to pop stash: {}", e)));
                            }
                        }
                    }
                }
            }

            Message::HandleStashPopConflict { stash_sha } => {
                // Stash pop resulted in conflict - offer to resolve with Claude
                self.model.ui_state.confirmation_scroll_offset = 0;
                self.model.ui_state.pending_confirmation = Some(PendingConfirmation {
                    message: "Stash pop resulted in conflicts.\n\nResolve with Claude? (y=resolve, n=abort)".to_string(),
                    action: PendingAction::StashConflict {
                        task_id: uuid::Uuid::nil(), // No task involved, just stash conflict
                        stash_sha,
                    },
                    animation_tick: 20,
                });
            }

            Message::StashThenMerge { task_id } => {
                // Stash uncommitted changes, track them, then proceed with merge
                let project_dir = self.model.active_project()
                    .map(|p| p.working_dir.clone());

                if let Some(project_dir) = project_dir {
                    match crate::worktree::create_tracked_stash(&project_dir, "Uncommitted changes before merge") {
                        Ok(Some(tracked)) => {
                            // Track the stash
                            if let Some(project) = self.model.active_project_mut() {
                                project.tracked_stashes.push(tracked);
                            }
                            // Now proceed with the merge
                            commands.push(Message::AcceptTask(task_id));
                        }
                        Ok(None) => {
                            // Nothing to stash, proceed directly
                            commands.push(Message::AcceptTask(task_id));
                        }
                        Err(e) => {
                            commands.push(Message::Error(format!(
                                "Failed to stash changes: {}", e
                            )));
                        }
                    }
                }
            }

            Message::EnterFeedbackMode(task_id) => {
                // Verify task exists and is in Review or InProgress status
                let task_status = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id).map(|t| t.status)
                });

                let can_enter = matches!(task_status, Some(TaskStatus::Review) | Some(TaskStatus::InProgress));

                if can_enter {
                    let is_live = task_status == Some(TaskStatus::InProgress);
                    // Enter feedback mode: set the feedback task and focus the input
                    self.model.ui_state.feedback_task_id = Some(task_id);
                    self.model.ui_state.focus = crate::model::FocusArea::TaskInput;
                    self.model.ui_state.clear_input();
                    // Ensure we're in insert mode for typing
                    self.model.ui_state.editor_state.mode = edtui::EditorMode::Insert;
                    let msg = if is_live {
                        "Enter live feedback (Esc to cancel, Enter to send)"
                    } else {
                        "Enter feedback (Esc to cancel, Enter to send)"
                    };
                    commands.push(Message::SetStatusMessage(Some(msg.to_string())));
                } else {
                    commands.push(Message::SetStatusMessage(Some(
                        "Task must be in Review or InProgress status to send feedback".to_string()
                    )));
                }
            }

            Message::CancelFeedbackMode => {
                if self.model.ui_state.feedback_task_id.is_some() {
                    self.model.ui_state.feedback_task_id = None;
                    self.model.ui_state.clear_input();
                    self.model.ui_state.focus = crate::model::FocusArea::KanbanBoard;
                    commands.push(Message::SetStatusMessage(None));
                }
            }

            Message::SendFeedback { task_id, feedback } => {
                // Always clear feedback mode first, regardless of outcome
                self.model.ui_state.feedback_task_id = None;
                self.model.ui_state.clear_input();
                self.model.ui_state.focus = crate::model::FocusArea::KanbanBoard;

                // Get task info needed for sending feedback
                let task_info = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id).map(|task| {
                        (
                            task.session_mode,
                            task.session_state,
                            task.status,
                        )
                    })
                });

                if let Some((session_mode, session_state, task_status)) = task_info {
                    // Check if CLI is currently controlling the session
                    let cli_is_active = matches!(
                        session_mode,
                        crate::model::SessionMode::CliInteractive |
                        crate::model::SessionMode::CliActivelyWorking |
                        crate::model::SessionMode::WaitingForCliExit
                    );

                    if cli_is_active {
                        // CLI has control - check session_mode (updated by hooks) to see if working
                        if session_mode == crate::model::SessionMode::CliActivelyWorking {
                            // CLI is actively working (hooks told us) - ask user what to do
                            commands.push(Message::ShowConfirmation {
                                message: "CLI working. i=interrupt, w=wait, o=open CLI, n=cancel".to_string(),
                                action: PendingAction::InterruptCliForFeedback { task_id, feedback },
                            });
                        } else {
                            // CLI is idle (CliInteractive) or closing (WaitingForCliExit) - kill it and send via SDK
                            commands.push(Message::DoSendFeedback { task_id, feedback });
                        }
                    } else if task_status == TaskStatus::InProgress {
                        // SDK mode and InProgress - check if SDK is actively working
                        if session_state == crate::model::ClaudeSessionState::Working {
                            // SDK is actively working - ask user what to do
                            commands.push(Message::ShowConfirmation {
                                message: "SDK working. i=interrupt, w=wait, n=cancel".to_string(),
                                action: PendingAction::InterruptSdkForFeedback { task_id, feedback },
                            });
                        } else {
                            // SDK is idle (WaitingForInput) - send live feedback
                            commands.push(Message::DoSendFeedback { task_id, feedback });
                        }
                    } else {
                        // Paused session (Review) with SDK mode - resume with feedback
                        commands.push(Message::DoSendFeedback { task_id, feedback });
                    }
                } else {
                    commands.push(Message::Error("Task not found".to_string()));
                }
            }

            Message::DoSendFeedback { task_id, feedback } => {
                // Actually send feedback (after confirmation or if CLI was idle)
                let task_info = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id).map(|task| {
                        (
                            task.claude_session_id.clone(),
                            task.tmux_window.clone(),
                            task.worktree_path.clone(),
                            project.slug(),
                            task.status,
                            task.session_mode.clone(),
                        )
                    })
                });

                if let Some((session_id_opt, tmux_window_opt, worktree_path_opt, project_slug, task_status, session_mode)) = task_info {
                    // Kill any CLI session that might be running
                    let task_id_str = task_id.to_string();
                    let _ = crate::tmux::kill_claude_cli_session(&task_id_str);

                    // Check if CLI had control - if so, we need to resume the SDK session
                    let cli_had_control = matches!(
                        session_mode,
                        crate::model::SessionMode::CliInteractive | crate::model::SessionMode::CliActivelyWorking
                    );

                    if task_status == TaskStatus::InProgress && !cli_had_control {
                        // SDK was in control - send live feedback to active SDK session
                        if let Some(ref client) = self.sidecar_client {
                            match client.send_prompt(task_id, &feedback, None) {
                                Ok(()) => {
                                    if let Some(project) = self.model.active_project_mut() {
                                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                            let truncated = if feedback.len() > 50 {
                                                format!("{}...", &feedback[..50])
                                            } else {
                                                feedback.clone()
                                            };
                                            task.log_activity(&format!("Live feedback: {}", truncated));
                                            task.last_activity_at = Some(chrono::Utc::now());
                                            task.sdk_command_count = task.sdk_command_count.saturating_add(1);
                                            task.session_mode = crate::model::SessionMode::SdkManaged;
                                        }
                                    }
                                    commands.push(Message::SetStatusMessage(Some(
                                        "Live feedback sent".to_string()
                                    )));
                                }
                                Err(e) => {
                                    commands.push(Message::Error(format!("Failed to send live feedback: {}", e)));
                                }
                            }
                        } else {
                            commands.push(Message::Error("Cannot send feedback: sidecar not connected".to_string()));
                        }
                    } else {
                        // Paused session (Review) OR CLI had control - resume SDK with feedback
                        if let Some(ref window_name) = tmux_window_opt {
                            let _ = crate::tmux::kill_task_window(&project_slug, window_name);
                        }

                        if let (Some(ref session_id), Some(ref worktree_path)) = (&session_id_opt, &worktree_path_opt) {
                            if let Some(ref client) = self.sidecar_client {
                                match client.resume_session(task_id, session_id, worktree_path, Some(&feedback)) {
                                    Ok(new_session_id) => {
                                        if let Some(project) = self.model.active_project_mut() {
                                            if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                                task.claude_session_id = Some(new_session_id);
                                                task.status = TaskStatus::InProgress;
                                                task.session_state = crate::model::ClaudeSessionState::Working;
                                                task.session_mode = crate::model::SessionMode::SdkManaged;
                                                task.last_activity_at = Some(chrono::Utc::now());
                                                task.sdk_command_count = task.sdk_command_count.saturating_add(1);
                                                task.tmux_window = None;
                                                let truncated = if feedback.len() > 50 {
                                                    format!("{}...", &feedback[..50])
                                                } else {
                                                    feedback.clone()
                                                };
                                                task.log_activity(&format!("Feedback sent: {}", truncated));
                                            }
                                            project.needs_attention = false;
                                            notify::clear_attention_indicator();
                                        }
                                        commands.push(Message::SelectColumn(TaskStatus::InProgress));
                                        commands.push(Message::SetStatusMessage(Some(
                                            "Feedback sent - task resumed".to_string()
                                        )));
                                    }
                                    Err(e) => {
                                        commands.push(Message::Error(format!("Failed to send feedback: {}", e)));
                                    }
                                }
                            } else {
                                commands.push(Message::Error("Cannot send feedback: sidecar not connected".to_string()));
                            }
                        } else {
                            let reason = match (&session_id_opt, &worktree_path_opt) {
                                (None, _) => "no session ID (task has no prior Claude session)",
                                (_, None) => "no worktree path",
                                _ => "unknown reason",
                            };
                            commands.push(Message::Error(format!("Cannot send feedback: {}", reason)));
                        }
                    }
                } else {
                    commands.push(Message::Error("Task not found".to_string()));
                }
            }

            Message::QueueFeedback { task_id, feedback } => {
                // Clear the confirmation dialog
                self.model.ui_state.pending_confirmation = None;

                // Queue feedback to be sent when Claude finishes current work
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.pending_feedback = Some(feedback.clone());
                        let truncated = if feedback.len() > 30 {
                            format!("{}...", &feedback[..30])
                        } else {
                            feedback
                        };
                        commands.push(Message::SetStatusMessage(Some(
                            format!("Feedback queued: {}", truncated)
                        )));
                    } else {
                        commands.push(Message::Error("Task not found".to_string()));
                    }
                }
            }

            Message::StartUpdateRebaseSession { task_id } => {
                // Start an SDK session for rebasing during update (NOT accept)
                // Uses Updating status so completion doesn't merge to main
                let task_info = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id).map(|task| {
                        (
                            task.worktree_path.clone(),
                            project.working_dir.clone(),
                            task.status,
                        )
                    })
                });

                if let Some((Some(worktree_path), project_dir, previous_status)) = task_info {
                    // Detect main branch name (master or main)
                    let main_branch = std::process::Command::new("git")
                        .current_dir(&project_dir)
                        .args(["rev-parse", "--abbrev-ref", "HEAD"])
                        .output()
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                        .unwrap_or_else(|_| "master".to_string());

                    // Generate the rebase prompt
                    let prompt = crate::worktree::generate_rebase_prompt(&main_branch);

                    if let Some(ref client) = self.sidecar_client {
                        match client.start_session(task_id, &worktree_path, &prompt, None) {
                            Ok(session_id) => {
                                // Update task with session ID and Updating status (NOT Accepting!)
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.claude_session_id = Some(session_id);
                                        task.status = TaskStatus::Updating;
                                        task.session_state = crate::model::ClaudeSessionState::Working;
                                        task.session_mode = crate::model::SessionMode::SdkManaged;
                                        task.last_activity_at = Some(chrono::Utc::now());
                                        task.last_tool_name = None;
                                        // Clear and start activity log
                                        task.clear_activity_log();
                                        task.log_activity("Starting worktree update...");
                                        task.log_activity("Rebasing onto main branch");
                                    }
                                }
                                commands.push(Message::SetStatusMessage(Some(
                                    "Updating worktree... Claude is resolving conflicts.".to_string()
                                )));
                            }
                            Err(e) => {
                                commands.push(Message::Error(format!("Failed to start update session: {}", e)));
                                // Reset task to previous status
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.status = previous_status;
                                    }
                                }
                            }
                        }
                    } else {
                        commands.push(Message::Error(
                            "Cannot start update: sidecar not connected.".to_string()
                        ));
                    }
                }
            }

            Message::CompleteUpdateTask(task_id) => {
                // Complete an update operation - verify rebase succeeded, then return to Review
                // Does NOT merge to main and does NOT mark as done!
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (
                            p.working_dir.clone(),
                            t.worktree_path.clone(),
                            t.status,
                        ))
                });

                if let Some((project_dir, worktree_path, status)) = task_info {
                    // Only process if task was updating
                    if status != TaskStatus::Updating {
                        return commands;
                    }

                    // Check if rebase is still in progress
                    if let Some(ref wt_path) = worktree_path {
                        if crate::worktree::is_rebase_in_progress(wt_path) {
                            commands.push(Message::Error(
                                "Rebase still in progress. Wait for Claude to finish.".to_string()
                            ));
                            return commands;
                        }
                    }

                    // Verify branch is now on top of main
                    match crate::worktree::verify_rebase_success(&project_dir, task_id) {
                        Ok(true) => {
                            // Rebase successful - return to Review status
                            if let Some(project) = self.model.active_project_mut() {
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                    task.status = TaskStatus::Review;
                                    task.session_state = crate::model::ClaudeSessionState::Paused;
                                }
                            }
                            commands.push(Message::SetStatusMessage(Some(
                                "✓ Worktree updated to latest main successfully.".to_string()
                            )));
                            commands.push(Message::RefreshGitStatus);
                        }
                        Ok(false) => {
                            // Rebase failed - return to Review status
                            if let Some(project) = self.model.active_project_mut() {
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                    task.status = TaskStatus::Review;
                                    task.session_state = crate::model::ClaudeSessionState::Paused;
                                }
                            }
                            commands.push(Message::Error(
                                "Update failed. Check the Claude session for errors.".to_string()
                            ));
                            commands.push(Message::RefreshGitStatus);
                        }
                        Err(e) => {
                            // Error checking - return to Review
                            if let Some(project) = self.model.active_project_mut() {
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                    task.status = TaskStatus::Review;
                                }
                            }
                            commands.push(Message::Error(format!("Error verifying update: {}", e)));
                            commands.push(Message::RefreshGitStatus);
                        }
                    }
                }
            }

            Message::PasteImage => {
                // Check clipboard for image and save it
                match crate::image::paste_image_from_clipboard() {
                    Ok(image_path) => {
                        // If we're editing a task, attach to that task
                        if let Some(task_id) = self.model.ui_state.editing_task_id {
                            if let Some(project) = self.model.active_project_mut() {
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                    task.images.push(image_path.clone());
                                }
                            }
                            commands.push(Message::SetStatusMessage(Some(
                                "Image attached to task".to_string()
                            )));
                        } else {
                            // Store image path to attach when task is created
                            self.model.ui_state.pending_images.push(image_path);
                            let count = self.model.ui_state.pending_images.len();
                            commands.push(Message::SetStatusMessage(Some(
                                format!("{} image{} ready to attach", count, if count == 1 { "" } else { "s" })
                            )));
                        }
                    }
                    Err(e) => {
                        commands.push(Message::SetStatusMessage(Some(
                            format!("No image in clipboard: {}", e)
                        )));
                    }
                }
            }

            Message::AttachImage { task_id, path } => {
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.images.push(path);
                    }
                }
            }

            Message::InputSubmit => {
                // Get text from editor
                let input = self.model.ui_state.get_input_text().trim().to_string();

                // Check if we're in feedback mode
                if let Some(task_id) = self.model.ui_state.feedback_task_id {
                    if !input.is_empty() {
                        commands.push(Message::SendFeedback { task_id, feedback: input });
                    } else {
                        // Empty feedback cancels the mode
                        commands.push(Message::CancelFeedbackMode);
                    }
                }
                else if !input.is_empty() {
                    // Check if we're editing an existing task or creating a new one
                    if let Some(task_id) = self.model.ui_state.editing_task_id {
                        commands.push(Message::UpdateTask { task_id, title: input });
                    } else {
                        commands.push(Message::CreateTask(input));
                    }
                }
            }

            Message::InputSubmitAndStart => {
                // Get text from editor
                let input = self.model.ui_state.get_input_text().trim().to_string();

                // Check if we're in feedback mode - Ctrl+S submits feedback same as Enter
                if let Some(task_id) = self.model.ui_state.feedback_task_id {
                    if !input.is_empty() {
                        commands.push(Message::SendFeedback { task_id, feedback: input });
                    } else {
                        // Empty feedback cancels the mode
                        commands.push(Message::CancelFeedbackMode);
                    }
                }
                // Check if we're in edit mode - Ctrl+S updates and starts if possible
                else if let Some(task_id) = self.model.ui_state.editing_task_id {
                    if !input.is_empty() {
                        // Check if task can be started after editing
                        let can_start = self.model.active_project()
                            .and_then(|p| p.tasks.iter().find(|t| t.id == task_id))
                            .map(|t| t.can_start())
                            .unwrap_or(false);
                        let is_git_repo = self.model.active_project()
                            .map(|p| p.is_git_repo())
                            .unwrap_or(false);

                        // First update the task
                        commands.push(Message::UpdateTask { task_id, title: input });

                        // Then start it if it can be started
                        if can_start {
                            if is_git_repo {
                                commands.push(Message::StartTaskWithWorktree(task_id));
                            } else {
                                commands.push(Message::StartTask(task_id));
                            }
                        }
                    }
                }
                // New task creation - create and immediately start
                else if !input.is_empty() {
                    // Take pending images before borrowing project
                    let pending_images = std::mem::take(&mut self.model.ui_state.pending_images);
                    let title_len = input.len();

                    // Check if git repo before mutable borrow
                    let is_git_repo = self.model.active_project()
                        .map(|p| p.is_git_repo())
                        .unwrap_or(false);

                    if let Some(project) = self.model.active_project_mut() {
                        let mut task = Task::new(input);
                        let task_id = task.id;
                        // Attach pending images
                        task.images = pending_images;
                        // Insert at beginning so newest tasks appear first in Planned
                        project.tasks.insert(0, task);

                        // Clear editor after creating task
                        self.model.ui_state.clear_input();
                        // Focus on the kanban board and select the new task
                        self.model.ui_state.focus = FocusArea::KanbanBoard;
                        self.model.ui_state.selected_column = TaskStatus::Planned;
                        self.model.ui_state.selected_task_idx = Some(0);
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;

                        // Request title summarization if title is long
                        if title_len > 40 {
                            commands.push(Message::RequestTitleSummary { task_id });
                        }

                        // Immediately start the task (use worktree isolation for git repos)
                        if is_git_repo {
                            commands.push(Message::StartTaskWithWorktree(task_id));
                        } else {
                            commands.push(Message::StartTask(task_id));
                        }
                    }
                }
            }

            Message::OpenExternalEditor => {
                // This is handled specially in main.rs where we have terminal access
                // If it reaches here, something went wrong - just ignore it
            }

            Message::ExternalEditorFinished(content) => {
                // Set the edited content as input and submit
                let input = content.trim().to_string();
                if !input.is_empty() {
                    self.model.ui_state.set_input_text(&input);
                    commands.push(Message::InputSubmit);
                }
                // Focus stays on TaskInput after external editor
            }

            Message::FocusChanged(area) => {
                self.model.ui_state.focus = area;
            }

            Message::NavigateUp => {
                // Handle ProjectTabs navigation separately
                if self.model.ui_state.focus == FocusArea::ProjectTabs {
                    // Up from ProjectTabs does nothing (can't go higher)
                    return vec![];
                }

                // Gather info first to avoid borrow issues
                let current_column = self.model.ui_state.selected_column;
                let above_status = match current_column {
                    TaskStatus::InProgress => Some(TaskStatus::Planned),
                    TaskStatus::NeedsInput => Some(TaskStatus::Queued),
                    TaskStatus::Review => Some(TaskStatus::InProgress),
                    TaskStatus::Done => Some(TaskStatus::NeedsInput),
                    _ => None, // Planned and Queued have nothing above
                };
                let above_tasks_len = above_status
                    .and_then(|s| self.model.active_project().map(|p| p.tasks_by_status(s).len()))
                    .unwrap_or(0);

                // Get current column task count and clamp index if needed
                let current_tasks_len = self.model.active_project()
                    .map(|p| p.tasks_by_status(current_column).len())
                    .unwrap_or(0);

                // Clamp selected index to valid range
                let idx = self.model.ui_state.selected_task_idx
                    .map(|i| if current_tasks_len > 0 { i.min(current_tasks_len - 1) } else { 0 });

                // Sync if needed
                if idx != self.model.ui_state.selected_task_idx {
                    self.model.ui_state.selected_task_idx = idx;
                }

                // Check if we're at the top of Planned or Queued and should move to ProjectTabs
                let is_top_row = matches!(current_column, TaskStatus::Planned | TaskStatus::Queued);
                let at_top_of_column = match idx {
                    None => true, // Empty column
                    Some(0) => true, // At first task
                    _ => false,
                };

                if is_top_row && at_top_of_column {
                    // Move to ProjectTabs
                    self.model.ui_state.focus = FocusArea::ProjectTabs;
                    // Set selected tab based on current column:
                    // Planned = left side, select based on active project
                    // For now, select the active project + 1 (since 0 = +project button)
                    self.model.ui_state.selected_project_tab_idx = self.model.active_project_idx + 1;
                    return vec![];
                }


                if let Some(idx) = idx {
                    if idx > 0 {
                        // Move up within column
                        self.model.ui_state.selected_task_idx = Some(idx - 1);
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;
                    } else if let Some(status) = above_status {
                        // At top of column - move to column above
                        self.save_scroll_offset();
                        self.model.ui_state.selected_column = status;
                        self.model.ui_state.selected_task_idx = if above_tasks_len > 0 {
                            Some(above_tasks_len - 1)
                        } else {
                            None
                        };
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;
                    }
                } else if let Some(status) = above_status {
                    // No task selected (empty column) - move to column above
                    self.save_scroll_offset();
                    self.model.ui_state.selected_column = status;
                    self.model.ui_state.selected_task_idx = if above_tasks_len > 0 {
                        Some(above_tasks_len - 1)
                    } else {
                        None
                    };
                    self.model.ui_state.title_scroll_offset = 0;
                    self.model.ui_state.title_scroll_delay = 0;
                }
            }

            Message::NavigateDown => {
                // Handle ProjectTabs navigation - down returns to KanbanBoard
                if self.model.ui_state.focus == FocusArea::ProjectTabs {
                    self.model.ui_state.focus = FocusArea::KanbanBoard;
                    // Ensure we're in one of the top row columns (Planned or Queued)
                    if !matches!(self.model.ui_state.selected_column, TaskStatus::Planned | TaskStatus::Queued) {
                        self.model.ui_state.selected_column = TaskStatus::Planned;
                    }
                    // Select the first item in the column
                    let tasks_len = self.model.active_project()
                        .map(|p| p.tasks_by_status(self.model.ui_state.selected_column).len())
                        .unwrap_or(0);
                    self.model.ui_state.selected_task_idx = if tasks_len > 0 { Some(0) } else { None };
                    return vec![];
                }

                // Gather info first to avoid borrow issues
                let (tasks_len, current_idx, below_status, below_tasks_len, needs_sync) = {
                    if let Some(project) = self.model.active_project() {
                        let tasks = project.tasks_by_status(self.model.ui_state.selected_column);
                        let tasks_len = tasks.len();
                        // Check if index is out of bounds and needs syncing
                        let (idx, needs_sync) = match self.model.ui_state.selected_task_idx {
                            Some(i) if i >= tasks_len && tasks_len > 0 => (tasks_len - 1, true),
                            Some(i) => (i, false),
                            None => (0, false),
                        };
                        // 2x3 grid navigation - move down in same column
                        let below = match self.model.ui_state.selected_column {
                            TaskStatus::Planned => Some(TaskStatus::InProgress),
                            TaskStatus::Queued => Some(TaskStatus::NeedsInput),
                            TaskStatus::InProgress => Some(TaskStatus::Review),
                            TaskStatus::NeedsInput => Some(TaskStatus::Done),
                            _ => None, // Review and Done have nothing below
                        };
                        let below_len = below
                            .map(|s| project.tasks_by_status(s).len())
                            .unwrap_or(0);
                        (tasks_len, idx, below, below_len, needs_sync)
                    } else {
                        (0, 0, None, 0, false)
                    }
                };

                // Sync selection if it was out of bounds
                if needs_sync {
                    self.model.ui_state.selected_task_idx = Some(current_idx);
                }

                if self.model.ui_state.selected_task_idx.is_none() && tasks_len > 0 {
                    // No selection but column has tasks - select first
                    self.model.ui_state.selected_task_idx = Some(0);
                    self.model.ui_state.title_scroll_offset = 0;
                    self.model.ui_state.title_scroll_delay = 0;
                } else if self.model.ui_state.selected_task_idx.is_none() && tasks_len == 0 {
                    // Empty column - move to column below or focus task input
                    if let Some(status) = below_status {
                        self.save_scroll_offset();
                        self.model.ui_state.selected_column = status;
                        self.model.ui_state.selected_task_idx = if below_tasks_len > 0 { Some(0) } else { None };
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;
                    } else {
                        // At bottom row (Review/Done) - focus task input
                        self.save_scroll_offset();
                        self.model.ui_state.focus = FocusArea::TaskInput;
                    }
                } else if current_idx + 1 < tasks_len {
                    self.model.ui_state.selected_task_idx = Some(current_idx + 1);
                    self.model.ui_state.title_scroll_offset = 0;
                    self.model.ui_state.title_scroll_delay = 0;
                } else if let Some(status) = below_status {
                    // At bottom of column - move to column below
                    self.save_scroll_offset();
                    self.model.ui_state.selected_column = status;
                    self.model.ui_state.selected_task_idx = if below_tasks_len > 0 { Some(0) } else { None };
                    self.model.ui_state.title_scroll_offset = 0;
                    self.model.ui_state.title_scroll_delay = 0;
                } else {
                    // At bottom of Review/Done column - focus task input
                    self.save_scroll_offset();
                    self.model.ui_state.focus = FocusArea::TaskInput;
                }
            }

            Message::NavigateLeft => {
                // Handle ProjectTabs navigation
                if self.model.ui_state.focus == FocusArea::ProjectTabs {
                    // Move left in project tabs (0 = +project, 1+ = projects)
                    if self.model.ui_state.selected_project_tab_idx > 0 {
                        self.model.ui_state.selected_project_tab_idx -= 1;
                    }
                    return vec![];
                }

                // Linear navigation through all columns: Planned -> Queued -> InProgress -> NeedsInput -> Review -> Done
                let columns = TaskStatus::all();
                if let Some(idx) = columns.iter().position(|&s| s == self.model.ui_state.selected_column) {
                    if idx > 0 {
                        self.save_scroll_offset();
                        let new_status = columns[idx - 1];
                        self.model.ui_state.selected_column = new_status;
                        // Restore saved scroll position or select first task
                        self.model.ui_state.selected_task_idx = self.get_restored_task_idx(new_status);
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;
                    }
                }
            }

            Message::NavigateRight => {
                // Handle ProjectTabs navigation
                if self.model.ui_state.focus == FocusArea::ProjectTabs {
                    // Move right in project tabs (0 = +project, 1..=n = projects)
                    // Max index is num_projects (for projects 1..num_projects, and +project at 0)
                    let max_idx = self.model.projects.len();
                    if self.model.ui_state.selected_project_tab_idx < max_idx {
                        self.model.ui_state.selected_project_tab_idx += 1;
                    }
                    return vec![];
                }

                // Linear navigation through all columns: Planned -> Queued -> InProgress -> NeedsInput -> Review -> Done
                let columns = TaskStatus::all();
                if let Some(idx) = columns.iter().position(|&s| s == self.model.ui_state.selected_column) {
                    if idx + 1 < columns.len() {
                        self.save_scroll_offset();
                        let new_status = columns[idx + 1];
                        self.model.ui_state.selected_column = new_status;
                        // Restore saved scroll position or select first task
                        self.model.ui_state.selected_task_idx = self.get_restored_task_idx(new_status);
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;
                    }
                }
            }

            Message::NavigateToStart => {
                // Handle ProjectTabs navigation - jump to first tab
                if self.model.ui_state.focus == FocusArea::ProjectTabs {
                    self.model.ui_state.selected_project_tab_idx = 0;
                    return vec![];
                }

                // Jump to first task in current column
                let tasks_len = self.model.active_project()
                    .map(|p| p.tasks_by_status(self.model.ui_state.selected_column).len())
                    .unwrap_or(0);
                if tasks_len > 0 {
                    self.model.ui_state.selected_task_idx = Some(0);
                    self.model.ui_state.title_scroll_offset = 0;
                    self.model.ui_state.title_scroll_delay = 0;
                }
            }

            Message::NavigateToEnd => {
                // Handle ProjectTabs navigation - jump to last tab
                if self.model.ui_state.focus == FocusArea::ProjectTabs {
                    self.model.ui_state.selected_project_tab_idx = self.model.projects.len();
                    return vec![];
                }

                // Jump to last task in current column
                let tasks_len = self.model.active_project()
                    .map(|p| p.tasks_by_status(self.model.ui_state.selected_column).len())
                    .unwrap_or(0);
                if tasks_len > 0 {
                    self.model.ui_state.selected_task_idx = Some(tasks_len - 1);
                    self.model.ui_state.title_scroll_offset = 0;
                    self.model.ui_state.title_scroll_delay = 0;
                }
            }

            Message::ToggleHelp => {
                self.model.ui_state.show_help = !self.model.ui_state.show_help;
                // Reset scroll to top when opening help
                if self.model.ui_state.show_help {
                    self.model.ui_state.help_scroll_offset = 0;
                }
            }

            Message::ScrollHelpUp(lines) => {
                self.model.ui_state.help_scroll_offset =
                    self.model.ui_state.help_scroll_offset.saturating_sub(lines);
            }

            Message::ScrollHelpDown(lines) => {
                // Cap scroll so we can't scroll past the content
                // Help content is 36 lines; allow scrolling until last line is visible
                const HELP_CONTENT_LINES: usize = 36;
                let max_scroll = HELP_CONTENT_LINES.saturating_sub(1);
                self.model.ui_state.help_scroll_offset = self
                    .model
                    .ui_state
                    .help_scroll_offset
                    .saturating_add(lines)
                    .min(max_scroll);
            }

            Message::ToggleTaskPreview => {
                self.model.ui_state.show_task_preview = !self.model.ui_state.show_task_preview;
                // Reset to general tab when opening the modal
                if self.model.ui_state.show_task_preview {
                    self.model.ui_state.task_detail_tab = crate::model::TaskDetailTab::default();
                }
            }

            Message::TaskDetailNextTab => {
                let new_tab = self.model.ui_state.task_detail_tab.next();
                self.model.ui_state.task_detail_tab = new_tab;

                // Load git diff when switching to Git tab
                if new_tab == crate::model::TaskDetailTab::Git {
                    if let Some(task_id) = self.model.ui_state.selected_task_id {
                        // Check if we need to load the diff (not cached for this task)
                        let needs_load = self.model.ui_state.git_diff_cache
                            .as_ref()
                            .map(|(id, _)| *id != task_id)
                            .unwrap_or(true);
                        if needs_load {
                            return vec![Message::LoadGitDiff(task_id)];
                        }
                    }
                }
            }

            Message::TaskDetailPrevTab => {
                let new_tab = self.model.ui_state.task_detail_tab.prev();
                self.model.ui_state.task_detail_tab = new_tab;

                // Load git diff when switching to Git tab
                if new_tab == crate::model::TaskDetailTab::Git {
                    if let Some(task_id) = self.model.ui_state.selected_task_id {
                        // Check if we need to load the diff (not cached for this task)
                        let needs_load = self.model.ui_state.git_diff_cache
                            .as_ref()
                            .map(|(id, _)| *id != task_id)
                            .unwrap_or(true);
                        if needs_load {
                            return vec![Message::LoadGitDiff(task_id)];
                        }
                    }
                }
            }

            Message::ScrollGitDiffUp(lines) => {
                self.model.ui_state.git_diff_scroll_offset =
                    self.model.ui_state.git_diff_scroll_offset.saturating_sub(lines);
            }

            Message::ScrollGitDiffDown(lines) => {
                // Get the number of lines in the cached diff to cap scrolling
                let max_lines = self.model.ui_state.git_diff_cache
                    .as_ref()
                    .map(|(_, diff)| diff.lines().count())
                    .unwrap_or(0);
                let max_scroll = max_lines.saturating_sub(10); // Leave some visible lines
                self.model.ui_state.git_diff_scroll_offset = self
                    .model
                    .ui_state
                    .git_diff_scroll_offset
                    .saturating_add(lines)
                    .min(max_scroll);
            }

            Message::LoadGitDiff(task_id) => {
                // Reset scroll when loading new diff
                self.model.ui_state.git_diff_scroll_offset = 0;

                // Load the diff for this task
                if let Some(project) = self.model.active_project() {
                    match crate::worktree::get_task_diff(&project.working_dir, task_id) {
                        Ok(diff) => {
                            self.model.ui_state.git_diff_cache = Some((task_id, diff));
                        }
                        Err(e) => {
                            // Store empty diff with error message
                            self.model.ui_state.git_diff_cache = Some((
                                task_id,
                                format!("Error loading diff: {}", e),
                            ));
                        }
                    }
                }
            }

            Message::Tick => {
                // Increment animation frame for spinners
                self.model.ui_state.animation_frame = self.model.ui_state.animation_frame.wrapping_add(1);

                // Advance logo highlight animation if active (frames 1-5, then back to 0)
                // Frame 1 = lead-in (absorbs timing variance), frames 2-5 = highlight glides up
                // (frame 2 = feet, frame 3 = body, frame 4 = face, frame 5 = head)
                if self.model.ui_state.logo_shimmer_frame > 0 {
                    self.model.ui_state.logo_shimmer_frame += 1;
                    if self.model.ui_state.logo_shimmer_frame > 5 {
                        self.model.ui_state.logo_shimmer_frame = 0; // Animation complete
                    }
                }

                // Handle mascot eye animation timing
                if self.model.ui_state.eye_animation_ticks_remaining > 0 {
                    // Animation is playing, count down
                    self.model.ui_state.eye_animation_ticks_remaining -= 1;
                    if self.model.ui_state.eye_animation_ticks_remaining == 0 {
                        // Animation done, revert to normal eyes
                        self.model.ui_state.eye_animation = EyeAnimation::Normal;
                    }
                } else if self.model.ui_state.eye_animation_cooldown > 0 {
                    // Waiting for next animation
                    self.model.ui_state.eye_animation_cooldown -= 1;
                } else {
                    // Cooldown expired, trigger a random eye animation
                    self.model.ui_state.eye_animation = EyeAnimation::random();
                    // Animation lasts 2-3 ticks (200-300ms) for a quick, subtle effect
                    self.model.ui_state.eye_animation_ticks_remaining = 2;
                    // Next animation in 45-75 seconds (450-750 ticks at 100ms each)
                    // Use current time for randomness
                    let random_offset = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| (d.as_millis() % 300) as u16)
                        .unwrap_or(0);
                    self.model.ui_state.eye_animation_cooldown = 450 + random_offset;
                }

                // Rotate welcome messages when on welcome screen (no projects)
                // Only auto-rotate when the speech bubble is not focused
                if self.model.projects.is_empty() && !self.model.ui_state.welcome_bubble_focused {
                    if self.model.ui_state.welcome_message_cooldown > 0 {
                        self.model.ui_state.welcome_message_cooldown -= 1;
                    } else {
                        // Advance to next message
                        let count = crate::ui::welcome_message_count();
                        self.model.ui_state.welcome_message_idx =
                            (self.model.ui_state.welcome_message_idx + 1) % count;
                        // Reset cooldown (~8 seconds)
                        self.model.ui_state.welcome_message_cooldown = 80;
                    }
                }

                // Decay startup navigation hints after ~10 seconds
                if let Some(remaining) = self.model.ui_state.startup_hint_until_tick {
                    if remaining > 0 {
                        self.model.ui_state.startup_hint_until_tick = Some(remaining - 1);
                    } else {
                        self.model.ui_state.startup_hint_until_tick = None;
                    }
                }

                // Animate confirmation prompt highlight sweep
                if let Some(ref mut confirmation) = self.model.ui_state.pending_confirmation {
                    if confirmation.animation_tick > 0 {
                        confirmation.animation_tick -= 1;
                    }
                }

                // Decay status message after timeout
                if self.model.ui_state.status_message_decay > 0 {
                    self.model.ui_state.status_message_decay -= 1;
                    if self.model.ui_state.status_message_decay == 0 {
                        self.model.ui_state.status_message = None;
                    }
                }

                // Animate scroll for long task titles (every tick = ~100ms)
                // Wait ~1 second (10 ticks) before starting to scroll so user can read the first word
                const SCROLL_DELAY_TICKS: usize = 10;
                if let Some(task_idx) = self.model.ui_state.selected_task_idx {
                    if let Some(project) = self.model.active_project() {
                        let tasks = project.tasks_by_status(self.model.ui_state.selected_column);
                        if let Some(task) = tasks.get(task_idx) {
                            let title_len = task.title.chars().count();
                            // Only scroll if title is long (assume ~30 char display width)
                            if title_len > 25 {
                                if self.model.ui_state.title_scroll_delay < SCROLL_DELAY_TICKS {
                                    // Wait before starting to scroll
                                    self.model.ui_state.title_scroll_delay += 1;
                                } else {
                                    self.model.ui_state.title_scroll_offset += 1;
                                    // Wrap around with a pause at the start
                                    if self.model.ui_state.title_scroll_offset > title_len + 5 {
                                        self.model.ui_state.title_scroll_offset = 0;
                                        self.model.ui_state.title_scroll_delay = 0;
                                    }
                                }
                            }
                        }
                    }
                }

                // Refresh git status every ~5 seconds (50 ticks at 100ms per tick)
                if self.model.ui_state.animation_frame % 50 == 0 {
                    commands.push(Message::RefreshGitStatus);
                }

                // Fetch from remote every ~30 seconds (300 ticks at 100ms per tick)
                // to keep the ahead/behind indicators up to date
                if self.model.ui_state.animation_frame % 300 == 0 {
                    // Only fetch if there's no operation in progress
                    let should_fetch = self.model.active_project()
                        .map(|p| p.git_operation_in_progress.is_none())
                        .unwrap_or(false);
                    if should_fetch {
                        commands.push(Message::StartGitFetch);
                    }
                }
            }

            // === Quick Claude CLI Pane ===

            Message::OpenClaudeCliPane => {
                // Get the working directory of the active project
                if let Some(project) = self.model.active_project() {
                    let working_dir = project.working_dir.clone();
                    if let Err(e) = crate::tmux::split_pane_with_claude(&working_dir) {
                        commands.push(Message::Error(format!(
                            "Failed to open Claude pane: {}",
                            e
                        )));
                    }
                } else {
                    commands.push(Message::Error(
                        "No active project - cannot open Claude CLI".to_string(),
                    ));
                }
            }

            // === Configuration Modal ===

            Message::ShowConfigModal => {
                use crate::model::{ConfigModalState, ConfigField};

                // Get current project commands (or defaults)
                let temp_commands = self.model.active_project()
                    .map(|p| p.commands.clone())
                    .unwrap_or_default();
                let temp_editor = self.model.global_settings.default_editor;

                self.model.ui_state.config_modal = Some(ConfigModalState {
                    selected_field: ConfigField::default(),
                    editing: false,
                    edit_buffer: String::new(),
                    temp_commands,
                    temp_editor,
                });
            }

            Message::CloseConfigModal => {
                self.model.ui_state.config_modal = None;
            }

            Message::ConfigNavigateDown => {
                if let Some(ref mut config) = self.model.ui_state.config_modal {
                    config.selected_field = config.selected_field.next();
                }
            }

            Message::ConfigNavigateUp => {
                if let Some(ref mut config) = self.model.ui_state.config_modal {
                    config.selected_field = config.selected_field.prev();
                }
            }

            Message::ConfigEditField => {
                use crate::model::{ConfigField, Editor};

                if let Some(ref mut config) = self.model.ui_state.config_modal {
                    if config.selected_field == ConfigField::DefaultEditor {
                        if config.editing {
                            // Cycle to next editor
                            let editors = Editor::all();
                            let idx = editors.iter().position(|e| *e == config.temp_editor).unwrap_or(0);
                            config.temp_editor = editors[(idx + 1) % editors.len()];
                        } else {
                            // Enter edit mode
                            config.editing = true;
                        }
                    } else {
                        // Command field - enter text edit mode
                        if !config.editing {
                            // Set edit buffer to current value
                            config.edit_buffer = match config.selected_field {
                                ConfigField::CheckCommand => config.temp_commands.check.clone().unwrap_or_default(),
                                ConfigField::RunCommand => config.temp_commands.run.clone().unwrap_or_default(),
                                ConfigField::TestCommand => config.temp_commands.test.clone().unwrap_or_default(),
                                ConfigField::FormatCommand => config.temp_commands.format.clone().unwrap_or_default(),
                                ConfigField::LintCommand => config.temp_commands.lint.clone().unwrap_or_default(),
                                ConfigField::DefaultEditor => String::new(),
                            };
                            config.editing = true;
                        }
                    }
                }
            }

            Message::ConfigEditFieldPrev => {
                use crate::model::{ConfigField, Editor};

                if let Some(ref mut config) = self.model.ui_state.config_modal {
                    if config.selected_field == ConfigField::DefaultEditor && config.editing {
                        // Cycle to previous editor
                        let editors = Editor::all();
                        let idx = editors.iter().position(|e| *e == config.temp_editor).unwrap_or(0);
                        config.temp_editor = editors[(idx + editors.len() - 1) % editors.len()];
                    }
                }
            }

            Message::ConfigUpdateBuffer(new_buffer) => {
                if let Some(ref mut config) = self.model.ui_state.config_modal {
                    config.edit_buffer = new_buffer;
                }
            }

            Message::ConfigConfirmEdit => {
                use crate::model::ConfigField;

                if let Some(ref mut config) = self.model.ui_state.config_modal {
                    if config.selected_field == ConfigField::DefaultEditor {
                        // Editor field - just exit edit mode (cycling is done via h/l)
                        config.editing = false;
                    } else {
                        // Command field - save buffer to temp_commands
                        let value = if config.edit_buffer.is_empty() {
                            None
                        } else {
                            Some(config.edit_buffer.clone())
                        };

                        match config.selected_field {
                            ConfigField::CheckCommand => config.temp_commands.check = value,
                            ConfigField::RunCommand => config.temp_commands.run = value,
                            ConfigField::TestCommand => config.temp_commands.test = value,
                            ConfigField::FormatCommand => config.temp_commands.format = value,
                            ConfigField::LintCommand => config.temp_commands.lint = value,
                            ConfigField::DefaultEditor => {}
                        }

                        config.editing = false;
                        config.edit_buffer.clear();
                    }
                }
            }

            Message::ConfigCancelEdit => {
                if let Some(ref mut config) = self.model.ui_state.config_modal {
                    config.editing = false;
                    config.edit_buffer.clear();
                }
            }

            Message::ConfigSave => {
                // Extract values before borrowing mutably
                let (temp_editor, temp_commands) = if let Some(ref config) = self.model.ui_state.config_modal {
                    (config.temp_editor, config.temp_commands.clone())
                } else {
                    (self.model.global_settings.default_editor, crate::model::ProjectCommands::default())
                };

                // Save global settings
                self.model.global_settings.default_editor = temp_editor;

                // Save project commands
                if let Some(project) = self.model.active_project_mut() {
                    project.commands = temp_commands;
                }

                self.model.ui_state.config_modal = None;
                commands.push(Message::SetStatusMessage(Some("Configuration saved".to_string())));
            }

            Message::ConfigResetToDefaults => {
                use crate::model::ProjectCommands;

                // Get the project working dir first
                let detected = self.model.active_project()
                    .map(|p| ProjectCommands::detect(&p.working_dir))
                    .unwrap_or_default();

                if let Some(ref mut config) = self.model.ui_state.config_modal {
                    config.temp_commands = detected;
                    commands.push(Message::SetStatusMessage(Some("Reset to auto-detected defaults".to_string())));
                }
            }

            Message::TriggerRestart => {
                // Only build and restart for bootstrap mode (developing KanBlam itself)
                // For other projects, just show success - no need to restart the TUI
                let (is_bootstrap, debug_info) = self.model.active_project()
                    .map(|p| {
                        let exe_path = std::env::current_exe().ok();
                        let exe_canonical = exe_path.as_ref().and_then(|p| p.canonicalize().ok());
                        let project_canonical = p.working_dir.canonicalize().ok();
                        let is_boot = is_bootstrap_project(p);
                        let info = format!(
                            "exe={:?}, project={:?}, is_bootstrap={}",
                            exe_canonical, project_canonical, is_boot
                        );
                        (is_boot, info)
                    })
                    .unwrap_or((false, "no active project".to_string()));

                // Temporary debug: log to file
                let _ = std::fs::write("/tmp/kanblam-bootstrap-debug.txt", &debug_info);

                if !is_bootstrap {
                    // Not bootstrap mode - just show success, no build/restart needed
                    commands.push(Message::SetStatusMessage(Some(
                        "✓ Changes applied successfully.".to_string()
                    )));
                    return commands;
                }

                // Bootstrap mode: check for pending operations before restart
                let has_pending = self.model.active_project().map(|p| {
                    p.tasks.iter().any(|t| matches!(
                        t.status,
                        TaskStatus::Accepting | TaskStatus::Updating | TaskStatus::Applying
                    )) || p.main_worktree_lock.is_some()
                }).unwrap_or(false);

                if has_pending {
                    commands.push(Message::SetStatusMessage(Some(
                        "Cannot restart: operations in progress. Wait for them to complete.".to_string()
                    )));
                } else {
                    // Start async build before restarting
                    commands.push(Message::SetStatusMessage(Some(
                        "Building...".to_string()
                    )));
                    commands.push(Message::StartBuildForRestart);
                }
            }

            Message::StartBuildForRestart => {
                // Require async sender - fail explicitly if missing
                let sender = match self.async_sender.clone() {
                    Some(s) => s,
                    None => {
                        commands.push(Message::Error(
                            "Internal error: async_sender not configured.".to_string()
                        ));
                        return commands;
                    }
                };

                let current_dir = std::env::current_dir().unwrap_or_default();

// Detect if we're running debug or release build
                // Check the current executable path for "debug" or "release"
                let is_release = std::env::current_exe()
                    .map(|p| p.to_string_lossy().contains("/release/"))
                    .unwrap_or(true); // Default to release if we can't determine

                // Spawn build in background to keep UI responsive
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        let mut cmd = std::process::Command::new("cargo");
                        cmd.arg("build");
                        if is_release {
                            cmd.arg("--release");
                        }
                        cmd.current_dir(&current_dir).output()
                    }).await;

                    let msg = match result {
                        Ok(Ok(output)) if output.status.success() => {
                            Message::BuildCompleted
                        }
                        Ok(Ok(output)) => {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            let error_preview: String = stderr.lines().take(5).collect::<Vec<_>>().join("\n");
                            Message::BuildFailed { error: error_preview }
                        }
                        Ok(Err(e)) => {
                            Message::BuildFailed { error: format!("Failed to run cargo: {}", e) }
                        }
                        Err(e) => {
                            Message::BuildFailed { error: format!("Task panicked: {}", e) }
                        }
                    };

                    let _ = sender.send(msg);
                });
            }

            Message::BuildCompleted => {
                // Build succeeded, proceed with restart
                commands.push(Message::SetStatusMessage(Some(
                    "✓ Build succeeded. Restarting...".to_string()
                )));
                self.should_restart = true;
            }

            Message::BuildFailed { error } => {
                // Build failed - show error and ask to unapply if we have applied changes
                if let Some(task_id) = self.model.active_project().and_then(|p| p.applied_task_id) {
                    self.model.ui_state.confirmation_scroll_offset = 0;
                    self.model.ui_state.pending_confirmation = Some(PendingConfirmation {
                        message: format!(
                            "Build failed:\n{}\n\nUnapply the changes?",
                            error
                        ),
                        action: PendingAction::ForceUnapply(task_id),
                        animation_tick: 20,
                    });
                } else {
                    commands.push(Message::Error(format!(
                        "Build failed: {}", error
                    )));
                }
            }

            Message::Quit => {
                self.should_quit = true;
            }

            Message::QuitAndSwitchPane(_) => {
                // Legacy - just quit
                self.should_quit = true;
            }

            Message::OpenClaudeCliPane => {
                // Open a new pane to the right with a fresh Claude CLI session
                if let Some(project) = self.model.active_project() {
                    let working_dir = project.working_dir.clone();
                    if let Err(e) = crate::tmux::split_pane_with_claude(&working_dir) {
                        commands.push(Message::Error(format!("Failed to open Claude pane: {}", e)));
                    }
                } else {
                    commands.push(Message::Error("No active project".to_string()));
                }
            }

            Message::Error(err) => {
                // Display error in status bar so user actually sees it
                self.model.ui_state.status_message = Some(format!("❌ {}", err));
            }
        }

        // Keep selected_task_id in sync with selected_task_idx
        // This ensures that if tasks move around, we can find the right one
        if let Some(idx) = self.model.ui_state.selected_task_idx {
            if let Some(project) = self.model.active_project() {
                let tasks = project.tasks_by_status(self.model.ui_state.selected_column);
                self.model.ui_state.selected_task_id = tasks.get(idx).map(|t| t.id);
            }
        } else {
            self.model.ui_state.selected_task_id = None;
        }

        commands
    }
}

/// Load application state from disk
pub fn load_state() -> Result<AppModel> {
    use crate::model::ProjectTaskData;

    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kanclaude");

    let state_file = data_dir.join("state.json");

    if state_file.exists() {
        let content = std::fs::read_to_string(&state_file)?;
        let mut model: AppModel = serde_json::from_str(&content)?;

        // Load tasks from per-project files (with migration from global state)
        for project in &mut model.projects {
            let project_file = ProjectTaskData::file_path(&project.working_dir);
            if project_file.exists() {
                // New way: load from project directory
                project.load_tasks();
            }
            // else: keep tasks from global state (migration path)
            // They'll be saved to project dir on next save
        }

        Ok(model)
    } else {
        Ok(AppModel::default())
    }
}

/// Save application state to disk
/// Also saves tasks to per-project .kanblam/tasks.json files
pub fn save_state(model: &AppModel) -> Result<()> {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kanclaude");

    std::fs::create_dir_all(&data_dir)?;

    // Save tasks to each project's .kanblam directory
    for project in &model.projects {
        if let Err(e) = project.save_tasks() {
            eprintln!("Warning: Failed to save tasks for {}: {}", project.name, e);
        }
    }

    // Save global state (still includes tasks for backwards compatibility,
    // but we prefer loading from project dirs)
    let state_file = data_dir.join("state.json");
    let content = serde_json::to_string_pretty(model)?;
    std::fs::write(state_file, content)?;

    Ok(())
}

/// Run a project's check command to verify applied changes compile
/// Returns Ok(()) if check passes or no check command is configured,
/// Err with error message if check fails
pub fn run_project_check(project: &Project) -> Result<(), String> {
    use std::process::Command;

    let check_cmd = project.commands.effective_check(&project.working_dir);

    match check_cmd {
        None => Ok(()), // No check command configured or detected
        Some(cmd) => {
            // Parse the command (split on whitespace, first is program, rest are args)
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            if parts.is_empty() {
                return Ok(());
            }

            let program = parts[0];
            let args = &parts[1..];

            // Run the check command
            let output = Command::new(program)
                .args(args)
                .current_dir(&project.working_dir)
                .output();

            match output {
                Ok(result) => {
                    if result.status.success() {
                        Ok(())
                    } else {
                        let stderr = String::from_utf8_lossy(&result.stderr);
                        let stdout = String::from_utf8_lossy(&result.stdout);
                        // Return a concise error - first line of stderr or stdout
                        let error_line = stderr.lines().next()
                            .or_else(|| stdout.lines().next())
                            .unwrap_or("Check failed");
                        Err(format!("Build check failed: {}", error_line))
                    }
                }
                Err(e) => {
                    // Command failed to run (e.g., not found)
                    Err(format!("Failed to run '{}': {}", cmd, e))
                }
            }
        }
    }
}
