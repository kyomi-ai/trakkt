// SPDX-License-Identifier: AGPL-3.0-or-later

//! KVStore trait — unified key-value store abstraction.
//!
//! Implementations:
//! - [`crate::kv_store_redis::RedisKVStore`] — backed by Redis (production)
//! - [`crate::kv_store_memory::InMemoryKVStore`] — in-process HashMap (single-instance / no-Redis)

use std::{future::Future, pin::Pin, sync::Arc};

/// Object-safe, sendable future returned by a KV operation.
pub type KVFuture<'a, T> = Pin<Box<dyn Future<Output = crate::Result<T>> + Send + 'a>>;

/// Async key-value store with TTL support.
///
/// All implementations must be `Send + Sync` so the store can be held in
/// Axum's shared state (which requires `Clone + Send + Sync + 'static`).
///
/// Explicit boxed futures preserve object safety and the lifetime contract used
/// by `#[async_trait::async_trait]` implementations without adding a redundant
/// `#[must_use]` attribute to methods that already return must-use futures.
pub trait KVStore: Send + Sync {
    /// SET key to value with an optional TTL in seconds.
    fn set<'store, 'key, 'value, 'future>(
        &'store self,
        key: &'key str,
        value: &'value str,
        ttl_secs: Option<u64>,
    ) -> KVFuture<'future, ()>
    where
        'store: 'future,
        'key: 'future,
        'value: 'future,
        Self: 'future;

    /// GET key — returns `None` if the key is missing or expired.
    fn get<'store, 'key, 'future>(
        &'store self,
        key: &'key str,
    ) -> KVFuture<'future, Option<String>>
    where
        'store: 'future,
        'key: 'future,
        Self: 'future;

    /// DEL key.
    fn del<'store, 'key, 'future>(&'store self, key: &'key str) -> KVFuture<'future, ()>
    where
        'store: 'future,
        'key: 'future,
        Self: 'future;

    /// GETDEL — atomically get and delete.
    ///
    /// Returns `None` if the key is missing or expired.
    fn getdel<'store, 'key, 'future>(
        &'store self,
        key: &'key str,
    ) -> KVFuture<'future, Option<String>>
    where
        'store: 'future,
        'key: 'future,
        Self: 'future;

    /// INCR — atomically increment the integer value stored at key.
    ///
    /// Creates the key with value `1` if it is absent or expired.
    fn incr<'store, 'key, 'future>(&'store self, key: &'key str) -> KVFuture<'future, i64>
    where
        'store: 'future,
        'key: 'future,
        Self: 'future;

    /// EXPIRE — set a TTL on an existing key.
    ///
    /// No-op if the key is missing or already expired.
    fn expire<'store, 'key, 'future>(
        &'store self,
        key: &'key str,
        ttl_secs: u64,
    ) -> KVFuture<'future, ()>
    where
        'store: 'future,
        'key: 'future,
        Self: 'future;

    /// SADD — add a member to a set. Creates the set if it doesn't exist.
    fn sadd<'store, 'key, 'member, 'future>(
        &'store self,
        key: &'key str,
        member: &'member str,
    ) -> KVFuture<'future, ()>
    where
        'store: 'future,
        'key: 'future,
        'member: 'future,
        Self: 'future;

    /// SREM — remove a member from a set. No-op if member or key doesn't exist.
    fn srem<'store, 'key, 'member, 'future>(
        &'store self,
        key: &'key str,
        member: &'member str,
    ) -> KVFuture<'future, ()>
    where
        'store: 'future,
        'key: 'future,
        'member: 'future,
        Self: 'future;

    /// SMEMBERS — return all members of a set. Empty vec if key doesn't exist.
    fn smembers<'store, 'key, 'future>(
        &'store self,
        key: &'key str,
    ) -> KVFuture<'future, Vec<String>>
    where
        'store: 'future,
        'key: 'future,
        Self: 'future;

    /// SDEL — delete an entire set key.
    fn sdel<'store, 'key, 'future>(&'store self, key: &'key str) -> KVFuture<'future, ()>
    where
        'store: 'future,
        'key: 'future,
        Self: 'future;

    /// PING — health check.
    ///
    /// Default implementation performs a set / get / del round-trip so that
    /// all implementations get a working health check for free.
    fn ping<'store, 'future>(&'store self) -> KVFuture<'future, ()>
    where
        'store: 'future,
        Self: 'future,
    {
        Box::pin(async move {
            self.set("__ping__", "1", Some(5)).await?;
            self.get("__ping__").await?;
            self.del("__ping__").await?;
            Ok(())
        })
    }
}

