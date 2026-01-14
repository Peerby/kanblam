mod app;
mod hooks;
mod image;
mod message;
mod model;
mod notify;
mod tmux;
mod ui;
mod worktree;

use app::{load_state, save_state, App};
use chrono::Utc;
use hooks::{HookWatcher, WatcherEvent};
use message::Message;
use model::{FocusArea, HookSignal, TaskStatus};
use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind, MouseButton},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    layout::Rect,
    Terminal,
};
use std::io;
use std::time::Duration;


fn main() -> anyhow::Result<()> {
    // Check for CLI subcommands (used by hooks)
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "hook-signal" {
        return handle_hook_signal(&args[2..]);
    }
    // New signal subcommand for worktree-based hooks: kanclaude signal <event> <task-id>
    if args.len() > 1 && args[1] == "signal" {
        return handle_signal_command(&args[2..]);
    }

    // Load saved state
    let model = load_state().unwrap_or_default();
    let mut app = App::with_model(model);

    // Create hook watcher for completion detection
    let hook_watcher = HookWatcher::new().ok();

    // Process any signals that arrived while app was not running
    if let Some(ref watcher) = hook_watcher {
        let pending_events = watcher.process_all_pending();
        for event in pending_events {
            if let Some(msg) = convert_watcher_event(event) {
                let commands = app.update(msg);
                // Process any commands generated
                for cmd in commands {
                    app.update(cmd);
                }
            }
        }
    }

    // Fallback: Check tmux windows for InProgress tasks that are actually idle
    // This catches cases where signals were lost or had wrong session IDs
    detect_idle_tasks_from_tmux(&mut app);

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run the main loop
    let result = run_app(&mut terminal, &mut app, hook_watcher);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    // Save state on exit
    if let Err(e) = save_state(&app.model) {
        eprintln!("Failed to save state: {}", e);
    }

    result
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    hook_watcher: Option<HookWatcher>,
) -> anyhow::Result<()>
where
    B::Error: Send + Sync + 'static,
{
    loop {
        // Render
        terminal.draw(|frame| ui::view(frame, app))?;

        // Check for hook events (completion detection)
        if let Some(ref watcher) = hook_watcher {
            while let Some(event) = watcher.poll() {
                if let Some(msg) = convert_watcher_event(event) {
                    let commands = app.update(msg);
                    for cmd in commands {
                        app.update(cmd);
                    }
                }
            }
        }

        // Handle events with timeout for tick
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    // Only handle Press events, ignore Release and Repeat
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }

                    // Handle input mode directly with textarea
                    if app.model.ui_state.focus == FocusArea::TaskInput {
                        let messages = handle_textarea_input(key, app);
                        for msg in messages {
                            let commands = app.update(msg);
                            for cmd in commands {
                                app.update(cmd);
                            }
                        }
                    } else {
                        let messages = handle_key_event(key, app);
                        for msg in messages {
                            let commands = app.update(msg);
                            for cmd in commands {
                                app.update(cmd);
                            }
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    let size = terminal.size()?;
                    let rect = Rect::new(0, 0, size.width, size.height);
                    if let Some(msg) = handle_mouse_event(mouse, app, rect) {
                        let commands = app.update(msg);
                        for cmd in commands {
                            app.update(cmd);
                        }
                    }
                }
                _ => {}
            }
        } else {
            // Tick for background updates
            app.update(Message::Tick);
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Handle mouse events - clicks on columns and tasks
fn handle_mouse_event(
    mouse: event::MouseEvent,
    app: &App,
    size: Rect,
) -> Option<Message> {
    // Only handle left clicks and taps
    if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        return None;
    }

    let x = mouse.column;
    let y = mouse.row;

    // Fixed input height (must match ui/mod.rs)
    let input_height = 5u16;

    // Calculate layout regions (project bar at top now)
    let project_bar_height = 1u16;
    let status_height = 1u16;
    let kanban_height = size.height.saturating_sub(project_bar_height + input_height + status_height);

    let project_bar_y = 0u16;
    let kanban_y = project_bar_height;
    let input_y = project_bar_height + kanban_height;
    let status_y = project_bar_height + kanban_height + input_height;

    // Check if click is in project bar
    if y < kanban_y {
        // Project tabs are roughly spaced out
        // Estimate project width based on name lengths
        let mut current_x = 1u16; // Initial space
        for (idx, project) in app.model.projects.iter().enumerate() {
            // Width: " [N] name " + separator
            let tab_width = if idx < 9 {
                (5 + project.name.len() + 2) as u16
            } else {
                (2 + project.name.len() + 2) as u16
            };

            if x >= current_x && x < current_x + tab_width {
                return Some(Message::SwitchProject(idx));
            }
            current_x += tab_width + 3; // + separator " │ "
        }
        return None;
    }

    // Check if click is in kanban area
    if y >= kanban_y && y < input_y {
        let kanban_rel_y = y - kanban_y;
        // Kanban board has outer border (1 char each side)
        // Inside is a 2x3 grid layout:
        //   Row 0: Planned (left)    | Queued (right)
        //   Row 1: InProgress (left) | NeedsInput (right)
        //   Row 2: Review (left)     | Done (right)

        let inner_x = x.saturating_sub(1); // Account for left border
        let inner_y = kanban_rel_y.saturating_sub(1); // Account for top border (use relative y)
        let inner_width = size.width.saturating_sub(2);
        let inner_height = kanban_height.saturating_sub(2);

        // Determine which cell (2x3 grid)
        let half_width = inner_width / 2;
        let row_height = inner_height / 3;

        let is_right = inner_x >= half_width;
        let row = if inner_y < row_height {
            0
        } else if inner_y < row_height * 2 {
            1
        } else {
            2
        };

        let status = match (row, is_right) {
            (0, false) => TaskStatus::Planned,     // Row 0, left
            (0, true) => TaskStatus::Queued,       // Row 0, right
            (1, false) => TaskStatus::InProgress,  // Row 1, left
            (1, true) => TaskStatus::NeedsInput,   // Row 1, right
            (2, false) => TaskStatus::Review,      // Row 2, left
            (_, _) => TaskStatus::Done,            // Row 2, right (catch-all)
        };

        // Calculate task index within the cell
        // Each cell has its own border (1 line) + title area
        let cell_y = inner_y.saturating_sub(row as u16 * row_height);

        // Account for column border and title (roughly 2 lines)
        if cell_y >= 2 {
            let task_idx = (cell_y - 2) as usize;

            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(status);
                if task_idx < tasks.len() {
                    return Some(Message::ClickedTask { status, task_idx });
                }
            }
        }

        // Click on column header or empty space - just select the column
        return Some(Message::SelectColumn(status));
    }

    // Check if click is in input area
    if y >= input_y && y < status_y {
        return Some(Message::FocusChanged(FocusArea::TaskInput));
    }

    // Click in status bar - could add session switching here in the future
    // For now, status bar shows session info but isn't clickable
    let _ = (project_bar_y, status_y); // Suppress unused variable warnings

    None
}

