// SPDX-License-Identifier: AGPL-3.0-or-later

//! Wire protocol types for Trakkt Connect.
//!
//! Defines the bidirectional WebSocket message format used between the Trakkt
//! server and a customer-deployed Connect agent for terminal session management.
//!
//! The protocol is event-driven:
//! - Server sends [`ServerMessage`] commands (spawn, input, resize, kill).
//! - Agent sends [`AgentMessage`] events (output, session lifecycle, ready).
//!
//! Every message carrying session data includes a `session_id` for multiplexing
//! multiple terminal sessions over a single WebSocket connection.
//!
//! PTY data (terminal input/output) is base64-encoded in JSON text frames,
//! keeping the protocol debuggable in browser dev tools.

pub mod wire;

pub use wire::{AgentMessage, ServerMessage, SessionEventKind, SessionInfo};