/// Cheaply-cloneable, type-erased KV store handle.
///
/// Use this everywhere instead of a concrete type so that the Redis and
/// in-memory implementations are interchangeable at runtime.
pub type KVPool = Arc<dyn KVStore>;

/// Serialize `data` as JSON and store it under `key` with the given TTL in seconds.
pub async fn kv_store_json<T: serde::Serialize>(
    kv: &KVPool,
    key: &str,
    data: &T,
    ttl: u64,
) -> crate::Result<()> {
    let json = serde_json::to_string(data)?;
    kv.set(key, &json, Some(ttl)).await
}

/// Atomically get-and-delete `key`, deserializing the value as `T`.
///
/// Returns `None` if the key is absent or expired.
pub async fn kv_consume_json<T: serde::de::DeserializeOwned>(
    kv: &KVPool,
    key: &str,
) -> crate::Result<Option<T>> {
    match kv.getdel(key).await? {
        Some(json) => Ok(Some(serde_json::from_str(&json)?)),
        None => Ok(None),
    }
}

/// Get `key` without deleting it, deserializing the value as `T`.
///
/// Returns `None` if the key is absent or expired.
pub async fn kv_peek_json<T: serde::de::DeserializeOwned>(
    kv: &KVPool,
    key: &str,
) -> crate::Result<Option<T>> {
    match kv.get(key).await? {
        Some(json) => Ok(Some(serde_json::from_str(&json)?)),
        None => Ok(None),
    }
}

/// Create the appropriate [`KVPool`] based on the optional Redis URL.
///
/// - `Some(url)` → [`crate::kv_store_redis::RedisKVStore`] (connects to Redis)
/// - `None` → [`crate::kv_store_memory::InMemoryKVStore`] with a 30-second
///   background expiry sweep
pub async fn create_kv_store(redis_url: Option<&str>) -> crate::Result<KVPool> {
    match redis_url {
        Some(url) => {
            tracing::info!("KVStore: using Redis backend");
            let store = crate::kv_store_redis::RedisKVStore::new(url).await?;
            Ok(Arc::new(store))
        }
        None => {
            tracing::info!("KVStore: using in-memory backend (no Redis URL configured)");
            let pool = crate::kv_store_memory::InMemoryKVStore::new_pool();
            Ok(pool)
        }
    }
}

#[cfg(test)]
mod tests {
    fn require_send<T: Send>(value: T) -> T {
        value
    }

    #[tokio::test]
    async fn dyn_store_futures_are_send_and_borrow_their_inputs() {
        let kv = crate::kv_store_memory::InMemoryKVStore::new_pool();
        let key = String::from("borrowed-key");
        let value = String::from("borrowed-value");
        require_send(kv.set(&key, &value, Some(5)))
            .await
            .expect("storing borrowed strings through dyn KVStore");
        assert_eq!(
            require_send(kv.get(&key))
                .await
                .expect("reading through dyn KVStore"),
            Some(value)
        );
        require_send(kv.ping())
            .await
            .expect("running the default health check through dyn KVStore");
        assert_eq!(
            kv.get("__ping__")
                .await
                .expect("checking the default health check removed its key"),
            None
        );
    }
}
