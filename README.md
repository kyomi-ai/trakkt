# Trakkt

An open-source issue tracker built for speed. Linear-quality UX, single binary, self-hostable.

Built with Rust, Axum, Leptos, and Tailwind CSS. Ships as a single binary with the frontend embedded — no Node.js, no separate frontend deploy, no runtime dependencies beyond a database.

## Features

- **Issue tracking** — Create, assign, prioritize, and organize issues with markdown descriptions
- **Board and list views** — Kanban board and table views with real-time updates
- **Teams and projects** — Organize work across teams with custom workflows
- **Custom statuses and labels** — Define your own workflow stages and categorization
- **Keyboard-first** — Every action reachable via keyboard shortcuts
- **MCP server** — Built-in Model Context Protocol server for AI agent integration
- **Full auth system** — Password, passkeys (WebAuthn), Google OAuth, TOTP 2FA, email verification
- **Workspaces** — Multi-tenant with invites, roles, and ownership transfer
- **Three deployment modes** — SaaS, self-hosted (team), or personal (single-user, no login)

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

## Quick Start

### Personal Mode (simplest)

No database server needed — uses SQLite, no login required:

```bash
git clone https://github.com/kyomi-ai/trakkt.git
cd trakkt
cp .env.example .env

# Build frontend
cd crates/trakkt-ui && trunk build && cd ../..

# Run (SQLite, no auth)
TRAKKT_MODE=personal cargo run --package trakkt-server
```

Visit `http://localhost:3100`.

### Self-Hosted Mode (team)

Requires PostgreSQL:

```bash
# Set up .env with your PostgreSQL connection
DATABASE_URL=postgres://user:pass@localhost:5432/trakkt
JWT_SECRET_KEY=$(openssl rand -base64 32)
ENCRYPTION_KEY=$(openssl rand -base64 32)
TRAKKT_MODE=self_hosted

cargo run --package trakkt-server
```

### Docker

```bash
docker pull ghcr.io/kyomi-ai/trakkt:latest

docker run -p 3100:3100 \
  -e DATABASE_URL=postgres://user:pass@host/trakkt \
  -e JWT_SECRET_KEY=your-secret-key \
  -e ENCRYPTION_KEY=your-encryption-key \
  -e TRAKKT_MODE=self_hosted \
  ghcr.io/kyomi-ai/trakkt:latest
```

### Kubernetes

See [deploy/k8s/](deploy/k8s/) for complete Kubernetes manifests with PostgreSQL, Redis, and sealed secrets support.

## Deployment Modes

| Mode | `TRAKKT_MODE` | Database | Auth |
|------|---------------|----------|------|
| **SaaS** | `saas` | PostgreSQL + Redis | Full auth, email verification required |
| **Self-hosted** | `self_hosted` | PostgreSQL | Full auth; first user creates account directly if no SMTP |
| **Personal** | `personal` | SQLite | No login, auto-provisioned user and workspace |

## Crate Structure

```
trakkt/
├── crates/
│   ├── trakkt-core/    # Config, database, KV store, models, error types
│   ├── trakkt-auth/    # Auth, sessions, user/workspace/email services
│   ├── trakkt-types/   # Shared DTOs (serde-only, WASM-safe)
│   └── trakkt-ui/      # Leptos frontend (pages, components, server functions)
├── apps/
│   └── server/         # Axum binary (routes, middleware, MCP, state)
│       ├── migrations/         # PostgreSQL migrations
│       └── migrations-sqlite/  # SQLite migrations
└── deploy/
    └── k8s/            # Kubernetes deployment manifests
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). A signed [CLA](CLA.md) is required before your first PR can be merged.

## License

AGPL-3.0-or-later. See [LICENSE](LICENSE).

A commercial license is available for organizations that cannot comply with AGPL terms. Contact **legal@trakkt.app** for details.
