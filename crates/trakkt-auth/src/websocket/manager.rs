// SPDX-License-Identifier: AGPL-3.0-or-later

//! WebSocket connection manager with optional Redis pub/sub for multi-replica delivery.
//!
//! Each pod tracks its local WebSocket connections in a `DashMap`.
//!
//! Two operating modes:
//! - **Multi-replica** (`REDIS_URL` set): Cross-pod delivery goes through Redis
//!   PUBLISH/SUBSCRIBE on `ws:user:{user_id}`. Each user with at least one local
//!   connection has a background Redis subscriber. Subscriber delivers incoming
//!   messages to all local WS connections for that user. When the last connection
//!   for a user closes, the subscriber is cancelled.
//! - **Single-instance** (`REDIS_URL` absent): Messages are delivered directly
//!   to local connections — no pub/sub overhead, no external dependency.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use futures_util::StreamExt;
use trakkt_core::{DbPool, RedisPool, WebSocketMessage};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;

/// Maximum number of WebSocket connections allowed per user.
/// Prevents a single user from exhausting server resources.
const MAX_CONNECTIONS_PER_USER: usize = 10;

/// Capacity of the bounded mpsc channel between the WebSocket manager
/// and each connection's outbound task. A client that lets this fill is not
/// keeping up; its connection is terminated rather than served a message with
/// a hole in it (see [`WebSocketManager::deliver_to_local_user`]).
const WS_CHANNEL_CAPACITY: usize = 1024;

/// Sender half of an mpsc channel — the WS endpoint writes received
/// `axum::extract::ws::Message` items to this.
pub type WsSender = mpsc::Sender<String>;

/// Signal that asks a connection's socket tasks to shut down.
///
/// Firing it is the only way the manager can close a socket: since the
/// connection's own `tx` lives in its receive task for the connection's whole
/// lifetime, dropping the manager's sender (or removing the connection from
/// the map) never closes the outbound channel.
pub type KillSignal = Arc<Notify>;

/// Marks a connection as mid-catch-up (a bootstrap or delta stream in flight).
///
/// A catch-up stream writes with `send().await`, so it keeps the outbound
/// channel at capacity for as long as it runs — on a large workspace that is
/// thousands of frames. Every live edit landing in that window would otherwise
/// see a full buffer and kill a connection that is working exactly as intended.
/// Set it with [`CatchUpGuard`], never by hand.
pub type CatchUpFlag = Arc<AtomicBool>;

/// Marks a connection as catching up for as long as the guard is alive.
///
/// The flag has to be cleared on *every* exit path — a connection left flagged
/// would be permanently exempt from disconnect-on-full, silently reinstating
/// the message loss this whole mechanism exists to prevent. `Drop` covers the
/// early returns, the `?`s and a task aborted mid-stream; a manual reset does
/// not. It does not cover a panic here: the release profile sets
/// `panic = "abort"`, so the process dies before any destructor runs — which
/// clears the flag anyway, along with everything else.
///
/// One connection is never catching up twice at once: its socket task awaits
/// each client message before reading the next, so the streams are sequential.
pub struct CatchUpGuard(CatchUpFlag);

impl CatchUpGuard {
    /// Flag `flag`'s connection as catching up until the returned guard drops.
    pub fn new(flag: &CatchUpFlag) -> Self {
        flag.store(true, Ordering::Release);
        Self(Arc::clone(flag))
    }
}

impl Drop for CatchUpGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Monotonically increasing ID for connection deduplication.
static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

/// A tracked WebSocket connection (sender + unique ID + kill signal).
#[derive(Debug, Clone)]
pub struct TrackedConnection {
    pub id: u64,
    pub sender: WsSender,
    /// Fired to tear this connection down; see [`KillSignal`].
    pub kill: KillSignal,
    /// Set while this connection is being caught up; see [`CatchUpFlag`].
    pub catching_up: CatchUpFlag,
}

