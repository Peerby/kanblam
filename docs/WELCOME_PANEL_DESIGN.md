# Welcome Panel Design

## Overview

When no projects are loaded, replace the empty kanban board with a welcoming, interactive onboarding experience featuring the mascot as a guide.

## Design Goals

1. **Warm Welcome**: Make first-time users feel welcomed, not lost
2. **Progressive Disclosure**: Show just enough to get started, with help available on demand
3. **Personality**: Use the mascot's animations to create delight and guide attention
4. **Clear CTA**: Make the primary action (open a project) obvious and easy
5. **Teach by Showing**: Preview what the app does through visual hints

---

## Visual Design

### Layout (Full Terminal - 80+ cols, 30+ rows)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ [!] +project │                                         ▄▓▓▓▓▄    █ █ ▄▀█... │
│                                                        ▓ ▪▪ ▓▒▒  █▀▄ █▀█... │
│                                                        ▓▓▓▓▓▓▒   █ █ █ █... │
├─────────────────────────────────────────────────────────────────▀▀─▀▀───────┤
│                                                                             │
│                           ┌──────────────────────────────┐                  │
│                           │  Welcome to KanBlam!         │                  │
│                           │                              │                  │
│         ▄▓▓▓▓▄            │  Orchestrate parallel Claude │                  │
│         ▓ ▪▪ ▓▒▒  ◄───────│  sessions with kanban flow.  │                  │
│         ▓▓▓▓▓▓▒           │                              │                  │
│          ▀▀ ▀▀            │  Each task runs in its own   │                  │
│                           │  git worktree - no conflicts │                  │
│                           │  between parallel work!      │                  │
│                           └──────────────────────────────┘                  │
│                                                                             │
│                    ╭─────────────────────────────────────╮                  │
│                    │  Press  !  to open a project        │                  │
│                    │         ─                           │                  │
│                    │  or navigate to [+project] above    │                  │
│                    ╰─────────────────────────────────────╯                  │
│                                                                             │
│                                                                             │
│              ┌─────────────────────────────────────────────┐                │
│              │  Quick Start                                │                │
│              │                                             │                │
│              │   1. Open a git project        [!]         │                │
│              │   2. Create a task             [i]         │                │
│              │   3. Start Claude session      [s]         │                │
│              │   4. Review & merge changes    [a]         │                │
│              │                                             │                │
│              │  Press [?] anytime for full help           │                │
│              └─────────────────────────────────────────────┘                │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ New Task                                               i insert  ^V img     │
├─────────────────────────────────────────────────────────────────────────────┤
│ ↑↓←→ navigate   enter select   ! open project   ? help                      │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Layout (Medium Terminal - 60-80 cols)

```
┌────────────────────────────────────────────────────────┐
│ [!] +project │                       ▄▓▓▓▄   KB       │
├────────────────────────────────────▀▀─▀▀───────────────┤
│                                                        │
│                  Welcome to KanBlam!                   │
│                                                        │
│       ▄▓▓▓▓▄     Orchestrate parallel Claude          │
│       ▓ ▪▪ ▓▒▒   sessions with kanban flow.           │
│       ▓▓▓▓▓▓▒                                         │
│        ▀▀ ▀▀     Each task runs in its own            │
│                  isolated git worktree.               │
│                                                        │
│         ╭────────────────────────────────╮            │
│         │  Press  !  to open a project   │            │
│         ╰────────────────────────────────╯            │
│                                                        │
│         Quick Start:                                   │
│          [!] Open project  [i] New task               │
│          [s] Start Claude  [?] Help                   │
│                                                        │
├────────────────────────────────────────────────────────┤
│ New Task                                               │
├────────────────────────────────────────────────────────┤
│ ! open project   ? help                                │
└────────────────────────────────────────────────────────┘
```

### Layout (Compact Terminal - <60 cols)

```
┌──────────────────────────────────────┐
│ [!] +project                  KANBLAM│
├──────────────────────────────────────┤
│                                      │
│       Welcome to KanBlam!            │
│                                      │
│  ▄▓▓▓▓▄   Parallel Claude sessions   │
│  ▓ ▪▪ ▓▒▒ with kanban task flow.     │
│  ▓▓▓▓▓▓▒                             │
│   ▀▀ ▀▀  Press ! to get started      │
│                                      │
│  [!] Open   [i] Task   [?] Help      │
│                                      │
├──────────────────────────────────────┤
│ New Task                             │
├──────────────────────────────────────┤
│ ! open project   ? help              │
└──────────────────────────────────────┘
```

