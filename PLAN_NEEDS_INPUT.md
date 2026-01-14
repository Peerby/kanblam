# Plan: Automatic "Needs Input" Detection

## Goal
Automatically detect when Claude Code requires user input and move the task from "In Progress" to "Needs Input" status.

## Research Summary

### Available Hook Events
Based on [Claude Code Hooks documentation](https://code.claude.com/docs/en/hooks):

| Hook Event | When it Fires |
|------------|---------------|
| `Stop` | When Claude finishes responding |
| `Notification` | When Claude sends notifications (permission requests, idle timeout) |
| `SessionEnd` | When the session terminates |
| `PreToolUse` | Before a tool is executed |
| `PostToolUse` | After a tool completes |

### Notification Hook - Key for "Needs Input"
The `Notification` hook is ideal for detecting when Claude needs user input. It supports matchers:

- **`permission_prompt`** - Claude needs permission to use a tool
- **`idle_prompt`** - Claude has been waiting for input 60+ seconds
- **`elicitation_dialog`** - MCP tool needs user input

### Stdin Data Format
Hooks receive JSON via stdin:
```json
{
  "session_id": "abc123",
  "transcript_path": "/path/to/transcript.jsonl",
  "cwd": "/project/directory",
  "hook_event_name": "Notification",
  "message": "Claude needs your permission to use Bash",
  "notification_type": "permission_prompt"
}
```

### Known Issues (from GitHub issues)
1. [#11964](https://github.com/anthropics/claude-code/issues/11964) - `notification_type` field sometimes missing
2. [#8320](https://github.com/anthropics/claude-code/issues/8320) - Idle notifications not always triggering
3. Workaround: Parse `message` field as fallback

---

## Implementation Plan

### Phase 1: Add Notification and UserPromptSubmit Hook Support

**1.1 Update hook installer** (`src/hooks/installer.rs`)
```rust
// Add Notification hook - detects when Claude needs input
let notification_hook = json!([
    {
        "matcher": "permission_prompt",
        "hooks": [{
            "type": "command",
            "command": format!("{} hook-signal --event=needs-input --type=permission", kanclaude_bin)
        }]
    },
    {
        "matcher": "idle_prompt",
        "hooks": [{
            "type": "command",
            "command": format!("{} hook-signal --event=needs-input --type=idle", kanclaude_bin)
        }]
    }
]);

// Add UserPromptSubmit hook - detects when user provides input
// This moves task from "Needs Input" back to "In Progress"
let user_prompt_hook = json!([{
    "hooks": [{
        "type": "command",
        "command": format!("{} hook-signal --event=input-provided", kanclaude_bin)
    }]
}]);
```

**1.2 Update signal file format** (`src/hooks/watcher.rs`)
```rust
pub struct HookSignalFile {
    pub event: String,           // "stop", "end", "needs-input"
    pub session_id: String,
    pub project_dir: PathBuf,
    pub timestamp: String,
    pub reason: String,
    pub notification_type: Option<String>,  // NEW: "permission", "idle", etc.
    pub message: Option<String>,            // NEW: Original notification message
}
```

**1.3 Add WatcherEvent variant**
```rust
pub enum WatcherEvent {
    ClaudeStopped { ... },
    SessionEnded { ... },
    NeedsInput {                    // NEW
        session_id: String,
        project_dir: PathBuf,
        input_type: String,         // "permission", "idle", "elicitation"
        message: Option<String>,
    },
    InputProvided {                 // NEW - user submitted input
        session_id: String,
        project_dir: PathBuf,
    },
    Error(String),
}
```

### Phase 2: Handle "Needs Input" Events

**2.1 Update main.rs hook signal handler**
```rust
fn handle_hook_signal(args: &[String]) -> anyhow::Result<()> {
    // Parse --event and --type arguments
    // For "needs-input" events, also capture notification_type and message
    // Write signal file with additional fields
}
```

**2.2 Update app.rs message handler**
```rust
Message::HookSignalReceived(signal) => {
    match signal.event.as_str() {
        "stop" => {
            // Current behavior: move to Review (task completed)
            task.status = TaskStatus::Review;
        }
        "needs-input" => {
            // NEW: move to NeedsInput (waiting for user)
            task.status = TaskStatus::NeedsInput;
            project.needs_attention = true;
            notify::play_attention_sound();
            notify::set_attention_indicator(&project.name);
        }
        "input-provided" => {
            // NEW: user provided input, move back to InProgress
            // Only if task is currently in NeedsInput state
            if task.status == TaskStatus::NeedsInput {
                task.status = TaskStatus::InProgress;
                project.needs_attention = false;
                notify::clear_attention_indicator();
            }
        }
        "end" => { ... }
    }
}
```

### Phase 3: Distinguish "Completed" vs "Needs Input" on Stop

Currently, `Stop` always moves tasks to Review. We should analyze the context:

**Option A: Use Notification Hook Only (Recommended)**
- Keep `Stop` -> Review behavior
- Use `Notification` hook for NeedsInput
- Simpler, relies on Claude Code's native detection

**Option B: Analyze Transcript on Stop**
- Read transcript file from `transcript_path`
- Check if last Claude message ends with a question
- More complex, requires transcript parsing

**Recommendation:** Start with Option A (Notification hook only). If idle_prompt notifications work reliably, this is sufficient. Can add transcript analysis later if needed.

### Phase 4: UI Behavior for "Needs Input"

**4.1 Enter key behavior**
- In NeedsInput column: Enter should restart/continue the task (like Review)
- Sends the task title/description back to Claude

**4.2 Visual indicators**
- Red color (already implemented)
- Badge showing count (like Review column)
- Sound notification when task enters NeedsInput

---

## Data Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Claude Code                                  │
│                                                                      │
│   [Working on task...]                                              │
│              │                                                       │
│              ▼                                                       │
│   ┌─────────────────────┐    ┌─────────────────────┐               │
│   │ Completes response  │    │ Needs user input    │               │
│   │ (Stop hook fires)   │    │ (Notification hook) │               │
│   └─────────┬───────────┘    └─────────┬───────────┘               │
│             │                          │                            │
│             │                          ▼                            │
│             │                ┌─────────────────────┐               │
│             │                │ User provides input │               │
│             │                │(UserPromptSubmit)   │               │
│             │                └─────────┬───────────┘               │
└─────────────┼──────────────────────────┼────────────────────────────┘
              │                          │
              ▼                          ▼
    ┌─────────────────┐        ┌─────────────────────────┐
    │ kanclaude       │        │ kanclaude hook-signal   │
    │ hook-signal     │        │ --event=needs-input     │
    │ --event=stop    │        │ --event=input-provided  │
    └────────┬────────┘        └───────────┬─────────────┘
             │                             │
             ▼                             ▼
    ┌─────────────────────────────────────────────────────┐
    │              ~/.kanclaude/signals/                   │
    │   signal-stop-*.json                                │
    │   signal-needs-input-*.json                         │
    │   signal-input-provided-*.json                      │
    └─────────────────────┬───────────────────────────────┘
                          │
                          ▼ (file watcher)
    ┌─────────────────────────────────────────────────────┐
    │                  KanClaude TUI                       │
    │                                                      │
    │   Stop event      → Task moves to "Review"          │
    │   NeedsInput      → Task moves to "Needs Input"     │
    │   InputProvided   → Task moves to "In Progress"     │
    └─────────────────────────────────────────────────────┘
```

## Complete Task Lifecycle

```
┌─────────┐     Start      ┌─────────────┐
│ Planned │ ─────────────► │ In Progress │ ◄─────────────┐
└─────────┘                └──────┬──────┘               │
                                  │                      │
                    ┌─────────────┼─────────────┐        │
                    │             │             │        │
                    ▼             ▼             │        │
           ┌────────────┐  ┌─────────────┐     │        │
           │   Review   │  │ Needs Input │ ────┘        │
           │(completed) │  │  (waiting)  │              │
           └─────┬──────┘  └─────────────┘              │
                 │              │                        │
                 │              │ (UserPromptSubmit)     │
                 │              └────────────────────────┘
                 ▼
           ┌──────────┐
           │   Done   │
           └──────────┘
```

---

## Implementation Checklist

- [ ] **Phase 1: Hook Infrastructure**
  - [ ] Add `notification_type` and `message` fields to `HookSignalFile`
  - [ ] Add `NeedsInput` variant to `WatcherEvent`
  - [ ] Add `InputProvided` variant to `WatcherEvent`
  - [ ] Update `install_hooks()` to add Notification hook
  - [ ] Update `install_hooks()` to add UserPromptSubmit hook
  - [ ] Update `hooks_installed()` to check for new hooks
  - [ ] Update CLI `hook-signal` to accept `--type` argument

- [ ] **Phase 2: Event Handling**
  - [ ] Handle `needs-input` event in `convert_watcher_event()`
  - [ ] Handle `input-provided` event in `convert_watcher_event()`
  - [ ] Handle `needs-input` in `app.update()` - move task to NeedsInput status
  - [ ] Handle `input-provided` in `app.update()` - move task back to InProgress

- [ ] **Phase 3: UI Polish**
  - [ ] Add badge count for NeedsInput column (like Review)
  - [ ] Ensure Enter key works to restart task from NeedsInput
  - [ ] Test notification sound plays

- [ ] **Phase 4: Testing**
  - [ ] Test with permission prompts (use a tool that requires permission)
  - [ ] Test with idle timeout (wait 60+ seconds)
  - [ ] Test full cycle: Planned -> InProgress -> NeedsInput -> InProgress -> Review -> Done

---

## Future Enhancements

1. **Transcript Analysis** - Parse transcript to show why Claude needs input
2. **Quick Actions** - Allow responding to permission prompts from KanClaude
3. **Auto-approve** - Option to auto-approve certain tool permissions
4. **Notification Type Display** - Show "Permission needed" vs "Waiting for input" in UI

---

## Sources
- [Claude Code Hooks Reference](https://code.claude.com/docs/en/hooks)
- [GitHub Issue #11964 - notification_type field missing](https://github.com/anthropics/claude-code/issues/11964)
- [GitHub Issue #8320 - Idle notifications not triggering](https://github.com/anthropics/claude-code/issues/8320)