impl PartialEq for TrackedConnection {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TrackedConnection {}

impl Hash for TrackedConnection {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// Everything a caller needs to drive one registered WebSocket connection.
///
/// `rx` feeds the connection's outbound socket task. `tx` addresses *this*
/// connection alone — request/response traffic (sync bootstrap, delta,
/// complete, reset) must use it so a response is never fanned out to the
/// user's other connections, which would corrupt their sync watermarks.
pub struct ConnectionHandle {
    /// Unique ID of this connection, required to `disconnect()` it later.
    pub id: u64,
    /// Receiving end of the connection's outbound queue.
    pub rx: mpsc::Receiver<String>,
    /// Sending end of the connection's outbound queue.
    pub tx: WsSender,
    /// Fired by the manager when this connection must be torn down. The caller
    /// owns the teardown: it has to stop driving the socket when this fires,
    /// otherwise the connection lives on unregistered and deaf.
    pub kill: KillSignal,
    /// The caller must hold a [`CatchUpGuard`] on this for the whole of any
    /// bootstrap or delta stream it writes to `tx`, or that stream's own
    /// backpressure will get the connection killed.
    pub catching_up: CatchUpFlag,
}

/// Redis pub/sub channel prefix for user messages.
const REDIS_CHANNEL_PREFIX: &str = "ws:user:";

/// Internal shared state behind Arc (cheap clones).
struct Inner {
    /// Local WS connections per user_id.
    connections: DashMap<String, HashSet<TrackedConnection>>,
    /// Active Redis subscriber tasks per user_id.
    subscribers: DashMap<String, JoinHandle<()>>,
    /// `Some` = multi-replica mode (Redis pub/sub enabled).
    /// `None` = single-instance mode (direct local delivery, no pub/sub).
    /// Tuple is (ConnectionManager for PUBLISH, redis_url for subscriber connections).
    redis: Option<(RedisPool, String)>,
    /// Database pool (for broadcast_to_workspace — queries workspace_users).
    db: DbPool,
}

/// Unified WebSocket manager. Cheaply cloneable (inner Arc).
#[derive(Clone)]
pub struct WebSocketManager {
    inner: Arc<Inner>,
}

impl WebSocketManager {
    /// Create a new manager.
    ///
    /// Pass `Some((pool, url))` to enable multi-replica Redis pub/sub mode.
    /// Pass `None` to run in single-instance mode with direct local delivery.
    pub fn new(redis: Option<(RedisPool, String)>, db: DbPool) -> Self {
        Self {
            inner: Arc::new(Inner {
                connections: DashMap::new(),
                subscribers: DashMap::new(),
                redis,
                db,
            }),
        }
    }

    /// Get a clone of the Redis connection pool, if Redis is configured.
    pub fn redis_pool(&self) -> Option<RedisPool> {
        self.inner.redis.as_ref().map(|(pool, _)| pool.clone())
    }

    /// Register a new WebSocket connection for a user.
    ///
    /// Returns a [`ConnectionHandle`] on success, or `Err` if the user has
    /// reached `MAX_CONNECTIONS_PER_USER`. The caller should forward items from
    /// `handle.rx` to the actual WebSocket sink, and use `handle.tx` to address
    /// this connection alone.
    ///
    /// Starts a Redis subscriber if this is the first connection for the user.
    pub fn connect(&self, user_id: &str) -> Result<ConnectionHandle, String> {
        // Prune stale connections before checking the limit.
        // A connection is stale when its mpsc sender is closed (the outbound
        // task dropped the receiver — e.g., after a server restart killed the
        // TCP socket). Without this, dead connections count toward the limit
        // and block new connections indefinitely.
        if let Some(mut conns) = self.inner.connections.get_mut(user_id) {
            let before = conns.len();
            conns.retain(|c| !c.sender.is_closed());
            let pruned = before - conns.len();
            if pruned > 0 {
                tracing::info!(
                    user_id,
                    pruned,
                    remaining = conns.len(),
                    "Pruned stale WebSocket connections"
                );
            }
        }

        // Enforce per-user connection limit.
        if let Some(conns) = self.inner.connections.get(user_id)
            && conns.len() >= MAX_CONNECTIONS_PER_USER
        {
            tracing::warn!(
                user_id,
                limit = MAX_CONNECTIONS_PER_USER,
                "WebSocket connection rejected: per-user limit reached"
            );
            return Err("per-user connection limit reached".into());
        }

        let conn_id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(WS_CHANNEL_CAPACITY);
        let kill: KillSignal = Arc::new(Notify::new());
        let catching_up: CatchUpFlag = Arc::new(AtomicBool::new(false));

        let conn = TrackedConnection {
            id: conn_id,
            sender: tx.clone(),
            kill: Arc::clone(&kill),
            catching_up: Arc::clone(&catching_up),
        };

        // Add to local connections.
        self.inner
            .connections
            .entry(user_id.to_string())
            .or_default()
            .insert(conn);

        // Start Redis subscriber if first connection for this user (multi-replica mode only).
        if self.inner.redis.is_some() && !self.inner.subscribers.contains_key(user_id) {
            self.start_redis_subscriber(user_id);
        }

        // Send heartbeat immediately (directly, not via Redis).
        let heartbeat = WebSocketMessage::new(trakkt_core::MessageType::Heartbeat);
        if let Ok(json) = serde_json::to_string(&heartbeat) {
            self.deliver_to_local_user(user_id, &json);
        }

        Ok(ConnectionHandle {
            id: conn_id,
            rx,
            tx,
            kill,
            catching_up,
        })
    }

