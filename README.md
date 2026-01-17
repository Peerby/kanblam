# KanBlam!

```
  ▄▓▓▓▓▄      █ █ ▄▀█ █▄ █ ██▄ █   ▄▀█ █▀▄▀█
  ▓ ▀▀ ▓▒▒    █▀▄ █▀█ █ ▀█ █▀▄ █   █▀█ █ ▀ █
  ▓▓▓▓▓▓▒     █ █ █ █ █  █ ██▀ █▄▄ █ █ █   █
   ▀▀ ▀▀
```

> **Open source by [Peerby](https://www.peerby.com)** — We're building the future of sharing. [Join us!](https://www.peerby.com/careers)

**A TUI Kanban board for orchestrating parallel Claude Code sessions**

KanBlam! is a terminal-based task manager that lets you run multiple Claude Code AI coding sessions simultaneously, each in complete isolation through git worktrees. Plan your tasks, queue them up, and watch Claude work on them in parallel—then review, accept, or discard the results. It's vibe coding at the speed of sound.

### The Problem

Running multiple Claude Code sessions is powerful, but it quickly becomes chaotic. You've got Claude working on a feature in one terminal, fixing a bug in another, and refactoring something in a third. Before you know it, you're juggling tmux panes, losing track of which session is doing what, forgetting to check on that task you started an hour ago, and accidentally committing to the wrong branch. Sound familiar?

KanBlam keeps you sane. It gives you a single dashboard to see all your Claude sessions at a glance, tracks their status automatically, and ensures each task runs in complete isolation so nothing steps on anything else.

### Work From Anywhere

Because KanBlam is a TUI (terminal user interface), it scales perfectly to any screen size—including your phone. Fire up a terminal client like Termius, SSH into your dev machine, and manage your Claude sessions from the coffee shop, the couch, or the commute. Queue up tasks, check on progress, review and merge completed work—all from your pocket.

## Features

- **Parallel AI Sessions** — Run multiple Claude Code instances simultaneously, each working on different tasks
- **Git Worktree Isolation** — Each task gets its own worktree and branch, preventing conflicts between parallel sessions
- **Kanban Workflow** — Visual board with columns: Planned → Queued → In Progress → Review → Done
- **SDK Integration** — Deep integration with Claude Code Agent SDK for programmatic session control
- **Interactive Handoff** — Seamlessly switch between automated and interactive Claude sessions
- **Smart Notifications** — Audio alerts and tmux status updates when tasks need attention
- **Image Attachments** — Paste screenshots directly into task descriptions

## How It Works

```
┌─────────────────────────────────────────────────────────────────────┐
│  Your Codebase                                                      │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│   main branch ─────────────────────────────────────────────────►    │
│        │                                                            │
│        ├── worktree: task-abc123 (claude/abc123)                    │
│        │       └── Claude working on "Add dark mode"                │
│        │                                                            │
│        ├── worktree: task-def456 (claude/def456)                    │
│        │       └── Claude working on "Fix login bug"                │
│        │                                                            │
│        └── worktree: task-ghi789 (claude/ghi789)                    │
│                └── Claude working on "Update API docs"              │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

1. **Plan** your tasks on the Kanban board
2. **Queue** them up for Claude to work on
3. **Watch** as Claude works on multiple tasks in parallel (each in isolation)
4. **Review** the changes when Claude finishes
5. **Accept** to merge into main, or **Discard** to throw away the work

## Installation

### Prerequisites

- **Rust** (1.70+) — for building the TUI
- **Node.js** (18+) — for the sidecar process
- **tmux** — for managing Claude sessions
- **Claude Code CLI** — the AI coding assistant

### Build from Source

```bash
# Clone the repository
git clone https://github.com/Peerby/kanblam.git
cd kanblam

# Build the Rust TUI
cargo build --release

# Set up the sidecar
cd sidecar
npm install
npm run build
```

### Run

```bash
# Start KanBlam
cargo run --release
```

## Usage

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `h/j/k/l` | Navigate (vim-style) |
| `Enter` | Select/confirm |
| `n` | New task |
| `q` | Queue task for Claude |
| `s` | Start queued tasks |
| `a` | Accept completed task (merge to main) |
| `d` | Discard task |
| `i` | Enter interactive mode with Claude |
| `Tab` | Switch focus area |
| `?` | Help |

### Task Lifecycle

```
                    ┌──────────┐
                    │ Planned  │  Create tasks, write descriptions
                    └────┬─────┘
                         │ queue
                         ▼
                    ┌──────────┐
                    │  Queued  │  Ready for Claude to pick up
                    └────┬─────┘
                         │ start
                         ▼
                    ┌──────────┐
                    │In Progress│  Claude is working on it
                    └────┬─────┘
                         │
              ┌──────────┴──────────┐
              ▼                     ▼
        ┌──────────┐          ┌──────────┐
        │  Review  │          │Needs Input│  Claude needs help
        └────┬─────┘          └──────────┘
             │
     ┌───────┴───────┐
     ▼               ▼
┌──────────┐   ┌──────────┐
│   Done   │   │ Discarded│
└──────────┘   └──────────┘
```

## Architecture

KanBlam uses a hybrid Rust/TypeScript architecture:

```
┌─────────────────────────────────────────────────────────────────┐
│                        KanBlam TUI (Rust)                       │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐             │
│  │   Kanban    │  │   Worktree  │  │    Tmux     │             │
│  │    Board    │  │   Manager   │  │   Control   │             │
│  └─────────────┘  └─────────────┘  └─────────────┘             │
│                           │                                     │
│                    Unix Socket IPC                              │
│                           │                                     │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │                   Sidecar (TypeScript)                      ││
│  │  ┌─────────────────────────────────────────────────────┐   ││
│  │  │            Claude Code Agent SDK                     │   ││
│  │  │    Session management, tool execution, streaming     │   ││
│  │  └─────────────────────────────────────────────────────┘   ││
│  └─────────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────────┘
```

### The Elm Architecture (TEA)

The TUI follows the Elm Architecture pattern:

- **Model** (`src/model/`) — All application state
- **Message** (`src/message.rs`) — Exhaustive enum of state changes
- **Update** (`src/app.rs`) — Pure state transitions
- **View** (`src/ui/`) — Renders state to terminal

### Key Components

| Component | Description |
|-----------|-------------|
| `src/worktree/` | Git worktree creation and management |
| `src/tmux/` | Tmux session/window/pane control |
| `src/sidecar/` | IPC client for TypeScript sidecar |
| `src/hooks/` | Claude Code hook integration |
| `src/notify/` | Audio and visual notifications |
| `sidecar/` | TypeScript SDK integration |

## Development

```bash
# Run the TUI in development
cargo run

# Run the sidecar in development
cd sidecar && npm run dev

# Type check the sidecar
cd sidecar && npm run typecheck

# Run tests
cd sidecar && npm test
```

## Why "KanBlam"?

It's a **Kan**ban board that goes **Blam!** — because when you're running multiple AI coding sessions in parallel, things get done fast.

Meet **Blam**, our mascot: a friendly little block character who represents the explosive productivity of parallel AI-assisted development.

## License

MIT — see [LICENSE](LICENSE)

---

<p align="center">
<b>Built with ❤️ by <a href="https://www.peerby.com">Peerby</a></b><br>
<i>We're on a mission to build the future of sharing. Interested in AI-powered development?</i><br>
<a href="https://www.peerby.com/careers">We're hiring!</a>
</p>
