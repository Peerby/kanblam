/**
 * Watcher Session - A read-only Claude session that observes the main worktree
 * and provides periodic helpful insights about the codebase.
 *
 * The watcher:
 * - Runs independently of task sessions
 * - Only reads files, never modifies anything
 * - Provides valuable insights: bugs, security issues, feature ideas, etc.
 * - Returns structured data with remark, description, and task instructions
 */

import { query, type Options } from '@anthropic-ai/claude-code';
import { execSync } from 'child_process';
import * as path from 'path';
import * as fs from 'fs';
import * as os from 'os';

/** Find the claude executable path */
function findClaudePath(): string | undefined {
  // Try which command first
  try {
    const result = execSync('which claude', { encoding: 'utf8' }).trim();
    if (result && fs.existsSync(result)) {
      return result;
    }
  } catch {
    // which failed, try other locations
  }

  // Common installation paths
  const homedir = os.homedir();
  const candidates = [
    path.join(homedir, '.nvm', 'versions', 'node', 'v20.19.0', 'bin', 'claude'),
    path.join(homedir, '.bun', 'bin', 'claude'),
    path.join(homedir, '.local', 'bin', 'claude'),
    '/usr/local/bin/claude',
    '/opt/homebrew/bin/claude',
  ];

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }

  return undefined;
}

export interface WatcherInsight {
  /** Short one-line remark (shown in bubble) */
  remark: string;
  /** Longer description (shown in modal) */
  description: string;
  /** Task instructions (can be used to create a task) */
  task: string;
}

export interface WatcherComment {
  timestamp: Date;
  comment: string;
  /** Full insight data if available */
  insight?: WatcherInsight;
  /** Optional mood/expression for the mascot */
  mood?: 'happy' | 'thinking' | 'concerned' | 'excited' | 'sleepy';
}

export type WatcherEventCallback = (comment: WatcherComment) => void;
export type WatcherObservingCallback = (isObserving: boolean) => void;

/** Focus areas for the watcher to analyze */
const FOCUS_TYPES = [
  'bug',
  'security',
  'feature_idea',
  'ux_improvement',
  'elegant_code',
  'refactor',
] as const;

type FocusType = typeof FOCUS_TYPES[number];

export class WatcherSession {
  private abortController: AbortController | null = null;
  private isRunning = false;
  private isObserving = false; // Guard against concurrent observations
  private intervalId: NodeJS.Timeout | null = null;
  private onComment: WatcherEventCallback;
  private onObserving: WatcherObservingCallback | undefined;
  private projectPath: string;
  private claudePath: string | undefined;

  /** Interval between observations in milliseconds (default: 15 minutes) */
  private readonly observationIntervalMs: number;

  constructor(
    projectPath: string,
    onComment: WatcherEventCallback,
    options?: {
      intervalMinutes?: number;
      claudePath?: string;
      onObserving?: WatcherObservingCallback;
    }
  ) {
    this.projectPath = projectPath;
    this.onComment = onComment;
    this.onObserving = options?.onObserving;
    this.observationIntervalMs = (options?.intervalMinutes ?? 15) * 60 * 1000;
    this.claudePath = options?.claudePath;
  }

  /**
   * Start the watcher session. It will observe the project periodically.
   */
  async start(): Promise<void> {
    if (this.isRunning) {
      console.log('[Watcher] Already running, ignoring start request');
      return;
    }

    this.isRunning = true;
    console.log(`[Watcher] Starting for ${this.projectPath}`);

    // Initial observation after a short delay
    setTimeout(() => {
      if (this.isRunning) {
        this.observe();
      }
    }, 30000); // First observation after 30 seconds

    // Then observe periodically
    this.intervalId = setInterval(() => {
      if (this.isRunning) {
        this.observe();
      }
    }, this.observationIntervalMs);
  }

  /**
   * Stop the watcher session.
   */
  stop(): void {
    console.log('[Watcher] Stopping');
    this.isRunning = false;

    if (this.intervalId) {
      clearInterval(this.intervalId);
      this.intervalId = null;
    }

    if (this.abortController) {
      this.abortController.abort();
      this.abortController = null;
    }
  }

  /**
   * Force an immediate observation (for testing or manual trigger).
   */
  async observeNow(): Promise<void> {
    await this.observe();
  }

  /**
   * Build the prompt for a specific focus type
   */
  private buildPrompt(focusType: FocusType): string {
    const focusDescriptions: Record<FocusType, string> = {
      bug: 'Find a potential bug or edge case that could cause issues',
      security: 'Identify a security concern or vulnerability',
      feature_idea: 'Suggest a valuable feature idea based on the codebase patterns',
      ux_improvement: 'Suggest a UX or UI improvement',
      elegant_code: 'Point out a particularly elegant or well-written piece of code',
      refactor: 'Suggest an impactful refactoring opportunity',
    };

    return `You are a supportive coding buddy reviewing this project. Your focus: ${focusDescriptions[focusType]}.

First, explore the codebase to understand its structure. Use Bash to run: git diff --stat HEAD~5, git log --oneline -10, and look at key files.

Then provide your insight in this EXACT XML format:

<insight>
<remark>A short, casual one-liner about what you found (can be humorous, include emojis!) - MAX 100 chars</remark>
<description>A supportive, detailed explanation as a teammate who wants the best for this project and the developer. 2-4 sentences.</description>
<task>Clear task instructions that could be fed into a coding assistant to address this insight. Be specific about files and changes needed.</task>
</insight>

IMPORTANT:
- The <remark> must be ONE LINE, under 100 characters, casual and fun
- The <description> should be encouraging and constructive
- The <task> should be actionable and specific
- Output ONLY the XML block, nothing else`;
  }

