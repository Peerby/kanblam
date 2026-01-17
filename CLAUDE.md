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
Planned → Queued → InProgress → (NeedsInput) → Review → (Accepting) → Done
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
