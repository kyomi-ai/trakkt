// SPDX-License-Identifier: AGPL-3.0-or-later

pub mod config;
pub mod constants;
pub mod db;
pub mod enums;
pub mod error;
pub mod kv_store;
pub mod kv_store_memory;
pub mod kv_store_redis;
pub mod models;
pub mod redis;
pub mod retry;
pub mod sql_compat;
#[cfg(feature = "test-helpers")]
pub mod test_helpers;
pub mod websocket_types;

pub use config::Config;
pub use db::DbPool;
pub use error::{Error, Result};
pub use kv_store::{KVPool, KVStore, create_kv_store, kv_consume_json, kv_peek_json, kv_store_json};
pub use redis::RedisPool;
pub use websocket_types::{MessageType, WebSocketMessage};

/// Current terms of service version.
pub const TERMS_VERSION: &str = "1.0";
