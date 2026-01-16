/**
 * JSON-RPC 2.0 protocol types for Rust <-> TypeScript sidecar communication
 */

// Base JSON-RPC types
export interface JsonRpcRequest {
  jsonrpc: '2.0';
  id: number | string;
  method: string;
  params?: unknown;
}

export interface JsonRpcResponse {
  jsonrpc: '2.0';
  id: number | string;
  result?: unknown;
  error?: JsonRpcError;
}

export interface JsonRpcNotification {
  jsonrpc: '2.0';
  method: string;
  params?: unknown;
}

export interface JsonRpcError {
  code: number;
  message: string;
  data?: unknown;
}

// Request types from Rust
export interface StartSessionParams {
  task_id: string;
  worktree_path: string;
  prompt: string;
  images?: string[];
}

export interface ResumeSessionParams {
  task_id: string;
  session_id: string;
  worktree_path: string;
  prompt?: string;
}

export interface SendPromptParams {
  task_id: string;
  prompt: string;
  images?: string[];
}

export interface StopSessionParams {
  task_id: string;
}

// Response types
export interface StartSessionResult {
  session_id: string;
}

export interface ResumeSessionResult {
  session_id: string; // New session ID (may differ from input)
}

// Notification types to Rust
export type SessionEventType =
  | 'started'
  | 'stopped'
  | 'ended'
  | 'needs_input'
  | 'working'
  | 'tool_use'
  | 'output';

export interface SessionEventParams {
  task_id: string;
  event: SessionEventType;
  session_id?: string;
  message?: string;
  tool_name?: string;
  output?: string;
}

// Helper functions
export function createRequest(
  id: number | string,
  method: string,
  params?: unknown
): JsonRpcRequest {
  return { jsonrpc: '2.0', id, method, params };
}

export function createResponse(
  id: number | string,
  result?: unknown,
  error?: JsonRpcError
): JsonRpcResponse {
  const response: JsonRpcResponse = { jsonrpc: '2.0', id };
  if (error) {
    response.error = error;
  } else {
    response.result = result;
  }
  return response;
}

export function createNotification(
  method: string,
  params?: unknown
): JsonRpcNotification {
  return { jsonrpc: '2.0', method, params };
}

export function createSessionEvent(params: SessionEventParams): JsonRpcNotification {
  return createNotification('session_event', params);
}

// Error codes
export const ErrorCodes = {
  PARSE_ERROR: -32700,
  INVALID_REQUEST: -32600,
  METHOD_NOT_FOUND: -32601,
  INVALID_PARAMS: -32602,
  INTERNAL_ERROR: -32603,
  // Custom codes
  SESSION_NOT_FOUND: -32000,
  SESSION_ALREADY_EXISTS: -32001,
  SDK_ERROR: -32002,
} as const;
