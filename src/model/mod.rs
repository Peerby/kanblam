use crate::ui::logo::EyeAnimation;
use chrono::{DateTime, Utc};
use edtui::{
    EditorEventHandler, EditorMode, EditorState, Lines,
    actions::{Action, Composed, SelectInnerWord, DeleteSelection, SwitchMode, MoveWordForwardToEndOfWord, MoveWordBackward, MoveForward, MoveToFirst, MoveToEndOfLine, MoveToStartOfLine},
    actions::motion::{MoveToLastRow, MoveToFirstRow},
    events::{KeyEvent, KeyEventHandler, KeyEventRegister},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Available editors for external editing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Editor {
    #[default]
    Vim,
    Neovim,
    Nano,
    Emacs,
    Vscode,
    Zed,
    Helix,
}

impl Editor {
    /// Get all available editors
    pub fn all() -> &'static [Editor] {
        &[
            Editor::Vim,
            Editor::Neovim,
            Editor::Nano,
            Editor::Emacs,
            Editor::Vscode,
            Editor::Zed,
            Editor::Helix,
        ]
    }

    /// Get the display name for the editor
    pub fn name(&self) -> &'static str {
        match self {
            Editor::Vim => "Vim",
            Editor::Neovim => "Neovim",
            Editor::Nano => "Nano",
            Editor::Emacs => "Emacs",
            Editor::Vscode => "VS Code",
            Editor::Zed => "Zed",
            Editor::Helix => "Helix",
        }
    }

    /// Get the command to launch the editor
    pub fn command(&self) -> &'static str {
        match self {
            Editor::Vim => "vim",
            Editor::Neovim => "nvim",
            Editor::Nano => "nano",
            Editor::Emacs => "emacs",
            Editor::Vscode => "code --wait",
            Editor::Zed => "zed --wait",
            Editor::Helix => "hx",
        }
    }
}

/// Global settings (shared across all projects)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalSettings {
    /// Preferred editor for external editing (Ctrl-G in input mode)
    #[serde(default)]
    pub default_editor: Editor,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            default_editor: Editor::Vim,
        }
    }
}

/// Special entry types for directory browser
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpecialEntry {
    #[default]
    None,
    /// "[New Project Here]" action item
    NewProjectHere,
    /// Parent directory ".."
    ParentDir,
}

/// A directory entry
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub special: SpecialEntry,
}

/// State for a single Miller column
#[derive(Debug, Clone)]
pub struct MillerColumn {
    /// Directory this column displays
    pub dir: PathBuf,
    /// Entries in this column
    pub entries: Vec<DirEntry>,
    /// Selected index in this column
    pub selected_idx: usize,
}

/// Result of entering a selected item
#[derive(Debug, Clone)]
pub enum EnterResult {
    /// Navigated into a directory
    NavigatedInto,
    /// Selected a directory - open it as a project
    OpenProject(PathBuf),
    /// Selected "[New Project Here]" - enter create folder mode
    CreateNewProject,
    /// Nothing happened
    Nothing,
}

/// Miller columns directory browser for project selection
#[derive(Debug, Clone)]
pub struct DirectoryBrowser {
    /// The three columns: grandparent (0), parent (1), current (2)
    pub columns: [Option<MillerColumn>; 3],
    /// Which column is currently active (0, 1, or 2)
    pub active_column: usize,
}

impl MillerColumn {
    /// Load a column for a directory
    fn load(dir: PathBuf, include_new_project: bool) -> std::io::Result<Self> {
        let mut entries = Vec::new();

        // Add "[New Project Here]" if requested (for the active/rightmost column)
        if include_new_project {
            entries.push(DirEntry {
                name: "[New Project Here]".to_string(),
                path: dir.clone(),
                is_dir: false,
                special: SpecialEntry::NewProjectHere,
            });
        }

        // Add parent directory entry if not at root
        if dir.parent().is_some() {
            entries.push(DirEntry {
                name: "..".to_string(),
                path: dir.parent().unwrap().to_path_buf(),
                is_dir: true,
                special: SpecialEntry::ParentDir,
            });
        }

        // Read directory entries
        let read_dir = std::fs::read_dir(&dir)?;
        let mut dirs: Vec<DirEntry> = Vec::new();

        for entry in read_dir.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files/directories
            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                dirs.push(DirEntry {
                    name,
                    path,
                    is_dir: true,
                    special: SpecialEntry::None,
                });
            }
        }

        // Sort directories alphabetically
        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        entries.extend(dirs);

        Ok(Self {
            dir,
            entries,
            selected_idx: 0,
        })
    }

    /// Get the selected entry
    fn selected(&self) -> Option<&DirEntry> {
        self.entries.get(self.selected_idx)
    }

    /// Get the selected directory path (for regular directories, not special entries)
    fn selected_dir_path(&self) -> Option<&PathBuf> {
        self.selected().and_then(|e| {
            if e.is_dir && e.special == SpecialEntry::None {
                Some(&e.path)
            } else {
                None
            }
        })
    }
}

impl DirectoryBrowser {
    /// Create a new Miller columns browser starting at the given path
    pub fn new(start_dir: PathBuf) -> std::io::Result<Self> {
        let mut browser = Self {
            columns: [None, None, None],
            active_column: 2,
        };
        browser.navigate_to(start_dir)?;
        Ok(browser)
    }

    /// Navigate to a specific directory, setting up all columns
    fn navigate_to(&mut self, dir: PathBuf) -> std::io::Result<()> {
        // Current column (rightmost, index 2)
        let current = MillerColumn::load(dir.clone(), true)?;

        // Parent column (index 1)
        let parent = if let Some(parent_dir) = dir.parent() {
            Some(MillerColumn::load(parent_dir.to_path_buf(), false)?)
        } else {
            None
        };

        // Grandparent column (index 0)
        let grandparent = if let Some(ref parent_col) = parent {
            if let Some(gp_dir) = parent_col.dir.parent() {
                Some(MillerColumn::load(gp_dir.to_path_buf(), false)?)
            } else {
                None
            }
        } else {
            None
        };

        self.columns = [grandparent, parent, Some(current)];
        self.active_column = 2;

        // Select the current directory in parent column
        if let Some(ref mut parent_col) = self.columns[1] {
            if let Some(idx) = parent_col.entries.iter().position(|e| e.path == dir) {
                parent_col.selected_idx = idx;
            }
        }

        // Select the parent directory in grandparent column
        // Clone the parent dir first to avoid borrow checker issues
        let parent_dir_opt = self.columns[1].as_ref().map(|c| c.dir.clone());
        if let (Some(ref mut gp_col), Some(parent_dir)) = (&mut self.columns[0], parent_dir_opt) {
            if let Some(idx) = gp_col.entries.iter().position(|e| e.path == parent_dir) {
                gp_col.selected_idx = idx;
            }
        }

        Ok(())
    }

