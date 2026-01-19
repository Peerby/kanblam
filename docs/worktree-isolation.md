# Design: Worktree-Based Task Isolation

## Overview

Each task runs in an isolated git worktree with its own Claude Code session in tmux. This provides:
- Complete filesystem isolation between concurrent tasks
- Clean git history per task for review/accept/discard
- Persistent sessions that survive Kanblam restarts

## Architecture

```
~/.kanblam/
├── worktrees/
│   └── {project-slug}/
│       ├── task-abc123/          ← Git worktree (branch: claude/abc123)
│       │   ├── .claude/
│       │   │   └── settings.json ← Isolated Claude settings
│       │   └── ... project files
│       └── task-def456/
└── state/
    └── {project-slug}/
        └── tasks.json            ← Task metadata including session info

Main Project (/path/to/project)
├── .git/                         ← Shared by all worktrees
├── .claude/settings.json         ← Original project settings
└── ... project files
```

## Tmux Session Structure

```
┌─────────────────────────────────────────────────────────────┐
│ tmux session: kc-{project-slug}                             │
├─────────────────────────────────────────────────────────────┤
│ window 0: task-abc123  │ window 1: task-def456  │ ...       │
│ (cwd: worktree-abc123) │ (cwd: worktree-def456) │           │
│                        │                         │           │
│ Claude session running │ Claude waiting input   │           │
└─────────────────────────────────────────────────────────────┘
```

Each project gets ONE tmux session with multiple windows (one per active task).
This makes it easy to switch between tasks and keeps things organized.

## Claude Session Lifecycle

```
┌──────────────┐
│  NotStarted  │ Task created, no worktree yet
└──────┬───────┘
       │ User starts task (Enter on Planned)
       ▼
┌──────────────┐
│   Creating   │ Creating worktree, tmux window, starting claude
└──────┬───────┘
       │ Claude prompt detected
       ▼
┌──────────────┐
│    Ready     │ Claude ready, sending task description
└──────┬───────┘
       │ Task sent
       ▼
┌──────────────┐
│   Working    │ Claude is processing (InProgress column)
└──────┬───────┘
       │ Stop hook received
       ▼
┌──────────────┐
│   Paused     │ Claude waiting for input (Review column)
└──────┬───────┘
       │ User continues (c key)
       ▼
┌──────────────┐
│  Continuing  │ User interacting in tmux, or sent follow-up
└──────┬───────┘
       │ User accepts/discards
       ▼
┌──────────────┐
│   Cleanup    │ Merge/delete branch, remove worktree, close window
└──────────────┘
```

## Readiness Detection

### Problem
Claude Code takes a few seconds to start. We need to know when it's ready before sending the task.

### Solution: Watch for prompt pattern
```rust
async fn wait_for_claude_ready(session: &str, window: &str, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    loop {
        let output = capture_pane(session, window)?;

        // Claude Code shows ">" when ready for input
        // Also check for the welcome message pattern
        if output.contains("> ") || output.contains("What would you like to do?") {
            return Ok(());
        }

        if start.elapsed() > timeout {
            return Err(Error::ClaudeStartTimeout);
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
```

### Fallback: Fixed delay with verification
If pattern matching fails, wait 3 seconds then verify claude process exists.

## Claude Isolation Strategy

### Directory Restriction
Claude Code respects its working directory. By starting it in the worktree:
```bash
cd /path/to/worktree && claude
```
Claude will naturally operate within that directory.

### Settings Isolation
Create `.claude/settings.json` in each worktree:
```json
{
  "permissions": {
    "allow": [
      "Bash(*)",
      "Read(*)",
      "Edit(*)",
      "Write(*)",
      "Grep(*)",
      "Glob(*)"
    ]
  },
  "includeCoAuthoredBy": true,
  "hooks": {
    "Stop": [{
      "type": "command",
      "command": "kanblam signal stop $CLAUDE_SESSION_ID"
    }]
  }
}
```

### Environment Variables
Set these when spawning Claude:
```bash
CLAUDE_WORKING_DIR=/path/to/worktree
KANBLAM_TASK_ID=abc123
KANBLAM_PROJECT=my-project
```

### Trust Boundary
We trust Claude to:
- Respect the working directory
- Not `cd` out of the worktree
- Focus on the task at hand

We enforce:
- Worktree is the cwd
- Git branch is task-specific
- Hooks notify Kanblam of completion

## Key Operations

### 1. Start Task
```rust
async fn start_task(&mut self, task_id: Uuid) -> Result<()> {
    let task = self.get_task_mut(task_id)?;
    let project = self.current_project()?;

    // 1. Create worktree
    let worktree_path = self.create_worktree(&project, task_id)?;

    // 2. Setup isolated Claude settings
    self.setup_claude_settings(&worktree_path, task_id)?;

    // 3. Create/get tmux session and window
    let tmux_session = format!("kc-{}", project.slug);
    let window_name = format!("task-{}", task_id.to_string()[..8]);
    self.create_tmux_window(&tmux_session, &window_name, &worktree_path)?;

    // 4. Start Claude
    self.tmux_send_keys(&tmux_session, &window_name, "claude\n")?;

    // 5. Wait for ready
    self.wait_for_claude_ready(&tmux_session, &window_name).await?;

    // 6. Send task description
    let prompt = self.format_task_prompt(task);
    self.tmux_send_keys(&tmux_session, &window_name, &prompt)?;
    self.tmux_send_keys(&tmux_session, &window_name, "\n")?;

    // 7. Update state
    task.status = TaskStatus::InProgress;
    task.worktree_path = Some(worktree_path);
    task.tmux_window = Some(window_name);
    task.session_state = ClaudeSessionState::Working;

    Ok(())
}
```

