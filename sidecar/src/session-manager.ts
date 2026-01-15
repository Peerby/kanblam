/**
 * Manages Claude Code Agent SDK sessions
 */

import { query, type ClaudeCodeOptions } from '@anthropic-ai/claude-code';
import {
  type SessionEventParams,
  type StartSessionParams,
  type ResumeSessionParams,
  type SendPromptParams,
} from './protocol.js';

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

  async startSession(params: StartSessionParams): Promise<string> {
    const { task_id, worktree_path, prompt, images } = params;

    // Check if session already exists for this task
    if (this.sessions.has(task_id)) {
      throw new Error(`Session already exists for task ${task_id}`);
    }

    const abortController = new AbortController();

    const options: ClaudeCodeOptions = {
      cwd: worktree_path,
      abortController,
    };

    let sessionId = '';

    // Start processing in background
    this.processQuery(task_id, prompt, options, images).catch((err) => {
      console.error(`Session ${task_id} error:`, err);
      this.onEvent({
        task_id,
        event: 'ended',
        session_id: sessionId,
        message: err.message,
      });
    });

    // Wait briefly to capture session ID from init message
    // The actual session ID is captured in processQuery
    await new Promise((resolve) => setTimeout(resolve, 100));

    const session = this.sessions.get(task_id);
    if (session) {
      return session.sessionId;
    }

    // Return placeholder - real ID will be sent via event
    return 'pending';
  }

  async resumeSession(params: ResumeSessionParams): Promise<string> {
    const { task_id, session_id, prompt } = params;

    // Remove any existing session for this task
    const existing = this.sessions.get(task_id);
    if (existing) {
      existing.abortController.abort();
      this.sessions.delete(task_id);
    }

    const abortController = new AbortController();

    const options: ClaudeCodeOptions = {
      resume: session_id,
      abortController,
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
      prompt,
    });
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

  private async processQuery(
    taskId: string,
    prompt: string,
    options: ClaudeCodeOptions,
    images?: string[]
  ): Promise<void> {
    let sessionId = '';
    let hasStarted = false;

    try {
      const response = query({
        prompt,
        options: {
          ...options,
          // Include images if provided
          ...(images?.length && { images }),
        },
      });

      for await (const message of response) {
        // Capture session ID from init message
        if (message.type === 'system' && message.subtype === 'init') {
          sessionId = message.session_id;

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
        }

        // Handle different message types
        if (message.type === 'assistant') {
          // Claude is responding
          if (message.content) {
            this.onEvent({
              task_id: taskId,
              event: 'output',
              session_id: sessionId,
              output: typeof message.content === 'string'
                ? message.content
                : JSON.stringify(message.content),
            });
          }
        }

        if (message.type === 'tool_use') {
          this.onEvent({
            task_id: taskId,
            event: 'tool_use',
            session_id: sessionId,
            tool_name: message.name,
          });

          this.onEvent({
            task_id: taskId,
            event: 'working',
            session_id: sessionId,
          });
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
    } finally {
      // Mark session as inactive but keep it for potential resume
      const session = this.sessions.get(taskId);
      if (session) {
        session.isActive = false;
      }
    }
  }
}