    /// Get the active column
    fn active_column_mut(&mut self) -> Option<&mut MillerColumn> {
        self.columns[self.active_column].as_mut()
    }

    /// Get the active column (immutable)
    pub fn active_column_ref(&self) -> Option<&MillerColumn> {
        self.columns[self.active_column].as_ref()
    }

    /// Move selection up in active column
    pub fn move_up(&mut self) {
        if let Some(col) = self.active_column_mut() {
            if col.selected_idx > 0 {
                col.selected_idx -= 1;
                self.sync_child_columns();
            }
        }
    }

    /// Move selection down in active column
    pub fn move_down(&mut self) {
        if let Some(col) = self.active_column_mut() {
            if !col.entries.is_empty() && col.selected_idx < col.entries.len() - 1 {
                col.selected_idx += 1;
                self.sync_child_columns();
            }
        }
    }

    /// Jump to first entry in active column
    pub fn move_to_start(&mut self) {
        if let Some(col) = self.active_column_mut() {
            if col.selected_idx != 0 {
                col.selected_idx = 0;
                self.sync_child_columns();
            }
        }
    }

    /// Jump to last entry in active column
    pub fn move_to_end(&mut self) {
        if let Some(col) = self.active_column_mut() {
            if !col.entries.is_empty() {
                let last = col.entries.len() - 1;
                if col.selected_idx != last {
                    col.selected_idx = last;
                    self.sync_child_columns();
                }
            }
        }
    }

    /// Page up in active column
    pub fn page_up(&mut self, count: usize) {
        if let Some(col) = self.active_column_mut() {
            let old_idx = col.selected_idx;
            col.selected_idx = col.selected_idx.saturating_sub(count);
            if col.selected_idx != old_idx {
                self.sync_child_columns();
            }
        }
    }

    /// Page down in active column
    pub fn page_down(&mut self, count: usize) {
        if let Some(col) = self.active_column_mut() {
            if !col.entries.is_empty() {
                let old_idx = col.selected_idx;
                col.selected_idx = (col.selected_idx + count).min(col.entries.len() - 1);
                if col.selected_idx != old_idx {
                    self.sync_child_columns();
                }
            }
        }
    }

    /// Jump to first entry starting with character in active column
    pub fn jump_to_letter(&mut self, c: char) {
        let lower_c = c.to_ascii_lowercase();
        if let Some(col) = self.active_column_mut() {
            // Find first regular directory entry starting with this letter
            for (idx, entry) in col.entries.iter().enumerate() {
                // Skip special entries
                if entry.special != SpecialEntry::None {
                    continue;
                }
                if entry
                    .name
                    .chars()
                    .next()
                    .map(|first| first.to_ascii_lowercase() == lower_c)
                    .unwrap_or(false)
                {
                    if col.selected_idx != idx {
                        col.selected_idx = idx;
                        self.sync_child_columns();
                    }
                    return;
                }
            }
        }
    }

    /// Move focus left to parent column
    pub fn move_left(&mut self) {
        if self.active_column > 0 && self.columns[self.active_column - 1].is_some() {
            self.active_column -= 1;
        } else if self.active_column == 0 {
            // At leftmost column, try to navigate up to show grandparent
            // Clone the dir first to avoid borrow checker issues
            let col_dir = self.columns[0].as_ref().map(|c| c.dir.clone());
            if let Some(dir) = col_dir {
                if let Some(parent) = dir.parent() {
                    // Shift to show the parent as the new rightmost column
                    let _ = self.shift_columns_left(parent.to_path_buf());
                }
            }
        }
    }

    /// Move focus right to child column or enter directory
    pub fn move_right(&mut self) -> std::io::Result<()> {
        if self.active_column < 2 && self.columns[self.active_column + 1].is_some() {
            // Move focus right
            self.active_column += 1;
        } else if self.active_column == 2 {
            // At rightmost column, enter selected directory
            if let Some(ref col) = self.columns[2] {
                if let Some(entry) = col.entries.get(col.selected_idx) {
                    if entry.is_dir && entry.special == SpecialEntry::None {
                        self.enter_directory(entry.path.clone())?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Shift all columns left and load new content for a directory
    fn shift_columns_left(&mut self, new_current_dir: PathBuf) -> std::io::Result<()> {
        self.navigate_to(new_current_dir)?;
        self.active_column = 2;
        Ok(())
    }

    /// Enter a directory, shifting columns
    fn enter_directory(&mut self, dir: PathBuf) -> std::io::Result<()> {
        self.navigate_to(dir)?;
        Ok(())
    }

    /// Sync child columns when selection changes in a non-rightmost column
    fn sync_child_columns(&mut self) {
        // If active column is not the rightmost, update columns to the right
        if self.active_column < 2 {
            if let Some(ref col) = self.columns[self.active_column] {
                if let Some(selected_path) = col.selected_dir_path() {
                    // Update the next column to show selected directory's contents
                    let is_rightmost_child = self.active_column == 1;
                    if let Ok(child_col) = MillerColumn::load(selected_path.clone(), is_rightmost_child) {
                        self.columns[self.active_column + 1] = Some(child_col);

                        // If we updated column 1, also update column 2
                        if self.active_column == 0 {
                            if let Some(ref col1) = self.columns[1] {
                                if let Some(child_path) = col1.selected_dir_path() {
                                    if let Ok(col2) = MillerColumn::load(child_path.clone(), true) {
                                        self.columns[2] = Some(col2);
                                    }
                                } else {
                                    // No valid selection in column 1, clear column 2
                                    self.columns[2] = None;
                                }
                            }
                        }
                    }
                } else {
                    // Selected item is not a navigable directory, clear child columns
                    for i in (self.active_column + 1)..3 {
                        self.columns[i] = None;
                    }
                }
            }
        }
    }

    /// Enter the selected item - opens project or navigates for special entries
    pub fn enter_selected(&mut self) -> std::io::Result<EnterResult> {
        let (entry_clone, col_dir) = {
            let col = match self.columns[self.active_column].as_ref() {
                Some(c) => c,
                None => return Ok(EnterResult::Nothing),
            };
            let entry = match col.entries.get(col.selected_idx) {
                Some(e) => e,
                None => return Ok(EnterResult::Nothing),
            };
            (entry.clone(), col.dir.clone())
        };

        match entry_clone.special {
            SpecialEntry::NewProjectHere => {
                // Enter create folder mode to create a new project
                Ok(EnterResult::CreateNewProject)
            }
            SpecialEntry::ParentDir => {
                // Navigate to parent (don't open as project)
                self.navigate_to(entry_clone.path)?;
                Ok(EnterResult::NavigatedInto)
            }
            SpecialEntry::None => {
                if entry_clone.is_dir {
                    // Open selected directory as project
                    Ok(EnterResult::OpenProject(entry_clone.path))
                } else {
                    Ok(EnterResult::Nothing)
                }
            }
        }
    }

    /// Get a preview of the selected directory's contents (for showing in next column)
    pub fn get_preview_entries(&self) -> Option<Vec<DirEntry>> {
        let col = self.columns[self.active_column].as_ref()?;
        let entry = col.entries.get(col.selected_idx)?;

        // Only preview regular directories
        if entry.special != SpecialEntry::None || !entry.is_dir {
            return None;
        }

        // Try to load preview entries
        let read_dir = std::fs::read_dir(&entry.path).ok()?;
        let mut dirs: Vec<DirEntry> = Vec::new();

        for dir_entry in read_dir.flatten() {
            let path = dir_entry.path();
            let name = dir_entry.file_name().to_string_lossy().to_string();

            // Skip hidden files/directories
            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                dirs.push(DirEntry {
                    name,
                    path,
                    is_dir: true,
                    special: SpecialEntry::None,
                });
            }
        }

        // Sort directories alphabetically
        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Some(dirs)
    }

    /// Get the currently selected entry in active column
    pub fn selected(&self) -> Option<&DirEntry> {
        self.columns[self.active_column]
            .as_ref()
            .and_then(|col| col.entries.get(col.selected_idx))
    }

    /// Get the current directory (from the active column)
    pub fn cwd(&self) -> Option<&PathBuf> {
        self.columns[self.active_column].as_ref().map(|col| &col.dir)
    }

    /// Create a new folder in the active column's directory and initialize it with git.
    pub fn create_folder(&mut self, name: &str) -> std::io::Result<PathBuf> {
        let current_dir = self.columns[self.active_column]
            .as_ref()
            .map(|col| col.dir.clone())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "No active column"))?;

        // Validate folder name
        if name.is_empty() || name == "." || name == ".." {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid folder name",
            ));
        }

        // Check for invalid characters
        if name.contains('/') || name.contains('\\') || name.contains('\0') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Folder name contains invalid characters",
            ));
        }

        let folder_path = current_dir.join(name);

        // Check if folder already exists
        if folder_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Folder already exists",
            ));
        }

        // Create the folder
        std::fs::create_dir(&folder_path)?;

        // Initialize git repository
        let output = std::process::Command::new("git")
            .args(["init"])
            .current_dir(&folder_path)
            .output()?;

        if !output.status.success() {
            // Clean up the folder if git init fails
            let _ = std::fs::remove_dir(&folder_path);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "Failed to initialize git repository: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            ));
        }

        // Refresh by re-navigating to current directory
        self.navigate_to(current_dir)?;

        // Select the newly created folder
        if let Some(ref mut col) = self.columns[self.active_column] {
            if let Some(idx) = col.entries.iter().position(|e| e.path == folder_path) {
                col.selected_idx = idx;
            }
        }

        Ok(folder_path)
    }
}

