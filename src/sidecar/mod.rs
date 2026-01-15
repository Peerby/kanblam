//! Sidecar module for Claude Code Agent SDK integration
//!
//! This module provides IPC communication with the TypeScript sidecar process
//! that manages Claude Code Agent SDK sessions.

pub mod client;
pub mod protocol;

pub use client::{ensure_sidecar_running, SidecarClient, SidecarEventReceiver};
pub use protocol::{SessionEventType, SidecarEvent};
