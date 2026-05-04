# Deferred Work

Items identified during v1 implementation reviews. Tracked here until moved to Linear tickets.

## Performance

- **N+1 label query in list_issues** — Each issue triggers a separate `get_issue_labels` call. Batch with a single IN query for all issue IDs. Affects `/issues` list endpoint at scale (100+ issues).
- **Atomic rate limiter** — `INCR` + `EXPIRE` are two separate Redis round-trips. If the server crashes between them, the rate-limit key never expires and the user is permanently locked out. Fix with a Lua script or `incr_with_expire` KV method.

## UX Polish

- **ConfirmDialog for destructive actions** — Label delete fires on a single click with no confirmation. Implement `ConfirmDialog` component per DESIGN.md and use it for all destructive operations (delete label, delete issue, delete comment).
- **Hardcoded hex colors in page callers** — Several pages use `hover:bg-[#F5F3EF]`, `text-[#DC2626]` etc. instead of design tokens (`hover:bg-surface-alt`, `text-destructive`). Sweep and replace.
- **Optimistic dropdown updates** — Status and priority dropdowns on issue detail show stale state after mutation until refetch completes. Add `set_status`/`set_priority` call in `on_select` handler.
- **Profile tab non-functional** — Display name input renders as editable but has no save action. Either wire it to `update_user` or make it disabled.
- **Settings form accessibility** — `<label>` elements lack `for` attributes, inputs lack `id`. Add matching pairs for screen reader support.

## Features (Phase 5-6)

- **Sync engine** — WebSocket + IndexedDB local-first protocol. Copy from Kyomi. Enables instant UI, offline support, real-time multi-tab updates.
- **Keyboard navigation** — `j`/`k` movement, `Enter` to open, `x` to select, `c` to create, `Cmd+K` command palette. Listed as v1 requirement in DESIGN.md.
- **Public REST API** — `/api/v1/` with 11 endpoints, token auth, scoped permissions, rate limiting.
- **MCP server** — Streamable HTTP at `/mcp`, 8 tools + 3 resources for AI agent integration.
- **Full-text search** — SQLite FTS5 / Postgres tsvector across issue titles and descriptions.
- **Notifications** — In-app notification bell with unread count, mark-as-read.
- **Passkey auth** — WebAuthn registration + login via `webauthn-rs`.
- **Dark mode** — CSS custom property swap via `.dark` class. Tokens defined in DESIGN.md.
- **API token management** — Settings page for creating/revoking workspace API tokens.
