# Kubernetes

Trakkt provides production-ready Kubernetes manifests in the [`deploy/k8s/`](https://github.com/kyomi-ai/trakkt/tree/main/deploy/k8s) directory. The manifests deploy a complete stack: PostgreSQL, Redis, and the Trakkt server with security hardening.

## Manifest Overview

| File | Resource | Description |
|------|----------|-------------|
| `00-namespace.yaml` | Namespace | Creates the `trakkt` namespace for all resources |
| `01-secrets.example.yaml` | Secret | Template for JWT key, encryption key, DB credentials, SMTP credentials. Copy to `01-secrets.yaml` and fill in real values. |
| `02-configmap.yaml` | ConfigMap | Non-secret configuration: deployment mode, database host/port, URLs, WebAuthn settings, SMTP host/port, logging level |
| `10-postgres.yaml` | StatefulSet + PVC + Service | PostgreSQL 16 with 10Gi persistent storage, liveness/readiness probes |
| `11-redis.yaml` | StatefulSet + PVC + Service | Redis 7 with AOF persistence, 5Gi storage, liveness/readiness probes |
| `20-trakkt.yaml` | Deployment + Service | Trakkt server with LoadBalancer service, health probes, security context, resource limits |
| `30-ingress.yaml` | IngressRoute (Traefik) | Traefik IngressRoute for routing traffic to the Trakkt service |

## Quick Start

### 1. Create secrets

Copy the example secrets file and fill in real values:

```bash
cp deploy/k8s/01-secrets.example.yaml deploy/k8s/01-secrets.yaml
```

Generate secure values:

```bash
# JWT secret (32+ characters)
openssl rand -base64 32

# Encryption key (base64-encoded 32 bytes)
openssl rand -base64 32

# Database password
openssl rand -base64 16
```

Edit `01-secrets.yaml` with the generated values. Never commit this file to git.

### 2. Configure environment

Edit `02-configmap.yaml` to set:

- `trakkt-mode`: `self_hosted` for team deployments
- `base-url` and `frontend-url`: your domain (e.g. `https://trakkt.example.com`)
- `webauthn-rp-id`: your domain (e.g. `trakkt.example.com`) for passkey support

### 3. Deploy

Apply all manifests in order:

```bash
kubectl apply -f deploy/k8s/00-namespace.yaml
kubectl apply -f deploy/k8s/01-secrets.yaml
kubectl apply -f deploy/k8s/02-configmap.yaml
kubectl apply -f deploy/k8s/10-postgres.yaml
kubectl apply -f deploy/k8s/11-redis.yaml
kubectl apply -f deploy/k8s/20-trakkt.yaml
```

Or apply the entire directory:

```bash
kubectl apply -f deploy/k8s/
```

### 4. Verify

```bash
# Check pod status
kubectl get pods -n trakkt

# Watch logs
kubectl logs -n trakkt deployment/trakkt -f

# Port forward for local access
kubectl port-forward -n trakkt svc/trakkt 8003:80
```

Visit `http://localhost:8003`.

## Security Hardening

The Trakkt deployment manifest includes production-grade security defaults:

```yaml
securityContext:
  readOnlyRootFilesystem: true
  allowPrivilegeEscalation: false
  runAsNonRoot: true
  runAsUser: 1000
  capabilities:
    drop:
      - ALL
```

Writable directories (`/app/data/attachments` and `/tmp`) are mounted as `emptyDir` volumes so the root filesystem stays read-only.

## Health Probes

The deployment configures both liveness and readiness probes against `/health`:

- **Liveness**: checks every 30s, starting after 30s, 3 failures to restart
- **Readiness**: checks every 10s, starting after 10s, 3 failures to remove from service

## Resource Limits

Default resource allocations:

| Component | Requests | Limits |
|-----------|----------|--------|
| Trakkt | 500m CPU, 512Mi RAM | 1000m CPU, 1Gi RAM |
| PostgreSQL | 250m CPU, 256Mi RAM | 500m CPU, 512Mi RAM |
| Redis | 100m CPU, 128Mi RAM | 200m CPU, 256Mi RAM |

Adjust these in the manifests based on your cluster capacity and workload.

## Storage

| Component | Volume | Size |
|-----------|--------|------|
| PostgreSQL | `postgres-pvc` | 10Gi |
| Redis | `redis-pvc` | 5Gi |
| Trakkt attachments | `emptyDir` | (ephemeral) |

For production, consider using S3-compatible object storage for attachments (`ATTACHMENT_STORAGE=s3`) rather than the ephemeral `emptyDir`.

## Ingress

The included `30-ingress.yaml` is a Traefik IngressRoute. For other ingress controllers (nginx, etc.), create an appropriate Ingress resource pointing to the `trakkt` service on port 80.

Example nginx Ingress:

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: trakkt
  namespace: trakkt
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt
spec:
  tls:
    - hosts:
        - trakkt.example.com
      secretName: trakkt-tls
  rules:
    - host: trakkt.example.com
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: trakkt
                port:
                  number: 80
```

## Scaling

For production at scale:

1. Increase `replicas` in `20-trakkt.yaml` (requires Redis for cross-instance WebSocket sync)
2. Use managed database services (RDS, Cloud SQL) instead of in-cluster PostgreSQL
3. Use managed cache (ElastiCache, Memorystore) instead of in-cluster Redis
4. Configure persistent volume storage classes for your cloud provider
5. Add TLS via cert-manager or a cloud load balancer

## Backup

### PostgreSQL

```bash
# Manual backup
kubectl exec -n trakkt postgres-0 -- pg_dump -U trakkt trakkt > backup.sql

# Restore
kubectl exec -i -n trakkt postgres-0 -- psql -U trakkt trakkt < backup.sql
```

For production, use managed database backups or configure WAL archiving.

## Cleanup

Remove all Trakkt resources:

```bash
kubectl delete namespace trakkt
```

This deletes all pods, services, secrets, and persistent volumes in the namespace.
