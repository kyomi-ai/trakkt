# Trakkt Kubernetes Setup Complete ✅

All k8s deployment infrastructure has been created. Here's what's ready:

## What's Been Created

### 1. **Dockerfile** (`/Dockerfile`)
- Multi-stage build: Rust + Trunk for Leptos frontend
- Uses `debian:bookworm-slim` runtime (not scratch, for compatibility)
- Embeds frontend assets at `/app/dist`
- Runs as non-root user (UID 1000)
- Includes health check endpoint

### 2. **K8s Manifests** (`/deploy/k8s/`)

#### Core Infrastructure
- **00-namespace.yaml**: `trakkt` namespace
- **01-secrets.yaml**: JWT secret, encryption key, DB password
- **02-configmap.yaml**: Configuration (mode, URLs, resource limits)

#### Databases
- **10-postgres.yaml**: PostgreSQL StatefulSet with PVC
  - 10Gi persistent volume
  - Automatic migration support
  - Health checks (pg_isready)
  
- **11-redis.yaml**: Redis StatefulSet with PVC
  - 5Gi persistent volume
  - AOF persistence enabled
  - Health checks (redis-cli ping)

#### Application
- **20-trakkt.yaml**: Trakkt Deployment + Service
  - Proper environment variable wiring
  - Liveness/readiness probes
  - LoadBalancer service for external access
  - Security context (read-only FS, non-root, no privesc)

### 3. **Deployment Scripts & Docs**
- **deploy.sh**: Automated deployment with validation
- **README.md**: Detailed k8s deployment guide
- **DEPLOYMENT.md**: Quick-start and troubleshooting
- **KUBERNETES_SETUP.md**: This file

## Next Steps

### Step 1: Build Docker Image

```bash
docker build -t trakkt:latest .
```

**Note**: This takes 10-20 minutes first time (Rust compilation is slow)

### Step 2: Verify Image Built

```bash
docker image ls | grep trakkt
# Should show: trakkt    latest    <image-id>    <timestamp>
```

### Step 3: Load Image into Cluster

**Option A: If using kind/minikube locally**
```bash
kind load docker-image trakkt:latest --name your-cluster-name
# or for minikube
minikube image load trakkt:latest
```

**Option B: If using remote cluster**
```bash
# Push to registry (Docker Hub, GCR, private registry, etc.)
docker tag trakkt:latest your-registry/trakkt:latest
docker push your-registry/trakkt:latest

# Update deploy/k8s/20-trakkt.yaml:
# Change: image: trakkt:latest
# To:     image: your-registry/trakkt:latest
```

### Step 4: Update Secrets (CRITICAL!)

Edit `deploy/k8s/01-secrets.yaml` and replace placeholder values with **strong, random values**:

```bash
cd deploy/k8s

# Generate JWT secret (32+ chars)
openssl rand -base64 32

# Generate encryption key (32-byte base64)
openssl rand -base64 32

# Generate DB password
openssl rand -base64 16
```

Example `01-secrets.yaml` (with real values):
```yaml
stringData:
  jwt-secret-key: "xA7qZ9nB2mK5pL8dF3gH6jX1vC4wE7rT0yP9uM2sQ5"
  encryption-key: "K8jX3qL7mZ9pA2bD5eF8gH1jK4lM7nP0qR3sT6uV9"
  db-password: "SecurePostgresPassword123456789"
  db-user: "trakkt"
```

### Step 5: Customize Configuration (Optional)

Edit `deploy/k8s/02-configmap.yaml` to customize:

- **trakkt-mode**: `self_hosted` (recommended) or `personal` or `saas`
- **URLs**: Update `base-url` and `frontend-url` for your domain
- **WebAuthn**: Update `webauthn-rp-id` for passkey support
- **Resource limits**: Adjust CPU/memory based on your needs

Example for `trakkt.example.com`:
```yaml
data:
  trakkt-mode: "self_hosted"
  base-url: "https://trakkt.example.com"
  frontend-url: "https://trakkt.example.com"
  webauthn-rp-id: "trakkt.example.com"
```

### Step 6: Deploy to Kubernetes

**Automatic (recommended)**:
```bash
cd deploy/k8s
./deploy.sh
```

**Manual** (if deploy.sh has issues):
```bash
cd deploy/k8s
kubectl apply -f 00-namespace.yaml
kubectl apply -f 01-secrets.yaml
kubectl apply -f 02-configmap.yaml
kubectl apply -f 10-postgres.yaml
kubectl apply -f 11-redis.yaml
kubectl apply -f 20-trakkt.yaml
```

### Step 7: Verify Deployment

```bash
# Check all pods are running
kubectl get pods -n trakkt

# Expected output:
# NAME                      READY   STATUS    RESTARTS   AGE
# postgres-0                1/1     Running   0          2m
# redis-0                   1/1     Running   0          2m
# trakkt-xxxxxxxxxx-xxxxx   1/1     Running   0          1m

# View logs
kubectl logs -n trakkt deployment/trakkt -f

# Port forward for local testing
kubectl port-forward -n trakkt svc/trakkt 8003:80

# Visit http://localhost:8003
```

