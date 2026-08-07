-- TRA-10025: remove the favorites that already outlived what they pointed at.
--
-- SQLite counterpart of migrations/20260807000000_prune_dangling_favorites.sql.
-- Byte-identical to it: every statement below is plain DML that both dialects
-- parse the same way, and there is no schema change to adapt. No table is
-- rebuilt, so none of the DROP-TABLE-fires-cascades hazard recorded in
-- migrations-sqlite/20260803100000_dual_backend_fk_parity.sql's header applies.
--
-- `favorites.target_id` is polymorphic — one TEXT column naming a row in
-- whichever table `target_type` selects — so no foreign key can express it and
-- nothing in either dialect's schema ever removed a favorite when its target was
-- deleted. From TRA-10025 each parent's delete path removes them itself, but
-- that only governs deletions from here on. This migration is the backlog: rows
-- whose target went before the delete paths learned to take them.
--
-- ─── No schema change, and why not ──────────────────────────────────────────
--
-- The obvious repair is a real foreign key — per-type nullable columns
-- (`issue_id`, `project_id`, …) each with ON DELETE CASCADE. It was rejected,
-- and the reason is worth recording here because this file is where the next
-- person will come looking. A database-level CASCADE removes the row *silently*.
-- `favorite` is a cached type: `sync_bootstrap` streams it and
-- `crates/trakkt-ui/src/cache/apply.rs` writes it to IndexedDB, so a favorite
-- that disappears server-side without a `sync_log` entry stays in its owner's
-- cache through every reconnect, with no later delta able to evict it — the
-- defect TRA-9971 and TRA-9957 exist for. The delete paths would therefore still
-- have to read the doomed rows and write those entries by hand, exactly as they
-- do now; the foreign key would add a schema migration and a SQLite table
-- rebuild without removing one line of that. It would convert a visible wrong
-- row in a table into an invisible wrong row in a browser.
--
-- ─── What is deleted ────────────────────────────────────────────────────────
--
-- Two kinds of row, both dead by construction.
--
-- 1. A favorite of a known type whose target no longer exists. It renders as
--    nothing, and re-arrives on every bootstrap because the server still has it.
--
-- 2. A favorite whose `target_type` is not one of the four
--    `trakkt_types::enums::FavoriteTarget` variants. Until TRA-10025
--    `add_favorite` took the type as a free string straight off an HTTP request
--    and stored whatever it was given, so such rows are possible. Nothing can
--    render one (the sidebar star compares against a known type), no delete path
--    covers one, and since TRA-10025 `remove_favorite` cannot even be called
--    with the type — it takes the enum — so the owner has no way to unpin it.
--
-- Both are unreachable rows, not user data with a lost pointer: `favorites`
-- carries nothing but the pointer.
--
-- ─── What does not happen, stated plainly ───────────────────────────────────
--
-- No `sync_log` entry is written for these deletes, and none can be: a migration
-- runs before the application is serving and has no workspace, no watermark and
-- no connections to deliver to. A client that cached one of these rows therefore
-- keeps it until its next `SyncReset` or cold start, both of which wipe
-- `favorite` (`ALL_CACHED_ENTITY_TYPES` in
-- `crates/trakkt-ui/src/cache/cached_types.rs`) and re-bootstrap. That is
-- strictly better than the status quo, which is what makes it acceptable here:
-- today the reset wipes the row and the bootstrap puts it straight back, because
-- the server still has it. After this migration the reset clears it for good.
--
-- Measured before writing this: the production database held four favorites, all
-- `target_type = 'team'`, all naming teams that still exist — zero dangling —
-- and the development database held none at all. `delete_team` is the one delete
-- path that already removed favorites before TRA-10025, which is exactly why the
-- only type in production is the only type that was ever cleaned up. So this
-- migration is expected to delete nothing on Trakkt's own databases; it is here
-- for deployments whose history is not Trakkt's.
--
-- `favorites` is the target of no foreign key in either dialect, so these
-- DELETEs cascade nothing.

DELETE FROM favorites
WHERE target_type NOT IN ('issue', 'project', 'team', 'view');

-- NOT EXISTS rather than NOT IN: NOT IN against a subquery yielding any NULL
-- matches nothing at all, silently deleting none of them. Every id column below
-- is a NOT NULL primary key today, so the two forms agree — this is the form
-- that keeps agreeing if that ever stops being true.
DELETE FROM favorites
WHERE target_type = 'issue'
  AND NOT EXISTS (SELECT 1 FROM issues i WHERE i.issue_id = favorites.target_id);

DELETE FROM favorites
WHERE target_type = 'project'
  AND NOT EXISTS (SELECT 1 FROM projects p WHERE p.project_id = favorites.target_id);

DELETE FROM favorites
WHERE target_type = 'team'
  AND NOT EXISTS (SELECT 1 FROM teams t WHERE t.team_id = favorites.target_id);

DELETE FROM favorites
WHERE target_type = 'view'
  AND NOT EXISTS (SELECT 1 FROM views v WHERE v.view_id = favorites.target_id);