/// Convert a watcher event to a message
fn convert_watcher_event(event: WatcherEvent) -> Option<Message> {
    match event {
        WatcherEvent::ClaudeStopped { session_id, project_dir } => {
            Some(Message::HookSignalReceived(HookSignal {
                event: "stop".to_string(),
                session_id,
                project_dir,
                timestamp: Utc::now(),
                transcript_path: None,
            }))
        }
        WatcherEvent::SessionEnded { session_id, project_dir, .. } => {
            Some(Message::HookSignalReceived(HookSignal {
                event: "end".to_string(),
                session_id,
                project_dir,
                timestamp: Utc::now(),
                transcript_path: None,
            }))
        }
        WatcherEvent::NeedsInput { session_id, project_dir, .. } => {
            Some(Message::HookSignalReceived(HookSignal {
                event: "needs-input".to_string(),
                session_id,
                project_dir,
                timestamp: Utc::now(),
                transcript_path: None,
            }))
        }
        WatcherEvent::InputProvided { session_id, project_dir } => {
            Some(Message::HookSignalReceived(HookSignal {
                event: "input-provided".to_string(),
                session_id,
                project_dir,
                timestamp: Utc::now(),
                transcript_path: None,
            }))
        }
        WatcherEvent::Working { session_id, project_dir } => {
            Some(Message::HookSignalReceived(HookSignal {
                event: "working".to_string(),
                session_id,
                project_dir,
                timestamp: Utc::now(),
                transcript_path: None,
            }))
        }
        WatcherEvent::Error(e) => {
            eprintln!("Hook watcher error: {}", e);
            None
        }
    }
}

