mod app;
mod hooks;
mod image;
mod message;
mod model;
mod notify;
mod sidecar;
mod tmux;
mod ui;
mod worktree;

use app::{load_state, save_state, App};
use chrono::Utc;
use hooks::{HookWatcher, WatcherEvent};
use message::Message;
use model::{EnterResult, FocusArea, HookSignal, TaskStatus};
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
use tokio::sync::mpsc;

/// Process commands recursively to ensure nested commands are also handled.
/// For example, CompleteAcceptTask may return ShowConfirmation which must be processed.
fn process_commands_recursively(app: &mut App, commands: Vec<Message>) {
    let mut pending = commands;
    while let Some(cmd) = pending.pop() {
        let more = app.update(cmd);
        pending.extend(more);
    }
}

/// Channel for receiving results from async background tasks
type AsyncResultReceiver = mpsc::UnboundedReceiver<Message>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    // Start sidecar and connect (keep handle to kill on exit)
    let _sidecar_child = match sidecar::ensure_sidecar_running() {
        Ok(child) => child, // Store handle to keep process alive
        Err(_) => None,
    };
    let sidecar_client = sidecar::SidecarClient::connect().ok();

    // Create event receiver for sidecar notifications
    let sidecar_receiver = sidecar::SidecarEventReceiver::connect().ok();

    // Create async task channel for background operations
    let (async_sender, async_receiver) = mpsc::unbounded_channel::<Message>();

    let mut app = App::with_model(model)
        .with_sidecar(sidecar_client)
        .with_async_sender(async_sender);

    // Create hook watcher for completion detection
    let mut hook_watcher = HookWatcher::new().ok();

    // Process any signals that arrived while app was not running
    // Signals are sorted chronologically and replayed in order
    if let Some(ref mut watcher) = hook_watcher {
        let pending_events = watcher.process_all_pending();
        for event in pending_events {
            if let Some(msg) = convert_watcher_event(event) {
                let commands = app.update(msg);
                process_commands_recursively(&mut app, commands);
            }
        }
    }

    // Fallback: Check tmux windows for InProgress tasks that are actually idle
    // This catches cases where signals were lost or had wrong session IDs
    detect_idle_tasks_from_tmux(&mut app);

    // Initial git status refresh for all tasks with worktrees
    let commands = app.update(Message::RefreshGitStatus);
    process_commands_recursively(&mut app, commands);

    // Initial git fetch to get remote status (ahead/behind indicators)
    let commands = app.update(Message::StartGitFetch);
    process_commands_recursively(&mut app, commands);

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?; // Clear screen to remove any cargo-watch output artifacts

    // Run the main loop
    let result = run_app(&mut terminal, &mut app, hook_watcher, sidecar_receiver, async_receiver);

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

fn run_app<B: ratatui::backend::Backend + std::io::Write>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mut hook_watcher: Option<HookWatcher>,
    mut sidecar_receiver: Option<sidecar::SidecarEventReceiver>,
    mut async_receiver: AsyncResultReceiver,
) -> anyhow::Result<()>
where
    B::Error: Send + Sync + 'static,
{
    // Deferred commands are processed after the next render for responsive UI
    let mut deferred_commands: std::collections::VecDeque<Message> = std::collections::VecDeque::new();

    loop {
        // Render first for responsive UI
        terminal.draw(|frame| ui::view(frame, app))?;

        // Process ONE deferred command per iteration (after render)
        // This ensures the UI stays responsive during multi-step operations
        if let Some(cmd) = deferred_commands.pop_front() {
            let more_commands = app.update(cmd);
            // Add new commands back to the queue for subsequent iterations
            for c in more_commands {
                deferred_commands.push_back(c);
            }
        }

        // Poll async task results (non-blocking)
        // These come from background operations like worktree creation and sidecar calls
        while let Ok(msg) = async_receiver.try_recv() {
            let commands = app.update(msg);
            for cmd in commands {
                deferred_commands.push_back(cmd);
            }
        }

        // Check for hook events (completion detection)
        if let Some(ref mut watcher) = hook_watcher {
            while let Some(event) = watcher.poll() {
                if let Some(msg) = convert_watcher_event(event) {
                    let commands = app.update(msg);
                    // Process commands recursively to handle nested commands
                    process_commands_recursively(app, commands);
                }
            }
        }

        // Poll sidecar events (SDK session notifications)
        if let Some(ref mut receiver) = sidecar_receiver {
            // Poll multiple times to catch queued events
            for _ in 0..10 {
                match receiver.try_recv(Duration::from_millis(1)) {
                    Ok(Some(event)) => {
                        let msg = Message::SidecarEvent(event);
                        let commands = app.update(msg);
                        // Process commands recursively to handle nested commands
                        // (e.g., CompleteAcceptTask returning ShowConfirmation)
                        process_commands_recursively(app, commands);
                    }
                    Ok(None) => break, // No more events
                    Err(_) => break,   // Error, stop polling
                }
            }
        }

        // Handle events with timeout for tick
        // Use shorter timeout when modal is open for responsive rendering
        let poll_timeout = if app.model.ui_state.interactive_modal.is_some() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(100)
        };

        if event::poll(poll_timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    // Only handle Press events, ignore Release and Repeat
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }

                    // Track consecutive ESC presses for showing help hints
                    // ESC increments counter, any other key resets it
                    if key.code == KeyCode::Esc {
                        app.model.ui_state.consecutive_esc_count =
                            app.model.ui_state.consecutive_esc_count.saturating_add(1);
                    } else {
                        app.model.ui_state.consecutive_esc_count = 0;
                    }

                    // Check if interactive modal is active
                    if app.model.ui_state.interactive_modal.is_some() {
                        let messages = handle_interactive_modal_input(key, app);
                        for msg in messages {
                            let commands = app.update(msg);
                            process_commands_recursively(app, commands);
                        }
                    } else if app.model.ui_state.is_open_project_dialog_open() {
                        // Handle open project dialog input directly
                        let messages = handle_open_project_dialog_input(key, app);
                        for msg in messages {
                            let commands = app.update(msg);
                            process_commands_recursively(app, commands);
                        }
                    } else if app.model.ui_state.focus == FocusArea::TaskInput {
                        // Handle input mode directly with textarea
                        let messages = handle_textarea_input(key, app);
                        for msg in messages {
                            // Handle external editor specially - needs terminal access
                            if matches!(msg, Message::OpenExternalEditor) {
                                if let Some(result) = open_external_editor(terminal, app) {
                                    let commands = app.update(Message::ExternalEditorFinished(result));
                                    process_commands_recursively(app, commands);
                                }
                            } else {
                                let commands = app.update(msg);
                                process_commands_recursively(app, commands);
                            }
                        }
                    } else {
                        let messages = handle_key_event(key, app);
                        for msg in messages {
                            let commands = app.update(msg);
                            // Defer commands to next iteration for responsive UI
                            deferred_commands.extend(commands);
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    // Ignore mouse events when modal is open
                    if app.model.ui_state.interactive_modal.is_some() {
                        continue;
                    }
                    let size = terminal.size()?;
                    let rect = Rect::new(0, 0, size.width, size.height);
                    if let Some(msg) = handle_mouse_event(mouse, app, rect) {
                        let commands = app.update(msg);
                        process_commands_recursively(app, commands);
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

        if app.should_restart {
            // Save state before restart
            if let Err(e) = save_state(&app.model) {
                eprintln!("Warning: Failed to save state before restart: {}", e);
            }

            // Restore terminal before restart
            disable_raw_mode()?;
            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
            terminal.show_cursor()?;

            // Restart the app using exec
            handle_restart()?;
        }
    }

    Ok(())
}

/// Handle hot restart by exec-ing the same binary
fn handle_restart() -> anyhow::Result<()> {
    let current_exe = std::env::current_exe()?;
    let args: Vec<String> = std::env::args().collect();

    // Use exec to replace current process with new instance
    let err = exec::Command::new(&current_exe)
        .args(&args[1..])
        .exec();

    // exec() only returns if it fails
    Err(anyhow::anyhow!("Failed to restart: {}", err))
}

/// Open the current input text in the configured external editor, returning the edited text.
/// Suspends the terminal, runs the editor on a temp file, then resumes.
/// Returns Some(text) if user saved and exited, None if user cancelled.
fn open_external_editor<B: ratatui::backend::Backend + std::io::Write>(
    terminal: &mut Terminal<B>,
    app: &App,
) -> Option<String> {
    use std::fs;
    use std::process::Command;

    // Get current input text
    let current_text = app.model.ui_state.get_input_text();

    // Create temp file with current content
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join(format!("kanclaude_input_{}.txt", std::process::id()));

    // Write current content to temp file
    if let Err(e) = fs::write(&temp_file, &current_text) {
        eprintln!("Failed to create temp file: {}", e);
        return None;
    }

    // Suspend terminal - leave alternate screen and disable raw mode
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();

    // Run the configured editor from settings
    let editor_cmd = app.model.global_settings.default_editor.command();
    // Split command in case it has arguments (e.g., "code --wait")
    let parts: Vec<&str> = editor_cmd.split_whitespace().collect();
    let status = if parts.len() > 1 {
        Command::new(parts[0])
            .args(&parts[1..])
            .arg(&temp_file)
            .status()
    } else {
        Command::new(editor_cmd)
            .arg(&temp_file)
            .status()
    };

    // Resume terminal - re-enter alternate screen and enable raw mode
    let _ = enable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    );
    let _ = terminal.hide_cursor();
    // Force a full redraw
    let _ = terminal.clear();

    // Check if vim succeeded and read result
    match status {
        Ok(exit_status) if exit_status.success() => {
            // Read the edited content
            match fs::read_to_string(&temp_file) {
                Ok(content) => {
                    let _ = fs::remove_file(&temp_file);
                    Some(content)
                }
                Err(_) => {
                    let _ = fs::remove_file(&temp_file);
                    None
                }
            }
        }
        _ => {
            // User cancelled or editor failed
            let _ = fs::remove_file(&temp_file);
            None
        }
    }
}

/// Handle keyboard input when the interactive modal is active
/// Ctrl-Esc closes the modal, PageUp/PageDown scroll, other keys are forwarded to tmux
fn handle_interactive_modal_input(key: event::KeyEvent, app: &mut App) -> Vec<Message> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Ctrl-Esc or Ctrl-q: close the modal and return to app
    if ctrl && (key.code == KeyCode::Esc || key.code == KeyCode::Char('q')) {
        return vec![Message::CloseInteractiveModal];
    }

    // PageUp/PageDown: scroll the modal view (don't forward to tmux)
    match key.code {
        KeyCode::PageUp => {
            if let Some(ref mut modal) = app.model.ui_state.interactive_modal {
                modal.scroll_offset = modal.scroll_offset.saturating_sub(10);
            }
            return vec![];
        }
        KeyCode::PageDown => {
            if let Some(ref mut modal) = app.model.ui_state.interactive_modal {
                modal.scroll_offset = modal.scroll_offset.saturating_add(10);
            }
            return vec![];
        }
        _ => {}
    }

    // Get the tmux target from the modal
    let Some(ref modal) = app.model.ui_state.interactive_modal else {
        return vec![];
    };

    // Forward the key to tmux
    let key_sequence = key_event_to_tmux_sequence(key);
    if !key_sequence.is_empty() {
        if let Err(_e) = tmux::send_key_to_pane(&modal.tmux_target, &key_sequence) {
            // Window is gone - close the modal automatically
            return vec![Message::CloseInteractiveModal];
        }
    }

    vec![]
}

