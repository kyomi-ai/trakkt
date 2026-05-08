# Trakkt Kubernetes Deployment Guide

This document provides a complete guide for deploying trakkt to a Kubernetes cluster with PostgreSQL and Redis.

## Overview

The deployment includes:
- **PostgreSQL StatefulSet**: Database for issue data
- **Redis StatefulSet**: Cache and session store
- **Trakkt Deployment**: Main application server
- **Services**: Internal and external networking
- **Secrets**: Sensitive configuration (JWT, encryption keys, passwords)
- **ConfigMap**: Non-sensitive configuration

## Directory Structure

```
deploy/k8s/
├── README.md                 # Detailed deployment documentation
├── deploy.sh                 # Quick deployment script
├── 00-namespace.yaml         # Kubernetes namespace
├── 01-secrets.yaml           # Secrets (JWT, encryption, DB password)
├── 02-configmap.yaml         # Configuration (URLs, modes, limits)
├── 10-postgres.yaml          # PostgreSQL StatefulSet
├── 11-redis.yaml             # Redis StatefulSet
└── 20-trakkt.yaml            # Trakkt Deployment and Service
```

## Quick Start

### 1. Build the Docker Image

```bash
cd /home/jason/repos/trakkt
docker build -t trakkt:latest .
```

### 2. Make Available to Cluster

**Option A: Push to Registry (Recommended for Remote Clusters)**
```bash
docker tag trakkt:latest your-registry.example.com/trakkt:latest
docker push your-registry.example.com/trakkt:latest

# Update deploy/k8s/20-trakkt.yaml:
# Change image: trakkt:latest → image: your-registry.example.com/trakkt:latest
```

**Option B: Load into Local Cluster (kind/minikube)**
```bash
kind load docker-image trakkt:latest --name your-cluster
# or for minikube:
minikube image load trakkt:latest
```

### 3. Update Secrets

Edit `deploy/k8s/01-secrets.yaml` with secure values:

```bash
# Generate secrets (from the k8s directory)
cd deploy/k8s

# JWT secret
openssl rand -base64 32

# Encryption key (base64-encoded 32-byte key)
openssl rand -base64 32

# DB password
openssl rand -base64 16
```

### 4. Deploy to Kubernetes

```bash
cd deploy/k8s
./deploy.sh
```

Or manually:
```bash
kubectl apply -f 00-namespace.yaml
kubectl apply -f 01-secrets.yaml
kubectl apply -f 02-configmap.yaml
kubectl apply -f 10-postgres.yaml
kubectl apply -f 11-redis.yaml
kubectl apply -f 20-trakkt.yaml
```

### 5. Verify Deployment

```bash
# Check pod status
kubectl get pods -n trakkt

# View logs
kubectl logs -n trakkt deployment/trakkt -f

# Port forward
kubectl port-forward -n trakkt svc/trakkt 8003:80

# Access at http://localhost:8003
```

## Environment Variables

Trakkt requires these environment variables (configured in k8s manifests):

### Required
- `DATABASE_URL`: PostgreSQL connection string
- `JWT_SECRET_KEY`: JWT signing secret (min 32 chars)
- `ENCRYPTION_KEY`: Base64-encoded 32-byte AES key
- `TRAKKT_MODE`: Deployment mode (personal, self_hosted, or saas)

### Optional (with defaults)
- `REDIS_URL`: Redis connection (if not set, uses in-memory store)
- `PORT`: Server port (default: 8003)
- `BASE_URL`: Backend URL (default: http://localhost:8003)
- `FRONTEND_URL`: Frontend URL (default: same as BASE_URL)
- `WEBAUTHN_RP_ID`: Passkey relying party ID
- `WEBAUTHN_RP_NAME`: Passkey display name
- `RUST_LOG`: Log level (info, debug, warn, error)

## Troubleshooting

### Pod won't start
```bash
kubectl describe pod -n trakkt <pod-name>
kubectl logs -n trakkt <pod-name>
```

### Database connection error
```bash
# Check PostgreSQL
kubectl exec -n trakkt postgres-0 -- psql -U trakkt -d trakkt -c "SELECT 1"

# Check connection string in logs
kubectl logs -n trakkt deployment/trakkt | grep DATABASE_URL
```

### Redis connection error
```bash
# Check Redis
kubectl exec -n trakkt redis-0 -- redis-cli ping

# Check Redis URL in logs
kubectl logs -n trakkt deployment/trakkt | grep REDIS_URL
```

### Frontend not loading
Check that Trunk built correctly and assets are in Docker image:
```bash
docker run trakkt:latest ls -la /app/dist
```

## Configuration

### Customize Deployment

Edit `deploy/k8s/02-configmap.yaml`:
- Change `trakkt-mode` for different deployment types
- Update URLs for your domain
- Adjust resource limits

Edit `deploy/k8s/01-secrets.yaml`:
- Replace placeholder secrets with real values
- Ensure secrets are strong and random

### Scale Resources

Edit `deploy/k8s/20-trakkt.yaml`:
- Increase replicas for multiple instances
- Adjust resource requests/limits based on load

## Production Considerations

1. **Use Managed Services**
   - CloudSQL/RDS for PostgreSQL
   - ElastiCache/Memorystore for Redis
   - Container registry for images

2. **Add Ingress**
   - Route TLS termination
   - Expose via domain name
   - Add rate limiting/WAF

3. **Configure Backups**
   - PostgreSQL WAL archiving
   - Regular snapshots
   - Recovery testing

4. **Monitoring**
   - Prometheus metrics
   - Logging (ELK, Cloud Logging)
   - Alerting on pod/database health

5. **Security**
   - Use network policies
   - RBAC for access control
   - Secret rotation
   - Pod security standards

## Cleanup

Remove all trakkt resources:
```bash
kubectl delete namespace trakkt
```

This removes all pods, services, secrets, configmaps, and persistent volumes.

## Additional Resources

- [Trakkt Repository](https://github.com/yourusername/trakkt)
- [Kubernetes Documentation](https://kubernetes.io/docs/)
- [PostgreSQL on Kubernetes](https://kubernetes.io/docs/tasks/run-application/run-replicated-stateful-application/)
- [Redis on Kubernetes](https://www.redhat.com/en/blog/redis-kubernetes)