/// Handle editor input mode - passes events to edtui
fn handle_textarea_input(key: event::KeyEvent, app: &mut App) -> Vec<Message> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let super_key = key.modifiers.contains(KeyModifiers::SUPER);

    match key.code {
        // Enter: submit unless line ends with \ (line continuation)
        KeyCode::Enter if !ctrl && !alt => {
            let text = app.model.ui_state.get_input_text();
            if text.ends_with('\\') {
                // Remove the backslash and insert a newline
                use edtui::actions::{Execute, DeleteChar, LineBreak};
                DeleteChar(1).execute(&mut app.model.ui_state.editor_state);
                LineBreak(1).execute(&mut app.model.ui_state.editor_state);
                vec![]
            } else {
                vec![Message::InputSubmit]
            }
        }

        // Ctrl+D also submits the task (explicit submit)
        KeyCode::Char('d') if ctrl => {
            vec![Message::InputSubmit]
        }

        // Ctrl+C unfocuses editor, cancels edit if editing (keeps pending images)
        KeyCode::Char('c') if ctrl => {
            if app.model.ui_state.editing_task_id.is_some() {
                vec![Message::CancelEdit]
            } else {
                app.model.ui_state.clear_input();
                vec![Message::FocusChanged(FocusArea::KanbanBoard)]
            }
        }

        // Ctrl+V pastes image from clipboard
        // Also handle raw control character (ASCII 22) that some terminals send
        KeyCode::Char('v') if ctrl => {
            vec![Message::PasteImage]
        }
        KeyCode::Char('\x16') => {
            vec![Message::PasteImage]
        }

        // Ctrl+U clears all pending images
        KeyCode::Char('u') if ctrl => {
            let count = app.model.ui_state.pending_images.len();
            app.model.ui_state.pending_images.clear();
            if count > 0 {
                vec![Message::SetStatusMessage(Some(format!("Cleared {} image{}", count, if count == 1 { "" } else { "s" })))]
            } else {
                vec![Message::SetStatusMessage(Some("No images to clear".to_string()))]
            }
        }

        // Ctrl+X removes the last pending image
        KeyCode::Char('x') if ctrl => {
            if let Some(_) = app.model.ui_state.pending_images.pop() {
                let remaining = app.model.ui_state.pending_images.len();
                if remaining > 0 {
                    vec![Message::SetStatusMessage(Some(format!("{} image{} remaining", remaining, if remaining == 1 { "" } else { "s" })))]
                } else {
                    vec![Message::SetStatusMessage(Some("Image removed".to_string()))]
                }
            } else {
                vec![Message::SetStatusMessage(Some("No images to remove".to_string()))]
            }
        }

        // Up arrow at position 0 moves focus to Kanban board
        KeyCode::Up => {
            let cursor = app.model.ui_state.editor_state.cursor;
            if cursor.row == 0 && cursor.col == 0 {
                vec![Message::FocusChanged(FocusArea::KanbanBoard)]
            } else {
                // Let edtui handle normal cursor movement
                app.model.ui_state.editor_event_handler.on_key_event(
                    key,
                    &mut app.model.ui_state.editor_state,
                );
                vec![]
            }
        }

        // All other keys (including plain Enter for newlines) go to edtui editor
        _ => {
            app.model.ui_state.editor_event_handler.on_key_event(
                key,
                &mut app.model.ui_state.editor_state,
            );
            vec![]
        }
    }
}

