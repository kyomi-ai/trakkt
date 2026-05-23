# Configuration

Trakkt is configured entirely through environment variables. The server reads them at startup and panics on missing required values to fail fast.

## Deployment Mode

| Variable | Description | Default |
|----------|-------------|---------|
| `TRAKKT_MODE` | Deployment mode: `saas`, `self_hosted`, or `personal`. Determines auth strategy, database backend, and UI surface. | `saas` |
| `SELF_HOSTED` | Legacy boolean flag. If `TRAKKT_MODE` is not set and `SELF_HOSTED=true`, the server runs in self-hosted mode. | `false` |

### Mode comparison

| Mode | `TRAKKT_MODE` | Database | Auth | Use case |
|------|---------------|----------|------|----------|
| SaaS | `saas` | PostgreSQL + Redis | Full auth, email verification | Multi-tenant hosted service |
| Self-hosted | `self_hosted` | PostgreSQL | Full auth; first user creates account directly if no SMTP | Team server |
| Personal | `personal` | SQLite | No login, auto-provisioned user and workspace | Single-user desktop/local use |

## Required Variables

These must be set in all modes except personal (which auto-generates defaults for SQLite):

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | Database connection string. PostgreSQL: `postgres://user:pass@host:5432/db`. SQLite: `sqlite://path/to/db.sqlite` |
| `JWT_SECRET_KEY` | Secret for signing JWT access tokens (HS256). Must be a secure random string. |
| `ENCRYPTION_KEY` | Base64-encoded 32-byte key for AES-256-GCM encryption of credentials at rest. |

## Server

| Variable | Description | Default |
|----------|-------------|---------|
| `PORT` | TCP port the server listens on. | `8003` |
| `BASE_URL` | Backend base URL for constructing OAuth redirect URIs. | `http://localhost:8003` |
| `FRONTEND_URL` | Frontend URL for constructing callback and redirect URLs. | Value of `BASE_URL` |
| `RUST_LOG` | Logging level filter (uses `tracing_subscriber` `EnvFilter` syntax). | `info` |
| `TRUNK_DIST_DIR` | Path to the pre-built Leptos frontend assets directory. | (compiled-in default) |

## Cache

| Variable | Description | Default |
|----------|-------------|---------|
| `REDIS_URL` | Redis connection string (e.g. `redis://localhost:6379/0`). When not set, falls back to an in-memory KV store suitable for single-instance deployments. | (none -- in-memory) |

## Authentication

| Variable | Description | Default |
|----------|-------------|---------|
| `PASSKEYS_ENABLED` | Enable passkey (WebAuthn) authentication. | `true` |
| `PASSWORD_AUTH_ENABLED` | Enable password-based authentication. | `true` |

### WebAuthn (Passkeys)

| Variable | Description | Default |
|----------|-------------|---------|
| `WEBAUTHN_RP_ID` | Relying Party ID for WebAuthn. Must match the domain users access (e.g. `trakkt.app` or `localhost`). | Extracted from `FRONTEND_URL` host |
| `WEBAUTHN_RP_NAME` | Relying Party display name shown in passkey prompts. | `Trakkt` |

### Google OAuth

| Variable | Description | Default |
|----------|-------------|---------|
| `GOOGLE_OAUTH_CLIENT_ID` | Google OAuth 2.0 client ID. When set (with secret), enables "Sign in with Google". | (none -- disabled) |
| `GOOGLE_OAUTH_CLIENT_SECRET` | Google OAuth 2.0 client secret. | (none) |

## Email (SMTP)

SMTP is optional. Without it, features like email verification and password reset are disabled. In self-hosted mode, the first user can create an account directly without email verification.

| Variable | Description | Default |
|----------|-------------|---------|
| `SMTP_HOST` | SMTP server hostname. Both `SMTP_HOST` and `SMTP_USER` must be set for SMTP to be enabled. | (none -- disabled) |
| `SMTP_PORT` | SMTP server port. | (none) |
| `SMTP_USER` | SMTP username for authentication. | (none) |
| `SMTP_PASSWORD` | SMTP password for authentication. | (none) |
| `SMTP_FROM_EMAIL` | "From" email address for outgoing mail. | (none) |
| `SMTP_FROM_NAME` | "From" display name for outgoing mail. | (none) |

## Attachments

| Variable | Description | Default |
|----------|-------------|---------|
| `ATTACHMENT_STORAGE` | Storage backend: `local` (filesystem) or `s3` (S3-compatible object storage). | `local` |
| `ATTACHMENT_LOCAL_PATH` | Filesystem path for local attachment storage. | `./data/attachments` |

### S3 storage (when `ATTACHMENT_STORAGE=s3`)

| Variable | Description | Default |
|----------|-------------|---------|
| `ATTACHMENT_S3_ENDPOINT` | S3-compatible endpoint URL. | (none) |
| `ATTACHMENT_S3_BUCKET` | S3 bucket name. | (none) |
| `ATTACHMENT_S3_ACCESS_KEY` | S3 access key. | (none) |
| `ATTACHMENT_S3_SECRET_KEY` | S3 secret key. | (none) |
| `ATTACHMENT_S3_REGION` | S3 region. | (none) |

## Notifications

| Variable | Description | Default |
|----------|-------------|---------|
| `SLACK_FEEDBACK_WEBHOOK_URL` | Slack webhook URL for admin notifications (signups, feedback, etc.). | (none -- disabled) |
| `SUPPORT_EMAIL` | Support email address shown in admin notifications. | `support@trakkt.app` |

## Billing (SaaS only)

| Variable | Description | Default |
|----------|-------------|---------|
| `STRIPE_SECRET_KEY` | Stripe API secret key. When set, enables subscription billing. | (none -- disabled) |

## GitHub Integration

| Variable | Description | Default |
|----------|-------------|---------|
| `GITHUB_APP_ID` | GitHub App ID. When set, enables commit/branch/PR linking to issues. | (none -- disabled) |
| `GITHUB_APP_PRIVATE_KEY_PATH` | Path to the GitHub App PEM private key file. Required when `GITHUB_APP_ID` is set. | (none) |
| `GITHUB_APP_NAME` | GitHub App name. | `trakkt` |