## Deployment Architecture

```
┌─────────────────────────────────────────────────┐
│          Kubernetes Cluster (trakkt ns)         │
├─────────────────────────────────────────────────┤
│                                                 │
│  ┌──────────────┐  ┌──────────────┐             │
│  │  Trakkt Pod  │  │   Trakkt Pod │ (replicas) │
│  │ :8003        │  │   :8003      │             │
│  └──────────────┘  └──────────────┘             │
│         │                  │                    │
│         └──────┬───────────┘                    │
│              Service                           │
│         LoadBalancer:80 ──> :8003              │
│                                                 │
│  Database Pool          Cache                  │
│  ┌─────────────┐        ┌──────────┐           │
│  │ PostgreSQL  │        │  Redis   │           │
│  │   :5432     │        │  :6379   │           │
│  └─────────────┘        └──────────┘           │
│      PVC:10Gi              PVC:5Gi             │
│                                                 │
└─────────────────────────────────────────────────┘
```

## Configuration Mapping

| K8s Component | Source | Environment Variable | Usage |
|---|---|---|---|
| Secrets | `01-secrets.yaml` | JWT_SECRET_KEY, ENCRYPTION_KEY, DB password | Authentication, encryption, DB auth |
| ConfigMap | `02-configmap.yaml` | TRAKKT_MODE, BASE_URL, FRONTEND_URL, etc. | Application behavior, URLs, feature flags |
| Postgres | `10-postgres.yaml` | DATABASE_URL | Persistent data storage |
| Redis | `11-redis.yaml` | REDIS_URL | Session cache, real-time updates |

## Environment Variables Configured

### Required (auto-generated in k8s)
- `DATABASE_URL`: postgresql://trakkt:password@postgres:5432/trakkt
- `REDIS_URL`: redis://redis:6379/0
- `JWT_SECRET_KEY`: [from secret]
- `ENCRYPTION_KEY`: [from secret]

### Optional (from ConfigMap)
- `TRAKKT_MODE`: self_hosted (configurable)
- `BASE_URL`: http://trakkt.local (update this!)
- `FRONTEND_URL`: http://trakkt.local (update this!)
- `WEBAUTHN_RP_ID`: trakkt.local (update this!)
- `PORT`: 8003 (usually don't change)
- `RUST_LOG`: info (info/debug/warn/error)

## Common Issues & Solutions

### Build fails with "trunk not found"
→ Trunk is auto-installed via `cargo install`. If it fails, ensure internet access.

### Pod stuck in Pending
```bash
kubectl describe pod -n trakkt <pod-name>
# Check for node capacity, storage class issues, or pull errors
```

### Database migration fails
```bash
kubectl logs -n trakkt deployment/trakkt | grep -i migration
# Verify DATABASE_URL is correct and postgres is running
```

### Redis connection error
```bash
kubectl exec -n trakkt redis-0 -- redis-cli ping
# Should return PONG
```

### Frontend 404 (not found)
```bash
# Check image was built with assets
docker run trakkt:latest ls -la /app/dist
# Should show index.html and other Leptos files
```

## Production Checklist

Before going to production:

- [ ] Use strong, randomly-generated secrets (not placeholders)
- [ ] Use managed PostgreSQL (RDS, Cloud SQL, etc.)
- [ ] Use managed Redis (ElastiCache, Memorystore, etc.)
- [ ] Add TLS/HTTPS (via Ingress + cert-manager)
- [ ] Configure backup strategy
- [ ] Set up monitoring (Prometheus, Grafana)
- [ ] Configure log aggregation (ELK, Cloud Logging)
- [ ] Review network policies
- [ ] Test disaster recovery
- [ ] Scale to multiple replicas
- [ ] Use resource quotas and limits

## Support

For issues:
1. Check logs: `kubectl logs -n trakkt deployment/trakkt -f`
2. Check pod events: `kubectl describe pod -n trakkt <pod-name>`
3. Check resources: `kubectl top pod -n trakkt`
4. Review manifests: Compare to examples in `deploy/k8s/README.md`

## Files Summary

```
trakkt/
├── Dockerfile                          # Multi-stage build
├── DEPLOYMENT.md                       # Quick start guide
├── KUBERNETES_SETUP.md                 # This file
└── deploy/k8s/
    ├── README.md                       # Detailed k8s docs
    ├── deploy.sh                       # Auto deployment script
    ├── 00-namespace.yaml               # Namespace
    ├── 01-secrets.yaml                 # Secrets (EDIT THIS!)
    ├── 02-configmap.yaml               # Configuration (CUSTOMIZE)
    ├── 10-postgres.yaml                # PostgreSQL
    ├── 11-redis.yaml                   # Redis
    └── 20-trakkt.yaml                  # Trakkt app
```

## Next Command

When ready to deploy, run:

```bash
cd deploy/k8s
./deploy.sh
```

Happy deploying! 🚀
