-- TRA-9920: scope per-user sync_log rows so delta sync stops leaking them.
--
-- SQLite counterpart of migrations/20260610600000_sync_log_visibility_user_id.sql.
-- Same schema change and same classification rule; see that file for the full
-- derivation. Summary:
--
--   NULL      -> workspace-visible (every member of workspace_id sees the row)
--   <user_id> -> visible only to that user
--
--   notification             -> owner (notifications.user_id)
--   favorite                 -> owner (favorites.user_id)
--   notification_preferences -> owner (notification_preferences.user_id)
--   view                     -> NULL when is_shared, else owner (views.created_by),
--                               derived from view_service::list_views' WHERE clause
--                               `workspace_id = $1 AND (created_by = $2 OR is_shared = 1)`
--
-- Every other entity_type stays NULL — workspace-visible by design.
--
-- Dialect differences from the Postgres file: TEXT instead of VARCHAR,
-- json_extract() instead of ->>, correlated subqueries instead of UPDATE ... FROM,
-- and is_shared is INTEGER 0/1 rather than BOOLEAN.

ALTER TABLE sync_log ADD COLUMN visibility_user_id TEXT DEFAULT NULL;

CREATE INDEX IF NOT EXISTS idx_sync_log_visibility
    ON sync_log (workspace_id, visibility_user_id, sync_id);

-- notification: the payload carries the recipient; fall back to the source row.
UPDATE sync_log
SET visibility_user_id = COALESCE(
        json_extract(data, '$.user_id'),
        (SELECT n.user_id FROM notifications n WHERE n.notification_id = sync_log.entity_id)
    )
WHERE entity_type = 'notification';

-- favorite: payload is always NULL, so join the source table. The EXISTS guard
-- makes this an inner join, matching the Postgres `UPDATE ... FROM favorites`:
-- rows whose favorite is already gone keep visibility_user_id = NULL rather than
-- being rewritten to NULL by a subquery that found nothing.
UPDATE sync_log
SET visibility_user_id = (
        SELECT f.user_id FROM favorites f WHERE f.favorite_id = sync_log.entity_id
    )
WHERE entity_type = 'favorite'
  AND EXISTS (SELECT 1 FROM favorites f WHERE f.favorite_id = sync_log.entity_id);

-- notification_preferences: payload carries the owner; fall back to the source row.
UPDATE sync_log
SET visibility_user_id = COALESCE(
        json_extract(data, '$.user_id'),
        (SELECT p.user_id FROM notification_preferences p WHERE p.preference_id = sync_log.entity_id)
    )
WHERE entity_type = 'notification_preferences';

-- view: only unshared views are per-user. Shared views stay NULL so they keep
-- reaching the whole workspace, exactly as list_views exposes them.
UPDATE sync_log
SET visibility_user_id = (
        SELECT v.created_by FROM views v
        WHERE v.view_id = sync_log.entity_id AND v.is_shared = 0
    )
WHERE entity_type = 'view'
  AND EXISTS (
        SELECT 1 FROM views v
        WHERE v.view_id = sync_log.entity_id AND v.is_shared = 0
    );