---

## Mascot Behavior

### Eye Animations for Welcome State

The mascot should feel alive and guide attention:

| Animation | When | Purpose |
|-----------|------|---------|
| `LookUp` | Every ~10s | Looking at the +project button above |
| `Blink` | Every ~8s | Natural blinking |
| `WinkRight` | After 30s idle | Playful encouragement |
| `Wide` | When user presses any key | Alertness |
| `Heart` | When user opens project dialog | Excitement |
| `StarEyes` | When first project is added | Celebration |

### Larger Mascot for Welcome Panel

For the centered welcome mascot, use a 2x scale version:

```
    ▄▄▓▓▓▓▓▓▄▄
   ▓▓        ▓▓▒▒
   ▓▓  ▪  ▪  ▓▓▒▒
   ▓▓   ▀▀   ▓▓▒▒
   ▓▓▓▓▓▓▓▓▓▓▓▓▒▒
    ▀▀▀▀  ▀▀▀▀
```

Or keep standard size but with a speech bubble pointer:

```
    ▄▓▓▓▓▄     ╭─────────────────────────╮
    ▓ ▪▪ ▓▒▒◄──│ Welcome to KanBlam!     │
    ▓▓▓▓▓▓▒    │ Let's get you started.  │
     ▀▀ ▀▀     ╰─────────────────────────╯
```

---

## Speech Bubble System

### Bubble Styles

```rust
pub enum BubbleStyle {
    /// Standard rounded bubble with pointer
    Speech,
    /// Thought bubble with cloud pointer
    Thought,
    /// Highlighted call-to-action box
    Action,
}
```

### Bubble Content Rotation

The mascot cycles through helpful messages:

```rust
const WELCOME_MESSAGES: &[(&str, &str)] = &[
    ("Welcome to KanBlam!", "Orchestrate parallel Claude\nsessions with kanban flow."),
    ("Isolated Worktrees", "Each task gets its own git\nbranch - no conflicts!"),
    ("Ready to start?", "Press ! to open a project\nor create a new one."),
    ("Tip: Parallel Power", "Run multiple Claude sessions\nat once for faster iteration."),
];
```

Messages rotate every ~8 seconds with a smooth transition (fade or slide).

---

## Onboarding States

### State Machine

```rust
pub enum OnboardingState {
    /// First launch, no projects ever added
    FirstLaunch,
    /// Has opened projects before, but none currently loaded
    ReturningEmpty,
    /// Project loaded, but no tasks yet
    ProjectNoTasks,
    /// Has tasks, normal operation
    Active,
}
```

### State-Specific Content

| State | Mascot Message | Primary CTA | Secondary Hints |
|-------|----------------|-------------|-----------------|
| `FirstLaunch` | "Welcome! Let's get you set up..." | `! Open Project` | Show Quick Start guide |
| `ReturningEmpty` | "Welcome back! Open a project to continue." | `! Open Project` | Show recent projects if available |
| `ProjectNoTasks` | "Project loaded! Create your first task." | `i New Task` | Explain task workflow |
| `Active` | (normal kanban view) | — | — |

---

## Implementation Plan

### Phase 1: Core Welcome Panel

1. **New module**: `src/ui/welcome.rs`
   - `render_welcome_panel(frame, area, app)` - main renderer
   - `WelcomeSize` enum (Full, Medium, Compact)
   - Helper functions for responsive layout

2. **Modify `src/ui/mod.rs`**:
   ```rust
   // In view() function, before render_kanban:
   if app.model.projects.is_empty() {
       render_welcome_panel(frame, chunks[1], app);
   } else {
       render_kanban(frame, chunks[1], app);
   }
   ```

3. **New state in `UiState`**:
   ```rust
   pub struct UiState {
       // ... existing fields ...

       /// Current welcome message index (for rotation)
       pub welcome_message_idx: usize,
       /// Ticks until next message rotation
       pub welcome_message_cooldown: u16,
       /// Onboarding state tracking
       pub onboarding_state: OnboardingState,
   }
   ```