    /// Unregister a WebSocket connection.
    ///
    /// If this was the last connection for the user, cancels the Redis subscriber.
    ///
    /// Uses `remove_if` to atomically check emptiness and remove, preventing a
    /// TOCTOU race where `connect()` could add a new connection between the
    /// retain and the removal.
    pub fn disconnect(&self, user_id: &str, connection_id: u64) {
        // Step 1: Remove the specific connection (holds shard write lock during get_mut).
        if let Some(mut conns) = self.inner.connections.get_mut(user_id) {
            conns.retain(|c| c.id != connection_id);
        }

        // Step 2: Atomically remove the entry only if still empty.
        // remove_if holds the shard lock while checking the predicate, so if
        // connect() inserted a new connection between step 1 and step 2, the
        // set won't be empty and this is a no-op.
        let removed_last = self
            .inner
            .connections
            .remove_if(user_id, |_, conns| conns.is_empty())
            .is_some();

        if removed_last && self.inner.redis.is_some()
            && let Some((_, handle)) = self.inner.subscribers.remove(user_id)
        {
            handle.abort();
        }
    }

    // ── Delivery (private) ────────────────────────────────────────────────

    /// Route a pre-serialized JSON message to a user.
    ///
    /// The caller doesn't know or care whether delivery uses Redis pub/sub
    /// (multi-pod) or direct local dispatch (single-instance). This is the
    /// single decision point for that routing.
    async fn deliver(&self, user_id: &str, json: &str) {
        if let Some((redis, _)) = &self.inner.redis {
            let channel = format!("{REDIS_CHANNEL_PREFIX}{user_id}");
            let mut conn = redis.clone();
            if let Err(e) = redis::cmd("PUBLISH")
                .arg(&channel)
                .arg(json)
                .query_async::<i64>(&mut conn)
                .await
            {
                tracing::error!("Redis PUBLISH to {channel} failed: {e}");
            }
        } else {
            self.deliver_to_local_user(user_id, json);
        }
    }

    // ── Public send methods ─────────────────────────────────────────────

    /// Send a typed WebSocketMessage to a user.
    pub async fn send_to_user(&self, user_id: &str, message: WebSocketMessage) {
        let json = match serde_json::to_string(&message) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("Failed to serialize WS message: {e}");
                return;
            }
        };
        self.deliver(user_id, &json).await;
    }

    /// Send a pre-serialized JSON string to a user.
    pub async fn send_to_user_raw(&self, user_id: &str, json: &str) {
        self.deliver(user_id, json).await;
    }

