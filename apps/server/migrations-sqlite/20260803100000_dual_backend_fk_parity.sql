-- TRA-9999 (absorbing TRA-9969, TRA-9990, TRA-10000): bring the SQLite schema's
-- foreign keys and primary-key nullability up to what Postgres already enforces.
--
-- The Postgres counterpart of this version is a documented no-op: every
-- constraint below already exists there. Production runs Postgres, so Postgres
-- is the reference and SQLite is what moves.
--
-- ─── What was actually wrong ────────────────────────────────────────────────
--
-- Measured by replaying all 42 migrations of each directory into a throwaway
-- database and reading `pg_constraint` / `PRAGMA foreign_key_list` — not by
-- reading migration text, which is misleading here because earlier migrations
-- rebuild tables and a constraint declared in one file need not survive into
-- the final schema. Postgres carried 78 foreign keys, SQLite 66.
--
-- Twelve keys existed on Postgres and not at all on SQLite. Eleven are fixed
-- here; `issues.milestone_id` is deliberately left out — see the closing note.
-- `issue_activities` had zero foreign keys of any kind.
--
-- Four keys existed on both with a different ON DELETE: `attachments.workspace_id`,
-- `notification_preferences.user_id`, `notification_preferences.workspace_id` and
-- `views.team_id` were all CASCADE on Postgres and NO ACTION on SQLite. That is
-- the dangerous shape: deleting the parent succeeds in production and is
-- rejected outright on SQLite, so a change ships green through CI and breaks on
-- one backend only. TRA-9989 was this exact shape and made an issue undeletable
-- in production.
--
-- Three TEXT primary keys were nullable on SQLite: `feedback.id`,
-- `issue_activities.activity_id`, `notification_preferences.preference_id`. A
-- non-INTEGER PRIMARY KEY gets no implicit NOT NULL in SQLite, so a row with no
-- primary key at all is insertable — and more than one, because NULLs do not
-- compare equal. Verified against the replayed schema: two NULL-id rows were
-- accepted into `feedback` before this migration.
--
-- `sync_log.sync_id`, `user_auth_methods.id` and `workspace_users.id` are also
-- reported nullable by `PRAGMA table_info`, and are deliberately untouched. All
-- three are `INTEGER PRIMARY KEY AUTOINCREMENT`, i.e. rowid aliases, where a
-- NULL bind is the documented way to request the next value. Adding NOT NULL
-- there would break every insert that relies on it.
--
-- ─── Why every table is rebuilt rather than altered ─────────────────────────
--
-- SQLite cannot ALTER a constraint, so each table is recreated and its rows
-- copied across. Two facts make the ordering below load-bearing rather than
-- cosmetic, and both were checked rather than assumed:
--
--   1. Foreign keys are enforced throughout. `crates/trakkt-core/src/db.rs`
--      issues `PRAGMA foreign_keys=ON` on the pool, and sqlx wraps every SQLite
--      migration in a transaction unconditionally: `sqlx-sqlite-0.8.6`,
--      `src/migrate.rs`, `Migrate::apply` opens one before executing the
--      script. sqlx-core does parse a leading `-- no-transaction` into
--      `Migration::no_tx`, but only the Postgres migrator reads that field —
--      `no_tx` appears nowhere in the SQLite one, so the directive would be
--      accepted and ignored here. And `PRAGMA foreign_keys` is a no-op inside
--      an open transaction: setting it to OFF between BEGIN and COMMIT and
--      reading `pragma_foreign_keys` back still returns 1. The documented "turn
--      foreign keys off, rebuild, turn them on" recipe is unavailable either
--      way.
--
--   2. With enforcement on, DROP TABLE performs an implicit DELETE of every row
--      first, and that DELETE fires ON DELETE actions on referencing tables.
--      Dropping a table that something references destroys the referencing
--      rows. Reproduced on the replayed schema: dropping `attachments` with one
--      `issue_attachments` row present left `issue_attachments` empty, and
--      `PRAGMA defer_foreign_keys=ON` did not prevent it.
--
-- So each table below is dropped only at a point where nothing references it.
-- `PRAGMA foreign_key_list` over the replayed schema says which tables those
-- are: of the eleven tables whose constraints change here, only `attachments`
-- is referenced by another table (`issue_attachments.attachment_id`, ON DELETE
-- CASCADE), which is why that one pair is handled last and differently. `github_installations` is
-- rebuilt before `github_events` and `github_links` because those two gain
-- references to it in this migration and would otherwise be reachable by its
-- drop.
--
-- Every column list below is the table as it exists after all 42 migrations
-- replay, read from `PRAGMA table_info`, not as its creating migration declares
-- it. Dropping a column added by a later ALTER would silently destroy that
-- column's production data. Declared types are copied across verbatim on the
-- narrower ground that this migration's job is to change constraints and
-- nothing else: `notification_preferences` really does hold seven columns
-- declared BOOLEAN beside six declared INTEGER, added by ALTER TABLE at
-- different times, and `PRAGMA table_info` reports the spelling. Rewriting them
-- to agree would be an unrequested change that every schema comparison sees.

