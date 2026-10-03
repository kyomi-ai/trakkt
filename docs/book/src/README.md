# Trakkt

Trakkt is an open-source issue tracker built for speed. It delivers Linear-quality UX as a single self-hostable binary with no runtime dependencies beyond a database.

## Key Features

- **Single binary** -- the Rust backend embeds the Leptos SSR frontend, WASM hydration layer, and all static assets. No Node.js, no separate frontend deploy.
- **PostgreSQL or SQLite** -- use PostgreSQL for team and SaaS deployments, or SQLite for personal single-user mode with zero setup.
- **Self-hostable** -- run on your own infrastructure with Docker, Kubernetes, or a bare binary. Three deployment modes: SaaS (multi-tenant), self-hosted (team), and personal (single-user, no login).
- **Keyboard-first** -- every action is reachable via keyboard shortcuts. `j`/`k` navigation, single-key shortcuts, `Cmd+K` command palette.
- **Real-time sync** -- WebSocket-based live updates across all connected clients. Changes appear instantly without polling.
- **Full auth system** -- password login, passkeys (WebAuthn), Google OAuth, TOTP two-factor authentication, and email verification.
- **REST API** -- a unified API surface with 36 operations covering issues, comments, labels, teams, statuses, relations, projects, milestones, attachments, activities, and GitHub integration. OpenAPI 3.1.0 spec included.
- **MCP server** -- built-in Model Context Protocol server so AI agents (Claude Code, etc.) can create issues, query status, and manage your tracker programmatically.
- **GitHub integration** -- link commits, branches, and pull requests to issues. Look up issues by commit SHA or branch name.
- **Teams and projects** -- organize work across teams with custom workflows, milestones, and scoped labels.
- **Board and list views** -- Kanban board and table views with filtering, sorting, and real-time updates.

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust, Axum 0.8, SQLx 0.8 |
| Frontend | Leptos 0.8 (SSR + WASM hydration) |
| Styling | Tailwind CSS v4 |
| Icons | Phosphor Icons |
| Auth | JWT (HS256), Argon2, WebAuthn, TOTP |
| Email | Lettre (SMTP) |
| Database | PostgreSQL (SaaS/team) or SQLite (personal) |
| Cache | Redis (SaaS) or in-memory KV (self-hosted) |

## License

Trakkt is licensed under [AGPL-3.0-or-later](https://github.com/kyomi-ai/trakkt/blob/main/LICENSE).