fn handle_key_event(key: event::KeyEvent, app: &App) -> Vec<Message> {
    // Handle confirmation dialogs first
    if app.model.ui_state.pending_confirmation.is_some() {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => vec![Message::ConfirmAction],
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => vec![Message::CancelAction],
            // Allow 1-9 to cancel and switch to that project
            KeyCode::Char(c @ '1'..='9') => {
                let project_idx = (c as usize) - ('1' as usize);
                if project_idx < app.model.projects.len() {
                    vec![Message::CancelAction, Message::SwitchProject(project_idx)]
                } else {
                    vec![]
                }
            }
            _ => vec![],
        };
    }

    // Clear status message on any key press
    if app.model.ui_state.status_message.is_some() {
        return vec![Message::SetStatusMessage(None)];
    }

    // Handle help overlay
    if app.model.ui_state.show_help {
        return vec![Message::ToggleHelp];
    }

    // Handle task preview modal
    if app.model.ui_state.show_task_preview {
        return vec![Message::ToggleTaskPreview];
    }

    // Handle queue dialog if open
    if app.model.ui_state.is_queue_dialog_open() {
        return handle_queue_dialog_key(key, app);
    }

    // Normal mode keybindings
    match key.code {
        // Queue task / Quit
        // In Planned column with a task selected: open queue dialog
        // Otherwise: quit
        KeyCode::Char('q') => {
            if app.model.ui_state.selected_column == TaskStatus::Planned {
                if let Some(project) = app.model.active_project() {
                    // Check if there are running sessions to queue for
                    let running_sessions = project.tasks_with_active_sessions();
                    if !running_sessions.is_empty() {
                        // Get selected task
                        let tasks = project.tasks_by_status(TaskStatus::Planned);
                        if let Some(idx) = app.model.ui_state.selected_task_idx {
                            if let Some(task) = tasks.get(idx) {
                                return vec![Message::ShowQueueDialog(task.id)];
                            }
                        }
                    }
                }
            }
            // No running sessions or not in Planned - quit
            vec![Message::Quit]
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => vec![Message::Quit],

        // Help
        KeyCode::Char('?') => vec![Message::ToggleHelp],

        // Navigation
        KeyCode::Char('h') | KeyCode::Left => vec![Message::NavigateLeft],
        KeyCode::Char('l') | KeyCode::Right => vec![Message::NavigateRight],
        KeyCode::Char('j') | KeyCode::Down => vec![Message::NavigateDown],
        KeyCode::Char('k') | KeyCode::Up => vec![Message::NavigateUp],

        // Focus switching
        KeyCode::Tab => {
            let next_focus = match app.model.ui_state.focus {
                FocusArea::KanbanBoard => FocusArea::TaskInput,
                FocusArea::TaskInput => FocusArea::ProjectTabs,
                FocusArea::ProjectTabs => FocusArea::KanbanBoard,
                FocusArea::OutputViewer => FocusArea::KanbanBoard, // Legacy
            };
            vec![Message::FocusChanged(next_focus)]
        }

        // Switch to task's tmux window
        KeyCode::Char('o') => {
            // If a task with a tmux window is selected, switch to that task's window
            if let Some(project) = app.model.active_project() {
                let column = app.model.ui_state.selected_column;
                let tasks = project.tasks_by_status(column);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        if task.tmux_window.is_some() {
                            return vec![Message::SwitchToTaskWindow(task.id)];
                        }
                    }
                }
            }
            vec![]
        }

        // Open test shell in task's worktree
        KeyCode::Char('t') => {
            let column = app.model.ui_state.selected_column;
            // Only for tasks with worktrees (InProgress, Review, NeedsInput)
            if matches!(column, TaskStatus::InProgress | TaskStatus::Review | TaskStatus::NeedsInput) {
                if let Some(project) = app.model.active_project() {
                    let tasks = project.tasks_by_status(column);
                    if let Some(idx) = app.model.ui_state.selected_task_idx {
                        if let Some(task) = tasks.get(idx) {
                            if task.worktree_path.is_some() {
                                return vec![Message::OpenTestShell(task.id)];
                            }
                        }
                    }
                }
            }
            vec![]
        }

        // Apply task changes to main worktree (for testing)
        KeyCode::Char('a') => {
            let column = app.model.ui_state.selected_column;
            // Only in Review column for tasks with a git branch
            if column == TaskStatus::Review {
                if let Some(project) = app.model.active_project() {
                    let tasks = project.tasks_by_status(column);
                    if let Some(idx) = app.model.ui_state.selected_task_idx {
                        if let Some(task) = tasks.get(idx) {
                            // Check for git_branch (branch may exist even if worktree is gone)
                            if task.git_branch.is_some() {
                                return vec![Message::ApplyTaskChanges(task.id)];
                            }
                        }
                    }
                }
            }
            vec![]
        }

        // Unapply task changes from main worktree
        KeyCode::Char('u') => {
            if app.model.ui_state.applied_task_id.is_some() {
                return vec![Message::UnapplyTaskChanges];
            }
            vec![]
        }

        // Reload Claude hooks (Ctrl-R) - install hooks for active project
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            vec![Message::ReloadClaudeHooks]
        }

        // Enter input mode
        KeyCode::Char('i') => vec![Message::FocusChanged(FocusArea::TaskInput)],

        // Start/Continue task
        // In Planned/Queued: Start with worktree isolation
        // In Review: Continue the task
        // In InProgress: Switch to task window
        KeyCode::Enter => {
            if let Some(project) = app.model.active_project() {
                let column = app.model.ui_state.selected_column;
                let tasks = project.tasks_by_status(column);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        match column {
                            TaskStatus::Planned | TaskStatus::Queued => {
                                // Start with worktree isolation if it's a git repo
                                if project.is_git_repo() {
                                    return vec![Message::StartTaskWithWorktree(task.id)];
                                } else {
                                    // Fall back to legacy start (without worktree)
                                    return vec![Message::StartTask(task.id)];
                                }
                            }
                            TaskStatus::Review | TaskStatus::NeedsInput => {
                                // Continue the task (switch to tmux window)
                                if task.worktree_path.is_some() {
                                    return vec![Message::ContinueTask(task.id)];
                                } else {
                                    // Legacy: reset without worktree
                                    return vec![Message::StartTask(task.id)];
                                }
                            }
                            TaskStatus::InProgress => {
                                // Switch to task window
                                if task.tmux_window.is_some() {
                                    return vec![Message::SwitchToTaskWindow(task.id)];
                                }
                            }
                            TaskStatus::Accepting => {
                                // Task is being rebased - can't interact via Enter
                            }
                            TaskStatus::Done => {
                                // Can't do anything with done tasks via Enter
                            }
                        }
                    }
                }
            }
            vec![]
        }

        // Continue task from Review (alias for Enter in Review column)
        KeyCode::Char('c') => {
            if app.model.ui_state.selected_column == TaskStatus::Review {
                if let Some(project) = app.model.active_project() {
                    let tasks = project.tasks_by_status(TaskStatus::Review);
                    if let Some(idx) = app.model.ui_state.selected_task_idx {
                        if let Some(task) = tasks.get(idx) {
                            if task.worktree_path.is_some() {
                                return vec![Message::ContinueTask(task.id)];
                            } else {
                                return vec![Message::StartTask(task.id)];
                            }
                        }
                    }
                }
            }
            vec![]
        }

        // Accept task (merge and cleanup) - 'y' in Review column
        KeyCode::Char('y') => {
            if app.model.ui_state.selected_column == TaskStatus::Review {
                if let Some(project) = app.model.active_project() {
                    let tasks = project.tasks_by_status(TaskStatus::Review);
                    if let Some(idx) = app.model.ui_state.selected_task_idx {
                        if let Some(task) = tasks.get(idx) {
                            // Don't accept tasks that are already being accepted
                            if task.status == TaskStatus::Accepting {
                                return vec![];
                            }
                            if task.worktree_path.is_some() {
                                return vec![Message::SmartAcceptTask(task.id)];
                            } else {
                                // Legacy: just mark as done
                                return vec![Message::MoveTask {
                                    task_id: task.id,
                                    to_status: TaskStatus::Done,
                                }];
                            }
                        }
                    }
                }
            }
            vec![]
        }

        // Discard task (delete worktree without merging) - 'n' in Review column (without modifiers)
        KeyCode::Char('n') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.model.ui_state.selected_column == TaskStatus::Review {
                if let Some(project) = app.model.active_project() {
                    let tasks = project.tasks_by_status(TaskStatus::Review);
                    if let Some(idx) = app.model.ui_state.selected_task_idx {
                        if let Some(task) = tasks.get(idx) {
                            if task.worktree_path.is_some() {
                                return vec![Message::DiscardTask(task.id)];
                            } else {
                                // Legacy: move back to planned
                                return vec![Message::MoveTask {
                                    task_id: task.id,
                                    to_status: TaskStatus::Planned,
                                }];
                            }
                        }
                    }
                }
            }
            vec![]
        }

        // 'r' key: Reset task in Review/InProgress/NeedsInput, or move to Review from other columns
        KeyCode::Char('r') => {
            let column = app.model.ui_state.selected_column;
            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(column);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        // In Review, InProgress, or NeedsInput: reset the task (clean up and reset to top of Planned)
                        if matches!(column, TaskStatus::Review | TaskStatus::InProgress | TaskStatus::NeedsInput) {
                            return vec![Message::ResetTask(task.id)];
                        }
                        // In other columns: move to Review
                        if column != TaskStatus::Review {
                            return vec![Message::MoveTask {
                                task_id: task.id,
                                to_status: model::TaskStatus::Review,
                            }];
                        }
                    }
                }
            }
            vec![]
        }

        // Delete task or divider
        KeyCode::Char('d') => {
            // If a divider is selected, delete it (no confirmation needed)
            if app.model.ui_state.selected_is_divider {
                return vec![Message::DeleteDivider];
            }
            // Otherwise ask for confirmation before deleting the task
            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(app.model.ui_state.selected_column);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        let title = if task.title.len() > 30 {
                            format!("{}...", &task.title[..27])
                        } else {
                            task.title.clone()
                        };
                        return vec![Message::ShowConfirmation {
                            message: format!("Delete '{}'? (y/n)", title),
                            action: model::PendingAction::DeleteTask(task.id),
                        }];
                    }
                }
            }
            vec![]
        }

        // Edit task or divider
        KeyCode::Char('e') => {
            // If a divider is selected, edit the divider title
            if app.model.ui_state.selected_is_divider {
                return vec![Message::EditDivider];
            }
            // Otherwise edit the task
            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(app.model.ui_state.selected_column);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        return vec![Message::EditTask(task.id)];
                    }
                }
            }
            vec![]
        }

        // Move back to Planned (from Review or Queued)
        KeyCode::Char('p') => {
            let column = app.model.ui_state.selected_column;
            if column == model::TaskStatus::Review || column == model::TaskStatus::Queued {
                if let Some(project) = app.model.active_project() {
                    let tasks = project.tasks_by_status(column);
                    if let Some(idx) = app.model.ui_state.selected_task_idx {
                        if let Some(task) = tasks.get(idx) {
                            return vec![Message::MoveTask {
                                task_id: task.id,
                                to_status: model::TaskStatus::Planned,
                            }];
                        }
                    }
                }
            }
            vec![]
        }

        // Mark as Done
        KeyCode::Char('x') => {
            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(app.model.ui_state.selected_column);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        return vec![Message::MoveTask {
                            task_id: task.id,
                            to_status: model::TaskStatus::Done,
                        }];
                    }
                }
            }
            vec![]
        }

        // Move task up in list
        KeyCode::Char('+') | KeyCode::Char('=') => vec![Message::MoveTaskUp],

        // Move task down in list
        KeyCode::Char('-') | KeyCode::Char('_') => vec![Message::MoveTaskDown],

        // Toggle divider below task
        KeyCode::Char('|') => vec![Message::ToggleDivider],

        // Column switching with Shift+1-6 (or !, @, #, $, %, ^)
        // 2x3 grid: Row 1: Planned|Queued, Row 2: InProgress|NeedsInput, Row 3: Review|Done
        KeyCode::Char('!') => vec![Message::SelectColumn(model::TaskStatus::Planned)],
        KeyCode::Char('@') => vec![Message::SelectColumn(model::TaskStatus::Queued)],
        KeyCode::Char('#') => vec![Message::SelectColumn(model::TaskStatus::InProgress)],
        KeyCode::Char('$') => vec![Message::SelectColumn(model::TaskStatus::NeedsInput)],
        KeyCode::Char('%') => vec![Message::SelectColumn(model::TaskStatus::Review)],
        KeyCode::Char('^') => vec![Message::SelectColumn(model::TaskStatus::Done)],

        // Project switching (1-9)
        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
            let idx = c.to_digit(10).unwrap() as usize - 1;
            vec![Message::SwitchProject(idx)]
        }

        // Paste image (Ctrl+V)
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            vec![Message::PasteImage]
        }

        // View task details (v without modifiers, or Space)
        KeyCode::Char('v') => {
            // Only show preview if a task is selected (not a divider)
            if app.model.ui_state.selected_task_idx.is_some()
                && !app.model.ui_state.selected_is_divider
                && !app.model.ui_state.selected_is_divider_above
            {
                vec![Message::ToggleTaskPreview]
            } else {
                vec![]
            }
        }
        KeyCode::Char(' ') => {
            // Only show preview if a task is selected (not a divider)
            if app.model.ui_state.selected_task_idx.is_some()
                && !app.model.ui_state.selected_is_divider
                && !app.model.ui_state.selected_is_divider_above
            {
                vec![Message::ToggleTaskPreview]
            } else {
                vec![]
            }
        }

        _ => vec![],
    }
}