/// Convert a crossterm KeyEvent to a tmux send-keys sequence
fn key_event_to_tmux_sequence(key: event::KeyEvent) -> String {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    match key.code {
        // Special keys
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Backspace => "BSpace".to_string(),
        KeyCode::Esc => "Escape".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::Delete => "DC".to_string(),
        KeyCode::Insert => "IC".to_string(),

        // Function keys
        KeyCode::F(n) => format!("F{}", n),

        // Character keys with modifiers
        KeyCode::Char(c) => {
            if ctrl {
                // Ctrl+key: send as C-<key>
                format!("C-{}", c)
            } else if alt {
                // Alt+key: send as M-<key>
                format!("M-{}", c)
            } else {
                // Plain character - may need escaping for tmux
                match c {
                    ';' => "\\;".to_string(),
                    ' ' => "Space".to_string(),
                    _ => c.to_string(),
                }
            }
        }

        // Unhandled keys
        _ => String::new(),
    }
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
    // Header height is dynamic based on terminal size (must match ui/mod.rs)
    let show_full_logo = crate::ui::logo::should_show_full_logo(size.width, size.height);
    let header_height = if show_full_logo { 4u16 } else { 1u16 };
    let status_height = 1u16;
    let kanban_height = size.height.saturating_sub(header_height + input_height + status_height);

    let header_y = 0u16;
    let kanban_y = header_height;
    let input_y = header_height + kanban_height;
    let status_y = header_height + kanban_height + input_height;

    // Check if click is in header area (project bar + logo)
    if y < kanban_y {
        // Check if click is on the logo (right side, when full logo is shown)
        if show_full_logo {
            let logo_start_x = size.width.saturating_sub(crate::ui::logo::FULL_LOGO_WIDTH);
            if x >= logo_start_x && y < 4 {
                // Click on the mascot/logo - trigger blink animation
                return Some(Message::TriggerMascotBlink);
            }
        }

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
        //   Row 0: Planned (left)    | InProgress (right)
        //   Row 1: Testing (left)    | NeedsInput (right)
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

        // 2x3 grid: Row1 = Planned|InProgress, Row2 = Testing|NeedsInput, Row3 = Review|Done
        let status = match (row, is_right) {
            (0, false) => TaskStatus::Planned,     // Row 0, left
            (0, true) => TaskStatus::InProgress,   // Row 0, right
            (1, false) => TaskStatus::Testing,     // Row 1, left
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
    let _ = (header_y, status_y); // Suppress unused variable warnings

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
                input_type: String::new(),
            }))
        }
        WatcherEvent::SessionEnded { session_id, project_dir, .. } => {
            Some(Message::HookSignalReceived(HookSignal {
                event: "end".to_string(),
                session_id,
                project_dir,
                timestamp: Utc::now(),
                transcript_path: None,
                input_type: String::new(),
            }))
        }
        WatcherEvent::NeedsInput { session_id, project_dir, input_type } => {
            Some(Message::HookSignalReceived(HookSignal {
                event: "needs-input".to_string(),
                session_id,
                project_dir,
                timestamp: Utc::now(),
                transcript_path: None,
                input_type,
            }))
        }
        WatcherEvent::InputProvided { session_id, project_dir } => {
            Some(Message::HookSignalReceived(HookSignal {
                event: "input-provided".to_string(),
                session_id,
                project_dir,
                timestamp: Utc::now(),
                transcript_path: None,
                input_type: String::new(),
            }))
        }
        WatcherEvent::Working { session_id, project_dir } => {
            Some(Message::HookSignalReceived(HookSignal {
                event: "working".to_string(),
                session_id,
                project_dir,
                timestamp: Utc::now(),
                transcript_path: None,
                input_type: String::new(),
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

    let _super_key = key.modifiers.contains(KeyModifiers::SUPER);

    match key.code {
        // Ctrl+S: Submit and immediately start the task
        KeyCode::Char('s') if ctrl => {
            vec![Message::InputSubmitAndStart]
        }

        // Enter behavior depends on editor mode:
        // - Normal mode: submit (or line continuation if ends with \)
        // - Insert mode: insert newline (handled by edtui)
        // - Search mode: go to first search result (handled by edtui)
        KeyCode::Enter if !ctrl && !alt => {
            use edtui::EditorMode;
            match app.model.ui_state.editor_state.mode {
                EditorMode::Normal => {
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
                EditorMode::Insert | EditorMode::Search | EditorMode::Visual => {
                    // Let edtui handle: LineBreak in Insert, FindFirst in Search
                    app.model.ui_state.editor_event_handler.on_key_event(
                        key,
                        &mut app.model.ui_state.editor_state,
                    );
                    vec![]
                }
            }
        }

        // Ctrl+D also submits the task (explicit submit)
        KeyCode::Char('d') if ctrl => {
            vec![Message::InputSubmit]
        }

        // Escape: always pass to edtui for vim mode switching (Insert -> Normal)
        // Never unfocuses - use Ctrl+C or Up at position 0 to exit
        KeyCode::Esc => {
            app.model.ui_state.editor_event_handler.on_key_event(
                key,
                &mut app.model.ui_state.editor_state,
            );
            vec![]
        }

        // Ctrl+C unfocuses editor, cancels edit/feedback if active (keeps content for new tasks)
        KeyCode::Char('c') if ctrl => {
            if app.model.ui_state.feedback_task_id.is_some() {
                vec![Message::CancelFeedbackMode]
            } else if app.model.ui_state.editing_task_id.is_some() {
                vec![Message::CancelEdit]
            } else {
                // Just unfocus, keep the content
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

        // Ctrl+G opens input in external editor (vim)
        KeyCode::Char('g') if ctrl => {
            vec![Message::OpenExternalEditor]
        }

        // Up arrow at position 0 moves focus to Kanban board (keeps content)
        KeyCode::Up => {
            let cursor = app.model.ui_state.editor_state.cursor;
            if cursor.row == 0 && cursor.col == 0 {
                // Just unfocus, keep the content
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
    // Handle confirmation dialogs first - ignore all other input except expected keys
    if let Some(ref confirmation) = app.model.ui_state.pending_confirmation {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => vec![Message::ConfirmAction],
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => vec![Message::CancelAction],
            // 'i' key for interrupt - same as ConfirmAction for feedback dialogs
            KeyCode::Char('i') | KeyCode::Char('I') => {
                match &confirmation.action {
                    model::PendingAction::InterruptSdkForFeedback { .. } |
                    model::PendingAction::InterruptCliForFeedback { .. } => {
                        vec![Message::ConfirmAction]
                    }
                    _ => vec![Message::RestartConfirmationAnimation],
                }
            }
            // 'w' key for wait (queue) - store feedback to send when Claude finishes
            KeyCode::Char('w') | KeyCode::Char('W') => {
                match &confirmation.action {
                    model::PendingAction::InterruptSdkForFeedback { task_id, feedback } |
                    model::PendingAction::InterruptCliForFeedback { task_id, feedback } => {
                        vec![Message::QueueFeedback { task_id: *task_id, feedback: feedback.clone() }]
                    }
                    _ => vec![Message::RestartConfirmationAnimation],
                }
            }
            // 'o' key for open CLI - available for both SDK and CLI feedback dialogs
            KeyCode::Char('o') | KeyCode::Char('O') => {
                match &confirmation.action {
                    model::PendingAction::InterruptSdkForFeedback { task_id, .. } |
                    model::PendingAction::InterruptCliForFeedback { task_id, .. } => {
                        // Cancel the feedback and open CLI instead
                        vec![Message::CancelAction, Message::OpenInteractiveModal(*task_id)]
                    }
                    _ => vec![Message::RestartConfirmationAnimation],
                }
            }
            // 'K' key for keep markers - available for StashConflict dialogs
            // (lowercase 'k' handled below with scroll, but also checks for StashConflict)
            KeyCode::Char('K') => {
                match &confirmation.action {
                    model::PendingAction::StashConflict { task_id, .. } => {
                        // Keep conflict markers for manual resolution
                        vec![Message::KeepStashConflictMarkers(*task_id)]
                    }
                    _ => vec![Message::RestartConfirmationAnimation],
                }
            }
            // 's' key for stash changes - available for StashConflict and DirtyMainBeforeMerge dialogs
            KeyCode::Char('s') | KeyCode::Char('S') => {
                match &confirmation.action {
                    model::PendingAction::StashConflict { task_id, .. } => {
                        // Stash user's changes and apply task cleanly
                        vec![Message::StashUserChangesAndApply(*task_id)]
                    }
                    model::PendingAction::DirtyMainBeforeMerge { task_id } => {
                        // Stash changes before merge, then proceed
                        vec![Message::StashThenMerge { task_id: *task_id }]
                    }
                    _ => vec![Message::RestartConfirmationAnimation],
                }
            }
            // 'c' key for commit changes - available for DirtyMainBeforeMerge dialogs
            KeyCode::Char('c') | KeyCode::Char('C') => {
                match &confirmation.action {
                    model::PendingAction::DirtyMainBeforeMerge { .. } => {
                        // Commit changes, then proceed with merge (handled via ConfirmAction)
                        vec![Message::ConfirmAction]
                    }
                    _ => vec![Message::RestartConfirmationAnimation],
                }
            }
            // Allow 1-9 to cancel and switch to that project
            KeyCode::Char(c @ '1'..='9') => {
                let project_idx = (c as usize) - ('1' as usize);
                if project_idx < app.model.projects.len() {
                    vec![Message::CancelAction, Message::SwitchProject(project_idx)]
                } else {
                    // Invalid project number - restart animation to signal prompt is active
                    vec![Message::RestartConfirmationAnimation]
                }
            }
            // Scroll keys for multiline confirmation modals (e.g., conflict details)
            KeyCode::Char('j') | KeyCode::Down => {
                if confirmation.message.contains('\n') {
                    vec![Message::ScrollConfirmationDown]
                } else {
                    vec![Message::RestartConfirmationAnimation]
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                // 'k' is used for "keep markers" in StashConflict, so check action first
                match &confirmation.action {
                    model::PendingAction::StashConflict { task_id, .. } => {
                        vec![Message::KeepStashConflictMarkers(*task_id)]
                    }
                    _ if confirmation.message.contains('\n') => {
                        vec![Message::ScrollConfirmationUp]
                    }
                    _ => vec![Message::RestartConfirmationAnimation],
                }
            }
            // Any other key: restart the highlight animation to signal the prompt is active
            _ => vec![Message::RestartConfirmationAnimation],
        };
    }

    // Note: Status messages are cleared via tick, not by consuming keypresses

    // Handle help overlay - scroll keys navigate, others close
    if app.model.ui_state.show_help {
        return handle_help_modal_key(key);
    }

    // Handle stash modal if open
    if app.model.ui_state.show_stash_modal {
        return handle_stash_modal_key(key);
    }

    // Handle task preview modal - allow action keys to work, only close on Esc/Enter/Space/?
    if app.model.ui_state.show_task_preview {
        return handle_task_preview_modal_key(key, app);
    }

    // Handle queue dialog if open
    if app.model.ui_state.is_queue_dialog_open() {
        return handle_queue_dialog_key(key, app);
    }

    // Handle config modal if open
    if app.model.ui_state.is_config_modal_open() {
        return handle_config_modal_key(key, app);
    }

    // Normal mode keybindings
    match key.code {
        // Quit
        KeyCode::Char('q') => vec![Message::Quit],
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => vec![Message::Quit],

        // Close current project (Ctrl+D)
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if !app.model.projects.is_empty() {
                vec![Message::CloseProject(app.model.active_project_idx)]
            } else {
                vec![]
            }
        }

        // Help
        KeyCode::Char('?') => vec![Message::ToggleHelp],

        // Settings/Config (Ctrl-,)
        KeyCode::Char(',') if key.modifiers.contains(KeyModifiers::CONTROL) => vec![Message::ShowConfigModal],

        // Quick Claude CLI pane (Ctrl-T)
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            vec![Message::OpenClaudeCliPane]
        }

        // Git remote operations
        // P = Pull from remote (uppercase)
        KeyCode::Char('P') => vec![Message::StartGitPull],
        // p = Push to remote (lowercase)
        KeyCode::Char('p') => vec![Message::StartGitPush],

        // Stash management
        // S = Toggle stash modal (uppercase)
        KeyCode::Char('S') => vec![Message::ToggleStashModal],

        // Welcome screen speech bubble navigation
        KeyCode::Char('j') | KeyCode::Down if app.model.projects.is_empty() && !app.model.ui_state.welcome_bubble_focused => {
            // Focus the speech bubble
            vec![Message::WelcomeBubbleFocus]
        }
        KeyCode::Char('k') | KeyCode::Up if app.model.projects.is_empty() && app.model.ui_state.welcome_bubble_focused => {
            // Unfocus the speech bubble
            vec![Message::WelcomeBubbleUnfocus]
        }
        KeyCode::Char('h') | KeyCode::Left if app.model.projects.is_empty() && app.model.ui_state.welcome_bubble_focused => {
            // Previous message
            vec![Message::WelcomeMessagePrev]
        }
        KeyCode::Char('l') | KeyCode::Right if app.model.projects.is_empty() && app.model.ui_state.welcome_bubble_focused => {
            // Next message
            vec![Message::WelcomeMessageNext]
        }

        // Navigation
        KeyCode::Char('h') | KeyCode::Left => vec![Message::NavigateLeft],
        KeyCode::Char('l') | KeyCode::Right => vec![Message::NavigateRight],
        KeyCode::Char('j') | KeyCode::Down => vec![Message::NavigateDown],
        KeyCode::Char('k') | KeyCode::Up => vec![Message::NavigateUp],
        KeyCode::Home | KeyCode::Char('g') => vec![Message::NavigateToStart],
        KeyCode::End | KeyCode::Char('G') => vec![Message::NavigateToEnd],

        // Enter on welcome screen (no projects) opens project dialog
        KeyCode::Enter if app.model.projects.is_empty() => {
            vec![Message::ShowOpenProjectDialog { slot: 0 }]
        }

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

        // Open combined tmux session (Claude on left, shell on right)
        KeyCode::Char('o') => {
            let column = app.model.ui_state.selected_column;
            // Only for tasks with worktrees (InProgress, Review, NeedsInput)
            if matches!(column, TaskStatus::InProgress | TaskStatus::Review | TaskStatus::NeedsInput) {
                if let Some(project) = app.model.active_project() {
                    let tasks = project.tasks_by_status(column);
                    if let Some(idx) = app.model.ui_state.selected_task_idx {
                        if let Some(task) = tasks.get(idx) {
                            if task.worktree_path.is_some() {
                                return vec![Message::OpenInteractiveModal(task.id)];
                            }
                        }
                    }
                }
            }
            vec![]
        }

        // Open combined session in detached mode (Shift-O)
        KeyCode::Char('O') => {
            let column = app.model.ui_state.selected_column;
            if matches!(column, TaskStatus::InProgress | TaskStatus::Review | TaskStatus::NeedsInput) {
                if let Some(project) = app.model.active_project() {
                    let tasks = project.tasks_by_status(column);
                    if let Some(idx) = app.model.ui_state.selected_task_idx {
                        if let Some(task) = tasks.get(idx) {
                            if task.worktree_path.is_some() {
                                return vec![Message::OpenInteractiveDetached(task.id)];
                            }
                        }
                    }
                }
            }
            vec![]
        }

        // Apply task changes to main worktree for testing - 'a' in Review column
        KeyCode::Char('a') => {
            let column = app.model.ui_state.selected_column;
            if column == TaskStatus::Review {
                if let Some(project) = app.model.active_project() {
                    let tasks = project.tasks_by_status(column);
                    if let Some(idx) = app.model.ui_state.selected_task_idx {
                        if let Some(task) = tasks.get(idx) {
                            // Don't apply if task is already being merged
                            if task.status == TaskStatus::Accepting {
                                return vec![];
                            }
                            return vec![Message::SmartApplyTask(task.id)];
                        }
                    }
                }
            }
            vec![]
        }

        // Merge task (finalize changes and mark done) - 'm' in Review column
        // If changes are applied, commit them; otherwise do full merge
        KeyCode::Char('m') => {
            let column = app.model.ui_state.selected_column;
            if column == TaskStatus::Review {
                // Check if there are applied changes for the selected task
                let applied_task_id = app.model.active_project()
                    .and_then(|p| p.applied_task_id);

                if let Some(project) = app.model.active_project() {
                    let tasks = project.tasks_by_status(column);
                    if let Some(idx) = app.model.ui_state.selected_task_idx {
                        if let Some(task) = tasks.get(idx) {
                            // Don't merge tasks that are already being merged
                            if task.status == TaskStatus::Accepting {
                                return vec![];
                            }

                            // If this task's changes are currently applied, commit them
                            if applied_task_id == Some(task.id) {
                                return vec![Message::ShowConfirmation {
                                    message: "Commit applied changes and mark done? (y/n)".to_string(),
                                    action: model::PendingAction::CommitAppliedChanges(task.id),
                                }];
                            }

                            // Otherwise do full merge
                            return vec![Message::ShowConfirmation {
                                message: "Merge all changes and mark done? (y/n)".to_string(),
                                action: model::PendingAction::AcceptTask(task.id),
                            }];
                        }
                    }
                }
            }
            vec![]
        }

        // Merge only (keep worktree and task in Review) - Shift+M in Review column
        KeyCode::Char('M') if app.model.ui_state.selected_column == TaskStatus::Review => {
            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(TaskStatus::Review);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        // Don't merge tasks that are already being merged
                        if task.status == TaskStatus::Accepting {
                            return vec![];
                        }

                        return vec![Message::ShowConfirmation {
                            message: "Merge changes to main? (keeps worktree) (y/n)".to_string(),
                            action: model::PendingAction::MergeOnlyTask(task.id),
                        }];
                    }
                }
            }
            vec![]
        }

        // Unapply task changes (remove applied changes from main worktree)
        KeyCode::Char('u') => {
            // If there's an applied task, unapply it
            let has_applied = app.model.active_project()
                .map(|p| p.applied_task_id.is_some())
                .unwrap_or(false);
            if has_applied {
                return vec![Message::UnapplyTaskChanges];
            }
            vec![]
        }

        // Rebase selected task's worktree to latest main (Review only)
        KeyCode::Char('r') if app.model.ui_state.selected_column == TaskStatus::Review => {
            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(TaskStatus::Review);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        if task.worktree_path.is_some() {
                            return vec![Message::UpdateWorktreeToMain(task.id)];
                        }
                    }
                }
            }
            vec![]
        }

        // Enter input mode
        KeyCode::Char('i') => vec![Message::FocusChanged(FocusArea::TaskInput)],

        // View task details (Enter opens task preview modal) or select project tab
        KeyCode::Enter => {
            // Handle ProjectTabs selection
            if app.model.ui_state.focus == FocusArea::ProjectTabs {
                let selected_idx = app.model.ui_state.selected_project_tab_idx;
                if selected_idx == 0 {
                    // 0 = +project button - open the dialog
                    // Find the next available slot (for consistency with existing behavior)
                    let num_projects = app.model.projects.len();
                    if num_projects < 9 {
                        return vec![Message::ShowOpenProjectDialog { slot: num_projects }];
                    }
                    return vec![];
                } else {
                    // 1+ = actual projects (idx 1 = project 0, etc.)
                    let project_idx = selected_idx - 1;
                    if project_idx < app.model.projects.len() {
                        // Return to kanban board after switching
                        return vec![
                            Message::SwitchProject(project_idx),
                            Message::FocusChanged(FocusArea::KanbanBoard),
                        ];
                    }
                    return vec![];
                }
            }

            // Only show preview if a task is selected
            if app.model.ui_state.selected_task_idx.is_some() {
                vec![Message::ToggleTaskPreview]
            } else {
                vec![]
            }
        }

        // Start task - only available in Planned phase
        KeyCode::Char('s') if app.model.ui_state.selected_column == TaskStatus::Planned => {
            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(TaskStatus::Planned);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        // Start with worktree isolation if it's a git repo
                        if project.is_git_repo() {
                            return vec![Message::StartTaskWithWorktree(task.id)];
                        } else {
                            // Fall back to legacy start (without worktree)
                            return vec![Message::StartTask(task.id)];
                        }
                    }
                }
            }
            vec![]
        }

        // Decline task (discard changes and mark done) - 'd' in Review column
        KeyCode::Char('d') if app.model.ui_state.selected_column == TaskStatus::Review => {
            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(TaskStatus::Review);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        // Don't process tasks that are already being accepted
                        if task.status == TaskStatus::Accepting {
                            return vec![];
                        }
                        return vec![Message::ShowConfirmation {
                            message: "Discard all changes and mark done? (y/n)".to_string(),
                            action: model::PendingAction::DeclineTask(task.id),
                        }];
                    }
                }
            }
            vec![]
        }

        // Send feedback to a task in Review or InProgress - 'f' key
        KeyCode::Char('f') if matches!(
            app.model.ui_state.selected_column,
            TaskStatus::Review | TaskStatus::InProgress
        ) => {
            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(app.model.ui_state.selected_column);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        // Don't allow feedback on tasks being accepted
                        if task.status == TaskStatus::Accepting {
                            return vec![];
                        }
                        return vec![Message::EnterFeedbackMode(task.id)];
                    }
                }
            }
            vec![]
        }

        // Check if already merged (cleanup if merged externally) - 'c' in Review column
        KeyCode::Char('c') if app.model.ui_state.selected_column == TaskStatus::Review => {
            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(TaskStatus::Review);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        // Don't process tasks that are already being accepted
                        if task.status == TaskStatus::Accepting {
                            return vec![];
                        }
                        return vec![Message::CheckAlreadyMerged(task.id)];
                    }
                }
            }
            vec![]
        }

        // 'r' key: Move to Review (from InProgress, NeedsInput, Done)
        KeyCode::Char('r') => {
            let column = app.model.ui_state.selected_column;
            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(column);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        // Move to Review from InProgress, NeedsInput, or Done
                        if matches!(column, TaskStatus::InProgress | TaskStatus::NeedsInput | TaskStatus::Done) {
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

        // Delete task
        KeyCode::Char('d') => {
            // Ask for confirmation before deleting the task
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

        // Edit task
        KeyCode::Char('e') => {
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

        // 'x' key: Reset task (cleanup worktree and move to Planned)
        KeyCode::Char('x') => {
            let column = app.model.ui_state.selected_column;
            if let Some(project) = app.model.active_project() {
                let tasks = project.tasks_by_status(column);
                if let Some(idx) = app.model.ui_state.selected_task_idx {
                    if let Some(task) = tasks.get(idx) {
                        // Reset works on InProgress, NeedsInput, Review, Done
                        if matches!(column, TaskStatus::InProgress | TaskStatus::NeedsInput | TaskStatus::Review | TaskStatus::Done) {
                            return vec![Message::ShowConfirmation {
                                message: format!("Reset '{}'? This will clean up worktree and move to Planned. (y/n)", task.title),
                                action: model::PendingAction::ResetTask(task.id),
                            }];
                        }
                    }
                }
            }
            vec![]
        }

        // Move task up in list
        KeyCode::Char('+') | KeyCode::Char('=') => vec![Message::MoveTaskUp],

        // Move task down in list
        KeyCode::Char('-') | KeyCode::Char('_') => vec![Message::MoveTaskDown],

        // Column switching with 1-6
        // 2x3 grid: Row 1: Planned|InProgress, Row 2: Testing|NeedsInput, Row 3: Review|Done
        KeyCode::Char('1') => vec![Message::SelectColumn(model::TaskStatus::Planned)],
        KeyCode::Char('2') => vec![Message::SelectColumn(model::TaskStatus::InProgress)],
        KeyCode::Char('3') => vec![Message::SelectColumn(model::TaskStatus::Testing)],
        KeyCode::Char('4') => vec![Message::SelectColumn(model::TaskStatus::NeedsInput)],
        KeyCode::Char('5') => vec![Message::SelectColumn(model::TaskStatus::Review)],
        KeyCode::Char('6') => vec![Message::SelectColumn(model::TaskStatus::Done)],

        // Project switching (Shift+1-9: !@#$%^&*() )
        // ! = open new project dialog, @=project 0, #=project 1, etc.
        KeyCode::Char('!') => {
            // Open new project dialog (if under 9 projects)
            let num_projects = app.model.projects.len();
            if num_projects < 9 {
                vec![Message::ShowOpenProjectDialog { slot: num_projects }]
            } else {
                vec![]
            }
        }
        KeyCode::Char(c) if "@#$%^&*(".contains(c) => {
            let shift_chars = ['@', '#', '$', '%', '^', '&', '*', '('];
            let idx = shift_chars.iter().position(|&ch| ch == c).unwrap();
            if idx < app.model.projects.len() {
                // Switch to existing project
                vec![Message::SwitchProject(idx)]
            } else {
                // Project doesn't exist at this slot
                vec![]
            }
        }

        // Paste image (Ctrl+V)
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            vec![Message::PasteImage]
        }

        // View task details (Space)
        KeyCode::Char(' ') => {
            // Only show preview if a task is selected
            if app.model.ui_state.selected_task_idx.is_some() {
                vec![Message::ToggleTaskPreview]
            } else {
                vec![]
            }
        }

        // ESC pressed multiple times shows the startup help bar
        KeyCode::Esc => {
            // If welcome bubble is focused, unfocus it first
            if app.model.projects.is_empty() && app.model.ui_state.welcome_bubble_focused {
                return vec![Message::WelcomeBubbleUnfocus];
            }
            // Track consecutive ESC presses - when count reaches 2, show hints
            let current_count = app.model.ui_state.consecutive_esc_count;
            if current_count >= 1 {
                // Second+ ESC - show the hints
                vec![Message::ShowStartupHints]
            } else {
                // First ESC - just increment the counter (handled by the match below)
                vec![]
            }
        }

        _ => vec![],
    }
}

/// Handle key events when the queue dialog is open
fn handle_queue_dialog_key(key: event::KeyEvent, _app: &App) -> Vec<Message> {
    match key.code {
        // Close dialog
        KeyCode::Esc => {
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

        // Jump to start
        KeyCode::Home | KeyCode::Char('g') => {
            vec![Message::QueueDialogNavigateToStart]
        }

        // Jump to end
        KeyCode::End | KeyCode::Char('G') => {
            vec![Message::QueueDialogNavigateToEnd]
        }

        // Confirm selection
        KeyCode::Enter => {
            vec![Message::QueueDialogConfirm]
        }

        _ => vec![],
    }
}

/// Handle key events when the config modal is open
fn handle_config_modal_key(key: event::KeyEvent, app: &App) -> Vec<Message> {
    let Some(ref config) = app.model.ui_state.config_modal else {
        return vec![Message::CloseConfigModal];
    };

    if config.editing {
        // Editing mode: capture text input or handle special keys
        if config.selected_field == model::ConfigField::DefaultEditor {
            // Editor field: arrow keys and h/l cycle through options
            match key.code {
                KeyCode::Esc => vec![Message::ConfigCancelEdit],
                KeyCode::Enter => vec![Message::ConfigConfirmEdit],
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('H') => {
                    vec![Message::ConfigEditFieldPrev]
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Char(' ') => {
                    vec![Message::ConfigEditField]
                }
                _ => vec![],
            }
        } else {
            // Command fields: text input
            match key.code {
                KeyCode::Esc => vec![Message::ConfigCancelEdit],
                KeyCode::Enter => vec![Message::ConfigConfirmEdit],
                KeyCode::Backspace => {
                    let mut new_buf = config.edit_buffer.clone();
                    new_buf.pop();
                    vec![Message::ConfigUpdateBuffer(new_buf)]
                }
                KeyCode::Char(c) => {
                    let mut new_buf = config.edit_buffer.clone();
                    new_buf.push(c);
                    vec![Message::ConfigUpdateBuffer(new_buf)]
                }
                _ => vec![],
            }
        }
    } else {
        // Navigation mode
        match key.code {
            // Save and close modal
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s') => {
                vec![Message::ConfigSave]
            }

            // Navigate up
            KeyCode::Up | KeyCode::Char('k') => {
                vec![Message::ConfigNavigateUp]
            }

            // Navigate down
            KeyCode::Down | KeyCode::Char('j') => {
                vec![Message::ConfigNavigateDown]
            }

            // Enter edit mode
            KeyCode::Enter | KeyCode::Char('l') => {
                vec![Message::ConfigEditField]
            }

            // Reset to defaults
            KeyCode::Char('r') => {
                vec![Message::ConfigResetToDefaults]
            }

            _ => vec![],
        }
    }
}

/// Handle key events when the help modal is open
/// j/k/Up/Down scroll by 1 line, PageUp/PageDown scroll by 10 lines
/// Any other key closes the modal
fn handle_help_modal_key(key: event::KeyEvent) -> Vec<Message> {
    match key.code {
        // Scroll down
        KeyCode::Char('j') | KeyCode::Down => vec![Message::ScrollHelpDown(1)],
        // Scroll up
        KeyCode::Char('k') | KeyCode::Up => vec![Message::ScrollHelpUp(1)],
        // Page down
        KeyCode::PageDown => vec![Message::ScrollHelpDown(10)],
        // Page up
        KeyCode::PageUp => vec![Message::ScrollHelpUp(10)],
        // Any other key closes the modal
        _ => vec![Message::ToggleHelp],
    }
}

/// Handle key events when the stash modal is open
/// j/k/Up/Down navigate, p pops the selected stash, d deletes with confirmation
/// Esc or S closes the modal
fn handle_stash_modal_key(key: event::KeyEvent) -> Vec<Message> {
    match key.code {
        // Close modal
        KeyCode::Esc | KeyCode::Char('S') | KeyCode::Char('q') => {
            vec![Message::ToggleStashModal]
        }

        // Navigate up
        KeyCode::Char('k') | KeyCode::Up => {
            vec![Message::StashModalNavigate(-1)]
        }

        // Navigate down
        KeyCode::Char('j') | KeyCode::Down => {
            vec![Message::StashModalNavigate(1)]
        }

        // Pop selected stash
        KeyCode::Char('p') | KeyCode::Enter => {
            vec![Message::PopSelectedStash]
        }

        // Delete selected stash (with confirmation)
        KeyCode::Char('d') => {
            vec![Message::DropSelectedStash]
        }

        _ => vec![],
    }
}

/// Handle key events when the task preview modal is open
/// Actions work directly from within the modal, closing it first
fn handle_task_preview_modal_key(key: event::KeyEvent, app: &App) -> Vec<Message> {
    // Get the selected task for context-aware action handling
    let task = app.model.active_project().and_then(|project| {
        let tasks = project.tasks_by_status(app.model.ui_state.selected_column);
        app.model.ui_state.selected_task_idx.and_then(|idx| tasks.get(idx).copied())
    });

    let Some(task) = task else {
        return vec![Message::ToggleTaskPreview];
    };

    // Check if we're on the git tab for scroll handling
    let on_git_tab = app.model.ui_state.task_detail_tab == crate::model::TaskDetailTab::Git;

    match key.code {
        // Close modal only on Esc, Enter, Space
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ') => {
            vec![Message::ToggleTaskPreview]
        }

        // Tab navigation: left/right/h/l (but not h/l on git tab - those are for scrolling)
        KeyCode::Left => {
            vec![Message::TaskDetailPrevTab]
        }
        KeyCode::Right => {
            vec![Message::TaskDetailNextTab]
        }
        KeyCode::Char('h') => {
            if on_git_tab {
                vec![] // Reserved for future horizontal scroll
            } else {
                vec![Message::TaskDetailPrevTab]
            }
        }
        KeyCode::Char('l') => {
            if on_git_tab {
                vec![] // Reserved for future horizontal scroll
            } else {
                vec![Message::TaskDetailNextTab]
            }
        }

        // Scroll git diff (j/k on git tab, or arrow keys)
        KeyCode::Char('j') | KeyCode::Down => {
            if on_git_tab {
                vec![Message::ScrollGitDiffDown(1)]
            } else {
                vec![]
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if on_git_tab {
                vec![Message::ScrollGitDiffUp(1)]
            } else {
                vec![]
            }
        }
        KeyCode::PageDown => {
            if on_git_tab {
                vec![Message::ScrollGitDiffDown(20)]
            } else {
                vec![]
            }
        }
        KeyCode::PageUp => {
            if on_git_tab {
                vec![Message::ScrollGitDiffUp(20)]
            } else {
                vec![]
            }
        }
        KeyCode::Home | KeyCode::Char('g') => {
            if on_git_tab {
                // Scroll to top by subtracting a large number
                vec![Message::ScrollGitDiffUp(100000)]
            } else {
                vec![]
            }
        }
        KeyCode::End | KeyCode::Char('G') => {
            if on_git_tab {
                // Scroll to bottom by adding a large number (will be capped)
                vec![Message::ScrollGitDiffDown(100000)]
            } else {
                vec![]
            }
        }

        // Open full help (closes modal, opens help)
        KeyCode::Char('?') => {
            vec![Message::ToggleTaskPreview, Message::ToggleHelp]
        }

        // ═══════════════════════════════════════════════════════════════════
        // PHASE-SPECIFIC ACTIONS (close modal then execute)
        // ═══════════════════════════════════════════════════════════════════

        // Start task - only available in Planned phase
        KeyCode::Char('s') if task.status == TaskStatus::Planned => {
            let mut msgs = vec![Message::ToggleTaskPreview];
            if let Some(project) = app.model.active_project() {
                if project.is_git_repo() {
                    msgs.push(Message::StartTaskWithWorktree(task.id));
                } else {
                    msgs.push(Message::StartTask(task.id));
                }
            }
            msgs
        }


        // Open combined tmux session (Claude on left, shell on right)
        KeyCode::Char('o') => {
            if matches!(task.status, TaskStatus::InProgress | TaskStatus::Review | TaskStatus::NeedsInput)
                && task.worktree_path.is_some()
            {
                vec![Message::ToggleTaskPreview, Message::OpenInteractiveModal(task.id)]
            } else {
                vec![]
            }
        }

        // Apply task changes to main worktree for testing - Review only
        KeyCode::Char('a') => {
            if task.status == TaskStatus::Review {
                // Don't apply if already being merged
                if task.status == TaskStatus::Accepting {
                    return vec![];
                }
                vec![Message::ToggleTaskPreview, Message::SmartApplyTask(task.id)]
            } else {
                vec![]
            }
        }

        // Merge task (finalize changes and mark done) - Review only
        KeyCode::Char('m') => {
            if task.status == TaskStatus::Review {
                // Don't merge tasks that are already being merged
                if task.status == TaskStatus::Accepting {
                    return vec![];
                }
                vec![
                    Message::ToggleTaskPreview,
                    Message::ShowConfirmation {
                        message: "Merge all changes and mark done? (y/n)".to_string(),
                        action: model::PendingAction::AcceptTask(task.id),
                    },
                ]
            } else {
                vec![]
            }
        }

        // Merge only (keep worktree and task in Review) - Shift+M in Review
        KeyCode::Char('M') => {
            if task.status == TaskStatus::Review {
                // Don't merge tasks that are already being merged
                if task.status == TaskStatus::Accepting {
                    return vec![];
                }
                vec![
                    Message::ToggleTaskPreview,
                    Message::ShowConfirmation {
                        message: "Merge changes to main? (keeps worktree) (y/n)".to_string(),
                        action: model::PendingAction::MergeOnlyTask(task.id),
                    },
                ]
            } else {
                vec![]
            }
        }

        // Edit task
        KeyCode::Char('e') => {
            vec![Message::ToggleTaskPreview, Message::EditTask(task.id)]
        }

        // Decline (Review) or Delete (other statuses) - with confirmation
        KeyCode::Char('d') => {
            if task.status == TaskStatus::Review {
                // Don't process tasks that are already being accepted
                if task.status == TaskStatus::Accepting {
                    return vec![];
                }
                vec![
                    Message::ToggleTaskPreview,
                    Message::ShowConfirmation {
                        message: "Discard all changes and mark done?".to_string(),
                        action: model::PendingAction::DeclineTask(task.id),
                    },
                ]
            } else {
                let title = if task.title.len() > 30 {
                    format!("{}...", &task.title[..27])
                } else {
                    task.title.clone()
                };
                vec![
                    Message::ToggleTaskPreview,
                    Message::ShowConfirmation {
                        message: format!("Delete '{}'? (y/n)", title),
                        action: model::PendingAction::DeleteTask(task.id),
                    },
                ]
            }
        }

        // Feedback: send follow-up instructions (Review only)
        KeyCode::Char('f') => {
            if task.status == TaskStatus::Review {
                // Don't allow feedback on tasks that are already being accepted
                if task.status == TaskStatus::Accepting {
                    return vec![];
                }
                vec![
                    Message::ToggleTaskPreview,
                    Message::EnterFeedbackMode(task.id),
                ]
            } else {
                vec![]
            }
        }

        // Move to Review (from InProgress, NeedsInput, Done) or Rebase worktree (in Review)
        KeyCode::Char('r') => {
            if matches!(task.status, TaskStatus::InProgress | TaskStatus::NeedsInput | TaskStatus::Done) {
                vec![
                    Message::ToggleTaskPreview,
                    Message::MoveTask {
                        task_id: task.id,
                        to_status: TaskStatus::Review,
                    },
                ]
            } else if task.worktree_path.is_some() && task.status == TaskStatus::Review {
                // Rebase worktree to latest main (Review only)
                vec![Message::ToggleTaskPreview, Message::UpdateWorktreeToMain(task.id)]
            } else {
                vec![]
            }
        }

        // Reset task (cleanup worktree and move to Planned)
        KeyCode::Char('x') => {
            if matches!(task.status, TaskStatus::InProgress | TaskStatus::NeedsInput | TaskStatus::Review | TaskStatus::Done) {
                vec![
                    Message::ToggleTaskPreview,
                    Message::ShowConfirmation {
                        message: format!("Reset '{}'? This will clean up worktree and move to Planned.", task.title),
                        action: model::PendingAction::ResetTask(task.id),
                    },
                ]
            } else {
                vec![]
            }
        }

        // Queue task (Planned only)
        KeyCode::Char('q') => {
            if task.status == TaskStatus::Planned {
                if let Some(project) = app.model.active_project() {
                    let running_sessions = project.tasks_with_active_sessions();
                    if !running_sessions.is_empty() {
                        return vec![Message::ToggleTaskPreview, Message::ShowQueueDialog(task.id)];
                    }
                }
            }
            // If not in Planned or no sessions, q closes the modal
            vec![Message::ToggleTaskPreview]
        }

        // Unapply task changes (remove applied changes from main worktree)
        KeyCode::Char('u') => {
            let has_applied = app.model.active_project()
                .map(|p| p.applied_task_id.is_some())
                .unwrap_or(false);
            if has_applied {
                return vec![Message::ToggleTaskPreview, Message::UnapplyTaskChanges];
            }
            vec![]
        }

        // Ignore other keys (don't close modal)
        _ => vec![],
    }
}

/// Handle key events when the open project dialog is open
fn handle_open_project_dialog_input(key: event::KeyEvent, app: &mut App) -> Vec<Message> {
    // Check if we're in create folder mode
    if let Some(ref input) = app.model.ui_state.create_folder_input {
        return handle_create_folder_input(key, input.clone(), app);
    }

    match key.code {
        // Close dialog
        KeyCode::Esc => {
            vec![Message::CloseOpenProjectDialog]
        }

        // Navigate up in active column
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(ref mut browser) = app.model.ui_state.directory_browser {
                browser.move_up();
            }
            vec![]
        }

        // Navigate down in active column
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(ref mut browser) = app.model.ui_state.directory_browser {
                browser.move_down();
            }
            vec![]
        }

        // Jump to start of active column
        KeyCode::Home | KeyCode::Char('g') => {
            if let Some(ref mut browser) = app.model.ui_state.directory_browser {
                browser.move_to_start();
            }
            vec![]
        }

        // Jump to end of active column
        KeyCode::End | KeyCode::Char('G') => {
            if let Some(ref mut browser) = app.model.ui_state.directory_browser {
                browser.move_to_end();
            }
            vec![]
        }

        // Page up - jump 5 folders up
        KeyCode::PageUp => {
            if let Some(ref mut browser) = app.model.ui_state.directory_browser {
                browser.page_up(5);
            }
            vec![]
        }

        // Page down - jump 5 folders down
        KeyCode::PageDown => {
            if let Some(ref mut browser) = app.model.ui_state.directory_browser {
                browser.page_down(5);
            }
            vec![]
        }

        // Move focus left to parent column
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => {
            if let Some(ref mut browser) = app.model.ui_state.directory_browser {
                browser.move_left();
            }
            vec![]
        }

        // Move focus right to child column (or enter directory if at rightmost)
        KeyCode::Right | KeyCode::Char('l') => {
            if let Some(ref mut browser) = app.model.ui_state.directory_browser {
                let _ = browser.move_right();
            }
            vec![]
        }

        // Enter/Space: open project or navigate into directory
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Some(ref mut browser) = app.model.ui_state.directory_browser {
                match browser.enter_selected() {
                    Ok(EnterResult::OpenProject(path)) => {
                        return vec![Message::ConfirmOpenProjectPath(path)];
                    }
                    Ok(EnterResult::CreateNewProject) => {
                        return vec![Message::EnterCreateFolderMode];
                    }
                    Ok(EnterResult::NavigatedInto) => {}
                    Ok(EnterResult::Nothing) => {}
                    Err(_) => {}
                }
            }
            vec![]
        }

        // Jump to first folder starting with typed letter (all letters work now)
        KeyCode::Char(c) if c.is_ascii_alphabetic() => {
            if let Some(ref mut browser) = app.model.ui_state.directory_browser {
                browser.jump_to_letter(c);
            }
            vec![]
        }

        _ => vec![]
    }
}

