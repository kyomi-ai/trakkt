// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the local cache is allowed to hold — the one place that decides it.
//!
//! Three separate things used to answer that question, and nothing made them
//! agree:
//!
//! * **what is written** — [`enqueue_cache_writes`](crate::cache::apply::enqueue_cache_writes)
//!   persisted anything carrying a payload, minus an exclusion list;
//! * **what is wiped** — a hand-maintained `ALL_CACHED_ENTITY_TYPES` array in
//!   [`sync_engine`](crate::cache::sync_engine) driving `DeleteAllOfType` on
//!   `SyncReset` and on the no-cursor cold start;
//! * **what is removed per entity** — a hand-written `match` over entity types
//!   in the same function's `Delete` arm.
//!
//! The insert path being generic while the delete path was enumerated is what
//! made rows leak: a type that was persisted but had no delete arm had its row
//! written and never removed, so it outlived the entity until the next full
//! reset. `attachment`, `issue_relation`, `notification_preferences` and
//! `workspace_settings` were all in that state (TRA-9966).
//!
//! All three now read [`cache_rows_written_by`]. It maps one entity type to the
//! cache rows a frame of that type writes, and everything else is derived from
//! that mapping: the wipe list is its union over
//! [`entity_types::ALL`](trakkt_types::sync::entity_types::ALL), and the
//! per-entity delete is the very same slice the write path was gated on. There
//! is no second list to disagree with the first.
//!
//! Not target-gated: this is pure Rust over string constants, so the invariants
//! it defines are unit tested natively rather than only under `wasm-pack test`.

use trakkt_types::sync::entity_types;

/// Entity types that arrive on the sync stream and are deliberately **not**
/// written to the local cache.
///
/// Every one is here for the same reason, and it is a conjunction of two facts
/// that were each checked per type:
///
/// 1. **Nothing in this client reads the row back.** Hydration reads eight types
///    (`hydrate_store_from_db` in [`sync_engine`](crate::cache::sync_engine)),
///    and the issue detail page reads `comment` and `issue_content` on demand.
///    No other type is read out of IndexedDB anywhere in the crate.
/// 2. **No bootstrap streams it** (`sync_bootstrap` in
///    `apps/server/src/routes/websocket.rs` streams eleven types, none of these).
///    So a cache of one of these could never be a *list* — only whichever deltas
///    happened to arrive while some tab was open, which is an arbitrary subset
///    that no future reader could trust either.
///
/// Each of these types *is* read by the UI — just never from here. Their frames
/// bump a version counter in
/// [`apply_action_to_memory`](crate::cache::apply::apply_action_to_memory) and
/// the page refetches from a server function, which is why dropping the row
/// changes nothing on screen. `release` is the exception with no reader at all.
///
/// # Per-type evidence
///
/// * `release` — no route, page, server function, hydration step or on-demand
///   read mentions releases; they are created and listed exclusively through the
///   API and MCP tools. The user-visible half of publishing a release does reach
///   the UI, but as the `issue` updates the same transaction emits.
/// * `activity` — both readers (the issue timeline and the workspace feed) call
///   `list_issue_activities` / `list_workspace_activities` when the activity
///   counter bumps. Persisting activities is unbounded growth in step with every
///   status change, comment and field edit in the workspace.
/// * `attachment` — the issue detail page's attachment section reads
///   `list_issue_attachments`. Its rows were written and, before TRA-9966,
///   removed by nothing at all: the delete path had no arm for it (traced to
///   `be3bfad^`).
/// * `issue_attachment` — the same list, the same server function. No row is
///   written today in any case, because `attach_to_issue` in
///   `crates/trakkt-auth/src/attachment_service.rs` sends a `None` payload; the
///   entry here is what stops TRA-9979 from silently starting to write one when
///   it gives that call a real payload.
/// * `notification_preferences` — the notification settings page reads
///   `get_notification_preferences`.
/// * `issue_relation` — the relations section reads `list_issue_relations`.
///
/// # What adding an entry does, and what removing one does
///
/// Adding a type here takes it off the write path, off the reset wipe and off
/// the per-entity delete in one edit, because all three are derived from
/// [`cache_rows_written_by`]. It does **not** touch the memory half: the version
/// counter these types bump is what their readers subscribe to, and it must keep
/// firing.
///
/// Rows already written before a type was added here are cleared by the
/// `SCHEMA_HASH` bump in [`db`](crate::cache::db) that accompanies the change —
/// nothing else would ever evict them, since a type here is wiped by no reset.
///
/// Giving one of these a cached reader means the reverse: stream it from the
/// bootstrap so the cache is a whole list rather than a fragment, and drop the
/// entry.
const NOT_CACHED: &[&str] = &[
    entity_types::RELEASE,
    entity_types::ACTIVITY,
    entity_types::ATTACHMENT,
    entity_types::ISSUE_ATTACHMENT,
    entity_types::NOTIFICATION_PREFERENCES,
    entity_types::ISSUE_RELATION,
];