/// Application state following The Elm Architecture
#[derive(Serialize, Deserialize)]
pub struct AppModel {
    pub projects: Vec<Project>,
    pub active_project_idx: usize,
    /// Global settings (shared across all projects)
    #[serde(default)]
    pub global_settings: GlobalSettings,
    #[serde(skip)]
    pub ui_state: UiState,
}

impl Default for AppModel {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            active_project_idx: 0,
            global_settings: GlobalSettings::default(),
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

}

/// A stash that we created and are tracking for the user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedStash {
    /// The stash reference (e.g., "stash@{0}")
    pub stash_ref: String,
    /// Human-readable description of why this stash was created
    pub description: String,
    /// When the stash was created
    pub created_at: DateTime<Utc>,
    /// Number of files changed in this stash
    pub files_changed: usize,
    /// Summary of changed files (first few file names)
    pub files_summary: String,
    /// The stash's commit SHA (for stable identification even if index changes)
    pub stash_sha: String,
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

    // Applied changes state (persisted so unapply works after restart)
    /// Task ID whose changes are currently applied to main worktree (for testing)
    /// When set, user can press 'u' to unapply the changes
    #[serde(default)]
    pub applied_task_id: Option<Uuid>,
    /// Stash ref created when applying task changes (to restore original work on unapply)
    #[serde(default)]
    pub applied_stash_ref: Option<String>,
    /// Whether Claude resolved conflicts during apply (affects completion routing and unapply)
    /// When true, the patch file contains the combined changes (task + resolution)
    #[serde(default)]
    pub applied_with_conflict_resolution: bool,

    /// Stashes we created that the user may want to restore
    /// Tracked so we can show an indicator and offer to pop/delete them
    #[serde(default)]
    pub tracked_stashes: Vec<TrackedStash>,

    // Main worktree lock state (prevents concurrent git operations)
    /// Task ID that currently has exclusive access to the main worktree
    /// Set during Accept/Apply operations that modify main's git state
    /// NOT persisted - lock is transient and resets on app restart
    #[serde(skip)]
    pub main_worktree_lock: Option<MainWorktreeLock>,

    /// Custom commands for this project (optional overrides for auto-detected defaults)
    #[serde(default)]
    pub commands: ProjectCommands,

    // Remote tracking status (transient - not persisted)
    /// Number of commits ahead of remote (local commits not pushed)
    #[serde(skip)]
    pub remote_ahead: usize,
    /// Number of commits behind remote (remote commits not pulled)
    #[serde(skip)]
    pub remote_behind: usize,
    /// Whether there's a configured remote tracking branch
    #[serde(skip)]
    pub has_remote: bool,
    /// Whether a git operation (fetch/pull/push) is currently in progress
    #[serde(skip)]
    pub git_operation_in_progress: Option<GitOperation>,
}

/// Custom commands for a project. All fields are optional - when None,
/// the system will auto-detect based on project files (Cargo.toml, package.json, etc.)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectCommands {
    /// Command to verify the project compiles/type-checks (e.g., "cargo check", "npm run build", "tsc --noEmit")
    /// Used after applying changes to verify they don't break the build
    pub check: Option<String>,

    /// Command to run the project (e.g., "cargo run", "npm start", "python main.py")
    pub run: Option<String>,

    /// Command to run tests (e.g., "cargo test", "npm test", "pytest")
    pub test: Option<String>,

    /// Command to format code (e.g., "cargo fmt", "npm run format", "black .")
    pub format: Option<String>,

    /// Command to lint code (e.g., "cargo clippy", "npm run lint", "ruff check .")
    pub lint: Option<String>,
}

