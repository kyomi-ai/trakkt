-- TRA-9989: make notifications.issue_id cascade when its issue is deleted.
--
-- `notifications_issue_id_fkey` was created in 20260507000000_issue_tracker.sql
-- with no `ON DELETE` clause, which is NO ACTION. Nothing deletes notification
-- rows either — `bulk_delete_notifications` sets `deleted_at` and leaves the row
-- in place — so a notification's reference to its issue is never released. An
-- issue with any notification row therefore cannot be deleted at all: the
-- `DELETE FROM issues` in `issue_service::delete_issue` fails on this
-- constraint, and there is no way round it from the UI. Notifications are
-- written for an issue's watchers on all eleven of the `TYPE_*` events in
-- `notification_service` — a new comment, and status, assignee, priority,
-- label, due-date, estimate, milestone, project, team and relation changes — so
-- in practice any issue a second person has touched is undeletable.
--
-- CASCADE rather than SET NULL, deliberately:
--
--   * A notification's whole content is a reference to an issue. The row
--     carries a type, a read flag and an actor; everything a reader needs —
--     the issue title, number and team key — is joined from `issues` at read
--     time (see the SELECT in `notification_service::list_notifications`).
--     Detached from its issue the row cannot be rendered and cannot be acted
--     on, so keeping it preserves a row, not information.
--   * SET NULL would require `issue_id` to become nullable, and every reader to
--     handle the null. `issue_id` is a plain `String` on both
--     `notification_service::NotificationRow` and the `Notification` DTO in
--     `crates/trakkt-types/src/models.rs`, so it would have to become
--     `Option<String>` on both, and every use of it with them. The row that
--     came back would carry no title, number or team key — those already come
--     from the `LEFT JOIN issues` / `LEFT JOIN teams` and are already
--     `Option` — leaving an inbox entry that renders as nothing and navigates
--     nowhere.
--   * It matches the two other tables an issue delete empties, `comments` and
--     `issue_relations`, both of which already declare ON DELETE CASCADE.
--
-- The trade-off, stated plainly: a user's inbox silently loses an entry they
-- may never have read, and nothing tells them it existed. That is a real loss.
-- It is accepted because the alternative keeps an entry that points at nothing
-- — and because today's behaviour, an issue that cannot be deleted, is worse
-- than both.
--
-- `issue_service::delete_issue` reads the cascaded notification ids before the
-- DELETE and writes one `sync_log` delete entry per row, each scoped to that
-- notification's own recipient, so clients evict them instead of holding them
-- forever.

ALTER TABLE public.notifications
    DROP CONSTRAINT IF EXISTS notifications_issue_id_fkey;

ALTER TABLE public.notifications
    ADD CONSTRAINT notifications_issue_id_fkey
    FOREIGN KEY (issue_id) REFERENCES public.issues(issue_id) ON DELETE CASCADE;
