// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sync protocol types shared between trakkt-auth (service layer) and
//! trakkt-ui (WebSocket client).

use serde::{Deserialize, Serialize};

/// Action type for sync log entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncActionType {
    Insert,
    Update,
    Delete,
}

/// A single sync log entry broadcast to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncAction {
    pub sync_id: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub workspace_id: String,
    pub action: SyncActionType,
    pub data: Option<serde_json::Value>,
    pub timestamp: String,
}

/// Server->client sync response envelope.
///
/// Tagged enum serialized as `{"type": "sync_action", ...}` etc.
/// Used by the WebSocket handler to stream bootstrap and delta data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncResponse {
    SyncAction(SyncAction),
    SyncComplete { last_sync_id: i64 },
    SyncReset,
}

/// Well-known entity type constants for sync log entries.
///
/// The constants and [`entity_types::ALL`] come out of a single
/// [`declare_entity_types!`] invocation, so a type cannot exist as a constant
/// without also being in the list. Every consumer that needs "every entity type
/// there is" — the client's cache, and the tests that police it — reads `ALL`
/// rather than restating the set, which is the only reason a new declaration
/// cannot be forgotten somewhere else.
pub mod entity_types {
    /// Declare the entity type constants **and** the complete list of them from
    /// one invocation.
    ///
    /// Rust cannot enumerate a module's constants, so before this the universe
    /// of entity types could only be recovered by writing it out a second time —
    /// which is what `ALL_CACHED_ENTITY_TYPES` in `trakkt-ui` was, and how it
    /// came to be missing eight types (TRA-9940). Emitting both from the same
    /// input removes the second list rather than testing it: there is no edit
    /// that adds a constant without adding it to `ALL`.
    macro_rules! declare_entity_types {
        ($($name:ident = $value:literal;)+) => {
            $(pub const $name: &str = $value;)+

            /// Every entity type declared above, in declaration order.
            ///
            /// This is the universe the client's cache is defined against: see
            /// `cache_rows_written_by` in
            /// `crates/trakkt-ui/src/cache/cached_types.rs`, which maps each of
            /// these to the cache rows a frame of that type writes, and derives
            /// the reset wipe from the same mapping.
            pub const ALL: &[&str] = &[$($name),+];
        };
    }

    declare_entity_types! {
        WORKSPACE_SETTINGS = "workspace_settings";
        ISSUE = "issue";
        COMMENT = "comment";
        LABEL = "label";
        NOTIFICATION = "notification";
        TEAM = "team";
        STATUS = "status";
        PROJECT = "project";
        PROJECT_MILESTONE = "project_milestone";
        PROJECT_MEMBER = "project_member";
        PROJECT_UPDATE = "project_update";
        VIEW = "view";
        FAVORITE = "favorite";
        RELEASE = "release";
        ISSUE_RELATION = "issue_relation";
        ATTACHMENT = "attachment";
        ISSUE_ATTACHMENT = "issue_attachment";
        ACTIVITY = "activity";
        ISSUE_CONTENT = "issue_content";
        NOTIFICATION_PREFERENCES = "notification_preferences";
    }
}

/// An entity the sync bootstrap can put on the wire, and the two facts a frame
/// carrying it needs: what type it is, and how the client addresses it.
///
/// Both used to be restated at each entity's call site in
/// `handle_sync_bootstrap` (`apps/server/src/routes/websocket.rs`) — an
/// `entity_type` constant, and beside it a bare string naming the JSON key the
/// id was then looked up under, e.g. `"issue_id"`. That string had to agree
/// with the entity's serde field name, and nothing checked that it did: a
/// `#[serde(rename)]`, a renamed field, or a plain typo made the lookup miss,
/// and before the runtime guard in `stream_entities` (TRA-9960) the entity went
/// out with an empty `entity_id`, was certified by the trailing `SyncComplete`
/// watermark, and was never re-sent. Eleven such strings, one per batch, each a
/// silent workspace-wide staleness bug for its own entity type.
///
/// Implementing this instead moves both facts onto the type. `entity_id`
/// returns a borrow of the id field itself, so the id no longer travels through
/// the encoded payload at all — which is also why a `#[serde(rename)]` on that
/// field can no longer break *addressing*, though it still renames the key in
/// the payload, which the client decodes with this same type and so follows
/// automatically. A field that does not exist, or is not a `String`, is a
/// compile error at the impl.
///
/// `Serialize` is a supertrait because an entity that cannot be encoded cannot
/// be streamed; the bootstrap needs both from the same type.
pub trait SyncEntity: Serialize {
    /// The `entity_type` tag on every frame carrying this entity.
    ///
    /// Always one of the [`entity_types`] constants, never a fresh literal:
    /// the client's cache is defined against [`entity_types::ALL`] (see
    /// `cache/cached_types.rs` in `trakkt-ui`), so a string outside that set
    /// has no mapping on the other end.
    const ENTITY_TYPE: &'static str;

    /// The entity's primary key, as the client addresses it.
    ///
    /// This is what `cache/apply.rs` keys its IndexedDB upsert on, so it must
    /// be the row's actual identity and not a display field — two rows sharing
    /// a value here overwrite each other in the client's cache.
    fn entity_id(&self) -> &str;
}

#[cfg(test)]
mod entity_type_tests {
    use std::collections::BTreeSet;

    use super::entity_types;

    /// Two constants sharing a wire string would make every consumer that keys
    /// off `ALL` — the cache's row mapping, its reset wipe — silently treat them
    /// as one type.
    #[test]
    fn every_declared_entity_type_is_a_distinct_wire_string() {
        let unique: BTreeSet<&str> = entity_types::ALL.iter().copied().collect();

        assert_eq!(
            unique.len(),
            entity_types::ALL.len(),
            "`entity_types::ALL` holds a duplicate wire string: {:?}",
            entity_types::ALL
        );
    }

    /// `ALL` is emitted by the same macro invocation that declares the
    /// constants, so an empty one would mean the invocation itself is gone and
    /// every derived list downstream had quietly become empty.
    #[test]
    fn the_declared_entity_types_are_not_empty() {
        assert!(
            !entity_types::ALL.is_empty(),
            "`entity_types::ALL` is empty — everything derived from it, including the \
             client's cache wipe, is now a no-op"
        );
    }
}