### 2. Continue Task (from Review)
```rust
async fn continue_task(&mut self, task_id: Uuid) -> Result<()> {
    let task = self.get_task(task_id)?;
    let project = self.current_project()?;

    // Check if tmux window still exists
    let tmux_session = format!("kc-{}", project.slug);
    let window = task.tmux_window.as_ref().ok_or(Error::NoSession)?;

    if !self.tmux_window_exists(&tmux_session, window)? {
        // Session died, need to restart
        return self.restart_task(task_id).await;
    }

    // Focus the window (user will interact directly)
    self.tmux_select_window(&tmux_session, window)?;

    // Optionally attach if running in terminal
    // Or just update UI to show "Session active - press 'a' to attach"

    task.status = TaskStatus::InProgress;
    task.session_state = ClaudeSessionState::Continuing;

    Ok(())
}
```

### 3. Accept Task
```rust
async fn accept_task(&mut self, task_id: Uuid) -> Result<()> {
    let task = self.get_task(task_id)?;
    let project = self.current_project()?;

    // 1. Close tmux window
    if let Some(window) = &task.tmux_window {
        let session = format!("kc-{}", project.slug);
        self.tmux_kill_window(&session, window)?;
    }

    // 2. Merge branch to main (or user's base branch)
    let branch = format!("claude/{}", task_id);
    self.git_merge_squash(&project.path, &branch)?;

    // 3. Remove worktree
    if let Some(worktree) = &task.worktree_path {
        self.git_worktree_remove(worktree)?;
    }

    // 4. Delete branch
    self.git_branch_delete(&project.path, &branch)?;

    // 5. Update task status
    task.status = TaskStatus::Done;
    task.worktree_path = None;
    task.tmux_window = None;

    Ok(())
}
```

### 4. Discard Task
```rust
async fn discard_task(&mut self, task_id: Uuid) -> Result<()> {
    // Same as accept but skip the merge step
    // Just cleanup worktree and branch
}
```

## Data Model Updates

```rust
// src/model/task.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub images: Vec<PathBuf>,

    // Session management
    pub worktree_path: Option<PathBuf>,
    pub tmux_window: Option<String>,
    pub session_state: ClaudeSessionState,
    pub git_branch: Option<String>,

    // Timestamps
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaudeSessionState {
    NotStarted,
    Creating,
    Ready,
    Working,
    Paused,      // Waiting for user input (in Review)
    Continuing,  // User is interacting
    Ended,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub path: PathBuf,
    pub tasks: Vec<Task>,
    pub tmux_session: Option<String>,  // kc-{slug}
}
```

## UI Updates

### Kanban Board
- Show session state indicator on task cards
- InProgress: 🔄 or spinner
- Review (Paused): ⏸️
- Review (Continuing): 🔄

### Keyboard Shortcuts (Review column)
- `Enter` or `c`: Continue task (focus tmux window)
- `a`: Attach to tmux session in terminal
- `y`: Accept (merge + cleanup)
- `n` or `d`: Discard (cleanup only)
- `p`: Move back to Planned (pause but keep worktree)

### Status Bar
- Show active Claude sessions count
- Show if any task needs attention

## File Structure

```
src/
├── worktree/
│   ├── mod.rs           # Worktree lifecycle management
│   ├── git.rs           # Git worktree commands
│   └── settings.rs      # Claude settings generation
├── session/
│   ├── mod.rs           # Claude session orchestration
│   ├── tmux.rs          # Tmux window management
│   ├── readiness.rs     # Claude ready detection
│   └── lifecycle.rs     # Start/continue/accept/discard
└── signal/
    ├── mod.rs           # Hook signal handling
    └── watcher.rs       # File system watcher for signals
```

## Error Handling

### Worktree Creation Fails
- Check if branch already exists (stale from crashed session)
- Offer to clean up and retry

### Claude Won't Start
- Timeout after 30 seconds
- Show error, offer to retry or open terminal manually

### Tmux Session Dies
- Detect on next poll/interaction
- Offer to restart task (creates new session in same worktree)

### Merge Conflicts
- Alert user
- Offer to open in terminal for manual resolution
- Keep task in Review until resolved

## Implementation Phases

### Phase 1: Core Worktree Management
- [ ] Git worktree create/remove functions
- [ ] Worktree path configuration
- [ ] Claude settings generation for worktree

### Phase 2: Tmux Session Management
- [ ] Session/window creation
- [ ] Readiness detection
- [ ] Send keys / capture output

### Phase 3: Task Lifecycle Integration
- [ ] Start task → create worktree → start Claude
- [ ] Hook signals → update task state
- [ ] Continue task → focus window
- [ ] Accept/discard → merge/cleanup

### Phase 4: UI Updates
- [ ] Session state indicators
- [ ] New keyboard shortcuts
- [ ] Error state display

## Security Considerations

1. **Worktree paths**: Use sanitized task IDs, no user input in paths
2. **Shell injection**: Quote all paths and user content properly
3. **Git operations**: Validate branch names
4. **Cleanup**: Always cleanup on discard, even partial failures
