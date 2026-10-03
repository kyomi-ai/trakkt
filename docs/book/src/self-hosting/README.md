# Self-Hosting

Trakkt is designed to be self-hosted. It ships as a single binary with the frontend embedded, so deployment requires no Node.js, no separate frontend server, and no build tools on the target machine.

## Deployment Options

| Option | Best for | Complexity |
|--------|----------|-----------|
| [Docker](docker.md) | Quick setup, single-server deployments | Low |
| [Kubernetes](kubernetes.md) | Production, high availability, team infrastructure | Medium |
| Binary | Minimal setups, custom orchestration | Low |

## Requirements

Regardless of deployment method, Trakkt needs:

- **PostgreSQL 14+** -- the primary data store for team and SaaS deployments. In personal mode, SQLite is used instead.
- **Redis** (optional) -- required for multi-instance deployments to share WebSocket state and KV cache. Single-instance deployments use an in-memory KV store.
- **Port 8003** (default) -- the HTTP port the server listens on. Configurable via the `PORT` environment variable.

## Deployment Modes

Set `TRAKKT_MODE` to choose the right mode for your use case:

- **`self_hosted`** -- full authentication (password, passkeys, optional Google OAuth). The first user creates their account directly. Recommended for team deployments.
- **`personal`** -- no login, SQLite backend, auto-provisioned user and workspace. Recommended for single-user local use.

## Security

Trakkt's Docker image runs with security hardening by default:

- Non-root user (`trakkt`, UID 1000)
- Read-only root filesystem (when using the Kubernetes manifests)
- Dropped Linux capabilities
- No privilege escalation

See the [Configuration](../getting-started/configuration.md) page for all environment variables including TLS, SMTP, and authentication settings.

## Database Migrations

Trakkt automatically runs database migrations on startup. There is no separate migration step -- just start the server and it will bring the schema up to date. Both PostgreSQL and SQLite migrations are maintained in parallel.