-- ─── github_installations ───────────────────────────────────────────────────
--
-- Gains workspace_id -> workspaces CASCADE and github_app_id -> github_apps
-- CASCADE, matching Postgres. Both parents own the installation outright: an
-- installation is meaningless once its workspace or its GitHub App is gone, and
-- Postgres has always deleted it with them.
--
-- Rebuilt first. Nothing references it in the pre-migration SQLite schema, so
-- the drop is inert; after this migration `github_events` and `github_links`
-- point at it, so a later position would put those rows in reach of the drop.

CREATE TABLE github_installations_new (
    installation_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    github_app_id TEXT NOT NULL REFERENCES github_apps(github_app_id) ON DELETE CASCADE,
    github_installation_id INTEGER NOT NULL UNIQUE,
    account_login TEXT NOT NULL,
    account_type TEXT NOT NULL,
    target_repos TEXT,
    access_token_encrypted TEXT,
    token_expires_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    suspended_at TEXT
);

INSERT INTO github_installations_new (
    installation_id, workspace_id, github_app_id, github_installation_id,
    account_login, account_type, target_repos, access_token_encrypted,
    token_expires_at, created_at, suspended_at
)
SELECT
    installation_id, workspace_id, github_app_id, github_installation_id,
    account_login, account_type, target_repos, access_token_encrypted,
    token_expires_at, created_at, suspended_at
FROM github_installations;

DROP TABLE github_installations;

ALTER TABLE github_installations_new RENAME TO github_installations;

CREATE INDEX IF NOT EXISTS idx_github_installations_workspace
    ON github_installations(workspace_id);
CREATE INDEX IF NOT EXISTS idx_github_installations_github_id
    ON github_installations(github_installation_id);

-- ─── github_events ──────────────────────────────────────────────────────────
--
-- Gains installation_id -> github_installations SET NULL, matching Postgres.
-- SET NULL rather than CASCADE because the column is already nullable and the
-- row is an audit record of a webhook delivery: it stays true after the
-- installation is removed, and the delivery id is the part worth keeping.