impl ProjectCommands {
    /// Auto-detect commands based on files in the project directory
    pub fn detect(project_dir: &PathBuf) -> Self {
        let mut commands = ProjectCommands::default();

        // Check for Rust (Cargo.toml)
        if project_dir.join("Cargo.toml").exists() {
            commands.check = Some("cargo check".to_string());
            commands.run = Some("cargo run".to_string());
            commands.test = Some("cargo test".to_string());
            commands.format = Some("cargo fmt".to_string());
            commands.lint = Some("cargo clippy".to_string());
            return commands;
        }

        // Check for Node.js (package.json)
        if project_dir.join("package.json").exists() {
            // Check if it's a TypeScript project
            if project_dir.join("tsconfig.json").exists() {
                commands.check = Some("npx tsc --noEmit".to_string());
            } else {
                // For JS, try npm run build if it exists, otherwise skip
                commands.check = Some("npm run build --if-present".to_string());
            }
            commands.run = Some("npm start".to_string());
            commands.test = Some("npm test".to_string());
            commands.format = Some("npm run format --if-present".to_string());
            commands.lint = Some("npm run lint --if-present".to_string());
            return commands;
        }

        // Check for Python (pyproject.toml or setup.py)
        if project_dir.join("pyproject.toml").exists() || project_dir.join("setup.py").exists() {
            commands.check = Some("python -m py_compile *.py".to_string());
            commands.test = Some("pytest".to_string());
            commands.format = Some("black .".to_string());
            commands.lint = Some("ruff check .".to_string());
            return commands;
        }

        // Check for Go (go.mod)
        if project_dir.join("go.mod").exists() {
            commands.check = Some("go build ./...".to_string());
            commands.run = Some("go run .".to_string());
            commands.test = Some("go test ./...".to_string());
            commands.format = Some("go fmt ./...".to_string());
            commands.lint = Some("golangci-lint run".to_string());
            return commands;
        }

        // Check for Makefile
        if project_dir.join("Makefile").exists() {
            commands.check = Some("make".to_string());
            commands.test = Some("make test".to_string());
            return commands;
        }

        commands
    }

    /// Get the effective check command (configured or auto-detected)
    pub fn effective_check(&self, project_dir: &PathBuf) -> Option<String> {
        self.check.clone().or_else(|| Self::detect(project_dir).check)
    }

    /// Get the effective run command (configured or auto-detected)
    pub fn effective_run(&self, project_dir: &PathBuf) -> Option<String> {
        self.run.clone().or_else(|| Self::detect(project_dir).run)
    }

    /// Get the effective test command (configured or auto-detected)
    pub fn effective_test(&self, project_dir: &PathBuf) -> Option<String> {
        self.test.clone().or_else(|| Self::detect(project_dir).test)
    }
}

/// Represents an exclusive lock on the main worktree for git operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MainWorktreeLock {
    /// The task that holds the lock
    pub task_id: Uuid,
    /// What operation is being performed
    pub operation: MainWorktreeOperation,
}

/// Operations that require exclusive access to the main worktree
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MainWorktreeOperation {
    /// Merging task changes into main (Accept flow)
    Accepting,
    /// Applying task changes for testing (Apply flow)
    Applying,
}

/// Git remote operations (fetch/pull/push)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitOperation {
    /// Fetching from remote to check status
    Fetching,
    /// Pulling changes from remote
    Pulling,
    /// Pushing changes to remote
    Pushing,
}

impl GitOperation {
    pub fn label(&self) -> &'static str {
        match self {
            GitOperation::Fetching => "Fetching...",
            GitOperation::Pulling => "Pulling...",
            GitOperation::Pushing => "Pushing...",
        }
    }
}

impl Project {
    pub fn new(name: String, working_dir: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            working_dir: working_dir.clone(),
            tasks: Vec::new(),
            needs_attention: false,
            created_at: Utc::now(),
            captured_output: String::new(),
            applied_task_id: None,
            applied_stash_ref: None,
            applied_with_conflict_resolution: false,
            tracked_stashes: Vec::new(),
            main_worktree_lock: None,
            commands: ProjectCommands::default(), // Will auto-detect when needed
            remote_ahead: 0,
            remote_behind: 0,
            has_remote: false,
            git_operation_in_progress: None,
        }
    }

    /// Format a task reference for display in messages: "[abc123] title truncat..."
    /// Short ID (6 chars) + truncated title (max 20 chars)
    /// Uses short_title if available, otherwise truncates the full title
    fn format_task_ref(&self, task_id: Uuid) -> String {
        let short_id = &task_id.to_string()[..6];
        let title = self.tasks.iter()
            .find(|t| t.id == task_id)
            .map(|t| {
                // Prefer short_title if available
                let display_title = t.short_title.as_ref().unwrap_or(&t.title);
                if display_title.len() > 20 {
                    format!("{}..", &display_title[..18])
                } else {
                    display_title.clone()
                }
            })
            .unwrap_or_else(|| "unknown".to_string());
        format!("[{}] {}", short_id, title)
    }

    /// Try to acquire exclusive lock on main worktree for a git operation.
    /// Returns Ok(()) if lock acquired, Err with reason if another operation is in progress.
    pub fn try_lock_main_worktree(&mut self, task_id: Uuid, operation: MainWorktreeOperation) -> Result<(), String> {
        // Check if changes are applied (blocks accept operations)
        if operation == MainWorktreeOperation::Accepting {
            if let Some(applied_id) = self.applied_task_id {
                if applied_id != task_id {
                    let task_ref = self.format_task_ref(applied_id);
                    return Err(format!(
                        "Cannot accept: changes from {} are applied. Press 'u' to unapply first.",
                        task_ref
                    ));
                }
            }
        }

        // Check existing lock
        if let Some(ref lock) = self.main_worktree_lock {
            if lock.task_id == task_id {
                // Same task already has lock - that's fine (re-entry)
                return Ok(());
            }
            let task_ref = self.format_task_ref(lock.task_id);
            let op_name = match lock.operation {
                MainWorktreeOperation::Accepting => "accepting",
                MainWorktreeOperation::Applying => "applying",
            };
            return Err(format!(
                "Cannot proceed: currently {} {}. Wait for it to complete.",
                op_name, task_ref
            ));
        }

        // Acquire the lock
        self.main_worktree_lock = Some(MainWorktreeLock { task_id, operation });
        Ok(())
    }

    /// Release the main worktree lock. Only the task holding the lock can release it.
    pub fn release_main_worktree_lock(&mut self, task_id: Uuid) {
        if let Some(ref lock) = self.main_worktree_lock {
            if lock.task_id == task_id {
                self.main_worktree_lock = None;
            }
        }
    }

    /// Check if main worktree is locked and by whom
    pub fn main_worktree_lock_info(&self) -> Option<(Uuid, MainWorktreeOperation, String)> {
        self.main_worktree_lock.as_ref().map(|lock| {
            let task_title = self.tasks.iter()
                .find(|t| t.id == lock.task_id)
                .map(|t| t.title.clone())
                .unwrap_or_else(|| "unknown task".to_string());
            (lock.task_id, lock.operation, task_title)
        })
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
        // Accepting, Updating, and Applying tasks appear in the Review column
        self.tasks.iter().filter(|t| {
            t.status == status ||
            (status == TaskStatus::Review && (t.status == TaskStatus::Accepting || t.status == TaskStatus::Updating || t.status == TaskStatus::Applying))
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

    /// Move a task to the end of tasks with a given status.
    /// This ensures newly transitioned tasks appear at the bottom of their column.
    /// Returns true if the task was found and moved.
    pub fn move_task_to_end_of_status(&mut self, task_id: Uuid, new_status: TaskStatus) -> bool {
        if let Some(idx) = self.tasks.iter().position(|t| t.id == task_id) {
            let mut task = self.tasks.remove(idx);
            task.status = new_status;

            // Find the position after the last task with this status
            let insert_pos = self.tasks.iter()
                .rposition(|t| t.status == new_status)
                .map(|pos| pos + 1)
                .unwrap_or_else(|| {
                    // No tasks with this status yet - find appropriate position
                    // Status order: Planned, Queued, InProgress, NeedsInput, Review, Done
                    // Insert before any tasks with a "later" status
                    self.tasks.iter()
                        .position(|t| t.status > new_status)
                        .unwrap_or(self.tasks.len())
                });

            self.tasks.insert(insert_pos, task);
            true
        } else {
            false
        }
    }

    pub fn needs_input_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.status == TaskStatus::NeedsInput).count()
    }

    /// Count of tasks needing attention (Review column + NeedsInput) in this project
    /// Includes Review, Accepting, Updating, Applying (all shown in Review column)
    pub fn attention_count(&self) -> usize {
        self.tasks.iter().filter(|t| {
            matches!(t.status,
                TaskStatus::Review |
                TaskStatus::Accepting |
                TaskStatus::Updating |
                TaskStatus::Applying |
                TaskStatus::NeedsInput
            )
        }).count()
    }
}

