# Tane (種)

A Rust app starter template with full authentication, workspace management, settings UI, and MCP server built in. Fork it, rename it, add your domain code.

Built with the same stack as [Kyomi](https://kyomi.ai) -- battle-tested patterns for auth, sessions, email verification, and multi-tenant workspaces extracted into a reusable starting point.

## What's Included

**Authentication:**
- Password-based signup and login
- Passkey / WebAuthn (register, login, account recovery)
- Google OAuth
- TOTP two-factor authentication
- Email verification (link-based)
- Account recovery (forgot password flow)
- Session management (view active sessions, revoke, logout all)

**Workspaces:**
- Create workspace on signup
- Invite members by email
- Role management (owner, admin, member)
- Transfer ownership (dual-party email confirmation)
- Workspace switcher for multi-workspace users

**Settings UI:**
- Profile (display name)
- Appearance (light / dark / system theme)
- Security (password, TOTP, passkeys, sessions)
- Workspace (name)
- Team (invite, remove, change roles, transfer ownership)

**MCP Server:**
- Streamable HTTP transport at `/mcp`
- Session-based authentication
- Ships with a `hello` tool as a starting point

**Infrastructure:**
- SQLite (self-hosted / personal) or PostgreSQL (SaaS)
- Redis (SaaS) or in-memory KV (self-hosted)
- JWT dual-token auth (access + refresh)
- Rate limiting
- WebSocket support

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Backend | Rust, Axum 0.8, SQLx 0.8 |
| Frontend | Leptos 0.8 (SSR + WASM hydration) |
| Styling | Tailwind CSS v4 |
| Icons | Phosphor Icons |
| Auth | JWT (HS256), Argon2/bcrypt, WebAuthn |
| Email | Lettre (SMTP) |
| 2FA | TOTP (totp-rs) |
| MCP | Streamable HTTP (SSE) |

## Crate Structure

```
tane/
├── Cargo.toml              # Workspace root
├── crates/
│   ├── trakkt-core/          # Config, database, KV store, models, error types
│   ├── trakkt-auth/          # Auth service, sessions, user/workspace/email services
│   ├── trakkt-types/         # Shared DTOs (serde-only, WASM-safe)
│   └── trakkt-ui/            # Leptos frontend (pages, components, server functions)
│       └── Trunk.toml      # WASM build config
├── apps/
│   └── server/             # Axum binary (routes, middleware, MCP, state)
│       ├── migrations/         # PostgreSQL migrations
│       └── migrations-sqlite/  # SQLite migrations
└── docs/
```

## Deployment Modes

Set via the `TRAKKT_MODE` environment variable:

| Mode | `TRAKKT_MODE` | Database | Auth Behavior |
|------|-------------|----------|---------------|
| **SaaS** | `saas` | PostgreSQL + Redis | Full auth, email verification required |
| **Self-hosted** | `self_hosted` | SQLite | Full auth; if no SMTP, first user creates account directly (no email verification) |
| **Personal** | `personal` | SQLite | No login required, auto-provisioned user and workspace |

## Running in Development

### Prerequisites

- Rust (stable)
- [Trunk](https://trunkrs.dev/) (`cargo install trunk`)
- [tailwindcss CLI](https://tailwindcss.com/blog/standalone-cli) (v4)
- PostgreSQL (for SaaS mode) or nothing (SQLite mode)

### Environment Variables

Create a `.env` file or export these:

```bash
# Required
DATABASE_URL=postgres://user:pass@localhost:5432/tane   # or sqlite://./data/trakkt.db
JWT_SECRET_KEY=your-secret-key-here
ENCRYPTION_KEY=base64-encoded-32-byte-key

# Optional
TRAKKT_MODE=self_hosted                    # saas | self_hosted | personal
PORT=8003                                # default: 8003
FRONTEND_URL=http://localhost:5173       # Trunk dev server
BASE_URL=http://localhost:8003           # Backend URL

# SMTP (required for email verification in saas mode)
SMTP_HOST=smtp.example.com
SMTP_USER=you@example.com
SMTP_PASSWORD=your-smtp-password
SMTP_FROM=noreply@example.com

# Google OAuth (optional)
GOOGLE_OAUTH_CLIENT_ID=...
GOOGLE_OAUTH_CLIENT_SECRET=...

# WebAuthn (optional, derived from FRONTEND_URL by default)
WEBAUTHN_RP_ID=localhost
WEBAUTHN_RP_NAME=Tane
```

### Build and Run

```bash
# Terminal 1: Build the WASM frontend (watches for changes)
cd crates/trakkt-ui
trunk build --watch

# Terminal 2: Run the server
cargo run --package trakkt-server
```

The server starts at `http://localhost:8003`. In development with Trunk, the frontend is served from `http://localhost:5173` with proxy to the backend.

## Creating a New App From This Template

1. Fork or clone this repository:
   ```bash
   git clone https://github.com/kyomi-ai/tane.git myapp
   cd myapp
   ```

2. Rename the crates (replace `tane` with your app name, e.g., `myapp`):
   ```bash
   # Rename directories
   mv crates/trakkt-core crates/myapp-core
   mv crates/trakkt-auth crates/myapp-auth
   mv crates/trakkt-types crates/myapp-types
   mv crates/trakkt-ui crates/myapp-ui

   # Rename in all files
   find . -name "*.toml" -o -name "*.rs" | xargs sed -i 's/trakkt-core/myapp-core/g'
   find . -name "*.toml" -o -name "*.rs" | xargs sed -i 's/trakkt-auth/myapp-auth/g'
   find . -name "*.toml" -o -name "*.rs" | xargs sed -i 's/trakkt-types/myapp-types/g'
   find . -name "*.toml" -o -name "*.rs" | xargs sed -i 's/trakkt-ui/myapp-ui/g'
   find . -name "*.toml" -o -name "*.rs" | xargs sed -i 's/trakkt-server/myapp-server/g'
   find . -name "*.toml" -o -name "*.rs" | xargs sed -i 's/trakkt_core/myapp_core/g'
   find . -name "*.toml" -o -name "*.rs" | xargs sed -i 's/trakkt_auth/myapp_auth/g'
   find . -name "*.toml" -o -name "*.rs" | xargs sed -i 's/trakkt_types/myapp_types/g'
   find . -name "*.toml" -o -name "*.rs" | xargs sed -i 's/trakkt_ui/myapp_ui/g'
   find . -name "*.toml" -o -name "*.rs" | xargs sed -i 's/trakkt_server/myapp_server/g'

   # Update binary name in apps/server/Cargo.toml
   sed -i 's/name = "tane"/name = "myapp"/' apps/server/Cargo.toml

   # Update env var prefix if desired
   find . -name "*.rs" | xargs sed -i 's/TRAKKT_MODE/MYAPP_MODE/g'
   ```

3. Add your domain code:
   - Models go in `crates/myapp-core/src/models.rs`
   - Business logic services go in `crates/myapp-auth/src/` (or create a new service crate)
   - UI pages go in `crates/myapp-ui/src/pages/`
   - API routes go in `apps/server/src/routes/`
   - MCP tools go in `apps/server/src/mcp/tools.rs`

4. Run migrations, build, and start developing.

## License

AGPL-3.0-or-later
