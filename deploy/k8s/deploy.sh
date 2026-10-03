#!/bin/bash
# Quick deployment script for trakkt to k8s

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "🚀 Deploying trakkt to Kubernetes..."

# Check kubectl is available
if ! command -v kubectl &> /dev/null; then
    echo "❌ kubectl not found. Please install kubectl."
    exit 1
fi

# Create namespace
echo "📦 Creating namespace..."
kubectl apply -f 00-namespace.yaml

# Create secrets
echo "🔐 Creating secrets..."
kubectl apply -f 01-secrets.yaml

# Create config
echo "⚙️  Creating configuration..."
kubectl apply -f 02-configmap.yaml

# Deploy PostgreSQL
echo "🐘 Deploying PostgreSQL..."
kubectl apply -f 10-postgres.yaml

# Wait for PostgreSQL to be ready
echo "⏳ Waiting for PostgreSQL to be ready..."
kubectl wait --for=condition=ready pod -l app=postgres -n trakkt --timeout=300s

# Deploy Redis
echo "🔴 Deploying Redis..."
kubectl apply -f 11-redis.yaml

# Wait for Redis to be ready
echo "⏳ Waiting for Redis to be ready..."
kubectl wait --for=condition=ready pod -l app=redis -n trakkt --timeout=300s

# Deploy MinIO (attachment storage)
echo "📦 Deploying MinIO..."
kubectl apply -f 12-minio.yaml

# Wait for MinIO to be ready
echo "⏳ Waiting for MinIO to be ready..."
kubectl wait --for=condition=ready pod -l app=minio -n trakkt --timeout=300s

# Wait for bucket initialization
echo "⏳ Waiting for bucket initialization..."
kubectl wait --for=condition=complete job/minio-create-bucket -n trakkt --timeout=300s

# Deploy trakkt
echo "🎯 Deploying trakkt..."
kubectl apply -f 20-trakkt.yaml

# Wait for trakkt to be ready
echo "⏳ Waiting for trakkt to be ready..."
kubectl wait --for=condition=ready pod -l app=trakkt -n trakkt --timeout=300s

echo ""
echo "✅ Deployment complete!"
echo ""
echo "Service information:"
kubectl get svc -n trakkt

echo ""
echo "Pod status:"
kubectl get pods -n trakkt

echo ""
echo "To access trakkt, run:"
echo "  kubectl port-forward -n trakkt svc/trakkt 8003:80"
echo ""
echo "Then visit: http://localhost:8003"
echo ""
echo "To view logs:"
echo "  kubectl logs -n trakkt deployment/trakkt -f"
