use crate::message::Message;
use crate::model::{AppModel, FocusArea, PendingAction, PendingConfirmation, Project, Task, TaskStatus, SessionMode};
use crate::notify;
use crate::sidecar::SidecarClient;
use anyhow::Result;
use chrono::Utc;
use std::path::PathBuf;

/// Application state and update logic (TEA pattern)
pub struct App {
    pub model: AppModel,
    pub should_quit: bool,
    /// Sidecar client for SDK session management (if available)
    pub sidecar_client: Option<SidecarClient>,
}

impl App {
    pub fn new() -> Self {
        Self {
            model: AppModel::default(),
            should_quit: false,
            sidecar_client: None,
        }
    }

    pub fn with_model(model: AppModel) -> Self {
        Self {
            model,
            should_quit: false,
            sidecar_client: None,
        }
    }

    pub fn with_sidecar(mut self, client: Option<SidecarClient>) -> Self {
        self.sidecar_client = client;
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
                self.model.ui_state.selected_is_divider = false;
                self.model.ui_state.selected_is_divider_above = false;
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

        let visual_idx = if let Some(project) = self.model.active_project() {
            let tasks = project.tasks_by_status(column);

            if let Some(idx) = task_idx {
                // Check if first task has divider_above
                let has_divider_above = tasks.first().map(|t| t.divider_above).unwrap_or(false);

                // If selecting divider_above, visual index is 0
                if self.model.ui_state.selected_is_divider_above && idx == 0 {
                    0
                } else {
                    // Count dividers before selected task
                    let dividers_before: usize = tasks.iter()
                        .take(idx)
                        .filter(|t| t.divider_below)
                        .count();
                    // Start with task_idx + dividers before
                    let mut visual = idx + dividers_before;
                    // Add 1 if there's a divider_above (shifts everything down)
                    if has_divider_above {
                        visual += 1;
                    }
                    // If divider below is selected, add 1 to select the divider itself
                    if self.model.ui_state.selected_is_divider {
                        visual += 1;
                    }
                    visual
                }
            } else {
                0
            }
        } else {
            0
        };

        self.model.ui_state.column_scroll_offsets[column.index()] = visual_idx;
    }