/// A single entry in the task activity log
#[derive(Debug, Clone)]
pub struct ActivityLogEntry {
    /// When this activity occurred
    pub timestamp: DateTime<Utc>,
    /// Short description of the activity
    pub message: String,
}

impl ActivityLogEntry {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            message: message.into(),
        }
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
    /// CLI session is actively working (received "working" hook but not "stop"/"end")
    /// SDK must not resume until CLI completes its turn
    CliActivelyWorking,
    /// Modal closed, waiting for CLI to exit before resuming SDK
    WaitingForCliExit,
}

/// A task to be executed by Claude Code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    /// Short summary title generated by Claude for display in kanban cards
    #[serde(default)]
    pub short_title: Option<String>,
    pub status: TaskStatus,
    pub images: Vec<PathBuf>,
    pub claude_session_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
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

    // === SDK/CLI handoff tracking ===

    /// Counter of SDK commands run for this session (incremented on each SDK query)
    /// Used to detect if CLI terminal has stale state
    #[serde(default)]
    pub sdk_command_count: u32,
    /// SDK command count when CLI terminal was last opened/refreshed
    /// If sdk_command_count > cli_opened_at_command_count, CLI needs refresh
    #[serde(default)]
    pub cli_opened_at_command_count: u32,
    /// Feedback queued to be sent when Claude finishes current work
    /// Used when user sends feedback while SDK/CLI is actively working
    #[serde(skip)]
    pub pending_feedback: Option<String>,

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

    // === Activity log (for UI feedback during Accepting/Updating) ===

    /// Recent activity log entries (not persisted)
    #[serde(skip)]
    pub activity_log: Vec<ActivityLogEntry>,

    // === Git status cache (updated periodically) ===

    /// Cached git status for the worktree (lines added)
    #[serde(skip)]
    pub git_additions: usize,
    /// Cached git status for the worktree (lines deleted)
    #[serde(skip)]
    pub git_deletions: usize,
    /// Cached git status for the worktree (files changed)
    #[serde(skip)]
    pub git_files_changed: usize,
    /// Cached git status for the worktree (commits ahead of main)
    #[serde(skip)]
    pub git_commits_ahead: usize,
    /// Cached git status for the worktree (commits behind main)
    #[serde(skip)]
    pub git_commits_behind: usize,
    /// When the git status was last updated
    #[serde(skip)]
    pub git_status_updated_at: Option<DateTime<Utc>>,
}