/// The two rows an `issue` frame writes.
///
/// An issue's body is split out of the issue record and stored under its own
/// entity type so hydration does not bulk-load every description in the
/// workspace — see the issue arm of
/// [`enqueue_cache_writes`](crate::cache::apply::enqueue_cache_writes). No
/// server ever sends an `issue_content` frame; the row exists only as a side
/// effect of an issue one, and the delete of an issue has to take it with it.
const ISSUE_ROWS: &[&str] = &[entity_types::ISSUE, entity_types::ISSUE_CONTENT];

/// The cache rows a frame of `entity_type` writes, and therefore the rows its
/// delete has to remove and a reset has to wipe.
///
/// Empty means the cache holds nothing for this type: either it is on
/// [`NOT_CACHED`], or it is not an entity type this protocol declares at all.
/// Both are cases where writing a row would be writing something nothing can
/// ever remove — an undeclared type is in no list a reset iterates, so its rows
/// would survive every wipe.
///
/// This is the whole of the cache's membership rule. The write path is gated on
/// it, the per-entity delete iterates it, and [`all_cached_entity_types`] is its
/// union — so "written but never wiped" and "wiped but never written" are both
/// unrepresentable rather than merely tested for.
pub fn cache_rows_written_by(entity_type: &str) -> &'static [&'static str] {
    // Resolved against the declared universe rather than passed through, so the
    // returned rows are `'static` constants and an undeclared type is rejected
    // instead of being cached under whatever string arrived on the wire.
    let Some(declared) = entity_types::ALL.iter().find(|known| **known == entity_type) else {
        return &[];
    };
    if NOT_CACHED.contains(declared) {
        return &[];
    }
    if *declared == entity_types::ISSUE {
        return ISSUE_ROWS;
    }
    std::slice::from_ref(declared)
}

/// Every entity type a `SyncReset` — and the no-cursor cold start that wipes the
/// same way — has to clear out of the cache.
///
/// The membership rule is not "types this client reads back". It is "types the
/// cache can ever hold a row of", which is a strictly larger set: it includes
/// `issue_content`, which no server ever sends, and every type persisted for a
/// reader that does not exist yet. A type missing from here would be wiped by
/// nothing, so its rows would outlive the reset that exists to guarantee a clean
/// slate.
///
/// Derived from [`cache_rows_written_by`] over every declared entity type, so it
/// cannot disagree with what the write path persists. Order follows
/// [`entity_types::ALL`], which makes the wipe deterministic and its tests
/// readable; duplicates are dropped, because `issue_content` is reachable both
/// as its own declaration and as the second row of an `issue`.
pub fn all_cached_entity_types() -> Vec<&'static str> {
    let mut wiped: Vec<&'static str> = Vec::new();
    for entity_type in entity_types::ALL {
        for row in cache_rows_written_by(entity_type) {
            if !wiped.contains(row) {
                wiped.push(row);
            }
        }
    }
    wiped
}

/// Cache row types that only ever exist as a side effect of some *other* type's
/// frame, and so never arrive as a `SyncAction` of their own.
///
/// `issue_content` is the one today. It is derived rather than named — a row
/// type that some frame writes under a type other than its own — so a second
/// entity ever split out this way exempts itself, and one that stops being split
/// loses its exemption.
///
/// Used by the guard that every cached type reaches the reactive store:
/// `apply_action_to_memory` cannot have an arm for a type no server sends, and
/// demanding one would be demanding dead code.
pub fn side_effect_only_cache_types() -> Vec<&'static str> {
    let mut derived: Vec<&'static str> = Vec::new();
    for entity_type in entity_types::ALL {
        for row in cache_rows_written_by(entity_type) {
            if *row != *entity_type && !derived.contains(row) {
                derived.push(row);
            }
        }
    }
    derived
}