### Phase 2: Mascot Speech Bubbles

1. **New module**: `src/ui/speech_bubble.rs`
   - `render_speech_bubble(frame, area, content, style, pointer_side)`
   - Box-drawing characters for bubble outline
   - Word wrapping for content

2. **Mascot positioning**:
   - Welcome panel: Mascot on left, bubble on right
   - Pointer connects mascot face to bubble

### Phase 3: Contextual Eye Animations

1. **New eye animation triggers in `src/app.rs`**:
   ```rust
   // When on welcome screen, trigger contextual animations
   fn update_welcome_eye_animation(&mut self) {
       if self.model.projects.is_empty() {
           // Periodically look up at +project button
           if self.model.ui_state.animation_frame % 100 == 0 {
               self.trigger_eye_animation(EyeAnimation::LookUp);
           }
       }
   }
   ```

2. **Eye animation on events**:
   - `ShowOpenProjectDialog` → `Heart` eyes
   - `ProjectAdded` → `StarEyes` (animated)
   - Any keypress on welcome → `Wide` briefly

### Phase 4: Progressive Hints

1. **Track onboarding progress**:
   ```rust
   pub struct OnboardingProgress {
       pub has_opened_project: bool,
       pub has_created_task: bool,
       pub has_started_session: bool,
       pub has_merged_task: bool,
   }
   ```

2. **Contextual status bar messages**:
   - Replace generic hints with step-specific guidance
   - "Press ! to open your first project"
   - "Press i to create your first task"
   - etc.

---

## ASCII Art Components

### Speech Bubble (Right-pointing)

```
╭────────────────────────────╮
│ Message content here       │
│ with multiple lines        │
╰────────────────────────────╯
```

With pointer:
```
    ╭────────────────────────────╮
◄───│ Message content here       │
    │ with multiple lines        │
    ╰────────────────────────────╯
```

### Action Box (Highlighted CTA)

```
┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃  Press  !  to open project ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

### Quick Start Card

```
╔═══════════════════════════════════════╗
║  Quick Start                          ║
╟───────────────────────────────────────╢
║   1. Open a git project        [!]    ║
║   2. Create a task             [i]    ║
║   3. Start Claude session      [s]    ║
║   4. Review & merge changes    [a]    ║
╟───────────────────────────────────────╢
║  Press [?] anytime for full help      ║
╚═══════════════════════════════════════╝
```

---

## Color Scheme

| Element | Color | Notes |
|---------|-------|-------|
| Welcome title | `Rgb(80, 200, 120)` (KanBlam green) | Bold |
| Mascot | Warm gradient (yellow→orange→red→magenta) | Same as header |
| Speech bubble border | `Cyan` | Matches focused elements |
| CTA box border | `Yellow` | High visibility |
| Quick start keys | `Cyan` bold | Consistent with hints |
| Body text | `White` / `Gray` | Readable |
| Secondary text | `DarkGray` | De-emphasized |

---

## Responsive Breakpoints

| Terminal Size | Layout |
|---------------|--------|
| ≥100 cols, ≥35 rows | Full layout with large mascot, speech bubble, quick start card |
| ≥70 cols, ≥25 rows | Medium layout with standard mascot, inline text |
| ≥50 cols, ≥20 rows | Compact layout, minimal text, key shortcuts only |
| <50 cols or <20 rows | Minimal: just "Press ! to start" + mascot |

---

## Files to Create/Modify

### New Files
- `src/ui/welcome.rs` - Welcome panel rendering
- `src/ui/speech_bubble.rs` - Reusable speech bubble widget

### Modified Files
- `src/ui/mod.rs` - Conditionally render welcome vs kanban
- `src/ui/logo.rs` - Add larger mascot variant for welcome
- `src/model/mod.rs` - Add onboarding state tracking
- `src/app.rs` - Add welcome-specific eye animation logic
- `src/ui/status_bar.rs` - Show onboarding-aware hints

---

## Future Enhancements

1. **Recent Projects**: Show last 3 opened projects for quick access
2. **Animated Transitions**: Fade/slide when switching from welcome to kanban
3. **Tutorial Mode**: Step-by-step guided walkthrough with highlights
4. **Keyboard Shortcut Overlay**: Visual keyboard showing available keys
5. **Theme Selection**: Let users pick mascot mood/color on first launch