impl Task {
    pub fn new(title: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            title,
            description: String::new(),
            short_title: None,
            status: TaskStatus::Planned,
            images: Vec::new(),
            claude_session_id: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            // Worktree fields
            worktree_path: None,
            git_branch: None,
            tmux_window: None,
            session_state: ClaudeSessionState::NotStarted,
            session_mode: SessionMode::SdkManaged,
            // SDK/CLI handoff tracking
            sdk_command_count: 0,
            cli_opened_at_command_count: 0,
            pending_feedback: None,
            // Queueing
            queued_for_session: None,
            // Activity tracking
            accepting_started_at: None,
            last_activity_at: None,
            last_tool_name: None,
            activity_log: Vec::new(),
            // Git status cache
            git_additions: 0,
            git_deletions: 0,
            git_files_changed: 0,
            git_commits_ahead: 0,
            git_commits_behind: 0,
            git_status_updated_at: None,
        }
    }

    /// Check if this task has an active worktree session
    pub fn has_active_session(&self) -> bool {
        self.worktree_path.is_some() && self.session_state.is_active()
    }

    /// Add an entry to the activity log (keeps last 30 entries)
    pub fn log_activity(&mut self, message: impl Into<String>) {
        const MAX_LOG_ENTRIES: usize = 30;
        self.activity_log.push(ActivityLogEntry::new(message));
        if self.activity_log.len() > MAX_LOG_ENTRIES {
            self.activity_log.remove(0);
        }
    }

    /// Clear the activity log (e.g., when starting a new accept/update)
    pub fn clear_activity_log(&mut self) {
        self.activity_log.clear();
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
/// Ordered by typical progression: Planned -> Queued -> InProgress -> ... -> Done
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum TaskStatus {
    #[default]
    Planned,
    Queued,
    InProgress,
    NeedsInput,
    Review,
    Accepting, // Rebasing onto main before accepting
    Updating,  // Rebasing onto main without merging back (just updating worktree)
    Applying,  // Applying task changes to main worktree for testing
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
            TaskStatus::Updating => "Updating",
            TaskStatus::Applying => "Applying",
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
    /// Accepting, Updating, and Applying tasks appear in the Review column
    pub fn index(&self) -> usize {
        match self {
            TaskStatus::Planned => 0,
            TaskStatus::Queued => 1,
            TaskStatus::InProgress => 2,
            TaskStatus::NeedsInput => 3,
            TaskStatus::Review | TaskStatus::Accepting | TaskStatus::Updating | TaskStatus::Applying => 4,
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
    /// Scroll offset for the help modal (lines scrolled from top)
    pub help_scroll_offset: usize,
    pub pending_confirmation: Option<PendingConfirmation>,
    /// Scroll offset for confirmation modal (when content is large)
    pub confirmation_scroll_offset: usize,
    pub status_message: Option<String>,
    /// Tick countdown for status message decay (clears when reaches 0)
    pub status_message_decay: u16,
    /// If set, we're editing an existing task instead of creating a new one
    pub editing_task_id: Option<Uuid>,
    /// Scroll offset for long task titles (marquee effect)
    pub title_scroll_offset: usize,
    /// Delay counter before scrolling starts (ticks to wait)
    pub title_scroll_delay: usize,
    /// Pending images to attach to next created task
    pub pending_images: Vec<PathBuf>,
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

    // Task preview modal
    /// If true, show the task preview modal for the selected task
    pub show_task_preview: bool,
    /// Currently selected tab in the task detail modal
    pub task_detail_tab: TaskDetailTab,

    // Interactive terminal modal
    /// If set, the interactive modal is open for this task
    pub interactive_modal: Option<InteractiveModal>,

    // Open project dialog
    /// If set, the open project dialog is active for this slot (0-8 = keys 1-9)
    pub open_project_dialog_slot: Option<usize>,
    /// Directory browser for the open project dialog
    pub directory_browser: Option<DirectoryBrowser>,
    /// If Some, we're in create folder mode with the current input text
    pub create_folder_input: Option<String>,

    // Feedback mode
    /// If set, we're entering feedback for this task (task must be in Review status)
    /// The input area will be used to capture feedback text
    pub feedback_task_id: Option<Uuid>,

    // Logo shimmer animation (triggered on successful merge)
    /// Current shimmer position (0-7, where 0 = no shimmer, 1-4 = beam going up rows 4-1, 5-7 = fade out)
    /// The beam travels from bottom to top, lighting up each row with saturated colors
    pub logo_shimmer_frame: u8,

    // Mascot eye animation
    /// Current eye animation state (blink, wink, look around, etc.)
    pub eye_animation: EyeAnimation,
    /// Remaining ticks for the current eye animation (0 = animation done, revert to normal)
    pub eye_animation_ticks_remaining: u8,
    /// Ticks until the next random eye animation is triggered
    pub eye_animation_cooldown: u16,

    // Startup hint decay
    /// Tick count when the app started (used to decay the navigation hints after ~10 seconds)
    /// None means hints have already decayed or should not be shown
    pub startup_hint_until_tick: Option<usize>,

    // Project tabs navigation
    /// Selected index in project tabs when focus is ProjectTabs
    /// 0 = "+project" button, 1+ = project indices
    pub selected_project_tab_idx: usize,

    // ESC key tracking for showing help
    /// Counter for consecutive ESC key presses (resets on other keys)
    /// When this reaches 2, the startup hints are shown again
    pub consecutive_esc_count: u8,

    // Configuration modal
    /// If set, the configuration modal is open
    pub config_modal: Option<ConfigModalState>,

    // Stash modal
    /// If true, the stash management modal is open
    pub show_stash_modal: bool,
    /// Selected index in the stash list
    pub stash_modal_selected_idx: usize,

    // Git diff view in task detail modal
    /// Scroll offset for the git diff view (lines scrolled from top)
    pub git_diff_scroll_offset: usize,
    /// Cached git diff content for the currently viewed task
    pub git_diff_cache: Option<(Uuid, String)>,

    // Welcome panel state
    /// Current welcome message index (for rotation)
    pub welcome_message_idx: usize,
    /// Ticks until next message rotation (counts down from ~80 = 8 seconds at 100ms tick)
    pub welcome_message_cooldown: u16,
    /// Whether the welcome speech bubble is focused (for navigation)
    pub welcome_bubble_focused: bool,
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

/// Which field is selected in the config modal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfigField {
    #[default]
    DefaultEditor,
    CheckCommand,
    RunCommand,
    TestCommand,
    FormatCommand,
    LintCommand,
}

impl ConfigField {
    /// Get all config fields in display order
    pub fn all() -> &'static [ConfigField] {
        &[
            ConfigField::DefaultEditor,
            ConfigField::CheckCommand,
            ConfigField::RunCommand,
            ConfigField::TestCommand,
            ConfigField::FormatCommand,
            ConfigField::LintCommand,
        ]
    }
}

/// Tab selection in the task detail modal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskDetailTab {
    #[default]
    General,
    Git,
    Claude,
    Activity,
    Help,
}

impl TaskDetailTab {
    /// Get all tabs in order
    pub fn all() -> &'static [TaskDetailTab] {
        &[
            TaskDetailTab::General,
            TaskDetailTab::Git,
            TaskDetailTab::Claude,
            TaskDetailTab::Activity,
            TaskDetailTab::Help,
        ]
    }

    /// Get the display label for the tab
    pub fn label(&self) -> &'static str {
        match self {
            TaskDetailTab::General => "general",
            TaskDetailTab::Git => "git",
            TaskDetailTab::Claude => "claude",
            TaskDetailTab::Activity => "activity",
            TaskDetailTab::Help => "help",
        }
    }

    /// Move to the next tab (wraps around)
    pub fn next(&self) -> TaskDetailTab {
        match self {
            TaskDetailTab::General => TaskDetailTab::Git,
            TaskDetailTab::Git => TaskDetailTab::Claude,
            TaskDetailTab::Claude => TaskDetailTab::Activity,
            TaskDetailTab::Activity => TaskDetailTab::Help,
            TaskDetailTab::Help => TaskDetailTab::General,
        }
    }

    /// Move to the previous tab (wraps around)
    pub fn prev(&self) -> TaskDetailTab {
        match self {
            TaskDetailTab::General => TaskDetailTab::Help,
            TaskDetailTab::Git => TaskDetailTab::General,
            TaskDetailTab::Claude => TaskDetailTab::Git,
            TaskDetailTab::Activity => TaskDetailTab::Claude,
            TaskDetailTab::Help => TaskDetailTab::Activity,
        }
    }
}

