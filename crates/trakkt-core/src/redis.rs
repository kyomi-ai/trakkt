// SPDX-License-Identifier: AGPL-3.0-or-later

//! Redis connection manager via the `redis` crate.

use redis::aio::ConnectionManager;

/// Type alias for the Redis connection manager.
///
/// `ConnectionManager` automatically reconnects on failure — no manual retry
/// logic needed.  It is cheaply cloneable (internally Arc'd).
pub type RedisPool = ConnectionManager;

/// Create a Redis connection manager from the given URL.
pub async fn create_pool(redis_url: &str) -> crate::Result<RedisPool> {
    let client = redis::Client::open(redis_url)
        .map_err(|e| crate::Error::Internal(format!("invalid redis URL: {e}")))?;

    let manager = ConnectionManager::new(client)
        .await
        .map_err(|e| crate::Error::Internal(format!("redis connection failed: {e}")))?;

    tracing::info!("Redis connection manager ready");
    Ok(manager)
}

/// Run a PING command — useful for health endpoints.
pub async fn ping(conn: &mut RedisPool) -> crate::Result<()> {
    redis::cmd("PING")
        .query_async::<String>(conn)
        .await
        .map_err(|e| crate::Error::Internal(format!("redis ping failed: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::time::Duration;

    /// How long the test waits for Redis before declaring it unreachable.
    ///
    /// `create_pool` builds a [`ConnectionManager`], which retries with
    /// exponential backoff internally and takes roughly eight minutes to give
    /// up.  That behaviour is correct for production callers — a server should
    /// ride out a Redis restart — so `create_pool` is left alone and the bound
    /// is applied here, at the call site that wants to fail fast.
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

    /// Opens a connection manager against a live Redis and PINGs it.
    ///
    /// This test needs a real Redis listening on `REDIS_URL` (default
    /// `redis://localhost:6381`).  Run it explicitly:
    ///
    /// ```text
    /// cargo test -p trakkt-core -- --ignored
    /// ```
    ///
    /// # Why `#[ignore]` rather than skipping when Redis is absent
    ///
    /// The tempting alternative is to catch the connection error and return
    /// early.  Don't.  A test that returns early prints `test_redis_connects
    /// ... ok`, which is indistinguishable from a run where Redis *was* present
    /// and the code genuinely worked.  The day `create_pool` or `ping` breaks,
    /// every machine without Redis — which is most of them — would report
    /// success.  `#[ignore]` prints `1 ignored`, which says plainly that nothing
    /// was verified.  Please do not "improve" this into a silent skip.
    ///
    /// # Why port 6381 and not 6379
    ///
    /// 6381 is deliberate, not a typo.  These projects allocate Redis ports in a
    /// ladder — 6379 production, 6380 local development, 6381 tests — so that a
    /// test run can never reach a Redis the developer is using for something
    /// else.  (Postgres follows the same scheme: 5432/5433/5434, which is why
    /// `Config::test_config` points at 5434.)  Pointing this at the default 6379
    /// would make the test talk to whatever Redis happens to be running.  Set
    /// `REDIS_URL` to override.
    #[tokio::test]
    #[ignore = "requires a live Redis; run with `cargo test -p trakkt-core -- --ignored`"]
    async fn test_redis_connects() {
        let cfg = Config::test_config();
        let url = cfg.redis_url.as_deref().unwrap_or("redis://localhost:6381");

        let mut pool = match tokio::time::timeout(CONNECT_TIMEOUT, create_pool(url)).await {
            Ok(result) => result.expect("redis should connect"),
            Err(_elapsed) => panic!(
                "no Redis answered at {url} within {}s — start one (e.g. \
                 `podman run --rm -p 6381:6379 docker.io/library/redis:7-alpine`) or point \
                 REDIS_URL at an existing instance",
                CONNECT_TIMEOUT.as_secs()
            ),
        };

        ping(&mut pool).await.expect("ping should succeed");
    }
}