  /**
   * Parse the XML response from Claude
   */
  private parseInsight(response: string): WatcherInsight | null {
    try {
      // Extract content between XML tags
      const remarkMatch = response.match(/<remark>([\s\S]*?)<\/remark>/);
      const descMatch = response.match(/<description>([\s\S]*?)<\/description>/);
      const taskMatch = response.match(/<task>([\s\S]*?)<\/task>/);

      if (!remarkMatch || !descMatch || !taskMatch) {
        console.log('[Watcher] Failed to parse XML, raw response:', response.slice(0, 200));
        return null;
      }

      return {
        remark: remarkMatch[1].trim().slice(0, 100), // Enforce limit
        description: descMatch[1].trim(),
        task: taskMatch[1].trim(),
      };
    } catch (err) {
      console.error('[Watcher] Parse error:', err);
      return null;
    }
  }

  /**
   * Perform a single observation of the project.
   */
  private async observe(): Promise<void> {
    if (!this.isRunning) return;

    // Prevent concurrent observations
    if (this.isObserving) {
      console.log('[Watcher] Already observing, skipping...');
      return;
    }
    this.isObserving = true;

    console.log('[Watcher] Performing observation...');

    // Notify that we're starting to observe (for UI feedback)
    this.onObserving?.(true);

    // Pick a random focus type
    const focusType = FOCUS_TYPES[Math.floor(Math.random() * FOCUS_TYPES.length)];
    console.log(`[Watcher] Focus: ${focusType}`);

    const prompt = this.buildPrompt(focusType);

    // Get claude path - use provided or find it
    const claudePath = this.claudePath || findClaudePath();
    console.log(`[Watcher] Using claude path: ${claudePath}`);

    // Create fresh abort controller for this observation
    this.abortController = new AbortController();

    const options: Options = {
      cwd: this.projectPath,
      abortController: this.abortController,
      pathToClaudeCodeExecutable: claudePath,
      maxTurns: 10, // Allow plenty of turns for thorough exploration
      allowedTools: ['Bash', 'Read', 'Glob', 'Grep'],
    };

    console.log(`[Watcher] Starting query with focus: ${focusType}`);

    // Timeout after 60 seconds to prevent hanging
    let timeoutId: NodeJS.Timeout | null = setTimeout(() => {
      console.log('[Watcher] Query timeout after 60s, aborting...');
      this.abortController?.abort();
    }, 60000);

    const clearQueryTimeout = () => {
      if (timeoutId) {
        clearTimeout(timeoutId);
        timeoutId = null;
      }
    };

    try {
      const response = query({ prompt, options });
      let fullResponse = '';
      let turnCount = 0;

      for await (const message of response) {
        console.log(`[Watcher] Message type: ${message.type}`);
        if (message.type === 'assistant') {
          turnCount++;
          const apiMessage = message.message;
          if (apiMessage?.content) {
            for (const block of apiMessage.content) {
              console.log(`[Watcher] Block type: ${block.type}`);
              if (block.type === 'text' && 'text' in block) {
                fullResponse += (block as { type: 'text'; text: string }).text;
              }
            }
          }
        }
      }
      clearQueryTimeout();
      console.log(`[Watcher] Query done, ${turnCount} turns`);

      // Parse the XML response
      const insight = this.parseInsight(fullResponse);

      if (insight) {
        // Determine mood based on focus type
        const mood = this.focusToMood(focusType);

        console.log(`[Watcher] Raw comment: "${insight.remark}"`);
        this.onComment({
          timestamp: new Date(),
          comment: insight.remark,
          insight,
          mood,
        });

        console.log(`[Watcher] Comment: "${insight.remark}" (mood: ${mood})`);
      } else {
        // Fallback: use raw response as simple comment
        const comment = fullResponse.trim().slice(0, 100);
        if (comment) {
          this.onComment({
            timestamp: new Date(),
            comment,
            mood: 'happy',
          });
          console.log(`[Watcher] Fallback comment: "${comment}"`);
        }
      }
    } catch (err) {
      // Don't spam errors - watcher is non-critical
      if ((err as Error).name !== 'AbortError') {
        console.error('[Watcher] Observation failed:', err);
      }
    } finally {
      clearQueryTimeout();
      this.abortController = null;
      // Small delay before allowing next observation (subprocess cleanup)
      await new Promise(resolve => setTimeout(resolve, 2000));
      this.isObserving = false;
      // Notify that observation is complete
      this.onObserving?.(false);
    }
  }

  /**
   * Map focus type to mood
   */
  private focusToMood(focusType: FocusType): WatcherComment['mood'] {
    switch (focusType) {
      case 'bug':
      case 'security':
        return 'concerned';
      case 'feature_idea':
      case 'ux_improvement':
        return 'thinking';
      case 'elegant_code':
        return 'excited';
      case 'refactor':
        return 'thinking';
      default:
        return 'happy';
    }
  }
}