impl ConfigField {
    /// Get the display label for this field
    pub fn label(&self) -> &'static str {
        match self {
            ConfigField::DefaultEditor => "Default Editor",
            ConfigField::CheckCommand => "Check Command",
            ConfigField::RunCommand => "Run Command",
            ConfigField::TestCommand => "Test Command",
            ConfigField::FormatCommand => "Format Command",
            ConfigField::LintCommand => "Lint Command",
        }
    }

    /// Get the hint/description for this field
    pub fn hint(&self) -> &'static str {
        match self {
            ConfigField::DefaultEditor => "External editor for Ctrl-G (global setting)",
            ConfigField::CheckCommand => "e.g. cargo check, npm run build, tsc --noEmit",
            ConfigField::RunCommand => "e.g. cargo run, npm start, python main.py",
            ConfigField::TestCommand => "e.g. cargo test, npm test, pytest",
            ConfigField::FormatCommand => "e.g. cargo fmt, npm run format, black .",
            ConfigField::LintCommand => "e.g. cargo clippy, npm run lint, ruff check .",
        }
    }

    /// Whether this field is a global setting (vs project-specific)
    pub fn is_global(&self) -> bool {
        matches!(self, ConfigField::DefaultEditor)
    }

    /// Get the next field (wrapping)
    pub fn next(&self) -> ConfigField {
        let all = Self::all();
        let idx = all.iter().position(|f| f == self).unwrap_or(0);
        all[(idx + 1) % all.len()]
    }

    /// Get the previous field (wrapping)
    pub fn prev(&self) -> ConfigField {
        let all = Self::all();
        let idx = all.iter().position(|f| f == self).unwrap_or(0);
        all[(idx + all.len() - 1) % all.len()]
    }
}

/// State for the configuration modal
#[derive(Debug, Clone)]
pub struct ConfigModalState {
    /// Currently selected field
    pub selected_field: ConfigField,
    /// Whether we're in edit mode for the selected field
    pub editing: bool,
    /// Temporary value being edited (for text fields)
    pub edit_buffer: String,
    /// Temporary project commands (edited before save)
    pub temp_commands: ProjectCommands,
    /// Temporary global settings (edited before save)
    pub temp_editor: Editor,
}

/// Create vim mode handler with custom keybindings
fn create_vim_handler() -> EditorEventHandler {
    let mut key_handler = KeyEventHandler::vim_mode();

    // Add dw (delete word) in normal mode - delete word + trailing whitespace
    // Vim's dw deletes from cursor to start of next word (including whitespace).
    // We achieve this by:
    // 1. MoveWordForwardToEndOfWord - get to last char of current word
    // 2. MoveForward(1) - move one more char to include the trailing space
    //    (at end of line, MoveForward does nothing, so we just delete the word)
    // This handles both mid-line (word + space) and end-of-line (just word) cases.
    key_handler.insert(
        KeyEventRegister::n(vec![KeyEvent::Char('d'), KeyEvent::Char('w')]),
        Composed::new(SwitchMode(EditorMode::Visual))
            .chain(MoveWordForwardToEndOfWord(1))
            .chain(MoveForward(1))
            .chain(DeleteSelection)
            .chain(SwitchMode(EditorMode::Normal)),
    );

    // Add de (delete to end of word) - delete from cursor to end of current word
    key_handler.insert(
        KeyEventRegister::n(vec![KeyEvent::Char('d'), KeyEvent::Char('e')]),
        Composed::new(SwitchMode(EditorMode::Visual))
            .chain(MoveWordForwardToEndOfWord(1))
            .chain(DeleteSelection)
            .chain(SwitchMode(EditorMode::Normal)),
    );

    // Add db (delete backward to start of word) - delete from cursor back to start of previous word
    key_handler.insert(
        KeyEventRegister::n(vec![KeyEvent::Char('d'), KeyEvent::Char('b')]),
        Composed::new(SwitchMode(EditorMode::Visual))
            .chain(MoveWordBackward(1))
            .chain(DeleteSelection)
            .chain(SwitchMode(EditorMode::Normal)),
    );

    // Add diw (delete inner word) - delete just the word, no surrounding whitespace
    key_handler.insert(
        KeyEventRegister::n(vec![KeyEvent::Char('d'), KeyEvent::Char('i'), KeyEvent::Char('w')]),
        Composed::new(SelectInnerWord).chain(DeleteSelection),
    );

    // Add ^ (caret) to go to first non-blank character of line (same as _)
    // This is the standard vim binding that edtui doesn't include by default
    key_handler.insert(
        KeyEventRegister::n(vec![KeyEvent::Char('^')]),
        Action::from(MoveToFirst()),
    );
    key_handler.insert(
        KeyEventRegister::v(vec![KeyEvent::Char('^')]),
        Action::from(MoveToFirst()),
    );

    // Add dG (delete from cursor to end of buffer)
    // In vim, dG deletes from the current line to the end of the file
    key_handler.insert(
        KeyEventRegister::n(vec![KeyEvent::Char('d'), KeyEvent::Char('G')]),
        Composed::new(SwitchMode(EditorMode::Visual))
            .chain(MoveToLastRow())
            .chain(MoveToEndOfLine())
            .chain(DeleteSelection)
            .chain(SwitchMode(EditorMode::Normal)),
    );

    // Add dgg (delete from cursor to beginning of buffer)
    // In vim, dgg deletes from the current line to the beginning of the file
    key_handler.insert(
        KeyEventRegister::n(vec![KeyEvent::Char('d'), KeyEvent::Char('g'), KeyEvent::Char('g')]),
        Composed::new(SwitchMode(EditorMode::Visual))
            .chain(MoveToFirstRow())
            .chain(MoveToStartOfLine())
            .chain(DeleteSelection)
            .chain(SwitchMode(EditorMode::Normal)),
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
            help_scroll_offset: 0,
            pending_confirmation: None,
            confirmation_scroll_offset: 0,
            status_message: None,
            status_message_decay: 0,
            editing_task_id: None,
            title_scroll_offset: 0,
            title_scroll_delay: 0,
            pending_images: Vec::new(),
            animation_frame: 0,
            column_scroll_offsets: [0; 6],
            queue_dialog_task_id: None,
            queue_dialog_selected_idx: 0,
            show_task_preview: false,
            task_detail_tab: TaskDetailTab::default(),
            interactive_modal: None,
            open_project_dialog_slot: None,
            directory_browser: None,
            create_folder_input: None,
            feedback_task_id: None,
            logo_shimmer_frame: 0,
            // Mascot eye animation: start with normal eyes, trigger first animation in ~30-90 seconds
            eye_animation: EyeAnimation::Normal,
            eye_animation_ticks_remaining: 0,
            eye_animation_cooldown: 300 + (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| (d.as_millis() % 600) as u16)
                .unwrap_or(0)), // 30-90 seconds (300-900 ticks at 100ms each)
            // Show startup hints for first ~10 seconds (100 ticks at 100ms each)
            startup_hint_until_tick: Some(100),
            selected_project_tab_idx: 0,
            consecutive_esc_count: 0,
            config_modal: None,
            show_stash_modal: false,
            stash_modal_selected_idx: 0,
            git_diff_scroll_offset: 0,
            git_diff_cache: None,
            // Welcome panel: start at first message, rotate every ~8 seconds
            welcome_message_idx: 0,
            welcome_message_cooldown: 80,
            welcome_bubble_focused: false,
        }
    }
}