/// Handle key events when in create folder mode
fn handle_create_folder_input(key: event::KeyEvent, current_input: String, app: &mut App) -> Vec<Message> {
    match key.code {
        // Cancel create folder mode
        KeyCode::Esc => {
            vec![Message::CancelCreateFolderMode]
        }

        // Confirm and create folder
        KeyCode::Enter => {
            if !current_input.is_empty() {
                vec![Message::CreateFolder { name: current_input }]
            } else {
                vec![Message::CancelCreateFolderMode]
            }
        }

        // Delete last character
        KeyCode::Backspace => {
            let mut new_input = current_input;
            new_input.pop();
            app.model.ui_state.create_folder_input = Some(new_input);
            vec![]
        }

        // Add character to input
        KeyCode::Char(c) => {
            let mut new_input = current_input;
            new_input.push(c);
            app.model.ui_state.create_folder_input = Some(new_input);
            vec![]
        }

        _ => vec![]
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
/// Format: kanclaude signal <event> <task-id> [input-type]
fn handle_signal_command(args: &[String]) -> anyhow::Result<()> {
    if args.len() < 2 {
        return Err(anyhow::anyhow!("Usage: kanclaude signal <event> <task-id> [input-type]"));
    }

    let event = &args[0];
    let task_id = &args[1];
    let input_type = args.get(2).map(|s| s.as_str());

    // Get current working directory (the worktree)
    let cwd = std::env::current_dir().unwrap_or_default();

    // Write signal file with task_id as the session identifier
    // The watcher will pick this up and process it
    hooks::write_signal(event, task_id, &cwd, input_type)?;

    Ok(())
}

/// Detect tasks whose Claude sessions are actually idle (waiting for input)
/// This is a fallback for when signals are lost or have wrong session IDs
fn detect_idle_tasks_from_tmux(app: &mut App) {
    use std::process::Command;

    for project in &mut app.model.projects {
        let project_slug = project.slug();

        for task in &mut project.tasks {
            // Check InProgress and NeedsInput tasks with tmux windows
            // Both could have finished while app was closed
            if task.status != model::TaskStatus::InProgress
                && task.status != model::TaskStatus::NeedsInput {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn make_key_event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn test_key_event_to_tmux_enter() {
        let key = make_key_event(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(key_event_to_tmux_sequence(key), "Enter");
    }

    #[test]
    fn test_key_event_to_tmux_tab() {
        let key = make_key_event(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(key_event_to_tmux_sequence(key), "Tab");
    }

    #[test]
    fn test_key_event_to_tmux_backspace() {
        let key = make_key_event(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(key_event_to_tmux_sequence(key), "BSpace");
    }

    #[test]
    fn test_key_event_to_tmux_escape() {
        let key = make_key_event(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(key_event_to_tmux_sequence(key), "Escape");
    }

    #[test]
    fn test_key_event_to_tmux_arrows() {
        let test_cases = vec![
            (KeyCode::Up, "Up"),
            (KeyCode::Down, "Down"),
            (KeyCode::Left, "Left"),
            (KeyCode::Right, "Right"),
        ];

        for (code, expected) in test_cases {
            let key = make_key_event(code, KeyModifiers::NONE);
            assert_eq!(key_event_to_tmux_sequence(key), expected);
        }
    }

    #[test]
    fn test_key_event_to_tmux_navigation() {
        let test_cases = vec![
            (KeyCode::Home, "Home"),
            (KeyCode::End, "End"),
            (KeyCode::PageUp, "PageUp"),
            (KeyCode::PageDown, "PageDown"),
            (KeyCode::Delete, "DC"),
            (KeyCode::Insert, "IC"),
        ];

        for (code, expected) in test_cases {
            let key = make_key_event(code, KeyModifiers::NONE);
            assert_eq!(key_event_to_tmux_sequence(key), expected);
        }
    }

    #[test]
    fn test_key_event_to_tmux_function_keys() {
        for n in 1..=12 {
            let key = make_key_event(KeyCode::F(n), KeyModifiers::NONE);
            assert_eq!(key_event_to_tmux_sequence(key), format!("F{}", n));
        }
    }

    #[test]
    fn test_key_event_to_tmux_plain_char() {
        let key = make_key_event(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(key_event_to_tmux_sequence(key), "a");

        let key = make_key_event(KeyCode::Char('Z'), KeyModifiers::NONE);
        assert_eq!(key_event_to_tmux_sequence(key), "Z");
    }

    #[test]
    fn test_key_event_to_tmux_ctrl_char() {
        let key = make_key_event(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_event_to_tmux_sequence(key), "C-c");

        let key = make_key_event(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(key_event_to_tmux_sequence(key), "C-a");
    }

    #[test]
    fn test_key_event_to_tmux_alt_char() {
        let key = make_key_event(KeyCode::Char('x'), KeyModifiers::ALT);
        assert_eq!(key_event_to_tmux_sequence(key), "M-x");
    }

    #[test]
    fn test_key_event_to_tmux_space() {
        let key = make_key_event(KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(key_event_to_tmux_sequence(key), "Space");
    }

    #[test]
    fn test_key_event_to_tmux_semicolon() {
        let key = make_key_event(KeyCode::Char(';'), KeyModifiers::NONE);
        assert_eq!(key_event_to_tmux_sequence(key), "\\;");
    }

    #[test]
    fn test_key_event_to_tmux_unhandled() {
        // Null key should return empty
        let key = make_key_event(KeyCode::Null, KeyModifiers::NONE);
        assert_eq!(key_event_to_tmux_sequence(key), "");
    }
}
