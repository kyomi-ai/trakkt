# Trakkt Kubernetes Deployment

Complete k8s deployment for trakkt with PostgreSQL, Redis, and the trakkt server.

## Prerequisites

- Kubernetes cluster (tested with kind, minikube, or cloud k8s)
- kubectl configured to access your cluster
- Docker or container runtime for building the image

## Quick Start

### 1. Build the Docker image

```bash
cd /path/to/trakkt
docker build -t trakkt:latest .
```

If using a remote registry (e.g., Docker Hub, GCR), tag and push:

```bash
docker tag trakkt:latest your-registry/trakkt:latest
docker push your-registry/trakkt:latest
```

### 2. Configure secrets (IMPORTANT!)

Edit `01-secrets.yaml` and replace the placeholder values:

```yaml
stringData:
  jwt-secret-key: "your-secure-32-char-jwt-secret-here"
  encryption-key: "your-base64-encoded-32-byte-key"  # base64 encode a 32-byte random key
  db-password: "your-secure-postgres-password"
  db-user: "trakkt"
```

Generate secure values:

```bash
# Generate JWT secret (32+ chars)
openssl rand -base64 32

# Generate encryption key (base64-encoded 32 bytes)
openssl rand -base64 32

# Generate DB password
openssl rand -base64 16
```

### 3. Configure environment (optional)

Edit `02-configmap.yaml` to customize:

- `trakkt-mode`: `self_hosted` (recommended for team deployments), `personal`, or `saas`
- `base-url` and `frontend-url`: Update for your domain
- `webauthn-rp-id` and `webauthn-rp-name`: Configure for passkey support
- Database, Redis, and other service parameters

### 4. Deploy to Kubernetes

```bash
# Apply all manifests in order
kubectl apply -f 00-namespace.yaml
kubectl apply -f 01-secrets.yaml
kubectl apply -f 02-configmap.yaml
kubectl apply -f 10-postgres.yaml
kubectl apply -f 11-redis.yaml
kubectl apply -f 20-trakkt.yaml
```

Or apply all at once:

```bash
kubectl apply -f .
```

### 5. Verify deployment

```bash
# Check pod status
kubectl get pods -n trakkt

# Watch pod logs
kubectl logs -n trakkt deployment/trakkt -f

# Check services
kubectl get svc -n trakkt

# Port forward to test locally
kubectl port-forward -n trakkt svc/trakkt 8003:80

# Access at http://localhost:8003
```

## Configuration Details

### Secrets (01-secrets.yaml)

- `jwt-secret-key`: Used to sign JWT tokens. Must be 32+ characters.
- `encryption-key`: Base64-encoded 32-byte key for AES-256-GCM encryption at rest.
- `db-password`: PostgreSQL password for the `trakkt` user.
- `db-user`: PostgreSQL username (default: `trakkt`).

### ConfigMap (02-configmap.yaml)

- `trakkt-mode`: Deployment mode (`personal`, `self_hosted`, `saas`)
- `db-*`: Database connection parameters
- `redis-*`: Redis connection parameters
- `port`: Server listen port (default: 8003)
- `base-url`: Backend API URL (e.g., `http://trakkt.local`)
- `frontend-url`: Frontend URL for OAuth redirects
- `webauthn-*`: WebAuthn (passkey) relying party configuration

### Storage

- PostgreSQL: 10Gi persistent volume
- Redis: 5Gi persistent volume

Adjust `spec.resources.requests.storage` in `10-postgres.yaml` and `11-redis.yaml` as needed.

### Resource Limits

Adjust resource requests/limits in the manifests based on your cluster capacity:

- PostgreSQL: 256Mi/512Mi (requests/limits)
- Redis: 128Mi/256Mi
- Trakkt: 512Mi/1Gi

## Post-Deployment

### Initialize the Database

The trakkt server automatically runs migrations on startup. If `TRAKKT_MODE=self_hosted`, it will create an initial admin workspace and user.

### Access Trakkt

1. Get the LoadBalancer IP/hostname:

```bash
kubectl get svc -n trakkt trakkt
```

2. Update your hosts file or DNS to point to the service:

```
<EXTERNAL-IP>  trakkt.local
```

3. Visit http://trakkt.local

### Troubleshooting

#### Pod won't start

```bash
# Check pod logs
kubectl logs -n trakkt deployment/trakkt

# Describe pod for events
kubectl describe pod -n trakkt <pod-name>
```

#### Database connection errors

Verify PostgreSQL is running and accessible:

```bash
kubectl exec -n trakkt postgres-0 -- psql -U trakkt -d trakkt -c "SELECT 1"
```

#### Redis connection errors

Verify Redis is running:

```bash
kubectl exec -n trakkt redis-0 -- redis-cli ping
```

## Scaling

For production:

1. Increase Deployment replicas in `20-trakkt.yaml`
2. Use managed database services (RDS, Cloud SQL) instead of in-cluster PostgreSQL
3. Use managed cache services (ElastiCache, Memorystore) instead of in-cluster Redis
4. Add ingress configuration for TLS and routing
5. Configure persistent volume storage classes for your cloud provider

## Backup & Recovery

### PostgreSQL Backups

```bash
# Manual backup
kubectl exec -n trakkt postgres-0 -- pg_dump -U trakkt trakkt > backup.sql

# Restore from backup
kubectl exec -i -n trakkt postgres-0 -- psql -U trakkt trakkt < backup.sql
```

For production, use managed database backups or configure WAL archiving.

## Cleanup

```bash
# Remove all trakkt resources
kubectl delete namespace trakkt
```

This will delete all pods, services, and persistent volumes in the trakkt namespace.