/// Handle key events when the queue dialog is open
fn handle_queue_dialog_key(key: event::KeyEvent, app: &App) -> Vec<Message> {
    match key.code {
        // Close dialog
        KeyCode::Esc | KeyCode::Char('q') => {
            vec![Message::CloseQueueDialog]
        }

        // Navigate up
        KeyCode::Up | KeyCode::Char('k') => {
            vec![Message::QueueDialogNavigate(-1)]
        }

        // Navigate down
        KeyCode::Down | KeyCode::Char('j') => {
            vec![Message::QueueDialogNavigate(1)]
        }

        // Confirm selection
        KeyCode::Enter => {
            vec![Message::QueueDialogConfirm]
        }

        _ => vec![],
    }
}

/// Handle the hook-signal subcommand (called by Claude Code hooks)
fn handle_hook_signal(args: &[String]) -> anyhow::Result<()> {
    use std::io::Read;

    // Parse arguments
    let mut event = String::new();
    let mut input_type: Option<String> = None;
    for arg in args {
        if let Some(value) = arg.strip_prefix("--event=") {
            event = value.to_string();
        } else if let Some(value) = arg.strip_prefix("--type=") {
            input_type = Some(value.to_string());
        }
    }

    if event.is_empty() {
        return Err(anyhow::anyhow!("Missing --event argument"));
    }

    // Read hook input from stdin (JSON from Claude Code)
    let mut stdin_content = String::new();
    std::io::stdin().read_to_string(&mut stdin_content)?;

    // Parse the hook input
    let hook_input: serde_json::Value = serde_json::from_str(&stdin_content)
        .unwrap_or_else(|_| serde_json::json!({}));

    let session_id = hook_input
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let cwd = hook_input
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Write signal file for the watcher
    hooks::write_signal(&event, &session_id, &cwd, input_type.as_deref())?;

    Ok(())
}