    /// Broadcast a message to all members of a workspace.
    ///
    /// Serializes once, delivers to each member. Optionally excludes one
    /// user (typically the sender to avoid echo).
    pub async fn broadcast_to_workspace(
        &self,
        workspace_id: &str,
        message: WebSocketMessage,
        exclude_user_id: Option<&str>,
    ) {
        let json = match serde_json::to_string(&message) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("Failed to serialize WS message for broadcast: {e}");
                return;
            }
        };

        let members: Vec<(String,)> = match trakkt_core::db_fetch_all!(
            &self.inner.db,
            (String,),
            "SELECT user_id FROM workspace_users WHERE workspace_id = $1",
            workspace_id
        ) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!("Failed to query workspace members for broadcast: {e}");
                return;
            }
        };

        for (member_user_id,) in members {
            if exclude_user_id == Some(member_user_id.as_str()) {
                continue;
            }
            self.deliver(&member_user_id, &json).await;
        }
    }

    /// Broadcast a pre-serialized JSON string to all members of a workspace.
    ///
    /// Same as `broadcast_to_workspace` but accepts raw JSON instead of a
    /// `WebSocketMessage`. Used by the sync protocol to broadcast
    /// `SyncResponse` messages that don't wrap in the `WebSocketMessage` envelope.
    pub async fn broadcast_raw_to_workspace(
        &self,
        workspace_id: &str,
        json: &str,
    ) {
        let members: Vec<(String,)> = match trakkt_core::db_fetch_all!(
            &self.inner.db,
            (String,),
            "SELECT user_id FROM workspace_users WHERE workspace_id = $1",
            workspace_id
        ) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!("Failed to query workspace members for broadcast: {e}");
                return;
            }
        };

        for (member_user_id,) in members {
            self.deliver(&member_user_id, json).await;
        }
    }

    /// Get the count of local connections across all users on this pod.
    pub fn local_connection_count(&self) -> usize {
        self.inner
            .connections
            .iter()
            .map(|entry| entry.value().len())
            .sum()
    }

    /// Start a Redis subscriber for a user. Runs in a background tokio task.
    ///
    /// IMPORTANT: `ConnectionManager` (RedisPool) does NOT support SUBSCRIBE.
    /// We must create a fresh `redis::Client` for the subscriber connection.
    ///
    /// Only called when `self.inner.redis.is_some()`.
    fn start_redis_subscriber(&self, user_id: &str) {
        let (_, redis_url) = match &self.inner.redis {
            Some(r) => r,
            None => unreachable!("start_redis_subscriber called without Redis configured"),
        };
        let user_id_owned = user_id.to_string();
        let channel = format!("{REDIS_CHANNEL_PREFIX}{user_id_owned}");
        let redis_url = redis_url.clone();
        let manager = self.clone();

        let handle = tokio::spawn(async move {
            let user_id = user_id_owned;
            // Create a dedicated Redis connection for SUBSCRIBE.
            let client = match redis::Client::open(redis_url.as_str()) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Redis subscriber client creation failed for {user_id}: {e}");
                    return;
                }
            };

            let mut pubsub = match client.get_async_pubsub().await {
                Ok(ps) => ps,
                Err(e) => {
                    tracing::error!("Redis SUBSCRIBE connection failed for {user_id}: {e}");
                    return;
                }
            };

            if let Err(e) = pubsub.subscribe(&channel).await {
                tracing::error!("Redis SUBSCRIBE to {channel} failed: {e}");
                return;
            }

            tracing::debug!("Redis subscriber started for {user_id} on {channel}");

            let mut stream = pubsub.on_message();

            while let Some(msg) = stream.next().await {
                let payload: String = match msg.get_payload() {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("Bad Redis pub/sub payload for {user_id}: {e}");
                        continue;
                    }
                };

                manager.deliver_to_local_user(&user_id, &payload);
            }

            tracing::debug!("Redis subscriber ended for {user_id}");
        });

        self.inner.subscribers.insert(user_id.to_string(), handle);
    }

    /// Deliver a JSON-encoded message to all local WebSocket connections for a user.
    ///
    /// Uses `try_send` so one connection can never block delivery to the others.
    ///
    /// A full channel means the client is not draining its socket. Dropping the
    /// message there would lose a sync frame permanently and invisibly — the
    /// client's cursor only advances on `SyncComplete`, so nothing would ever
    /// re-fetch it. Instead the connection is killed and unregistered: the
    /// client reconnects and delta-syncs from its cursor, which recovers the
    /// missed change. Closed connections are unregistered as before.
    ///
    /// The exception is a connection mid-catch-up. A bootstrap or delta stream
    /// writes with `send().await`, so it holds the channel at capacity for its
    /// whole run; killing on `Full` there would kill every client loading a
    /// large workspace while anyone is editing it, and the client would
    /// reconnect straight back into the same bootstrap. Dropping the frame is
    /// safe *only* in that window: the in-flight stream's `SyncComplete`
    /// watermark was read before this change landed, so the client's cursor
    /// ends up below it and a later delta re-delivers it — bounded staleness,
    /// not permanent loss.
    ///
    /// Known residual: the flag clears when `SyncComplete` is queued, while the
    /// channel may still hold ~1024 undrained frames, so an edit in that gap
    /// can still kill the connection. Drain time is short and the client
    /// recovers by reconnecting, so this is accepted rather than overlooked.
    ///
    /// This is also the delivery path the Redis subscriber uses, so slow
    /// consumers are handled identically in single-pod and multi-pod mode.
    fn deliver_to_local_user(&self, user_id: &str, json: &str) {
        let mut stale_ids: Vec<u64> = Vec::new();

        if let Some(conns) = self.inner.connections.get(user_id) {
            for conn in conns.value().iter() {
                match conn.sender.try_send(json.to_string()) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        stale_ids.push(conn.id);
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        if conn.catching_up.load(Ordering::Acquire) {
                            tracing::warn!(
                                user_id,
                                connection_id = conn.id,
                                "WebSocket send buffer full during catch-up, dropping live frame"
                            );
                            continue;
                        }
                        tracing::warn!(
                            user_id,
                            connection_id = conn.id,
                            "WebSocket send buffer full, disconnecting slow sync consumer"
                        );
                        // `notify_one` leaves a permit behind, so the socket
                        // task is torn down even if it has not started waiting
                        // on the signal yet.
                        conn.kill.notify_one();
                        stale_ids.push(conn.id);
                    }
                }
            }
        }

        // Unregister the connections that are closed or being killed.
        if !stale_ids.is_empty()
            && let Some(mut conns) = self.inner.connections.get_mut(user_id)
        {
            conns.retain(|c| !stale_ids.contains(&c.id));
        }
    }
}

