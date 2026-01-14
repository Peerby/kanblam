use crate::message::Message;
use crate::model::{AppModel, FocusArea, PendingAction, PendingConfirmation, Project, Task, TaskStatus};
use crate::notify;
use anyhow::Result;
use chrono::Utc;
use std::path::PathBuf;

/// Application state and update logic (TEA pattern)
pub struct App {
    pub model: AppModel,
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            model: AppModel::default(),
            should_quit: false,
        }
    }

    pub fn with_model(model: AppModel) -> Self {
        Self {
            model,
            should_quit: false,
        }
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

    /// Switch tmux client to the session containing the Claude session for active project
    pub fn switch_to_claude_session(&self) {
        if let Some(project) = self.model.active_project() {
            if let Some(pane_id) = project.active_tmux_session() {
                let _ = crate::tmux::switch_to_session(pane_id);
            }
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
            }

            Message::EditTask(task_id) => {
                // Find the task and get its title (clone to avoid borrow issues)
                let task_title = self.model.active_project()
                    .and_then(|p| p.tasks.iter().find(|t| t.id == task_id))
                    .map(|t| t.title.clone());

                if let Some(title) = task_title {
                    // Set editor content
                    self.model.ui_state.set_input_text(&title);
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
                    } else if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                        task.status = to_status;
                        match to_status {
                            TaskStatus::InProgress => task.started_at = Some(Utc::now()),
                            TaskStatus::Done => task.completed_at = Some(Utc::now()),
                            _ => {}
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
                if let Some(project) = self.model.active_project_mut() {
                    // Get task status first
                    let task_status = project.tasks.iter()
                        .find(|t| t.id == task_id)
                        .map(|t| t.status);

                    // Handle restarting tasks from Review or NeedsInput
                    if matches!(task_status, Some(TaskStatus::Review) | Some(TaskStatus::NeedsInput)) {
                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                            task.status = TaskStatus::InProgress;
                            // Clear attention state since we're actively working on it
                            project.needs_attention = false;
                            notify::clear_attention_indicator();
                            commands.push(Message::SetStatusMessage(Some(
                                "Task restarted".to_string()
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
                    } else if matches!(task_status, Some(TaskStatus::Planned) | Some(TaskStatus::Queued)) {
                        // No active task - start immediately
                        // Check if we have an active session, if not spawn one
                        if project.tmux_sessions.is_empty() {
                            // Spawn a new session for this project
                            let working_dir = project.working_dir.clone();
                            let project_name = project.name.clone();
                            match crate::tmux::spawn_claude_session(&working_dir, &project_name) {
                                Ok(pane_id) => {
                                    project.add_session(pane_id.clone());
                                    commands.push(Message::SetStatusMessage(Some(
                                        format!("Started new Claude session: {}", pane_id)
                                    )));
                                    // Note: We'll need to wait for Claude to initialize
                                    // For now, just record the session - user can retry starting the task
                                    return commands;
                                }
                                Err(e) => {
                                    commands.push(Message::Error(format!("Failed to spawn Claude session: {}", e)));
                                    return commands;
                                }
                            }
                        }

                        // Get pane_id first (before mutable borrow of tasks)
                        let pane_id = project.active_tmux_session().cloned();

                        if let Some(task) = project.tasks.iter_mut().find(|t| t.id == task_id) {
                            // Start Claude with the task using the active session
                            if let Some(pane_id) = pane_id {
                                match crate::tmux::start_claude_task(
                                    &pane_id,
                                    &task.title,
                                    &task.images,
                                ) {
                                    Ok(()) => {
                                        task.status = TaskStatus::InProgress;
                                        task.started_at = Some(Utc::now());
                                    }
                                    Err(e) => {
                                        commands.push(Message::Error(format!("Failed to start Claude: {}", e)));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Message::SelectTask(idx) => {
                self.model.ui_state.selected_task_idx = idx;
                self.model.ui_state.title_scroll_offset = 0;
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
            }

            Message::ClickedTask { status, task_idx } => {
                self.model.ui_state.selected_column = status;
                self.model.ui_state.selected_task_idx = Some(task_idx);
                self.model.ui_state.focus = FocusArea::KanbanBoard;
                self.model.ui_state.selected_is_divider = false;
                self.model.ui_state.selected_is_divider_above = false;
                self.model.ui_state.title_scroll_offset = 0;
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

            Message::RefreshProjects => {
                // First, migrate any legacy tmux_session fields
                for project in &mut self.model.projects {
                    project.migrate_legacy_session();
                }

                // Scan tmux sessions for Claude instances
                if let Ok(sessions) = crate::tmux::detect_claude_sessions() {
                    for session in sessions {
                        // Check if we already have this project
                        let existing = self.model.projects.iter_mut().find(|p| {
                            p.working_dir == session.working_dir
                        });

                        if let Some(project) = existing {
                            // Add the detected session to existing project
                            project.add_session(session.pane_id);
                            // Check if hooks are installed
                            project.hooks_installed = crate::hooks::hooks_installed(&project.working_dir);
                        } else {
                            // Create new project for this session
                            let name = session.working_dir
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| session.session_name.clone());

                            let mut project = Project::new(name, session.working_dir.clone());
                            project.add_session(session.pane_id);
                            project.hooks_installed = crate::hooks::hooks_installed(&session.working_dir);
                            self.model.projects.push(project);
                        }
                    }
                }
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
                        // Just ask to reload
                        commands.push(Message::ShowConfirmation {
                            message: format!(
                                "Reload Claude for '{}'? This will run /exit then 'claude --continue'. (y/n)\n\
                                 Manual: wait for Claude to be idle, then /exit and run 'claude --continue'",
                                name
                            ),
                            action: PendingAction::ReloadClaude,
                        });
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
                            // Reload the active project's Claude session
                            if let Some(project) = self.model.active_project() {
                                if let Some(pane_id) = project.active_tmux_session().cloned() {
                                    if let Err(e) = crate::tmux::reload_claude_session(&pane_id) {
                                        commands.push(Message::Error(format!("Failed to reload: {}", e)));
                                    } else {
                                        commands.push(Message::SetStatusMessage(Some(
                                            "Claude reloading... hooks will be active on restart.".to_string()
                                        )));
                                    }
                                }
                            }
                        }
                        PendingAction::DeleteTask(task_id) => {
                            // Actually delete the task
                            commands.push(Message::DeleteTask(task_id));
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
                    }
                }
            }

            Message::SetStatusMessage(msg) => {
                self.model.ui_state.status_message = msg;
            }

            Message::NextSession => {
                if let Some(project) = self.model.active_project_mut() {
                    project.next_session();
                }
            }

            Message::PrevSession => {
                if let Some(project) = self.model.active_project_mut() {
                    project.prev_session();
                }
            }

            Message::SpawnNewSession => {
                if let Some(project) = self.model.active_project_mut() {
                    let working_dir = project.working_dir.clone();
                    let project_name = project.name.clone();
                    match crate::tmux::spawn_claude_session(&working_dir, &project_name) {
                        Ok(pane_id) => {
                            project.add_session(pane_id.clone());
                            // Make the new session active
                            project.active_session_idx = project.tmux_sessions.len() - 1;
                            commands.push(Message::SetStatusMessage(Some(
                                format!("Started new Claude session: {}", pane_id)
                            )));
                        }
                        Err(e) => {
                            commands.push(Message::Error(format!("Failed to spawn Claude session: {}", e)));
                        }
                    }
                }
            }

            Message::HookSignalReceived(signal) => {
                // Find project by working directory (canonicalize for robust matching)
                let signal_dir = signal.project_dir.canonicalize().unwrap_or(signal.project_dir.clone());
                if let Some(project) = self.model.projects.iter_mut().find(|p| {
                    let project_dir = p.working_dir.canonicalize().unwrap_or(p.working_dir.clone());
                    project_dir == signal_dir
                }) {
                    match signal.event.as_str() {
                        "stop" => {
                            // Claude finished responding - move task to Review
                            // Match by InProgress status (we only allow one at a time per project)
                            if let Some(task) = project.tasks.iter_mut().find(|t| {
                                t.status == TaskStatus::InProgress
                            }) {
                                task.status = TaskStatus::Review;
                                task.claude_session_id = Some(signal.session_id.clone());
                                project.needs_attention = true;
                                // Play notification sound and set tmux indicator
                                notify::play_attention_sound();
                                notify::set_attention_indicator(&project.name);
                            }
                            // Check if there's a queued task to start next
                            if let Some(next_task) = project.next_queued_task() {
                                let next_task_id = next_task.id;
                                commands.push(Message::StartTask(next_task_id));
                            }
                        }
                        "end" => {
                            // Session ended - move any InProgress task to Review
                            if let Some(task) = project.tasks.iter_mut().find(|t| {
                                t.status == TaskStatus::InProgress
                            }) {
                                task.status = TaskStatus::Review;
                                task.claude_session_id = Some(signal.session_id.clone());
                                project.needs_attention = true;
                                // Play notification sound and set tmux indicator
                                notify::play_attention_sound();
                                notify::set_attention_indicator(&project.name);
                            }
                            // Check if there's a queued task to start next
                            if let Some(next_task) = project.next_queued_task() {
                                let next_task_id = next_task.id;
                                commands.push(Message::StartTask(next_task_id));
                            }
                        }
                        "needs-input" => {
                            // Claude needs user input - move task to NeedsInput
                            if let Some(task) = project.tasks.iter_mut().find(|t| {
                                t.status == TaskStatus::InProgress
                            }) {
                                task.status = TaskStatus::NeedsInput;
                                task.claude_session_id = Some(signal.session_id.clone());
                                project.needs_attention = true;
                                // Play notification sound and set tmux indicator
                                notify::play_attention_sound();
                                notify::set_attention_indicator(&project.name);
                            }
                        }
                        "input-provided" | "working" => {
                            // User provided input or Claude is working again - move task back to InProgress
                            if let Some(task) = project.tasks.iter_mut().find(|t| {
                                t.status == TaskStatus::NeedsInput
                            }) {
                                task.status = TaskStatus::InProgress;
                                // Clear attention since Claude is actively working
                                project.needs_attention = false;
                                notify::clear_attention_indicator();
                            }
                        }
                        _ => {}
                    }
                    // Sync selection after task status changes to keep cursor on same task
                    self.sync_selection();
                } else {
                    // No project matched - this could indicate a path mismatch
                    commands.push(Message::SetStatusMessage(Some(
                        format!("Hook '{}' received but no matching project for: {}",
                            signal.event,
                            signal.project_dir.display())
                    )));
                }
            }

            Message::ClaudeOutputUpdated { project_id, output } => {
                // Store captured output for display
                if let Some(project) = self.model.projects.iter_mut().find(|p| p.id == project_id) {
                    project.captured_output = output;
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
                        }
                    } else if self.model.ui_state.selected_is_divider {
                        // If we're on a divider below, move back to the task
                        self.model.ui_state.selected_is_divider = false;
                        self.model.ui_state.title_scroll_offset = 0;
                    } else if idx == 0 && first_has_divider_above {
                        // At first task and there's a divider above - select it
                        self.model.ui_state.selected_is_divider_above = true;
                        self.model.ui_state.title_scroll_offset = 0;
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
                // If on a divider_below, move to next task
                } else if self.model.ui_state.selected_is_divider {
                    if current_idx + 1 < tasks_len {
                        self.model.ui_state.selected_task_idx = Some(current_idx + 1);
                        self.model.ui_state.selected_is_divider = false;
                        self.model.ui_state.title_scroll_offset = 0;
                    } else if let Some(status) = below_status {
                        // Move to column below
                        self.save_scroll_offset();
                        self.model.ui_state.selected_column = status;
                        self.model.ui_state.selected_task_idx = if below_tasks_len > 0 { Some(0) } else { None };
                        self.model.ui_state.selected_is_divider = false;
                        self.model.ui_state.selected_is_divider_above = false;
                        self.model.ui_state.title_scroll_offset = 0;
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
                } else if self.model.ui_state.selected_task_idx.is_none() && tasks_len == 0 {
                    // Empty column - move to column below or focus task input
                    if let Some(status) = below_status {
                        self.save_scroll_offset();
                        self.model.ui_state.selected_column = status;
                        self.model.ui_state.selected_task_idx = if below_tasks_len > 0 { Some(0) } else { None };
                        self.model.ui_state.selected_is_divider = false;
                        self.model.ui_state.selected_is_divider_above = false;
                        self.model.ui_state.title_scroll_offset = 0;
                    } else {
                        // At bottom row (Review/Done) - focus task input
                        self.save_scroll_offset();
                        self.model.ui_state.focus = FocusArea::TaskInput;
                    }
                } else if current_has_divider {
                    // Current task has a divider - select it
                    self.model.ui_state.selected_is_divider = true;
                    self.model.ui_state.title_scroll_offset = 0;
                } else if current_idx + 1 < tasks_len {
                    self.model.ui_state.selected_task_idx = Some(current_idx + 1);
                    self.model.ui_state.selected_is_divider = false;
                    self.model.ui_state.title_scroll_offset = 0;
                } else if let Some(status) = below_status {
                    // At bottom of column - move to column below
                    self.save_scroll_offset();
                    self.model.ui_state.selected_column = status;
                    self.model.ui_state.selected_task_idx = if below_tasks_len > 0 { Some(0) } else { None };
                    self.model.ui_state.selected_is_divider = false;
                    self.model.ui_state.selected_is_divider_above = false;
                    self.model.ui_state.title_scroll_offset = 0;
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
                    }
                }
            }

            Message::ToggleHelp => {
                self.model.ui_state.show_help = !self.model.ui_state.show_help;
            }

            Message::Tick => {
                // Increment animation frame for spinners
                self.model.ui_state.animation_frame = self.model.ui_state.animation_frame.wrapping_add(1);

                // Animate scroll for long task titles (every tick = ~100ms)
                if let Some(task_idx) = self.model.ui_state.selected_task_idx {
                    if let Some(project) = self.model.active_project() {
                        let tasks = project.tasks_by_status(self.model.ui_state.selected_column);
                        if let Some(task) = tasks.get(task_idx) {
                            let title_len = task.title.chars().count();
                            // Only scroll if title is long (assume ~30 char display width)
                            if title_len > 25 {
                                self.model.ui_state.title_scroll_offset += 1;
                                // Wrap around with a pause at the start
                                if self.model.ui_state.title_scroll_offset > title_len + 5 {
                                    self.model.ui_state.title_scroll_offset = 0;
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
                // TODO: Display error to user
                eprintln!("Error: {}", err);
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
