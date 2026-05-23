# Docker

The official Trakkt Docker image is available from GitHub Container Registry.

## Quick Start

```bash
docker pull ghcr.io/kyomi-ai/trakkt:latest

docker run -p 8003:8003 \
  -e DATABASE_URL=postgres://user:pass@host:5432/trakkt \
  -e JWT_SECRET_KEY=$(openssl rand -base64 32) \
  -e ENCRYPTION_KEY=$(openssl rand -base64 32) \
  -e TRAKKT_MODE=self_hosted \
  ghcr.io/kyomi-ai/trakkt:latest
```

## Docker Compose

For a complete self-contained deployment with PostgreSQL:

```yaml
version: "3.8"

services:
  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_DB: trakkt
      POSTGRES_USER: trakkt
      POSTGRES_PASSWORD: ${DB_PASSWORD:-changeme}
    volumes:
      - postgres_data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U trakkt -d trakkt"]
      interval: 10s
      timeout: 5s
      retries: 5

  trakkt:
    image: ghcr.io/kyomi-ai/trakkt:latest
    ports:
      - "8003:8003"
    environment:
      DATABASE_URL: postgres://trakkt:${DB_PASSWORD:-changeme}@postgres:5432/trakkt
      JWT_SECRET_KEY: ${JWT_SECRET_KEY}
      ENCRYPTION_KEY: ${ENCRYPTION_KEY}
      TRAKKT_MODE: self_hosted
      BASE_URL: http://localhost:8003
      FRONTEND_URL: http://localhost:8003
    volumes:
      - attachments:/app/data/attachments
    depends_on:
      postgres:
        condition: service_healthy

volumes:
  postgres_data:
  attachments:
```

Create a `.env` file alongside your `docker-compose.yml`:

```bash
DB_PASSWORD=$(openssl rand -base64 16)
JWT_SECRET_KEY=$(openssl rand -base64 32)
ENCRYPTION_KEY=$(openssl rand -base64 32)
```

Start the stack:

```bash
docker compose up -d
```

Trakkt will be available at `http://localhost:8003`.

## Image Details

The official image is based on `debian:bookworm-slim` and includes:

- The pre-built Trakkt server binary at `/app/trakkt`
- The pre-built Leptos frontend assets at `/app/dist`
- A non-root `trakkt` user (UID 1000)
- Default port `8003` (configurable via `PORT`)
- `TRUNK_DIST_DIR=/app/dist` pre-configured

### Health Check

The image includes a built-in health check:

```
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3
  CMD wget -qO- http://localhost:8003/health || exit 1
```

You can also probe the health endpoint directly:

```bash
curl http://localhost:8003/health
```

## Volume Mounts

| Path | Purpose |
|------|---------|
| `/app/data/attachments` | File attachment storage (when using local storage backend) |
| `/tmp` | Temporary files (required for read-only root filesystem) |

## Adding Redis

For multi-instance deployments, add Redis to share WebSocket state:

```yaml
  redis:
    image: redis:7-alpine
    command: redis-server --appendonly yes --appendfsync everysec
    volumes:
      - redis_data:/data
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 10s
      timeout: 5s
      retries: 5

  trakkt:
    environment:
      REDIS_URL: redis://redis:6379/0
    # ... rest of trakkt config
```

Without Redis, the server uses an in-memory KV store. This works for single-instance deployments but WebSocket broadcasts will not reach other instances.

## Building the Image

To build from source instead of using the pre-built image:

```bash
cd /path/to/trakkt
./scripts/build-image.sh
```

This script:
1. Strips local dev patches from `Cargo.toml`
2. Builds the server binary with `cargo build --release --bin trakkt`
3. Builds the WASM frontend with `trunk build --release`
4. Runs `docker build -t trakkt:latest .`

The binary and WASM frontend must be compiled on the same host so that `CARGO_MANIFEST_DIR` paths match -- Leptos server function hashes include this path.