// ── Native unit tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cached_type_owns_exactly_its_own_row() {
        assert_eq!(
            cache_rows_written_by(entity_types::LABEL),
            &[entity_types::LABEL],
            "an ordinary cached type is stored under its own entity type"
        );
    }

    #[test]
    fn an_issue_owns_its_body_row_as_well() {
        assert_eq!(
            cache_rows_written_by(entity_types::ISSUE),
            &[entity_types::ISSUE, entity_types::ISSUE_CONTENT],
            "the body is a separate record, so an issue's delete and the reset wipe both \
             have to account for it"
        );
    }

    #[test]
    fn a_not_cached_type_owns_no_rows() {
        for entity_type in NOT_CACHED {
            assert!(
                cache_rows_written_by(entity_type).is_empty(),
                "{entity_type} is on NOT_CACHED, so nothing may be written for it"
            );
        }
    }

    /// An entity type this protocol does not declare cannot be cached, because
    /// nothing would ever wipe it: the reset iterates the declared universe.
    #[test]
    fn an_undeclared_entity_type_owns_no_rows() {
        assert!(
            cache_rows_written_by("not_an_entity_type").is_empty(),
            "a row written under an undeclared type is in no list a reset iterates, so it \
             would survive every wipe"
        );
    }

    /// The invariant `SyncReset` rests on, stated over the mapping itself:
    /// nothing the cache can hold is missing from the wipe.
    ///
    /// It cannot fail as written — the wipe *is* the union — and that is the
    /// point of the change. The assertion is kept because it is the property a
    /// future edit could take away by giving the wipe a source of its own again,
    /// and because it names the invariant where someone reading the wipe will
    /// look for it.
    #[test]
    fn every_row_the_cache_can_hold_is_wiped_by_a_reset() {
        let wiped = all_cached_entity_types();

        for entity_type in entity_types::ALL {
            for row in cache_rows_written_by(entity_type) {
                assert!(
                    wiped.contains(row),
                    "a frame of `{entity_type}` writes a `{row}` row that no reset clears — \
                     it would outlive the reset that exists to leave a clean slate, and \
                     nothing else would ever remove it"
                );
            }
        }
    }

    /// The converse, which is just as much of a defect: a type the wipe promises
    /// to clear but the write path never writes is a promise this client does
    /// not keep, and it reads as evidence that the type is cached when it is
    /// not.
    #[test]
    fn nothing_is_wiped_that_the_cache_never_writes() {
        for row in all_cached_entity_types() {
            assert!(
                entity_types::ALL
                    .iter()
                    .any(|entity_type| cache_rows_written_by(entity_type).contains(&row)),
                "the reset wipes `{row}`, which no frame ever writes"
            );
        }
    }

    #[test]
    fn the_wipe_list_names_each_type_once() {
        let wiped = all_cached_entity_types();
        let mut seen: Vec<&str> = Vec::new();
        for row in &wiped {
            assert!(
                !seen.contains(row),
                "`{row}` is wiped twice — `issue_content` is reachable both as its own \
                 declaration and as the second row of an issue, and the union has to \
                 collapse that"
            );
            seen.push(row);
        }
    }

    #[test]
    fn the_issue_body_is_the_one_row_no_frame_carries() {
        assert_eq!(
            side_effect_only_cache_types(),
            vec![entity_types::ISSUE_CONTENT],
            "no server sends an `issue_content` frame — the row exists only because an \
             issue's body is split out of the issue record"
        );
    }

    /// Recorded as a decision rather than an accident: every type on
    /// `NOT_CACHED` is one whose reader refetches from a server function, so
    /// dropping the row cannot change anything on screen.
    #[test]
    fn the_cached_set_is_the_declared_set_minus_the_types_nothing_reads() {
        let mut expected: Vec<&str> = entity_types::ALL
            .iter()
            .copied()
            .filter(|t| !NOT_CACHED.contains(t))
            .collect();
        expected.sort_unstable();

        let mut cached = all_cached_entity_types();
        cached.sort_unstable();

        assert_eq!(
            cached, expected,
            "the cache holds exactly the declared types that are not on NOT_CACHED — if \
             these differ, some type is written under a name it was not declared with"
        );
    }
}
