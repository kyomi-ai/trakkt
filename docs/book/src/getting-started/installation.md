# Installation

Trakkt ships as a single binary with the frontend embedded. There are several ways to get it running.

## Docker (recommended)

Pull the official image from GitHub Container Registry:

```bash
docker pull ghcr.io/kyomi-ai/trakkt:latest
```

Run with the minimum required environment variables:

```bash
docker run -p 8003:8003 \
  -e DATABASE_URL=postgres://user:pass@host:5432/trakkt \
  -e JWT_SECRET_KEY=$(openssl rand -base64 32) \
  -e ENCRYPTION_KEY=$(openssl rand -base64 32) \
  -e TRAKKT_MODE=self_hosted \
  ghcr.io/kyomi-ai/trakkt:latest
```

The server will be available at `http://localhost:8003`.

### Required environment variables

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection string (e.g. `postgres://user:pass@host:5432/trakkt`) |
| `JWT_SECRET_KEY` | Secret key for signing JWT tokens. Generate with `openssl rand -base64 32`. |
| `ENCRYPTION_KEY` | Base64-encoded 32-byte key for AES-256-GCM encryption at rest. Generate with `openssl rand -base64 32`. |

See the [Configuration](configuration.md) page for all available environment variables.

## Docker Compose

For a complete setup with PostgreSQL, see the [Docker self-hosting guide](../self-hosting/docker.md).

## Kubernetes

See the [Kubernetes self-hosting guide](../self-hosting/kubernetes.md) for complete manifests with PostgreSQL, Redis, health checks, and security hardening.

## Personal Mode (SQLite, no login)

For single-user local use, Trakkt can run with SQLite and no authentication:

```bash
git clone https://github.com/kyomi-ai/trakkt.git
cd trakkt
cp .env.example .env
```

Build the frontend and run:

```bash
cd crates/trakkt-ui && trunk build && cd ../..
TRAKKT_MODE=personal cargo run --package trakkt-server
```

In personal mode, Trakkt uses SQLite (no database server needed) and automatically provisions a user and workspace on first boot.

## Building from source

Prerequisites:
- Rust 1.85+ (2024 edition)
- [Trunk](https://trunkrs.dev/) for building the WASM frontend
- PostgreSQL 14+ (for team/SaaS mode) or no database (for personal mode)

```bash
git clone https://github.com/kyomi-ai/trakkt.git
cd trakkt

# Build the frontend
cd crates/trakkt-ui && trunk build --release && cd ../..

# Build and run the server
cargo run --release --package trakkt-server
```

The server binary is at `target/release/trakkt`. The frontend assets are embedded from `crates/trakkt-ui/dist/`.

## Health check

Once running, verify the server is healthy:

```bash
curl http://localhost:8003/health
```