    /// Restore scroll position when entering a column
    /// Returns the task index to select based on saved offset
    fn get_restored_task_idx(&self, column: TaskStatus) -> Option<usize> {
        let saved_offset = self.model.ui_state.column_scroll_offsets[column.index()];
        if saved_offset == 0 {
            // No saved offset, select first task
            return if let Some(project) = self.model.active_project() {
                let tasks = project.tasks_by_status(column);
                if tasks.is_empty() { None } else { Some(0) }
            } else {
                None
            };
        }

        // Try to find the task at this visual position
        if let Some(project) = self.model.active_project() {
            let tasks = project.tasks_by_status(column);
            if tasks.is_empty() {
                return None;
            }

            let has_divider_above = tasks.first().map(|t| t.divider_above).unwrap_or(false);
            let mut visual_pos = if has_divider_above { 1 } else { 0 };

            for (idx, task) in tasks.iter().enumerate() {
                if visual_pos >= saved_offset {
                    return Some(idx);
                }
                visual_pos += 1; // The task itself
                if task.divider_below {
                    if visual_pos >= saved_offset {
                        return Some(idx);
                    }
                    visual_pos += 1;
                }
            }

            // If saved offset is beyond end, select last task
            Some(tasks.len().saturating_sub(1))
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
                if let Some(project) = self.model.active_project_mut() {
                    let mut task = Task::new(title);
                    // Attach pending images
                    task.images = pending_images;
                    // Insert at beginning so newest tasks appear first in Planned
                    project.tasks.insert(0, task);
                }
                // Clear editor after creating task
                self.model.ui_state.clear_input();
                // Focus on the kanban board and select the new task
                // (New tasks in Planned are sorted newest first, so index 0)
                self.model.ui_state.focus = FocusArea::KanbanBoard;
                self.model.ui_state.selected_column = TaskStatus::Planned;
                self.model.ui_state.selected_task_idx = Some(0);
                self.model.ui_state.selected_is_divider = false;
                self.model.ui_state.selected_is_divider_above = false;
                self.model.ui_state.title_scroll_offset = 0;
                self.model.ui_state.title_scroll_delay = 0;
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
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.title = title;
                    }
                }
                // Clear editing state and editor
                self.model.ui_state.editing_task_id = None;
                self.model.ui_state.clear_input();
                self.model.ui_state.focus = FocusArea::KanbanBoard;
            }

            Message::CancelEdit => {
                // Clear editing state and editor
                self.model.ui_state.editing_task_id = None;
                self.model.ui_state.editing_divider_id = None;
                self.model.ui_state.editing_divider_is_above = false;
                self.model.ui_state.clear_input();
                self.model.ui_state.focus = FocusArea::KanbanBoard;
            }

            Message::DeleteTask(task_id) => {
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
                            // Push to end (will be last in Done column)
                            project.tasks.push(task);
                        }
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

                // Move cursor to follow the task to Planned
                if follow_to_planned {
                    self.model.ui_state.selected_column = TaskStatus::Planned;
                    self.model.ui_state.selected_task_idx = Some(0);
                    self.model.ui_state.selected_is_divider = false;
                    self.model.ui_state.selected_is_divider_above = false;
                }
            }

            Message::MoveTaskUp => {
                // Move selected task or divider up within its column
                if let Some(selected_idx) = self.model.ui_state.selected_task_idx {
                    let status = self.model.ui_state.selected_column;
                    let is_divider = self.model.ui_state.selected_is_divider;
                    let is_divider_above = self.model.ui_state.selected_is_divider_above;

                    if is_divider_above {
                        // Can't move divider_above any higher, it's already at the top
                    } else if is_divider {
                        if selected_idx == 0 {
                            // Moving divider at index 0 up: convert to divider_above
                            let task_id = self.model.active_project()
                                .and_then(|p| p.tasks_by_status(status).first().map(|t| t.id));

                            if let Some(task_id) = task_id {
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        // Move divider from below to above
                                        let title = task.divider_title.take();
                                        task.divider_below = false;
                                        task.divider_above = true;
                                        task.divider_above_title = title;
                                    }
                                }
                                // Now selecting the divider above
                                self.model.ui_state.selected_is_divider = false;
                                self.model.ui_state.selected_is_divider_above = true;
                            }
                        } else {
                            // Moving a divider up: remove from current task, add to task above
                            let (current_task_id, above_task_id) = {
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

                            if let (Some(current_id), Some(above_id)) = (current_task_id, above_task_id) {
                                if let Some(project) = self.model.active_project_mut() {
                                    // Get divider title before removing
                                    let divider_title = project.tasks.iter()
                                        .find(|t| t.id == current_id)
                                        .and_then(|t| t.divider_title.clone());

                                    // Remove divider from current task
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == current_id) {
                                        task.divider_below = false;
                                        task.divider_title = None;
                                    }
                                    // Add divider to task above
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == above_id) {
                                        task.divider_below = true;
                                        task.divider_title = divider_title;
                                    }
                                    // Move selection up to follow the divider
                                    self.model.ui_state.selected_task_idx = Some(selected_idx - 1);
                                }
                            }
                        }
                    } else if selected_idx > 0 {
                        // Moving a task up
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
                                    // Swap dividers first so they stay in position
                                    let div_a = project.tasks[a].divider_below;
                                    let div_b = project.tasks[b].divider_below;
                                    project.tasks[a].divider_below = div_b;
                                    project.tasks[b].divider_below = div_a;
                                    // Then swap the tasks
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
                // Move selected task or divider down within its column
                if let Some(selected_idx) = self.model.ui_state.selected_task_idx {
                    let status = self.model.ui_state.selected_column;
                    let is_divider = self.model.ui_state.selected_is_divider;
                    let is_divider_above = self.model.ui_state.selected_is_divider_above;

                    if is_divider_above {
                        // Moving divider_above down: convert to divider_below of first task
                        let task_id = self.model.active_project()
                            .and_then(|p| p.tasks_by_status(status).first().map(|t| t.id));

                        if let Some(task_id) = task_id {
                            if let Some(project) = self.model.active_project_mut() {
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                    // Move divider from above to below
                                    let title = task.divider_above_title.take();
                                    task.divider_above = false;
                                    task.divider_below = true;
                                    task.divider_title = title;
                                }
                            }
                            // Now selecting the divider below
                            self.model.ui_state.selected_is_divider_above = false;
                            self.model.ui_state.selected_is_divider = true;
                        }
                    } else if is_divider {
                        // Moving a divider down: remove from current task, add to task below
                        let (current_task_id, below_task_id) = {
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

                        if let (Some(current_id), Some(below_id)) = (current_task_id, below_task_id) {
                            if let Some(project) = self.model.active_project_mut() {
                                // Get divider title before removing
                                let divider_title = project.tasks.iter()
                                    .find(|t| t.id == current_id)
                                    .and_then(|t| t.divider_title.clone());

                                // Remove divider from current task
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == current_id) {
                                    task.divider_below = false;
                                    task.divider_title = None;
                                }
                                // Add divider to task below
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == below_id) {
                                    task.divider_below = true;
                                    task.divider_title = divider_title;
                                }
                                // Move selection down to follow the divider
                                self.model.ui_state.selected_task_idx = Some(selected_idx + 1);
                            }
                        }
                    } else {
                        // Moving a task down
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
                                    // Swap dividers first so they stay in position
                                    let div_a = project.tasks[a].divider_below;
                                    let div_b = project.tasks[b].divider_below;
                                    project.tasks[a].divider_below = div_b;
                                    project.tasks[b].divider_below = div_a;
                                    // Then swap the tasks
                                    project.tasks.swap(a, b);
                                    // Selection follows the task
                                    self.model.ui_state.selected_task_idx = Some(selected_idx + 1);
                                }
                            }
                        }
                    }
                }
            }

            Message::ToggleDivider => {
                // Toggle divider below selected task
                if let Some(selected_idx) = self.model.ui_state.selected_task_idx {
                    // Don't toggle if we're on a divider
                    if self.model.ui_state.selected_is_divider {
                        return commands;
                    }
                    let status = self.model.ui_state.selected_column;
                    // Get task ID from the correctly sorted/filtered view
                    let task_id = self.model.active_project()
                        .and_then(|p| p.tasks_by_status(status).get(selected_idx).map(|t| t.id));

                    if let Some(task_id) = task_id {
                        if let Some(project) = self.model.active_project_mut() {
                            if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                task.divider_below = !task.divider_below;
                            }
                        }
                    }
                }
            }

            Message::DeleteDivider => {
                // Delete divider when one is selected
                let is_divider_above = self.model.ui_state.selected_is_divider_above;
                let is_divider_below = self.model.ui_state.selected_is_divider;

                if is_divider_above || is_divider_below {
                    if let Some(selected_idx) = self.model.ui_state.selected_task_idx {
                        let status = self.model.ui_state.selected_column;
                        // Get task ID from the correctly sorted/filtered view
                        let task_id = self.model.active_project()
                            .and_then(|p| p.tasks_by_status(status).get(selected_idx).map(|t| t.id));

                        if let Some(task_id) = task_id {
                            if let Some(project) = self.model.active_project_mut() {
                                if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                    if is_divider_above {
                                        task.divider_above = false;
                                        task.divider_above_title = None;
                                    } else {
                                        task.divider_below = false;
                                        task.divider_title = None;
                                    }
                                }
                            }
                        }
                        // Move selection back to the task
                        self.model.ui_state.selected_is_divider = false;
                        self.model.ui_state.selected_is_divider_above = false;
                    }
                }
            }

            Message::EditDivider => {
                // Edit divider title when a divider is selected
                let is_divider_above = self.model.ui_state.selected_is_divider_above;
                let is_divider_below = self.model.ui_state.selected_is_divider;

                if is_divider_above || is_divider_below {
                    if let Some(selected_idx) = self.model.ui_state.selected_task_idx {
                        let status = self.model.ui_state.selected_column;
                        // Get task info first (before mutating ui_state)
                        let task_info = self.model.active_project()
                            .and_then(|p| p.tasks_by_status(status).get(selected_idx)
                                .map(|t| {
                                    let title = if is_divider_above {
                                        t.divider_above_title.clone()
                                    } else {
                                        t.divider_title.clone()
                                    };
                                    (t.id, title, is_divider_above)
                                }));

                        if let Some((task_id, current_title, is_above)) = task_info {
                            // Now we can safely mutate ui_state
                            self.model.ui_state.set_input_text(&current_title.unwrap_or_default());
                            self.model.ui_state.editing_divider_id = Some(task_id);
                            self.model.ui_state.editing_divider_is_above = is_above;
                            self.model.ui_state.focus = FocusArea::TaskInput;
                        }
                    }
                }
            }

            Message::UpdateDividerTitle { task_id, title } => {
                let is_above = self.model.ui_state.editing_divider_is_above;
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        if is_above {
                            task.divider_above_title = title;
                        } else {
                            task.divider_title = title;
                        }
                    }
                }
                self.model.ui_state.editing_divider_id = None;
                self.model.ui_state.editing_divider_is_above = false;
                self.model.ui_state.clear_input();
                self.model.ui_state.focus = FocusArea::KanbanBoard;
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
                        }
                    }

                    // Defer the actual worktree creation to allow UI to render first
                    commands.push(Message::CreateWorktree { task_id, project_dir });
                }
            }

            Message::CreateWorktree { task_id, project_dir } => {
                // Create worktree (this is the slow operation, now deferred)
                match crate::worktree::create_worktree(&project_dir, task_id) {
                    Ok(worktree_path) => {
                        commands.push(Message::WorktreeCreated { task_id, worktree_path, project_dir });
                    }
                    Err(e) => {
                        commands.push(Message::WorktreeCreationFailed { task_id, error: e.to_string() });
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
                            }
                            commands.push(Message::Error(format!("Failed to check for changes: {}", e)));
                            return commands;
                        }
                    }

                    // Kill tmux window if exists
                    if let Some(ref window) = window_name {
                        let _ = crate::tmux::kill_task_window(&project_slug, window);
                    }

                    // Merge branch to main (should be fast-forward now)
                    if let Err(e) = crate::worktree::merge_branch(&project_dir, task_id) {
                        // Return to Review status on error
                        if let Some(project) = self.model.active_project_mut() {
                            if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                task.status = TaskStatus::Review;
                            }
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
                    }

                    commands.push(Message::SetStatusMessage(Some(
                        "Task accepted and merged to main.".to_string()
                    )));
                }
            }

            Message::DiscardTask(task_id) => {
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
                    self.model.ui_state.selected_is_divider = false;
                    self.model.ui_state.selected_is_divider_above = false;

                    commands.push(Message::SetStatusMessage(Some(
                        "Task reset to Planned (top). Press Enter to start fresh.".to_string()
                    )));
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
                    }
                }
            }

            Message::OpenTestShell(task_id) => {
                let test_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .and_then(|t| t.worktree_path.as_ref().map(|wt| (p.slug(), wt.clone())))
                });

                if let Some((project_slug, worktree_path)) = test_info {
                    match crate::tmux::create_test_shell(
                        &project_slug,
                        &task_id.to_string(),
                        &worktree_path,
                    ) {
                        Ok(_window_name) => {
                            commands.push(Message::SetStatusMessage(Some(
                                format!("Opened test shell in {}", worktree_path.display())
                            )));
                        }
                        Err(e) => {
                            commands.push(Message::Error(format!("Failed to open test shell: {}", e)));
                        }
                    }
                }
            }

            Message::ApplyTaskChanges(task_id) => {
                // Check if changes are already applied
                if self.model.ui_state.applied_task_id.is_some() {
                    commands.push(Message::SetStatusMessage(Some(
                        "Changes already applied. Press 'u' to unapply first.".to_string()
                    )));
                    return commands;
                }

                // Get task info and project dir
                let task_info = self.model.active_project().and_then(|p| {
                    p.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| (p.working_dir.clone(), t.git_branch.clone()))
                });

                if let Some((project_dir, git_branch)) = task_info {
                    // Check if the task has a git branch reference
                    let branch_name = match git_branch {
                        Some(b) => b,
                        None => {
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
                        commands.push(Message::Error(format!(
                            "Branch '{}' no longer exists. Task data is stale - the branch may have been deleted manually or during a failed accept.",
                            branch_name
                        )));
                        return commands;
                    }

                    match crate::worktree::apply_task_changes(&project_dir, task_id) {
                        Ok(stash_ref) => {
                            self.model.ui_state.applied_task_id = Some(task_id);
                            self.model.ui_state.applied_stash_ref = stash_ref;
                            commands.push(Message::SetStatusMessage(Some(
                                "Changes applied to main worktree. Press 'u' to unapply.".to_string()
                            )));
                        }
                        Err(e) => {
                            commands.push(Message::Error(format!("Failed to apply changes: {}", e)));
                        }
                    }
                }
            }

            Message::UnapplyTaskChanges => {
                if self.model.ui_state.applied_task_id.is_none() {
                    commands.push(Message::SetStatusMessage(Some(
                        "No changes applied to unapply.".to_string()
                    )));
                    return commands;
                }

                let project_dir = self.model.active_project().map(|p| p.working_dir.clone());
                let stash_ref = self.model.ui_state.applied_stash_ref.clone();

                if let Some(project_dir) = project_dir {
                    match crate::worktree::unapply_task_changes(&project_dir, stash_ref.as_deref()) {
                        Ok(()) => {
                            self.model.ui_state.applied_task_id = None;
                            self.model.ui_state.applied_stash_ref = None;
                            commands.push(Message::SetStatusMessage(Some(
                                "Changes unapplied. Original work restored.".to_string()
                            )));
                        }
                        Err(e) => {
                            commands.push(Message::Error(format!("Failed to unapply changes: {}", e)));
                        }
                    }
                }
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

                // Get task title for status message
                let task_title = self.model.active_project()
                    .and_then(|p| p.tasks.iter().find(|t| t.id == after_task_id))
                    .map(|t| t.title.clone())
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
                        t.images.clone(),
                        p.slug(),
                    ))
                });

                if let Some((next_task_id, title, images, project_slug)) = next_task_info {
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
                                    format!("Continuing with queued task: {}", title)
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
                self.model.ui_state.selected_is_divider = false;
                self.model.ui_state.selected_is_divider_above = false;
                self.model.ui_state.title_scroll_offset = 0;
                self.model.ui_state.title_scroll_delay = 0;
            }

            Message::ClickedTask { status, task_idx } => {
                self.model.ui_state.selected_column = status;
                self.model.ui_state.selected_task_idx = Some(task_idx);
                self.model.ui_state.focus = FocusArea::KanbanBoard;
                self.model.ui_state.selected_is_divider = false;
                self.model.ui_state.selected_is_divider_above = false;
                self.model.ui_state.title_scroll_offset = 0;
                self.model.ui_state.title_scroll_delay = 0;
            }

            Message::SwitchProject(idx) => {
                if idx < self.model.projects.len() {
                    self.model.active_project_idx = idx;
                    self.model.ui_state.selected_task_idx = None;
                    self.model.ui_state.selected_is_divider = false;
                    self.model.ui_state.selected_is_divider_above = false;
                    self.model.ui_state.focus = FocusArea::KanbanBoard;

                    // Check if this project needs hooks installed
                    if let Some(project) = self.model.projects.get(idx) {
                        if !project.hooks_installed {
                            let name = project.name.clone();
                            commands.push(Message::ShowConfirmation {
                                message: format!(
                                    "Hooks not installed or outdated for '{}'. Install? (y/n/1-9 switch)",
                                    name
                                ),
                                action: PendingAction::InstallHooks,
                            });
                        }
                    }
                }
            }

            Message::AddProject { name, working_dir } => {
                let project = Project::new(name, working_dir);
                self.model.projects.push(project);
            }

            Message::ReloadClaudeHooks => {
                if let Some(project) = self.model.active_project() {
                    let name = project.name.clone();
                    let needs_hooks = !project.hooks_installed;

                    if needs_hooks {
                        // Install hooks first, then ask to reload
                        commands.push(Message::ShowConfirmation {
                            message: format!(
                                "Hooks not installed or outdated for '{}'. Install? (y/n/1-9 switch)",
                                name
                            ),
                            action: PendingAction::InstallHooks,
                        });
                    } else {
                        // Hooks already installed - show status
                        commands.push(Message::SetStatusMessage(Some(
                            format!("Hooks already installed for '{}'. To reload: /exit then 'claude --continue'", name)
                        )));
                    }
                }
            }

            Message::ShowConfirmation { message, action } => {
                self.model.ui_state.pending_confirmation = Some(PendingConfirmation {
                    message,
                    action,
                });
            }

            Message::ConfirmAction => {
                if let Some(confirmation) = self.model.ui_state.pending_confirmation.take() {
                    match confirmation.action {
                        PendingAction::InstallHooks => {
                            // Install hooks for the active project only
                            if let Some(project) = self.model.active_project_mut() {
                                let name = project.name.clone();
                                if let Err(e) = crate::hooks::install_hooks(&project.working_dir) {
                                    commands.push(Message::Error(format!(
                                        "Failed to install hooks for {}: {}",
                                        name, e
                                    )));
                                } else {
                                    project.hooks_installed = true;
                                    // After installing, ask if user wants to reload
                                    commands.push(Message::ShowConfirmation {
                                        message: format!(
                                            "Hooks installed for '{}'! Reload Claude now to activate? (y/n)\n\
                                             Manual: wait for Claude to be idle, then /exit and run 'claude --continue'",
                                            name
                                        ),
                                        action: PendingAction::ReloadClaude,
                                    });
                                }
                            }
                        }
                        PendingAction::ReloadClaude => {
                            // Hooks installed - show manual reload instructions
                            commands.push(Message::SetStatusMessage(Some(
                                "Hooks installed! To reload: /exit in Claude, then run 'claude --continue'".to_string()
                            )));
                        }
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
                    }
                }
            }

            Message::CancelAction => {
                if let Some(confirmation) = self.model.ui_state.pending_confirmation.take() {
                    // Show manual instructions when user cancels
                    match confirmation.action {
                        PendingAction::InstallHooks => {
                            commands.push(Message::SetStatusMessage(Some(
                                "Hooks not installed/outdated. Press Ctrl-R later to install.".to_string()
                            )));
                        }
                        PendingAction::ReloadClaude => {
                            commands.push(Message::SetStatusMessage(Some(
                                "Manual reload: /exit in Claude, then run 'claude --continue'".to_string()
                            )));
                        }
                        PendingAction::DeleteTask(_) => {
                            // Just clear the confirmation, no message needed
                        }
                        PendingAction::MarkDoneNoMerge(_) => {
                            // Just clear the confirmation, task stays in Review
                            commands.push(Message::SetStatusMessage(Some(
                                "Task left in Review.".to_string()
                            )));
                        }
                    }
                }
            }

            Message::SetStatusMessage(msg) => {
                self.model.ui_state.status_message = msg;
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
                        let was_waiting_for_cli = project.tasks[idx].session_mode == crate::model::SessionMode::WaitingForCliExit;
                        let project_name = project.name.clone();

                        let task = &mut project.tasks[idx];
                        found_task = true;

                        // Check if we're waiting for CLI to exit (SDK handoff case)
                        if was_waiting_for_cli && matches!(signal.event.as_str(), "stop" | "end") {
                            // CLI exited - resume SDK session
                            // Note: Don't overwrite claude_session_id here - the signal uses task_id,
                            // but we want to keep the real SDK session_id that was set when session started
                            commands.push(Message::CliSessionEnded { task_id });
                            break;
                        }

                        match signal.event.as_str() {
                            "stop" => {
                                if was_accepting {
                                    // Task was rebasing - try to complete the accept
                                    // Keep status as Accepting, CompleteAcceptTask will verify and update
                                    commands.push(Message::CompleteAcceptTask(task_id));
                                } else if has_queued {
                                    // Don't move to review - send the queued task instead
                                    task.status = TaskStatus::Review;
                                    task.session_state = crate::model::ClaudeSessionState::Paused;
                                    // Don't play attention sound - we're continuing automatically
                                    commands.push(Message::SendQueuedTask { finished_task_id: task_id });
                                } else {
                                    // Normal stop - move to review and notify
                                    task.status = TaskStatus::Review;
                                    task.session_state = crate::model::ClaudeSessionState::Paused;
                                    project.needs_attention = true;
                                    notify::play_attention_sound();
                                    notify::set_attention_indicator(&project_name);
                                }
                            }
                            "end" => {
                                // If session ends while Accepting, cancel the accept
                                if was_accepting {
                                    commands.push(Message::SetStatusMessage(Some(
                                        "Accept cancelled: Claude session ended during rebase.".to_string()
                                    )));
                                }
                                task.status = TaskStatus::Review;
                                task.session_state = crate::model::ClaudeSessionState::Ended;
                                project.needs_attention = true;
                                notify::play_attention_sound();
                                notify::set_attention_indicator(&project.name);
                            }
                            "needs-input" => {
                                // Don't change status if task is Accepting (mid-rebase)
                                // or already in Review (Stop hook already fired - this is
                                // likely idle_prompt firing after completion, not a real question)
                                if !was_accepting && task.status != TaskStatus::Review {
                                    task.status = TaskStatus::NeedsInput;
                                    task.session_state = crate::model::ClaudeSessionState::Paused;
                                    project.needs_attention = true;
                                    notify::play_attention_sound();
                                    notify::set_attention_indicator(&project.name);
                                }
                                // If already in Review, ignore - Stop hook already handled it
                            }
                            "input-provided" | "working" => {
                                // Don't change status if task is Accepting (mid-rebase)
                                if !was_accepting {
                                    task.status = TaskStatus::InProgress;
                                }
                                task.session_state = crate::model::ClaudeSessionState::Working;
                                project.needs_attention = false;
                                notify::clear_attention_indicator();
                            }
                            _ => {}
                        }
                        break;
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
                // Worktree created successfully, now set up Claude settings
                if let Err(e) = crate::worktree::merge_with_project_settings(
                    &worktree_path,
                    &project_dir,
                    task_id,
                ) {
                    commands.push(Message::SetStatusMessage(Some(
                        format!("Warning: Could not set up Claude settings: {}", e)
                    )));
                }

                // Pre-trust the worktree
                if let Err(e) = crate::worktree::pre_trust_worktree(&worktree_path) {
                    commands.push(Message::SetStatusMessage(Some(
                        format!("Warning: Could not pre-trust worktree: {}", e)
                    )));
                }

                // Update task with worktree info
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.worktree_path = Some(worktree_path.clone());
                        task.git_branch = Some(format!("claude/{}", task_id));
                        task.session_state = crate::model::ClaudeSessionState::Starting;
                    }
                }

                // Start SDK session
                commands.push(Message::StartSdkSession { task_id });
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
                    // Start SDK session via sidecar (headless - no tmux)
                    if let Some(ref client) = self.sidecar_client {
                        let images_str: Option<Vec<String>> = if !images.is_empty() {
                            Some(images.iter().map(|p| p.to_string_lossy().to_string()).collect())
                        } else {
                            None
                        };

                        match client.start_session(task_id, &worktree_path, &title, images_str) {
                            Ok(session_id) => {
                                // Update task with session ID and state
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.claude_session_id = Some(session_id);
                                        task.session_state = crate::model::ClaudeSessionState::Working;
                                        task.session_mode = SessionMode::SdkManaged;
                                    }
                                }
                                commands.push(Message::SetStatusMessage(Some(
                                    format!("Task started via SDK in worktree: {}", worktree_path.display())
                                )));
                            }
                            Err(e) => {
                                commands.push(Message::Error(format!("Failed to start SDK session: {}", e)));
                                // Clean up worktree since SDK failed
                                let _ = crate::worktree::remove_worktree(&project_dir, &worktree_path);
                                // Reset task state
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.session_state = crate::model::ClaudeSessionState::NotStarted;
                                        task.status = TaskStatus::Planned;
                                        task.worktree_path = None;
                                        task.git_branch = None;
                                    }
                                }
                            }
                        }
                    } else {
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
                    }
                }
            }

            Message::SidecarEvent(event) => {
                // Handle events from the SDK sidecar
                use crate::sidecar::SessionEventType;

                let task_id = event.task_id;

                // Track if this was an Accepting task that stopped/ended (for smart merge completion)
                let mut was_accepting = false;

                for project in &mut self.model.projects {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        // Update session_id if provided
                        if let Some(ref session_id) = event.session_id {
                            task.claude_session_id = Some(session_id.clone());
                        }

                        // Check if task was in Accepting status (smart merge rebase in progress)
                        was_accepting = task.status == TaskStatus::Accepting;

                        match event.event_type {
                            SessionEventType::Started => {
                                // Don't override Accepting status if this is a rebase session
                                if task.status != TaskStatus::Accepting {
                                    task.status = TaskStatus::InProgress; // Move from Queued to InProgress
                                }
                                task.session_state = crate::model::ClaudeSessionState::Working;
                                task.session_mode = crate::model::SessionMode::SdkManaged;
                            }
                            SessionEventType::Stopped | SessionEventType::Ended => {
                                // If task was accepting (rebase) OR is already done, don't change status
                                // Note: Both stopped and ended events may arrive - if stopped triggered
                                // CompleteAcceptTask which moved task to Done, we must not reset it to Review
                                if !was_accepting && task.status != TaskStatus::Done {
                                    task.status = TaskStatus::Review;
                                    task.session_state = if event.event_type == SessionEventType::Ended {
                                        crate::model::ClaudeSessionState::Ended
                                    } else {
                                        crate::model::ClaudeSessionState::Paused
                                    };
                                    project.needs_attention = true;
                                    notify::play_attention_sound();
                                    notify::set_attention_indicator(&project.name);
                                }
                            }
                            SessionEventType::NeedsInput => {
                                task.status = TaskStatus::NeedsInput;
                                task.session_state = crate::model::ClaudeSessionState::Paused;
                                project.needs_attention = true;
                                notify::play_attention_sound();
                                notify::set_attention_indicator(&project.name);
                            }
                            SessionEventType::Working | SessionEventType::ToolUse => {
                                // Don't override Accepting status if this is a rebase session
                                if task.status != TaskStatus::Accepting {
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
            }

            Message::SdkSessionStarted { task_id, session_id } => {
                // Update task with session ID from SDK
                if let Some(project) = self.model.active_project_mut() {
                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.claude_session_id = Some(session_id);
                        task.session_state = crate::model::ClaudeSessionState::Working;
                        task.session_mode = crate::model::SessionMode::SdkManaged;
                    }
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

            Message::OpenInteractiveModal(task_id) => {
                // Gather task info
                let task_info = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id).map(|task| {
                        (
                            task.worktree_path.clone(),
                            task.claude_session_id.clone(),
                            task.session_state.clone(),
                        )
                    })
                });

                if let Some((worktree_path, session_id, _session_state)) = task_info {
                    // Require worktree path
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

                    // Always try to resume if we have a session_id
                    // This shows conversation history even for completed sessions
                    let resume_session_id = session_id.as_deref();

                    // Open tmux popup with Claude
                    if let Err(e) = crate::tmux::open_popup(&worktree_path, resume_session_id) {
                        commands.push(Message::Error(format!(
                            "Failed to open interactive popup: {}", e
                        )));
                        return commands;
                    }

                    // Update session mode to CLI
                    if let Some(project) = self.model.active_project_mut() {
                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                            task.session_mode = crate::model::SessionMode::CliInteractive;
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
                // Get the session_id from the task first (immutable borrow)
                let session_id = self.model.active_project().and_then(|project| {
                    project.tasks.iter().find(|t| t.id == task_id)
                        .and_then(|task| task.claude_session_id.clone())
                });

                // Resume the SDK session via sidecar
                if let Some(session_id) = session_id {
                    if let Some(ref client) = self.sidecar_client {
                        match client.resume_session(task_id, &session_id, None) {
                            Ok(new_session_id) => {
                                // Update task with new session ID and mode
                                if let Some(project) = self.model.active_project_mut() {
                                    if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                                        task.claude_session_id = Some(new_session_id);
                                        task.session_mode = crate::model::SessionMode::SdkManaged;
                                        task.session_state = crate::model::ClaudeSessionState::Working;
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
                    // No session to resume
                    commands.push(Message::Error("Cannot resume: no session ID found".to_string()));
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

                // Check if we're editing a divider title
                if let Some(task_id) = self.model.ui_state.editing_divider_id {
                    // Empty input clears the title, non-empty sets it
                    let title = if input.is_empty() { None } else { Some(input) };
                    commands.push(Message::UpdateDividerTitle { task_id, title });
                } else if !input.is_empty() {
                    // Check if we're editing an existing task or creating a new one
                    if let Some(task_id) = self.model.ui_state.editing_task_id {
                        commands.push(Message::UpdateTask { task_id, title: input });
                    } else {
                        commands.push(Message::CreateTask(input));
                    }
                }
            }

            Message::FocusChanged(area) => {
                self.model.ui_state.focus = area;
            }

            Message::NavigateUp => {
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

                // Check if first task has divider_above
                let first_has_divider_above = self.model.active_project()
                    .and_then(|p| {
                        let tasks = p.tasks_by_status(self.model.ui_state.selected_column);
                        tasks.first().map(|t| t.divider_above)
                    })
                    .unwrap_or(false);

                if let Some(idx) = idx {
                    // If we're on divider_above, move to column above
                    if self.model.ui_state.selected_is_divider_above {
                        if let Some(status) = above_status {
                            self.save_scroll_offset();
                            self.model.ui_state.selected_column = status;
                            self.model.ui_state.selected_task_idx = if above_tasks_len > 0 {
                                Some(above_tasks_len - 1)
                            } else {
                                None
                            };
                            self.model.ui_state.selected_is_divider = false;
                            self.model.ui_state.selected_is_divider_above = false;
                            self.model.ui_state.title_scroll_offset = 0;
                            self.model.ui_state.title_scroll_delay = 0;
                        }
                    } else if self.model.ui_state.selected_is_divider {
                        // If we're on a divider below, move back to the task
                        self.model.ui_state.selected_is_divider = false;
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;
                    } else if idx == 0 && first_has_divider_above {
                        // At first task and there's a divider above - select it
                        self.model.ui_state.selected_is_divider_above = true;
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;
                    } else if idx > 0 {
                        // Check if previous task has a divider - if so, select it
                        let prev_has_divider = self.model.active_project()
                            .and_then(|p| {
                                let tasks = p.tasks_by_status(self.model.ui_state.selected_column);
                                tasks.get(idx - 1).map(|t| t.divider_below)
                            })
                            .unwrap_or(false);

                        if prev_has_divider {
                            self.model.ui_state.selected_task_idx = Some(idx - 1);
                            self.model.ui_state.selected_is_divider = true;
                        } else {
                            self.model.ui_state.selected_task_idx = Some(idx - 1);
                            self.model.ui_state.selected_is_divider = false;
                        }
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
                        self.model.ui_state.selected_is_divider = false;
                        self.model.ui_state.selected_is_divider_above = false;
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
                    self.model.ui_state.selected_is_divider = false;
                    self.model.ui_state.selected_is_divider_above = false;
                    self.model.ui_state.title_scroll_offset = 0;
                    self.model.ui_state.title_scroll_delay = 0;
                }
            }

            Message::NavigateDown => {
                // Gather info first to avoid borrow issues
                let (tasks_len, current_idx, current_has_divider, below_status, below_tasks_len, needs_sync) = {
                    if let Some(project) = self.model.active_project() {
                        let tasks = project.tasks_by_status(self.model.ui_state.selected_column);
                        let tasks_len = tasks.len();
                        // Check if index is out of bounds and needs syncing
                        let (idx, needs_sync) = match self.model.ui_state.selected_task_idx {
                            Some(i) if i >= tasks_len && tasks_len > 0 => (tasks_len - 1, true),
                            Some(i) => (i, false),
                            None => (0, false),
                        };
                        let has_divider = tasks.get(idx).map(|t| t.divider_below).unwrap_or(false);
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
                        (tasks_len, idx, has_divider, below, below_len, needs_sync)
                    } else {
                        (0, 0, false, None, 0, false)
                    }
                };

                // Sync selection if it was out of bounds
                if needs_sync {
                    self.model.ui_state.selected_task_idx = Some(current_idx);
                }

                // If on a divider_above, move to first task
                if self.model.ui_state.selected_is_divider_above {
                    self.model.ui_state.selected_is_divider_above = false;
                    self.model.ui_state.selected_task_idx = if tasks_len > 0 { Some(0) } else { None };
                    self.model.ui_state.title_scroll_offset = 0;
                    self.model.ui_state.title_scroll_delay = 0;
                // If on a divider_below, move to next task
                } else if self.model.ui_state.selected_is_divider {
                    if current_idx + 1 < tasks_len {
                        self.model.ui_state.selected_task_idx = Some(current_idx + 1);
                        self.model.ui_state.selected_is_divider = false;
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;
                    } else if let Some(status) = below_status {
                        // Move to column below
                        self.save_scroll_offset();
                        self.model.ui_state.selected_column = status;
                        self.model.ui_state.selected_task_idx = if below_tasks_len > 0 { Some(0) } else { None };
                        self.model.ui_state.selected_is_divider = false;
                        self.model.ui_state.selected_is_divider_above = false;
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;
                    } else {
                        // At bottom of Review/Done - focus task input
                        self.save_scroll_offset();
                        self.model.ui_state.focus = FocusArea::TaskInput;
                        self.model.ui_state.selected_is_divider = false;
                    }
                } else if self.model.ui_state.selected_task_idx.is_none() && tasks_len > 0 {
                    // No selection but column has tasks - select first
                    self.model.ui_state.selected_task_idx = Some(0);
                    self.model.ui_state.selected_is_divider = false;
                    self.model.ui_state.selected_is_divider_above = false;
                    self.model.ui_state.title_scroll_offset = 0;
                    self.model.ui_state.title_scroll_delay = 0;
                } else if self.model.ui_state.selected_task_idx.is_none() && tasks_len == 0 {
                    // Empty column - move to column below or focus task input
                    if let Some(status) = below_status {
                        self.save_scroll_offset();
                        self.model.ui_state.selected_column = status;
                        self.model.ui_state.selected_task_idx = if below_tasks_len > 0 { Some(0) } else { None };
                        self.model.ui_state.selected_is_divider = false;
                        self.model.ui_state.selected_is_divider_above = false;
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;
                    } else {
                        // At bottom row (Review/Done) - focus task input
                        self.save_scroll_offset();
                        self.model.ui_state.focus = FocusArea::TaskInput;
                    }
                } else if current_has_divider {
                    // Current task has a divider - select it
                    self.model.ui_state.selected_is_divider = true;
                    self.model.ui_state.title_scroll_offset = 0;
                    self.model.ui_state.title_scroll_delay = 0;
                } else if current_idx + 1 < tasks_len {
                    self.model.ui_state.selected_task_idx = Some(current_idx + 1);
                    self.model.ui_state.selected_is_divider = false;
                    self.model.ui_state.title_scroll_offset = 0;
                    self.model.ui_state.title_scroll_delay = 0;
                } else if let Some(status) = below_status {
                    // At bottom of column - move to column below
                    self.save_scroll_offset();
                    self.model.ui_state.selected_column = status;
                    self.model.ui_state.selected_task_idx = if below_tasks_len > 0 { Some(0) } else { None };
                    self.model.ui_state.selected_is_divider = false;
                    self.model.ui_state.selected_is_divider_above = false;
                    self.model.ui_state.title_scroll_offset = 0;
                    self.model.ui_state.title_scroll_delay = 0;
                } else {
                    // At bottom of Review/Done column - focus task input
                    self.save_scroll_offset();
                    self.model.ui_state.focus = FocusArea::TaskInput;
                }
            }

            Message::NavigateLeft => {
                // Linear navigation through all columns: Planned -> Queued -> InProgress -> NeedsInput -> Review -> Done
                let columns = TaskStatus::all();
                if let Some(idx) = columns.iter().position(|&s| s == self.model.ui_state.selected_column) {
                    if idx > 0 {
                        self.save_scroll_offset();
                        let new_status = columns[idx - 1];
                        self.model.ui_state.selected_column = new_status;
                        // Restore saved scroll position or select first task
                        self.model.ui_state.selected_task_idx = self.get_restored_task_idx(new_status);
                        self.model.ui_state.selected_is_divider = false;
                        self.model.ui_state.selected_is_divider_above = false;
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;
                    }
                }
            }

            Message::NavigateRight => {
                // Linear navigation through all columns: Planned -> Queued -> InProgress -> NeedsInput -> Review -> Done
                let columns = TaskStatus::all();
                if let Some(idx) = columns.iter().position(|&s| s == self.model.ui_state.selected_column) {
                    if idx + 1 < columns.len() {
                        self.save_scroll_offset();
                        let new_status = columns[idx + 1];
                        self.model.ui_state.selected_column = new_status;
                        // Restore saved scroll position or select first task
                        self.model.ui_state.selected_task_idx = self.get_restored_task_idx(new_status);
                        self.model.ui_state.selected_is_divider = false;
                        self.model.ui_state.selected_is_divider_above = false;
                        self.model.ui_state.title_scroll_offset = 0;
                        self.model.ui_state.title_scroll_delay = 0;
                    }
                }
            }

            Message::ToggleHelp => {
                self.model.ui_state.show_help = !self.model.ui_state.show_help;
            }

            Message::ToggleTaskPreview => {
                self.model.ui_state.show_task_preview = !self.model.ui_state.show_task_preview;
            }

            Message::Tick => {
                // Increment animation frame for spinners
                self.model.ui_state.animation_frame = self.model.ui_state.animation_frame.wrapping_add(1);

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
            }

            Message::Quit => {
                self.should_quit = true;
            }

            Message::QuitAndSwitchPane(_) => {
                // Legacy - just quit
                self.should_quit = true;
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
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kanclaude");

    let state_file = data_dir.join("state.json");

    if state_file.exists() {
        let content = std::fs::read_to_string(&state_file)?;
        let model: AppModel = serde_json::from_str(&content)?;
        Ok(model)
    } else {
        Ok(AppModel::default())
    }
}

/// Save application state to disk
pub fn save_state(model: &AppModel) -> Result<()> {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kanclaude");

    std::fs::create_dir_all(&data_dir)?;

    let state_file = data_dir.join("state.json");
    let content = serde_json::to_string_pretty(model)?;
    std::fs::write(state_file, content)?;

    Ok(())
}
