/**
 * Manages Claude Code Agent SDK sessions
 */

import { query, type Options, type CanUseTool } from '@anthropic-ai/claude-code';
import {
  type SessionEventParams,
  type StartSessionParams,
  type ResumeSessionParams,
  type SendPromptParams,
  type SummarizeTitleParams,
  type SummarizeTitleResult,
} from './protocol.js';
import * as path from 'path';

export interface Session {
  taskId: string;
  sessionId: string;
  worktreePath: string;
  abortController: AbortController;
  isActive: boolean;
}

export type EventCallback = (event: SessionEventParams) => void;

export class SessionManager {
  private sessions: Map<string, Session> = new Map();
  private onEvent: EventCallback;

  constructor(onEvent: EventCallback) {
    this.onEvent = onEvent;
  }

  /**
   * Find the Claude Code executable path
   */
  private async findClaudePath(): Promise<string | undefined> {
    const { execSync } = await import('child_process');
    const path = await import('path');
    const fs = await import('fs');
    const os = await import('os');

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
      path.join(homedir, '.bun', 'bin', 'claude'),
      path.join(homedir, '.local', 'bin', 'claude'),
      '/usr/local/bin/claude',
    ];

    for (const candidate of candidates) {
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }

    return undefined;
  }

  async startSession(params: StartSessionParams): Promise<string> {
    const { task_id, worktree_path, prompt, images } = params;

    // If session already exists for this task, send the new prompt to it
    const existing = this.sessions.get(task_id);
    if (existing && existing.isActive) {
      console.log(`[SessionManager] Active session exists for task ${task_id}, sending prompt to existing session`);
      // Don't abort - just start a new query on the existing session
      // The SDK will queue/handle the new prompt appropriately
      this.processQuery(task_id, prompt, {
        resume: existing.sessionId,
        abortController: existing.abortController,
      }, images).catch((err) => {
        console.error(`Send to existing session ${task_id} error:`, err);
        this.onEvent({
          task_id,
          event: 'ended',
          session_id: existing.sessionId,
          message: err.message,
        });
      });
      return existing.sessionId;
    }

    // If session exists but is not active, clean it up first
    if (existing) {
      console.log(`[SessionManager] Inactive session exists for task ${task_id}, cleaning up`);
      this.sessions.delete(task_id);
    }

    const abortController = new AbortController();

    // Find Claude executable - try common locations
    const claudePath = process.env.CLAUDE_PATH ||
      (await this.findClaudePath());

    const options: Options = {
      cwd: worktree_path,
      abortController,
      pathToClaudeCodeExecutable: claudePath,
    };

    // Create a promise that resolves when session ID is captured
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        reject(new Error('Timeout waiting for SDK session to initialize'));
      }, 30000); // 30 second timeout

      // Start processing - this will capture session ID and resolve our promise
      this.processQueryWithCallback(
        task_id,
        prompt,
        options,
        images,
        (sessionId) => {
          clearTimeout(timeout);
          resolve(sessionId);
        },
        (err) => {
          clearTimeout(timeout);
          reject(err);
        }
      ).catch((err) => {
        // Catch any errors from the async processing itself
        clearTimeout(timeout);
        reject(err);
      });
    });
  }

  async resumeSession(params: ResumeSessionParams): Promise<string> {
    const { task_id, session_id, worktree_path, prompt } = params;

    // Remove any existing session for this task
    const existing = this.sessions.get(task_id);
    if (existing) {
      existing.abortController.abort();
      this.sessions.delete(task_id);
    }

    const abortController = new AbortController();

    // Find Claude executable
    const claudePath = process.env.CLAUDE_PATH || (await this.findClaudePath());

    const options: Options = {
      resume: session_id,
      cwd: worktree_path,
      abortController,
      pathToClaudeCodeExecutable: claudePath,
    };

    // Start processing with resume
    this.processQuery(task_id, prompt || '', options).catch((err) => {
      console.error(`Resume session ${task_id} error:`, err);
      this.onEvent({
        task_id,
        event: 'ended',
        session_id,
        message: err.message,
      });
    });

    // Wait briefly for session to initialize
    await new Promise((resolve) => setTimeout(resolve, 100));

    const session = this.sessions.get(task_id);
    return session?.sessionId || session_id;
  }

  async sendPrompt(params: SendPromptParams): Promise<void> {
    const { task_id, prompt, images } = params;

    const session = this.sessions.get(task_id);
    if (!session) {
      throw new Error(`No active session for task ${task_id}`);
    }

    // Resume the existing session with new prompt
    await this.resumeSession({
      task_id,
      session_id: session.sessionId,
      worktree_path: session.worktreePath,
      prompt,
    });
  }

  /**
   * Summarize a long task title into a short, clear summary for display in the kanban board.
   * Uses a one-shot SDK query to generate the summary.
   */
  async summarizeTitle(params: SummarizeTitleParams): Promise<SummarizeTitleResult> {
    const { task_id, title } = params;

    const prompt = `Summarize this task description into a brief, clear title (max 30 chars) for a kanban board card. Return ONLY the summary, nothing else.

Task: ${title}`;

    const claudePath = process.env.CLAUDE_PATH || (await this.findClaudePath());
    const abortController = new AbortController();

    const options: Options = {
      abortController,
      pathToClaudeCodeExecutable: claudePath,
      maxTurns: 1, // Single-turn query for summarization
    };

    let shortTitle = '';

    try {
      const response = query({ prompt, options });

      for await (const message of response) {
        if (message.type === 'assistant') {
          // Content is in message.message.content
          const apiMessage = message.message;
          if (apiMessage && apiMessage.content) {
            for (const block of apiMessage.content) {
              if (block.type === 'text' && 'text' in block) {
                shortTitle += (block as { type: 'text'; text: string }).text;
              }
            }
          }
        }
      }
    } catch (err) {
      console.error(`[SessionManager] Error summarizing title for task ${task_id}:`, err);
      // Fall back to truncating the original title
      shortTitle = title.slice(0, 27) + (title.length > 27 ? '...' : '');
    } finally {
      abortController.abort();
    }

    // Clean up the response - remove quotes, extra whitespace, and limit length
    shortTitle = shortTitle.trim().replace(/^["']|["']$/g, '').trim();
    if (shortTitle.length > 30) {
      shortTitle = shortTitle.slice(0, 27) + '...';
    }

    // If we didn't get a meaningful response, fall back to truncation
    if (!shortTitle || shortTitle.length < 3) {
      shortTitle = title.slice(0, 27) + (title.length > 27 ? '...' : '');
    }

    console.log(`[SessionManager] Summarized title for task ${task_id}: "${shortTitle}"`);

    return { short_title: shortTitle };
  }

  stopSession(taskId: string): void {
    const session = this.sessions.get(taskId);
    if (session) {
      session.abortController.abort();
      session.isActive = false;
      this.sessions.delete(taskId);
    }
  }

  getSession(taskId: string): Session | undefined {
    return this.sessions.get(taskId);
  }

  listSessions(): { taskId: string; sessionId: string; isActive: boolean }[] {
    return Array.from(this.sessions.values()).map(s => ({
      taskId: s.taskId,
      sessionId: s.sessionId,
      isActive: s.isActive,
    }));
  }

  stopAllSessions(): void {
    for (const [taskId, session] of this.sessions) {
      session.abortController.abort();
      session.isActive = false;
    }
    this.sessions.clear();
  }

  private async processQuery(
    taskId: string,
    prompt: string,
    options: Options,
    images?: string[]
  ): Promise<void> {
    await this.processQueryWithCallback(taskId, prompt, options, images, () => {}, () => {});
  }

  /**
   * Process a query with callbacks for when session ID is captured and on error.
   * This allows the caller to wait for the session to initialize.
   */
  private async processQueryWithCallback(
    taskId: string,
    prompt: string,
    options: Options,
    images: string[] | undefined,
    onSessionStarted: (sessionId: string) => void,
    onError: (err: Error) => void
  ): Promise<void> {
    let sessionId = '';
    let hasStarted = false;
    let sessionIdResolved = false;

    try {
      // Note: images are not directly supported by SDK options
      // TODO: Support images by including them in user message content
      if (images?.length) {
        console.warn('Images provided but not yet supported in SDK mode');
      }

      const response = query({
        prompt,
        options,
      });

      for await (const message of response) {
        // Capture session ID from init message
        if (message.type === 'system' && message.subtype === 'init') {
          sessionId = message.session_id;
          console.log(`[SessionManager] Captured session_id from SDK init: ${sessionId} for task ${taskId}`);

          // Store session
          this.sessions.set(taskId, {
            taskId,
            sessionId,
            worktreePath: options.cwd || process.cwd(),
            abortController: options.abortController || new AbortController(),
            isActive: true,
          });

          if (!hasStarted) {
            hasStarted = true;
            this.onEvent({
              task_id: taskId,
              event: 'started',
              session_id: sessionId,
            });
          }

          // Notify caller that session started with ID
          if (!sessionIdResolved) {
            sessionIdResolved = true;
            onSessionStarted(sessionId);
          }
        }

        // Handle different message types
        if (message.type === 'assistant') {
          // Claude is responding - content is in message.message.content
          const apiMessage = message.message;
          if (apiMessage && apiMessage.content) {
            // Extract text from content blocks
            let textContent = '';
            for (const block of apiMessage.content) {
              if (block.type === 'text' && 'text' in block) {
                textContent += (block as { type: 'text'; text: string }).text;
              }
            }

            if (textContent) {
              this.onEvent({
                task_id: taskId,
                event: 'output',
                session_id: sessionId,
                output: textContent,
              });
            }

            // Check for tool_use blocks in the content
            for (const block of apiMessage.content) {
              if (block.type === 'tool_use' && 'name' in block) {
                this.onEvent({
                  task_id: taskId,
                  event: 'tool_use',
                  session_id: sessionId,
                  tool_name: (block as { type: 'tool_use'; name: string }).name,
                });

                this.onEvent({
                  task_id: taskId,
                  event: 'working',
                  session_id: sessionId,
                });
              }
            }
          }
        }

        if (message.type === 'result') {
          // Session completed
          this.onEvent({
            task_id: taskId,
            event: 'stopped',
            session_id: sessionId,
          });
        }
      }
    } catch (err) {
      // Notify caller of error if session hasn't started yet
      if (!sessionIdResolved) {
        sessionIdResolved = true;
        onError(err instanceof Error ? err : new Error(String(err)));
      }
      throw err;
    } finally {
      // Mark session as inactive and notify TUI
      const session = this.sessions.get(taskId);
      if (session) {
        session.isActive = false;
        // Always emit 'ended' when session loop completes
        // This ensures TUI knows the session is done, regardless of how it ended
        console.log(`[SessionManager] Session ended for task ${taskId}`);
        this.onEvent({
          task_id: taskId,
          event: 'ended',
          session_id: session.sessionId,
        });
      }
    }
  }
}