/// Handle the signal subcommand for worktree-based hooks
/// Format: kanclaude signal <event> <task-id>
fn handle_signal_command(args: &[String]) -> anyhow::Result<()> {
    if args.len() < 2 {
        return Err(anyhow::anyhow!("Usage: kanclaude signal <event> <task-id>"));
    }

    let event = &args[0];
    let task_id = &args[1];

    // Get current working directory (the worktree)
    let cwd = std::env::current_dir().unwrap_or_default();

    // Write signal file with task_id as the session identifier
    // The watcher will pick this up and process it
    hooks::write_signal(event, task_id, &cwd, None)?;

    Ok(())
}

/// Detect InProgress tasks whose Claude sessions are actually idle (waiting for input)
/// This is a fallback for when signals are lost or have wrong session IDs
fn detect_idle_tasks_from_tmux(app: &mut App) {
    use std::process::Command;

    for project in &mut app.model.projects {
        let project_slug = project.slug();

        for task in &mut project.tasks {
            // Only check InProgress tasks with tmux windows
            if task.status != model::TaskStatus::InProgress {
                continue;
            }
            let Some(ref window_name) = task.tmux_window else {
                continue;
            };

            // Check if window exists
            if !tmux::task_window_exists(&project_slug, window_name) {
                continue;
            }

            // Capture the last 15 lines of the pane
            let target = format!("kc-{}:{}", project_slug, window_name);
            let output = Command::new("tmux")
                .args(["capture-pane", "-t", &target, "-p", "-S", "-15"])
                .output();

            if let Ok(output) = output {
                if output.status.success() {
                    let content = String::from_utf8_lossy(&output.stdout);

                    // Check for Claude's prompt indicators (idle state)
                    let is_idle = content.lines().rev().take(5).any(|line| {
                        let trimmed = line.trim();
                        // Claude's prompt character is ❯ (U+276F)
                        // Also check for > as fallback
                        (trimmed.starts_with('❯') || trimmed.starts_with('>'))
                            && !trimmed.contains("...")  // Skip loading indicators
                    });

                    if is_idle {
                        // Claude is waiting for input - move to Review
                        task.status = model::TaskStatus::Review;
                        task.session_state = model::ClaudeSessionState::Paused;
                    }
                }
            }
        }
    }
}