CREATE TABLE github_events_new (
    event_id TEXT NOT NULL PRIMARY KEY,
    github_delivery_id TEXT NOT NULL UNIQUE,
    installation_id TEXT REFERENCES github_installations(installation_id) ON DELETE SET NULL,
    event_type TEXT NOT NULL,
    action TEXT,
    payload_summary TEXT,
    processed_at TEXT,
    error TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

INSERT INTO github_events_new (
    event_id, github_delivery_id, installation_id, event_type, action,
    payload_summary, processed_at, error, created_at
)
SELECT
    event_id, github_delivery_id, installation_id, event_type, action,
    payload_summary, processed_at, error, created_at
FROM github_events;

DROP TABLE github_events;

ALTER TABLE github_events_new RENAME TO github_events;

CREATE INDEX IF NOT EXISTS idx_github_events_delivery
    ON github_events(github_delivery_id);

-- ─── github_links ───────────────────────────────────────────────────────────
--
-- Gains all three of its Postgres keys: workspace_id -> workspaces CASCADE,
-- issue_id -> issues CASCADE (this is TRA-9990) and installation_id ->
-- github_installations CASCADE. A link is a join between an issue and a GitHub
-- ref; none of the three parents can go away and leave it meaningful.

CREATE TABLE github_links_new (
    link_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    installation_id TEXT NOT NULL REFERENCES github_installations(installation_id) ON DELETE CASCADE,
    link_type TEXT NOT NULL,
    github_id INTEGER,
    github_node_id TEXT,
    repo_full_name TEXT NOT NULL,
    ref_identifier TEXT NOT NULL,
    title TEXT,
    state TEXT,
    url TEXT NOT NULL,
    author_login TEXT,
    close_intent INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

INSERT INTO github_links_new (
    link_id, workspace_id, issue_id, installation_id, link_type, github_id,
    github_node_id, repo_full_name, ref_identifier, title, state, url,
    author_login, close_intent, created_at, updated_at
)
SELECT
    link_id, workspace_id, issue_id, installation_id, link_type, github_id,
    github_node_id, repo_full_name, ref_identifier, title, state, url,
    author_login, close_intent, created_at, updated_at
FROM github_links;

DROP TABLE github_links;

ALTER TABLE github_links_new RENAME TO github_links;

CREATE INDEX IF NOT EXISTS idx_github_links_issue ON github_links(issue_id);
CREATE INDEX IF NOT EXISTS idx_github_links_repo ON github_links(repo_full_name);
-- Unique, from 20260517500000_github_links_dedup_fix.sql — the dedup key the
-- upsert in crates/trakkt-github depends on, not merely a lookup index.
CREATE UNIQUE INDEX IF NOT EXISTS idx_github_links_dedup
    ON github_links(workspace_id, issue_id, link_type, repo_full_name, ref_identifier);

-- ─── github_transition_rules ────────────────────────────────────────────────
--
-- Gains workspace_id -> workspaces CASCADE, matching Postgres. The rules are
-- per-workspace configuration and have no meaning outside one.

CREATE TABLE github_transition_rules_new (
    rule_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    trigger_event TEXT NOT NULL,
    close_intent_required INTEGER NOT NULL DEFAULT 0,
    target_status_category TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE(workspace_id, trigger_event, close_intent_required)
);

INSERT INTO github_transition_rules_new (
    rule_id, workspace_id, trigger_event, close_intent_required,
    target_status_category, enabled, created_at
)
SELECT
    rule_id, workspace_id, trigger_event, close_intent_required,
    target_status_category, enabled, created_at
FROM github_transition_rules;

DROP TABLE github_transition_rules;

ALTER TABLE github_transition_rules_new RENAME TO github_transition_rules;

-- ─── issue_activities ───────────────────────────────────────────────────────
--
-- Had no foreign keys at all. Gains its three Postgres keys, and NOT NULL on
-- its primary key (TRA-10000).
--
--   issue_id -> issues CASCADE (TRA-9990): the activity feed of a deleted issue
--   has nothing left to describe.
--   workspace_id -> workspaces CASCADE: same reasoning one level up.
--   actor_id -> users with NO ACTION, deliberately not CASCADE. The column is
--   already nullable and 20260610050000_activity_actor_nullable.sql made it so
--   on purpose; the activity is a historical record and must survive its actor.
--   NO ACTION is what Postgres has, and it means a user with activity rows
--   cannot be hard-deleted without those rows being dealt with first — which is
--   the intended pressure, not an oversight.

CREATE TABLE issue_activities_new (
    activity_id TEXT NOT NULL PRIMARY KEY,
    issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    actor_id TEXT REFERENCES users(user_id),
    action_type TEXT NOT NULL,
    field TEXT,
    old_value TEXT,
    new_value TEXT,
    metadata TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    action_source TEXT NOT NULL DEFAULT 'user',
    action_source_label TEXT
);

INSERT INTO issue_activities_new (
    activity_id, issue_id, workspace_id, actor_id, action_type, field,
    old_value, new_value, metadata, created_at, action_source,
    action_source_label
)
SELECT
    activity_id, issue_id, workspace_id, actor_id, action_type, field,
    old_value, new_value, metadata, created_at, action_source,
    action_source_label
FROM issue_activities;

DROP TABLE issue_activities;

ALTER TABLE issue_activities_new RENAME TO issue_activities;

CREATE INDEX IF NOT EXISTS idx_issue_activities_issue
    ON issue_activities(issue_id, created_at);
CREATE INDEX IF NOT EXISTS idx_issue_activities_workspace
    ON issue_activities(workspace_id);

-- ─── api_tokens ─────────────────────────────────────────────────────────────
--
-- Gains workspace_id -> workspaces with NO ACTION, deliberately not CASCADE.
-- NO ACTION is what Postgres has, and it is the right answer for a credential:
-- deleting a workspace must not quietly revoke tokens as a side effect, it must
-- fail until the tokens are dealt with explicitly. The column is nullable
-- (`ALTER TABLE api_tokens ADD COLUMN workspace_id TEXT` in
-- 20260507000000_issue_tracker.sql, after 20260502000000_baseline.sql had
-- created the table without it), so pre-existing workspace-less tokens are
-- unaffected: a NULL foreign key is never checked.

CREATE TABLE api_tokens_new (
    token_id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    expires_at TEXT,
    last_used TEXT,
    revoked_at TEXT,
    created_by TEXT,
    revoked_by TEXT,
    workspace_id TEXT REFERENCES workspaces(workspace_id),
    token_prefix TEXT,
    scopes TEXT DEFAULT '[]'
);

INSERT INTO api_tokens_new (
    token_id, user_id, name, token_hash, active, created_at, expires_at,
    last_used, revoked_at, created_by, revoked_by, workspace_id, token_prefix,
    scopes
)
SELECT
    token_id, user_id, name, token_hash, active, created_at, expires_at,
    last_used, revoked_at, created_by, revoked_by, workspace_id, token_prefix,
    scopes
FROM api_tokens;

DROP TABLE api_tokens;

ALTER TABLE api_tokens_new RENAME TO api_tokens;

CREATE INDEX IF NOT EXISTS idx_api_tokens_hash ON api_tokens (token_hash);
CREATE INDEX IF NOT EXISTS idx_api_tokens_user ON api_tokens (user_id);
CREATE INDEX IF NOT EXISTS idx_api_tokens_workspace ON api_tokens (workspace_id);

-- ─── notification_preferences ───────────────────────────────────────────────
--
-- Both keys change from NO ACTION to CASCADE, matching Postgres, and the
-- primary key gains NOT NULL (TRA-10000). A preference row is per (user,
-- workspace) settings with no standalone meaning, so it follows either parent
-- out. Under NO ACTION, deleting a user or a workspace that had ever opened the
-- notification settings page was rejected on SQLite and succeeded in
-- production.
--
-- The seven trailing columns are declared BOOLEAN and the six before them
-- INTEGER. That is what 20260610300000_notification_types_preferences.sql left
-- behind via ALTER TABLE ADD COLUMN, and both spellings are copied verbatim.
-- Not because the two behave differently -- BOOLEAN takes NUMERIC affinity and
-- INTEGER takes INTEGER affinity, and measured on this schema the two store
-- identical values ('1.5' becomes REAL in both, 'abc' stays TEXT in both;
-- SQLite documents the difference as visible only inside a CAST). The reason is
-- that the spelling is what `PRAGMA table_info` reports, and this migration
-- changes constraints only. Normalising it would be a separate change that
-- happened to ride along in a rebuild.

CREATE TABLE notification_preferences_new (
    preference_id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    notify_status_changes INTEGER NOT NULL DEFAULT 1,
    notify_comments INTEGER NOT NULL DEFAULT 1,
    notify_assignments INTEGER NOT NULL DEFAULT 1,
    notify_priority_changes INTEGER NOT NULL DEFAULT 1,
    notify_own_agent_actions INTEGER NOT NULL DEFAULT 0,
    notify_own_api_actions INTEGER NOT NULL DEFAULT 0,
    delivery_channel TEXT NOT NULL DEFAULT 'in_app',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    notify_label_changes BOOLEAN NOT NULL DEFAULT 1,
    notify_due_date_changes BOOLEAN NOT NULL DEFAULT 1,
    notify_estimate_changes BOOLEAN NOT NULL DEFAULT 1,
    notify_milestone_changes BOOLEAN NOT NULL DEFAULT 1,
    notify_project_changes BOOLEAN NOT NULL DEFAULT 1,
    notify_team_changes BOOLEAN NOT NULL DEFAULT 1,
    notify_relation_changes BOOLEAN NOT NULL DEFAULT 1,
    UNIQUE(user_id, workspace_id)
);

INSERT INTO notification_preferences_new (
    preference_id, user_id, workspace_id, notify_status_changes,
    notify_comments, notify_assignments, notify_priority_changes,
    notify_own_agent_actions, notify_own_api_actions, delivery_channel,
    created_at, updated_at, notify_label_changes, notify_due_date_changes,
    notify_estimate_changes, notify_milestone_changes, notify_project_changes,
    notify_team_changes, notify_relation_changes
)
SELECT
    preference_id, user_id, workspace_id, notify_status_changes,
    notify_comments, notify_assignments, notify_priority_changes,
    notify_own_agent_actions, notify_own_api_actions, delivery_channel,
    created_at, updated_at, notify_label_changes, notify_due_date_changes,
    notify_estimate_changes, notify_milestone_changes, notify_project_changes,
    notify_team_changes, notify_relation_changes
FROM notification_preferences;

DROP TABLE notification_preferences;

ALTER TABLE notification_preferences_new RENAME TO notification_preferences;

-- ─── views ──────────────────────────────────────────────────────────────────
--
-- team_id changes from NO ACTION to CASCADE, matching Postgres (TRA-9969). The
-- column is nullable — a view with no team is a workspace-level view — but a
-- view scoped to a team is part of that team's configuration, and Postgres has
-- deleted it with the team since 20260512000001_views_team_scope.sql. Under NO
-- ACTION, deleting a team that had any saved view was rejected on SQLite and
-- succeeded in production. workspace_id and created_by already cascaded on both
-- backends and are unchanged.

CREATE TABLE views_new (
    view_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    created_by TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    icon TEXT,
    filters TEXT NOT NULL DEFAULT '{}',
    display_options TEXT NOT NULL DEFAULT '{}',
    sort_order REAL DEFAULT 0,
    is_shared INTEGER DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    team_id TEXT REFERENCES teams(team_id) ON DELETE CASCADE,
    position INTEGER NOT NULL DEFAULT 0
);

INSERT INTO views_new (
    view_id, workspace_id, created_by, name, icon, filters, display_options,
    sort_order, is_shared, created_at, updated_at, team_id, position
)
SELECT
    view_id, workspace_id, created_by, name, icon, filters, display_options,
    sort_order, is_shared, created_at, updated_at, team_id, position
FROM views;

DROP TABLE views;

ALTER TABLE views_new RENAME TO views;

CREATE INDEX IF NOT EXISTS idx_views_workspace ON views (workspace_id);
CREATE INDEX IF NOT EXISTS idx_views_created_by ON views (created_by);
CREATE INDEX IF NOT EXISTS idx_views_team ON views (team_id);

-- ─── feedback ───────────────────────────────────────────────────────────────
--
-- No foreign key drift: its three keys already matched Postgres. Rebuilt solely
-- for NOT NULL on the primary key (TRA-10000). The two CHECK constraints are
-- carried across verbatim; dropping them here would widen the accepted value
-- sets that 20260521000000_feedback.sql established.

CREATE TABLE feedback_new (
    id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id),
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    feedback_type TEXT NOT NULL CHECK (feedback_type IN ('bug', 'feature', 'question')),
    description TEXT NOT NULL,
    screenshot_url TEXT,
    include_context INTEGER NOT NULL DEFAULT 1,
    context TEXT,
    status TEXT NOT NULL DEFAULT 'new' CHECK (status IN ('new', 'reviewed', 'resolved', 'closed')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    resolved_at TEXT,
    resolution_notes TEXT,
    resolved_by TEXT REFERENCES users(user_id)
);

INSERT INTO feedback_new (
    id, user_id, workspace_id, feedback_type, description, screenshot_url,
    include_context, context, status, created_at, resolved_at,
    resolution_notes, resolved_by
)
SELECT
    id, user_id, workspace_id, feedback_type, description, screenshot_url,
    include_context, context, status, created_at, resolved_at,
    resolution_notes, resolved_by
FROM feedback;

DROP TABLE feedback;

ALTER TABLE feedback_new RENAME TO feedback;

CREATE INDEX IF NOT EXISTS idx_feedback_workspace ON feedback(workspace_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_feedback_user ON feedback(user_id);
CREATE INDEX IF NOT EXISTS idx_feedback_status ON feedback(workspace_id, status);

-- ─── attachments, and issue_attachments with it ─────────────────────────────
--
-- attachments.workspace_id changes from NO ACTION to CASCADE, matching
-- Postgres. An attachment is workspace-scoped storage; production has deleted
-- it with the workspace since 20260517300000_attachments.sql, while SQLite
-- rejected the workspace delete outright.
--
-- `issue_attachments` is rebuilt here too, and not because its own schema is
-- wrong — it is not, and it comes out byte-identical in every column, key and
-- index. It is rebuilt because it is the one table in this migration that
-- references another one in it: `issue_attachments.attachment_id` points at
-- `attachments` ON DELETE CASCADE, so `DROP TABLE attachments` would take every
-- issue-attachment link with it, as reproduced in the header note. The rows are
-- therefore parked in a keyless staging table across the two drops and put back
-- afterwards.
--
-- The staging table is what makes this ordering independent of SQLite's
-- `legacy_alter_table` setting. The alternative — pointing a rebuilt
-- `issue_attachments` at `attachments_new` and letting the RENAME rewrite the
-- reference — works only while renames rewrite referencing tables, which that
-- pragma turns off.

CREATE TABLE attachments_new (
    attachment_id TEXT NOT NULL PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    filename TEXT NOT NULL,
    content_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    storage_path TEXT NOT NULL,
    uploaded_by TEXT NOT NULL REFERENCES users(user_id),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

INSERT INTO attachments_new (
    attachment_id, workspace_id, filename, content_type, size_bytes,
    storage_path, uploaded_by, created_at
)
SELECT
    attachment_id, workspace_id, filename, content_type, size_bytes,
    storage_path, uploaded_by, created_at
FROM attachments;

-- No constraints on the staging table: it exists only to hold the three columns
-- out of reach of the cascade, and re-declaring the keys would put it back in.
CREATE TABLE issue_attachments_carry (
    issue_id TEXT NOT NULL,
    attachment_id TEXT NOT NULL,
    created_at TEXT NOT NULL
);

INSERT INTO issue_attachments_carry (issue_id, attachment_id, created_at)
SELECT issue_id, attachment_id, created_at FROM issue_attachments;

-- Nothing references issue_attachments, so this drop cascades to nowhere; and
-- with it gone, nothing references attachments either.
DROP TABLE issue_attachments;

DROP TABLE attachments;

ALTER TABLE attachments_new RENAME TO attachments;

CREATE INDEX IF NOT EXISTS idx_attachments_workspace ON attachments(workspace_id);

-- Recreated exactly as 20260520000000_issue_attachments.sql declared it; no
-- later migration touched this table.
CREATE TABLE issue_attachments (
    issue_id TEXT NOT NULL REFERENCES issues(issue_id) ON DELETE CASCADE,
    attachment_id TEXT NOT NULL REFERENCES attachments(attachment_id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (issue_id, attachment_id)
);

INSERT INTO issue_attachments (issue_id, attachment_id, created_at)
SELECT issue_id, attachment_id, created_at FROM issue_attachments_carry;

DROP TABLE issue_attachments_carry;

CREATE INDEX IF NOT EXISTS idx_issue_attachments_issue ON issue_attachments(issue_id);
CREATE INDEX IF NOT EXISTS idx_issue_attachments_attachment ON issue_attachments(attachment_id);

-- ─── Deliberately not done: issues.milestone_id ─────────────────────────────
--
-- Postgres has issues.milestone_id -> project_milestones(milestone_id) ON
-- DELETE SET NULL. SQLite has no key there, and it is the twelfth of the twelve
-- and the only one left outstanding. Leaving it is a judgement call, recorded
-- here so it is visible rather than buried:
--
--   What it costs to leave. `delete_milestone`
--   (crates/trakkt-auth/src/project_service.rs) deletes the milestone row and
--   nothing else — it relies entirely on the schema to clear the pointers, and
--   says so in its doc comment. So on SQLite, deleting a milestone leaves every
--   issue holding a dead milestone_id, where Postgres nulls it. That is a real
--   divergence, but it is the milder kind: both backends accept the delete.
--   None of the four ON DELETE mismatches fixed above had that property — those
--   were accepted in production and rejected on SQLite, which is the shape that
--   ships green and breaks on one backend.
--
--   What it costs to do. Re-measure with `PRAGMA foreign_key_list` against the
--   schema as it stands rather than trusting a number written here, because
--   this migration itself moved it: `github_links.issue_id` and
--   `issue_activities.issue_id` above are two keys pointing at `issues` that
--   did not exist on SQLite before. Measured on the schema this file produces,
--   eleven foreign keys across ten tables reference `issues` — comments,
--   github_links, issue_activities, issue_attachments, issue_labels,
--   issue_relations (twice, via source_issue_id and target_issue_id),
--   issue_stars, issue_watchers, notifications and release_issues. Because DROP
--   TABLE fires cascades under enforced keys, rebuilding `issues` means
--   rebuilding every one of those alongside it in a single migration, each
--   staged in the manner used above, each with its own accumulated ALTER
--   history to replay correctly — on top of `issues` itself, which 19
--   migrations have touched. One column omitted from any of those column lists
--   destroys that column's production data silently.
--
-- Eleven tables rebuilt — the ten that reference `issues`, plus `issues` — to
-- add one SET NULL is not a trade worth taking alongside the rebuilds above. It
-- wants its own migration, its own review, and its own before/after column dump
-- per table. Tracked separately.
