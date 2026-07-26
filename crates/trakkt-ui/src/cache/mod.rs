// SPDX-License-Identifier: AGPL-3.0-or-later

//! Client-side persistent cache and reactive in-memory store for the sync
//! engine.
//!
//! ### `store` — reactive in-memory store (all targets)
//!
//! [`store::SyncStore`] is the single source of truth for list pages. It is
//! available on both SSR and WASM targets so page components that read from it
//! compile on both. On SSR the store is empty; pages show a loading state until
//! `initialized()` becomes `true` on the client after IDB hydration.
//!
//! ### `db` — IndexedDB persistence (WASM only)
//!
//! On WASM targets the persistent cache is backed by two IndexedDB object
//! stores (via `indexed_db_futures`) that survive page reloads without
//! round-tripping to the server on startup.  See [`db`] for the full schema
//! and API.

pub mod store;

/// Single FIFO writer that orders every cache write against the sync cursor.
///
/// Not target-gated: the queue and its ordering rules are pure Rust and unit
/// tested natively, while only the IndexedDB-backed sink is `wasm32`-only.
pub mod idb_writer;

/// The two halves of applying one `SyncAction`: the in-memory store update
/// every tab performs, and the cache writes only the leader tab performs.
///
/// Not target-gated: both halves are pure Rust over the store and the writer
/// queue, so which entity type bumps which version counter is tested natively.
pub mod apply;

/// The one-shot latch that keeps the leader tab's WebSocket dial behind
/// hydration, so no sync action can be applied to a store that is about to be
/// bulk-replaced.
///
/// Not target-gated: the latch is pure Rust and unit tested natively, while the
/// two halves it orders are `wasm32`-only.
pub mod hydration_gate;

/// Cross-tab leader election and the leader→follower broadcast channel.
///
/// Not target-gated: the wire format is pure Rust and tested natively, while
/// the Web Locks and `BroadcastChannel` bindings are `wasm32`-only.
pub mod tab_leader;

#[cfg(target_arch = "wasm32")]
pub mod db;

#[cfg(target_arch = "wasm32")]
pub mod websocket;

#[cfg(target_arch = "wasm32")]
pub mod sync_engine;

#[cfg(target_arch = "wasm32")]
pub use db::*;