impl std::fmt::Debug for WebSocketManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketManager")
            .field("local_users", &self.inner.connections.len())
            .field("local_connections", &self.local_connection_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Single-instance manager (no Redis) backed by a throwaway in-memory DB.
    /// None of the paths exercised here run a query; the pool only satisfies
    /// `WebSocketManager::new`.
    async fn test_manager() -> WebSocketManager {
        let db = DbPool::connect("sqlite::memory:")
            .await
            .expect("in-memory SQLite pool");
        WebSocketManager::new(None, db)
    }

    /// `connect()` heartbeats every local connection of the user, so a receiver
    /// starts with one frame per connect that happened after it registered.
    /// Drain them so the assertions below only observe test traffic.
    async fn drain_heartbeats(rx: &mut mpsc::Receiver<String>, expected: usize) {
        for _ in 0..expected {
            let frame = rx.recv().await.expect("heartbeat frame");
            let parsed: WebSocketMessage =
                serde_json::from_str(&frame).expect("heartbeat frame is a WebSocketMessage");
            assert!(
                matches!(parsed.msg_type, trakkt_core::MessageType::Heartbeat),
                "expected heartbeat, got {frame}"
            );
        }
    }

    #[tokio::test]
    async fn connection_sender_addresses_only_its_own_connection() {
        let manager = test_manager().await;
        let user_id = "usr_two_browsers";

        let mut first = manager.connect(user_id).expect("first connection");
        let mut second = manager.connect(user_id).expect("second connection");

        // `first` saw its own connect heartbeat plus the one `second` triggered.
        drain_heartbeats(&mut first.rx, 2).await;
        drain_heartbeats(&mut second.rx, 1).await;

        first
            .tx
            .send("bootstrap-for-first".to_string())
            .await
            .expect("send to the first connection");

        assert_eq!(
            first.rx.recv().await.as_deref(),
            Some("bootstrap-for-first")
        );
        assert!(
            matches!(second.rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "the second connection must not observe the first connection's sync traffic"
        );
    }

    #[tokio::test]
    async fn live_broadcast_still_reaches_every_connection_of_the_user() {
        let manager = test_manager().await;
        let user_id = "usr_two_browsers";

        let mut first = manager.connect(user_id).expect("first connection");
        let mut second = manager.connect(user_id).expect("second connection");

        drain_heartbeats(&mut first.rx, 2).await;
        drain_heartbeats(&mut second.rx, 1).await;

        manager.deliver_to_local_user(user_id, "workspace-broadcast");

        assert_eq!(first.rx.recv().await.as_deref(), Some("workspace-broadcast"));
        assert_eq!(second.rx.recv().await.as_deref(), Some("workspace-broadcast"));
    }

    /// Queue messages until the connection's outbound channel refuses more.
    /// The connection is left registered — only its buffer is full.
    fn saturate(handle: &ConnectionHandle) {
        let mut queued = 0;
        while handle.tx.try_send("backlog".to_string()).is_ok() {
            queued += 1;
        }
        assert!(
            queued > 0,
            "expected to queue at least one message before the buffer filled"
        );
    }

    /// A connection that fired its kill signal must be torn down promptly; the
    /// signal carries a permit, so this resolves without the socket task having
    /// been waiting beforehand.
    async fn assert_killed(handle: &ConnectionHandle, what: &str) {
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            handle.kill.notified(),
        )
        .await
        .unwrap_or_else(|_| panic!("{what}: kill signal was never fired"));
    }

    #[tokio::test]
    async fn saturated_connection_is_killed_and_unregistered() {
        let manager = test_manager().await;
        let user_id = "usr_slow_client";

        // The connect heartbeat is never read, so the buffer only fills up.
        let slow = manager.connect(user_id).expect("connection");
        saturate(&slow);
        assert_eq!(manager.local_connection_count(), 1);

        manager.deliver_to_local_user(user_id, "workspace-broadcast");

        assert_eq!(
            manager.local_connection_count(),
            0,
            "a connection that cannot take its sync frame must be unregistered"
        );
        assert_killed(&slow, "saturated connection").await;
    }

    #[tokio::test]
    async fn saturated_connection_survives_while_it_is_catching_up() {
        let manager = test_manager().await;
        let user_id = "usr_bootstrapping_client";

        // A bootstrap stream holds the channel at capacity for its whole run.
        let loading = manager.connect(user_id).expect("connection");
        let _catching_up = CatchUpGuard::new(&loading.catching_up);
        saturate(&loading);

        manager.deliver_to_local_user(user_id, "workspace-broadcast");

        assert_eq!(
            manager.local_connection_count(),
            1,
            "a connection mid-catch-up must stay registered when a live frame cannot fit"
        );
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                loading.kill.notified(),
            )
            .await
            .is_err(),
            "a connection mid-catch-up must not be killed"
        );
    }

    #[tokio::test]
    async fn a_finished_catch_up_no_longer_exempts_the_connection() {
        let manager = test_manager().await;
        let user_id = "usr_finished_bootstrap";

        let slow = manager.connect(user_id).expect("connection");
        // The guard's scope is the stream; once it ends the exemption ends.
        {
            let _catching_up = CatchUpGuard::new(&slow.catching_up);
        }
        saturate(&slow);

        manager.deliver_to_local_user(user_id, "workspace-broadcast");

        assert_eq!(
            manager.local_connection_count(),
            0,
            "the exemption must not outlive the catch-up stream"
        );
        assert_killed(&slow, "connection that finished catching up").await;
    }

    #[tokio::test]
    async fn one_saturated_connection_does_not_cost_a_healthy_one_its_frame() {
        let manager = test_manager().await;
        let user_id = "usr_two_browsers";

        let slow = manager.connect(user_id).expect("first connection");
        let mut healthy = manager.connect(user_id).expect("second connection");

        drain_heartbeats(&mut healthy.rx, 1).await;
        saturate(&slow);

        manager.deliver_to_local_user(user_id, "workspace-broadcast");

        assert_eq!(
            healthy.rx.recv().await.as_deref(),
            Some("workspace-broadcast"),
            "a healthy connection must still get the frame"
        );
        assert_eq!(
            manager.local_connection_count(),
            1,
            "only the saturated connection should be unregistered"
        );
        assert_killed(&slow, "saturated connection").await;
    }
}
