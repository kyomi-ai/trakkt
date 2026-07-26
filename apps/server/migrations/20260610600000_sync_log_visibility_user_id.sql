-- TRA-9920: scope per-user sync_log rows so delta sync stops leaking them.
--
-- `sync_log` is workspace-scoped, but some entity types are per-user. Delta
-- sync filtered only on workspace_id, so every member replayed every other
-- member's notifications, favorites, notification preferences and personal
-- views. `visibility_user_id` fixes that:
--
--   NULL      -> workspace-visible (every member of workspace_id sees the row)
--   <user_id> -> visible only to that user
--
-- Classification rule per entity_type (must match what sync_bootstrap exposes,
-- otherwise a client's dataset depends on which sync path it took):
--
--   notification             -> owner (notifications.user_id).
--                               Bootstrap: list_notifications(db, user_id, ...)
--                               is filtered by `n.user_id = $1`.
--   favorite                 -> owner (favorites.user_id).
--                               Bootstrap: list_favorites(db, user_id, ...) is
--                               filtered by `user_id = $1`.
--   notification_preferences -> owner (notification_preferences.user_id).
--                               Not in bootstrap at all; the row is a single
--                               user's settings, keyed UNIQUE(user_id,
--                               workspace_id), and the UI reads it through a
--                               per-user server function, never from the cache.
--   view                     -> NULL when is_shared, else owner (views.created_by).
--                               Derived from the bootstrap query
--                               view_service::list_views, whose WHERE clause is
--                               `workspace_id = $1 AND (created_by = $2 OR
--                               is_shared = TRUE)`: a shared view is visible to
--                               every member, an unshared one only to its
--                               creator.
--
-- Every other entity_type stays NULL — workspace-visible by design.
--
-- Backfill notes: `favorite` and `view` sync rows were written with a NULL
-- `data` payload, so their owner can only be recovered by joining the source
-- table on entity_id. Rows whose source record no longer exists (a delete
-- action, or a pruned entity) keep visibility_user_id = NULL. Those rows carry
-- no payload — only an entity_id the client either has or does not have — so
-- they disclose nothing, and applying a delete for an absent entity is a no-op.

ALTER TABLE sync_log ADD COLUMN visibility_user_id VARCHAR(100) DEFAULT NULL;

CREATE INDEX IF NOT EXISTS idx_sync_log_visibility
    ON sync_log (workspace_id, visibility_user_id, sync_id);

-- notification: the payload carries the recipient; fall back to the source row.
UPDATE sync_log s
SET visibility_user_id = COALESCE(
        s.data ->> 'user_id',
        (SELECT n.user_id FROM notifications n WHERE n.notification_id = s.entity_id)
    )
WHERE s.entity_type = 'notification';

-- favorite: payload is always NULL, so join the source table.
UPDATE sync_log s
SET visibility_user_id = f.user_id
FROM favorites f
WHERE f.favorite_id = s.entity_id
  AND s.entity_type = 'favorite';

-- notification_preferences: payload carries the owner; fall back to the source row.
UPDATE sync_log s
SET visibility_user_id = COALESCE(
        s.data ->> 'user_id',
        (SELECT p.user_id FROM notification_preferences p WHERE p.preference_id = s.entity_id)
    )
WHERE s.entity_type = 'notification_preferences';

-- view: only unshared views are per-user. Shared views stay NULL so they keep
-- reaching the whole workspace, exactly as list_views exposes them.
UPDATE sync_log s
SET visibility_user_id = v.created_by
FROM views v
WHERE v.view_id = s.entity_id
  AND s.entity_type = 'view'
  AND v.is_shared = FALSE;
