# Research: Git Mechanisms for Isolated Claude Session Changes

**Date:** 2025-01-14
**Status:** Research Complete

## Problem Statement

We need a system where:
1. Each Claude session's changes are recorded separately from the main codebase
2. Multiple Claude instances can work concurrently without interference
3. Changes can be reviewed, applied temporarily, accepted, or discarded
4. Integration into the main branch is clean and controlled

---

## Approach 1: Git Worktrees (Recommended)

**How it works:**
- Each Claude session gets its own worktree (separate working directory)
- All worktrees share the same `.git` directory and history
- Each worktree is on its own branch

**Commands:**
```bash
# Create worktree for a task
git worktree add ../kanblam-task-{uuid} -b claude/{task-id}

# Claude works in that directory
cd ../kanblam-task-{uuid} && claude

# Review changes
git log claude/{task-id}
git diff main..claude/{task-id}

# Accept: merge or cherry-pick
git checkout main && git merge claude/{task-id}

# Discard: remove worktree and branch
git worktree remove ../kanblam-task-{uuid}
git branch -D claude/{task-id}
```

**Pros:**
- Complete filesystem isolation - no conflicts during parallel work
- Standard git workflow for review/merge
- Lightweight (shares .git directory)
- Used by [Auto-Claude](https://github.com/AndyMik90/Auto-Claude) which supports up to 12 parallel agents

**Cons:**
- Each worktree needs its own `npm install`, build artifacts, etc.
- More disk space usage
- Need to manage worktree lifecycle

---

## Approach 2: GitButler-Style Virtual Branches (No Worktrees)

**How it works:**
- Single working directory
- Uses Claude Code's lifecycle hooks to track which session modified which files
- Automatically sorts changes into virtual branches per session
- Changes coexist in the working directory but are logically separated

**Integration:**
- Claude Code hooks notify GitButler when files change
- GitButler creates a branch per session automatically
- Each "chat round" becomes one commit with the prompt as context

**Pros:**
- No separate directories to manage
- No need to reinstall dependencies per worktree
- Automatic branch creation via hooks
- Can squash, reorder, split commits easily

**Cons:**
- Requires GitButler or similar tool
- More complex to implement ourselves
- Potential for visual confusion (multiple sessions' changes in one directory)

---

## Approach 3: Patch-Based Workflow

**How it works:**
- Claude sessions commit to temporary branches
- Extract changes as patch files: `git format-patch main..claude/{task-id}`
- Store patches in `.kanblam/patches/{task-id}/`
- Apply patches for review: `git apply --check patch.patch` then `git am patch.patch`

**Pros:**
- Patches are portable, reviewable text files
- Can be applied/unapplied cleanly
- Used by Linux kernel, Git project itself

**Cons:**
- More manual workflow
- Patches can become stale if main moves forward
- Less intuitive for modern developers

---

## Approach 4: Stacked Branches (git-branchless style)

**How it works:**
- Each task is a branch stacked on main
- Can reorder, edit, squash branches
- Tools like `git-branchless` or Graphite manage the stack

**Pros:**
- Clean linear history
- Easy to reorder/drop changes

**Cons:**
- Requires additional tooling
- Learning curve

---

## Recommended Architecture for Kanblam

Based on the research, we recommend **Approach 1 (Worktrees)** with some elements of **Approach 2 (Hooks)**:

```
┌─────────────────────────────────────────────────────────────┐
│                    Main Repository                          │
│                    /path/to/project                         │
│                    (branch: main)                           │
└─────────────────────────────────────────────────────────────┘
                              │
        ┌─────────────────────┼─────────────────────┐
        ▼                     ▼                     ▼
┌───────────────┐    ┌───────────────┐    ┌───────────────┐
│   Worktree 1  │    │   Worktree 2  │    │   Worktree 3  │
│  task-abc123  │    │  task-def456  │    │  task-ghi789  │
│ branch:       │    │ branch:       │    │ branch:       │
│ claude/abc123 │    │ claude/def456 │    │ claude/ghi789 │
└───────────────┘    └───────────────┘    └───────────────┘
     Claude 1             Claude 2             Claude 3
```

### Workflow

1. **Start Task**: Create worktree + branch for the task
2. **Claude Works**: Commits go to the task branch
3. **Review**: User can `git diff main..claude/{task}` or checkout to test
4. **Accept**: Merge/rebase to main, remove worktree
5. **Feedback**: User can commit feedback, Claude continues on same branch
6. **Discard**: Remove worktree + delete branch

### Key Commands to Implement

```bash
# Start task (Kanblam does this)
kanblam worktree create {task-id}
# → git worktree add ~/.kanblam/worktrees/{task-id} -b claude/{task-id}

# Preview changes
kanblam review {task-id}
# → git diff main..claude/{task-id}

# Apply to main for testing (temporary)
kanblam apply {task-id}
# → git stash && git cherry-pick --no-commit claude/{task-id}

# Accept and integrate
kanblam accept {task-id}
# → git checkout main && git merge --squash claude/{task-id}
# → git worktree remove ... && git branch -D claude/{task-id}

# Discard completely
kanblam discard {task-id}
# → git worktree remove ... && git branch -D claude/{task-id}
```

---

## Implementation Considerations

### Worktree Location
- Option A: `~/.kanblam/worktrees/{project}/{task-id}/`
- Option B: `{project}/../.kanblam-worktrees/{task-id}/`
- Option C: User-configurable

### Branch Naming Convention
- `claude/{task-id}` - simple, clear origin
- `claude/{project}/{task-id}` - if managing multiple projects
- `wip/claude/{task-id}` - indicates work-in-progress

### Commit Strategy
- Auto-commit on each Claude response? (like Auto-Claude)
- Let Claude commit naturally?
- Use hooks to track changes?

### Review UI in Kanblam
- Show diff in preview pane when task is in Review
- Allow "apply temporarily" to test changes
- Show commit log for the task branch
- Actions: Accept (merge), Feedback (continue), Discard (delete)

### Handling Dependencies
- For Node.js: Could symlink `node_modules` or use pnpm workspaces
- For Rust: Cargo handles this well with shared target directories
- General: Document that worktrees may need setup

---

## Sources

- [Git Worktrees Official Documentation](https://git-scm.com/docs/git-worktree)
- [Using Git Worktrees for Parallel AI Development - Steve Kinney](https://stevekinney.com/courses/ai-development/git-worktrees)
- [Auto-Claude - GitHub](https://github.com/AndyMik90/Auto-Claude)
- [Managing Multiple Claude Code Sessions Without Worktrees - GitButler](https://blog.gitbutler.com/parallel-claude-code)
- [Claude Code Best Practices - Anthropic](https://www.anthropic.com/engineering/claude-code-best-practices)
- [Git Cherry-Pick - Atlassian](https://www.atlassian.com/git/tutorials/cherry-pick)
- [Git Format-Patch Documentation](https://git-scm.com/docs/git-format-patch)
- [CCPM - Claude Code PM with GitHub Issues](https://github.com/automazeio/ccpm)