impl UiState {
    /// Check if the configuration modal is open
    pub fn is_config_modal_open(&self) -> bool {
        self.config_modal.is_some()
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

    /// Check if the open project dialog is open
    pub fn is_open_project_dialog_open(&self) -> bool {
        self.open_project_dialog_slot.is_some()
    }
}

/// A pending confirmation dialog
#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    pub message: String,
    pub action: PendingAction,
    /// Animation tick for the highlight sweep effect (starts at 20, counts down to 0)
    pub animation_tick: usize,
}

/// Actions that require user confirmation
#[derive(Debug, Clone)]
pub enum PendingAction {
    DeleteTask(Uuid),
    /// Mark task as done and clean up worktree (when nothing to merge)
    MarkDoneNoMerge(Uuid),
    CloseProject(usize),
    /// Accept task: merge changes and mark as done
    AcceptTask(Uuid),
    /// Decline task: discard changes and mark as done
    DeclineTask(Uuid),
    /// Clean up a task that was already merged (user confirmed after seeing report)
    CleanupMergedTask(Uuid),
    /// View-only merge report (no action on confirm, just dismiss)
    ViewMergeReport,
    /// Commit applied changes to main and complete the task
    CommitAppliedChanges(Uuid),
    /// Reset task: clean up worktree and move back to Planned
    ResetTask(Uuid),
    /// Force unapply using destructive reset (after surgical reversal failed)
    ForceUnapply(Uuid),
    /// Stash conflict options: y=solve with Claude, n=unapply, k=keep markers
    StashConflict { task_id: Uuid, stash_sha: String },
    /// Merge only: merge changes to main but keep worktree and task in Review
    MergeOnlyTask(Uuid),
    /// Interrupt SDK session to open CLI terminal (y=interrupt, n=cancel)
    InterruptSdkForCli(Uuid),
    /// SDK is working, user wants to send feedback (i=interrupt, w=wait, n=cancel)
    /// Stores task_id and the feedback text to send
    InterruptSdkForFeedback { task_id: Uuid, feedback: String },
    /// CLI is working, user wants to send feedback (i=interrupt, w=wait, o=open CLI, n=cancel)
    /// Stores task_id and the feedback text to send
    InterruptCliForFeedback { task_id: Uuid, feedback: String },
    /// Main worktree has uncommitted changes before merge
    /// Options: c=commit, s=stash, n=cancel
    DirtyMainBeforeMerge { task_id: Uuid },
    /// Offer to pop a tracked stash (after unapply or merge)
    /// Options: y=pop, n=skip
    PopTrackedStash { stash_sha: String },
    /// Project directory is not a git repository
    /// Options: y=initialize git, n=cancel
    InitGit { path: PathBuf, name: String, slot: usize },
    /// Git repository has no commits
    /// Options: y=create initial commit, n=cancel
    CreateInitialCommit { path: PathBuf, name: String, slot: usize },
    /// Apply conflict - show conflict details in scrollable modal
    /// Options: y=try smart apply with Claude, n=cancel
    ApplyConflict { task_id: Uuid, conflict_output: String },
    /// Project .gitignore is missing KanBlam entries (.claude/, worktrees/)
    /// Options: y=add entries, n=open anyway without adding
    UpdateGitignore {
        path: PathBuf,
        name: String,
        slot: usize,
        missing_entries: Vec<String>,
    },
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
    /// For needs-input events: "idle" (from idle_prompt) or "permission" (from permission_prompt)
    #[serde(default)]
    pub input_type: String,
}

// ============================================================================
// Per-Project Task Storage
// ============================================================================

/// Data stored in `.kanblam/tasks.json` within each project directory.
/// This keeps task state with the project, version-controlled and portable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectTaskData {
    /// Version for future migrations
    #[serde(default = "default_version")]
    pub version: u32,
    /// All tasks for this project
    pub tasks: Vec<Task>,
    /// Task ID whose changes are currently applied to main worktree
    #[serde(default)]
    pub applied_task_id: Option<Uuid>,
    /// Stash ref for unapply (legacy, kept for compatibility)
    #[serde(default)]
    pub applied_stash_ref: Option<String>,
    /// Custom commands for this project
    #[serde(default)]
    pub commands: ProjectCommands,
}

fn default_version() -> u32 { 1 }

impl Default for ProjectTaskData {
    fn default() -> Self {
        Self {
            version: 1,
            tasks: Vec::new(),
            applied_task_id: None,
            applied_stash_ref: None,
            commands: ProjectCommands::default(),
        }
    }
}

impl ProjectTaskData {
    /// Get the path to the tasks file for a project
    pub fn file_path(project_dir: &PathBuf) -> PathBuf {
        project_dir.join(".kanblam").join("tasks.json")
    }

    /// Load task data from a project directory.
    /// Returns default data if file doesn't exist.
    pub fn load(project_dir: &PathBuf) -> Self {
        let path = Self::file_path(project_dir);
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    match serde_json::from_str(&content) {
                        Ok(data) => return data,
                        Err(e) => {
                            eprintln!("Warning: Failed to parse {}: {}", path.display(), e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Warning: Failed to read {}: {}", path.display(), e);
                }
            }
        }
        Self::default()
    }

    /// Save task data to the project directory.
    /// Creates the .kanblam directory if it doesn't exist.
    pub fn save(&self, project_dir: &PathBuf) -> std::io::Result<()> {
        let kanblam_dir = project_dir.join(".kanblam");
        std::fs::create_dir_all(&kanblam_dir)?;

        let path = Self::file_path(project_dir);
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, content)
    }
}

impl Project {
    /// Load tasks and related data from the project's .kanblam directory.
    /// Call this when opening or switching to a project.
    pub fn load_tasks(&mut self) {
        let data = ProjectTaskData::load(&self.working_dir);
        self.tasks = data.tasks;
        self.applied_task_id = data.applied_task_id;
        self.applied_stash_ref = data.applied_stash_ref;
        self.commands = data.commands;

        // Regenerate worktree paths (they're not persisted, derived from project_dir + task_id)
        for task in &mut self.tasks {
            if task.git_branch.is_some() {
                let worktree_path = self.working_dir
                    .join("worktrees")
                    .join(format!("task-{}", task.id));
                if worktree_path.exists() {
                    task.worktree_path = Some(worktree_path);
                } else {
                    // Worktree was deleted, clear the reference
                    task.worktree_path = None;
                    task.git_branch = None;
                }
            }
        }
    }

    /// Save tasks and related data to the project's .kanblam directory.
    /// Call this periodically and when closing a project.
    pub fn save_tasks(&self) -> std::io::Result<()> {
        let data = ProjectTaskData {
            version: 1,
            tasks: self.tasks.clone(),
            applied_task_id: self.applied_task_id,
            applied_stash_ref: self.applied_stash_ref.clone(),
            commands: self.commands.clone(),
        };
        data.save(&self.working_dir)
    }
}
