# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

KanClaude is a TUI Kanban task manager for orchestrating parallel Claude Code sessions. It manages multiple isolated Claude sessions through git worktrees, with a Rust TUI for task management and a TypeScript sidecar for Claude Code Agent SDK integration.

## Build & Development Commands

```bash
# Build the Rust TUI
cargo build --release

# Run the TUI (also used for hook signals)
cargo run
cargo run -- signal <event> <task-id>  # Hook signal subcommand

# Sidecar (TypeScript)
cd sidecar
npm install
npm run build          # Build with esbuild
npm run dev            # Run with tsx
npm run typecheck      # Type check only
npm run test           # Run SDK integration tests
npm run test:ipc       # Run IPC integration tests
```

## Architecture

### The Elm Architecture (TEA)
The app follows TEA pattern with strict separation:
- **Model** (`src/model/mod.rs`): All application state - `AppModel`, `Project`, `Task`, `UiState`
- **Message** (`src/message.rs`): Exhaustive enum of all possible state changes
- **Update** (`src/app.rs`): Pure state transitions in `App::update(msg) -> Vec<Message>`
- **View** (`src/ui/`): Renders state to terminal via ratatui

### Git Worktree Isolation
Each task runs in its own git worktree (`src/worktree/`):
- Branch naming: `claude/{task-id}`
- Worktree path: `{project_dir}/worktrees/task-{task-id}`
- Tasks are completely isolated - no conflicts between parallel Claude sessions
- Accept merges branch to main; discard deletes worktree and branch

### Sidecar Architecture
TypeScript process (`sidecar/`) communicates with Rust TUI via Unix socket:
- JSON-RPC 2.0 protocol over `~/.kanclaude/sidecar.sock`
- Rust client: `src/sidecar/client.rs` + `src/sidecar/protocol.rs`
- TypeScript server: `sidecar/src/main.ts`
- Session management: `sidecar/src/session-manager.ts` wraps `@anthropic-ai/claude-code` SDK

### Session Management
Two modes for Claude sessions:
- **SDK-managed** (`SessionMode::SdkManaged`): Sidecar controls Claude via SDK
- **CLI-interactive** (`SessionMode::CliInteractive`): User interacts directly via `claude --resume`
- Handoff between modes uses tmux session control

### Key Modules
- `src/tmux/`: Tmux window/pane management for Claude sessions
- `src/hooks/`: Claude Code hook installation and signal processing
- `src/notify/`: Audio notifications and tmux status updates
- `src/image/`: Clipboard image handling for task attachments

### Task Lifecycle States
```
Planned → Queued → InProgress → (NeedsWork) → Review → (Accepting) → Done
                                                    ↓
                                                  Discard
```

### UI Focus Areas
Vim-style navigation between: `KanbanBoard` | `TaskInput` | `ProjectTabs` | `OutputViewer`

## Key Patterns

- Task selection tracked by both index and UUID (`selected_task_idx`, `selected_task_id`)
- State persisted to `.kanblam/tasks.json` per project
- Hook signals processed via filesystem watch (`~/.kanclaude/signals/`)
- Tmux session per project: `kc-{project-slug}`

## Protected Files

**NEVER modify the `.kanblam/` directory or its contents.** This directory stores KanBlam's task state and is managed exclusively by the TUI application. Changes made in worktrees are automatically excluded from merges to prevent stale task data from overwriting current state.

## Git Operations Principles

When working with git repositories, follow these principles strictly:

1. **Surgical Precision** - Never use global resets or discard uncommitted changes blindly. Undo exactly what was done, file by file, line by line. We're dealing with many moving parts and cannot assume anything about repo state beyond what we ourselves changed.

2. **Ask Permission for Destructive Actions** - Any destructive action requires explicit user confirmation. Never auto-destroy data.

3. **Default = Restore Original State** - If an operation fails or user aborts, undo precisely and restore what was there before. The user should end up exactly where they started.

4. **Stashes Are Sacred** - Only remove stashes after successful restore or explicit user confirmation. Stashes exist to keep user's work safe - treat them responsibly.

### Apply/Unapply Pattern

When applying task changes to main worktree:
- Stash user's uncommitted changes before applying
- Apply task patch surgically
- Restore user's changes via stash pop

If stash pop conflicts and user aborts:
1. Surgically reverse the task patch (`git apply -R <patch>`) - only touches task's files
2. Clear conflict state on affected files only
3. Pop the stash (now works because task changes are gone)
4. Result: Repo exactly as before

If surgical reversal fails: Don't auto-destroy. Show user what went wrong, ask what to do. Stash remains safe.
