/**
 * KanClaude Sidecar - Claude Code Agent SDK IPC server
 *
 * Communicates with the Rust TUI via Unix domain socket using JSON-RPC 2.0
 */

import * as net from 'net';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

import { SessionManager } from './session-manager.js';
import {
  type JsonRpcRequest,
  type JsonRpcResponse,
  type SessionEventParams,
  type StartSessionParams,
  type ResumeSessionParams,
  type SendPromptParams,
  type StopSessionParams,
  createResponse,
  createSessionEvent,
  ErrorCodes,
} from './protocol.js';

// Socket path - in user's runtime directory
const SOCKET_DIR = path.join(os.homedir(), '.kanclaude');
const SOCKET_PATH = path.join(SOCKET_DIR, 'sidecar.sock');

class SidecarServer {
  private server: net.Server;
  private sessionManager: SessionManager;
  private clients: Set<net.Socket> = new Set();

  constructor() {
    // Initialize session manager with event callback
    this.sessionManager = new SessionManager((event) => {
      this.broadcastEvent(event);
    });

    this.server = net.createServer((socket) => {
      this.handleConnection(socket);
    });
  }

  private handleConnection(socket: net.Socket): void {
    console.log('Client connected');
    this.clients.add(socket);

    let buffer = '';

    socket.on('data', async (data) => {
      buffer += data.toString();

      // Process complete JSON-RPC messages (newline-delimited)
      const lines = buffer.split('\n');
      buffer = lines.pop() || ''; // Keep incomplete line in buffer

      for (const line of lines) {
        if (line.trim()) {
          await this.handleMessage(socket, line);
        }
      }
    });

    socket.on('close', () => {
      console.log('Client disconnected');
      this.clients.delete(socket);
    });

    socket.on('error', (err) => {
      console.error('Socket error:', err);
      this.clients.delete(socket);
    });
  }

  private async handleMessage(socket: net.Socket, message: string): Promise<void> {
    let request: JsonRpcRequest;

    try {
      request = JSON.parse(message);
    } catch (err) {
      const response = createResponse(null as unknown as number, undefined, {
        code: ErrorCodes.PARSE_ERROR,
        message: 'Parse error',
      });
      this.send(socket, response);
      return;
    }

    // Handle the request
    const response = await this.handleRequest(request);
    this.send(socket, response);
  }

  private async handleRequest(request: JsonRpcRequest): Promise<JsonRpcResponse> {
    const { id, method, params } = request;

    try {
      switch (method) {
        case 'start_session': {
          const p = params as StartSessionParams;
          // Validate required params
          if (!p?.task_id || !p?.worktree_path || !p?.prompt) {
            return createResponse(id, undefined, {
              code: ErrorCodes.INVALID_PARAMS,
              message: 'Missing required params: task_id, worktree_path, prompt',
            });
          }
          const sessionId = await this.sessionManager.startSession(p);
          return createResponse(id, { session_id: sessionId });
        }

        case 'resume_session': {
          const p = params as ResumeSessionParams;
          // Validate required params
          if (!p?.task_id || !p?.session_id) {
            return createResponse(id, undefined, {
              code: ErrorCodes.INVALID_PARAMS,
              message: 'Missing required params: task_id, session_id',
            });
          }
          const sessionId = await this.sessionManager.resumeSession(p);
          return createResponse(id, { session_id: sessionId });
        }

        case 'send_prompt': {
          const p = params as SendPromptParams;
          // Validate required params
          if (!p?.task_id || !p?.prompt) {
            return createResponse(id, undefined, {
              code: ErrorCodes.INVALID_PARAMS,
              message: 'Missing required params: task_id, prompt',
            });
          }
          await this.sessionManager.sendPrompt(p);
          return createResponse(id, { success: true });
        }

        case 'stop_session': {
          const p = params as StopSessionParams;
          // Validate required params
          if (!p?.task_id) {
            return createResponse(id, undefined, {
              code: ErrorCodes.INVALID_PARAMS,
              message: 'Missing required param: task_id',
            });
          }
          this.sessionManager.stopSession(p.task_id);
          return createResponse(id, { success: true });
        }

        case 'get_session': {
          const p = params as { task_id: string };
          // Validate required params
          if (!p?.task_id) {
            return createResponse(id, undefined, {
              code: ErrorCodes.INVALID_PARAMS,
              message: 'Missing required param: task_id',
            });
          }
          const session = this.sessionManager.getSession(p.task_id);
          if (session) {
            return createResponse(id, {
              session_id: session.sessionId,
              is_active: session.isActive,
            });
          }
          return createResponse(id, undefined, {
            code: ErrorCodes.SESSION_NOT_FOUND,
            message: `Session not found for task ${p.task_id}`,
          });
        }

        case 'ping': {
          return createResponse(id, { pong: true });
        }

        default:
          return createResponse(id, undefined, {
            code: ErrorCodes.METHOD_NOT_FOUND,
            message: `Method not found: ${method}`,
          });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Unknown error';
      return createResponse(id, undefined, {
        code: ErrorCodes.INTERNAL_ERROR,
        message,
      });
    }
  }

  private broadcastEvent(event: SessionEventParams): void {
    const notification = createSessionEvent(event);
    const message = JSON.stringify(notification) + '\n';

    for (const client of this.clients) {
      try {
        client.write(message);
      } catch (err) {
        console.error('Failed to send to client:', err);
        this.clients.delete(client);
      }
    }
  }

  private send(socket: net.Socket, response: JsonRpcResponse): void {
    const message = JSON.stringify(response) + '\n';
    socket.write(message);
  }

  async start(): Promise<void> {
    // Ensure socket directory exists
    if (!fs.existsSync(SOCKET_DIR)) {
      fs.mkdirSync(SOCKET_DIR, { recursive: true });
    }

    // Remove existing socket file
    if (fs.existsSync(SOCKET_PATH)) {
      fs.unlinkSync(SOCKET_PATH);
    }

    return new Promise((resolve, reject) => {
      this.server.listen(SOCKET_PATH, () => {
        console.log(`Sidecar listening on ${SOCKET_PATH}`);
        resolve();
      });

      this.server.on('error', reject);
    });
  }

  stop(): void {
    // Stop all sessions
    for (const client of this.clients) {
      client.destroy();
    }
    this.clients.clear();

    this.server.close();

    // Clean up socket file
    if (fs.existsSync(SOCKET_PATH)) {
      fs.unlinkSync(SOCKET_PATH);
    }
  }
}

// Main entry point
async function main(): Promise<void> {
  const server = new SidecarServer();

  // Handle shutdown signals
  process.on('SIGINT', () => {
    console.log('\nShutting down...');
    server.stop();
    process.exit(0);
  });

  process.on('SIGTERM', () => {
    console.log('Received SIGTERM, shutting down...');
    server.stop();
    process.exit(0);
  });

  try {
    await server.start();
  } catch (err) {
    console.error('Failed to start sidecar:', err);
    process.exit(1);
  }
}

main();
