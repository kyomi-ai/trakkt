# Upgrading

Trakkt uses date-based version tags in the format `vYYYY.MM.DD.N` (e.g., `v2026.05.29.1`). Each release is published as:

- A **GitHub Release** at [github.com/kyomi-ai/trakkt/releases](https://github.com/kyomi-ai/trakkt/releases) with auto-generated release notes
- A **Docker image** on GitHub Container Registry, tagged with both the version and `latest`

## Database Migrations

Trakkt automatically runs database migrations on startup. There is no separate migration step -- upgrade the binary or image and restart. The server brings the schema up to date before accepting requests.

## Docker

Pull the latest image and restart:

```bash
docker pull ghcr.io/kyomi-ai/trakkt:latest
docker compose up -d
```

To pin to a specific version instead of `latest`:

```bash
docker pull ghcr.io/kyomi-ai/trakkt:2026.05.29.1
```

Then update the `image` tag in your `docker-compose.yml`:

```yaml
services:
  trakkt:
    image: ghcr.io/kyomi-ai/trakkt:2026.05.29.1
```

## Binary

1. Download the new binary from the [GitHub Releases](https://github.com/kyomi-ai/trakkt/releases) page or build from source.
2. Stop the running Trakkt process.
3. Replace the binary.
4. Start Trakkt again. Migrations run automatically on startup.

```bash
# Example: replace a systemd-managed binary
sudo systemctl stop trakkt
cp trakkt-new /usr/local/bin/trakkt
sudo systemctl start trakkt
```

## Kubernetes

Update the image tag in your deployment manifest and apply:

```bash
kubectl set image deployment/trakkt trakkt=ghcr.io/kyomi-ai/trakkt:2026.05.29.1 -n trakkt
```

Or edit `20-trakkt.yaml` with the new tag and reapply:

```bash
kubectl apply -f deploy/k8s/20-trakkt.yaml
```

### Zero-Downtime Upgrades

The Trakkt deployment manifest uses Kubernetes' default rolling update strategy. During an upgrade:

1. Kubernetes starts a new pod with the updated image.
2. The new pod runs migrations and starts the server.
3. The readiness probe (`/health`, checked every 10s) confirms the new pod is ready.
4. Traffic shifts to the new pod.
5. The old pod is terminated.

The old pod continues serving requests until the new pod passes its readiness check, so there is no downtime during upgrades.

For multi-replica deployments (which require Redis for cross-instance WebSocket sync), Kubernetes rolls pods one at a time by default, maintaining availability throughout.

## Before Upgrading

- **Check the release notes** on the [GitHub Releases](https://github.com/kyomi-ai/trakkt/releases) page for breaking changes or new required environment variables.
- **Back up your database** before major upgrades. See the [Kubernetes](kubernetes.md) page for backup instructions, or run `pg_dump` directly for Docker/binary deployments.

## Rollback

If an upgrade introduces issues, revert to the previous version:

```bash
# Docker
docker pull ghcr.io/kyomi-ai/trakkt:PREVIOUS_VERSION
docker compose up -d

# Kubernetes
kubectl set image deployment/trakkt trakkt=ghcr.io/kyomi-ai/trakkt:PREVIOUS_VERSION -n trakkt
```

Database migrations are forward-only. Rolling back the application version will not undo schema changes, but Trakkt maintains backward compatibility within the migration chain.
