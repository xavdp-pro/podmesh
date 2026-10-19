#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DEV_DATA="${ROOT_DIR}/data/dev"

MESHFS_PORT=${MESHFS_PORT:-7443}
PMBS_PORT=${PMBS_PORT:-8007}
VITE_PORT=${VITE_PORT:-5173}

echo "=== PodMesh Suite — Health Check Audit ==="

# 1. MeshFS
echo -n "[MeshFS HTTP :${MESHFS_PORT}] "
if curl -s "http://127.0.0.1:${MESHFS_PORT}/api/v1/meshfs/status" | grep -q "status"; then
    echo "OK (Active)"
else
    echo "FAILED"
fi

echo -n "[MeshFS Unix Socket] "
if [ -S "${DEV_DATA}/meshfs/meshfs.sock" ] || [ -S "/var/lib/meshfs/meshfs.sock" ]; then
    echo "OK (Socket exists)"
else
    echo "NOT FOUND"
fi

# 2. PMBS
echo -n "[PMBS HTTP :${PMBS_PORT}] "
if curl -s "http://127.0.0.1:${PMBS_PORT}/api/v1/pmbs/status" | grep -q "status"; then
    echo "OK (Active)"
else
    echo "FAILED"
fi

echo -n "[PMBS Unix Socket] "
if [ -S "${DEV_DATA}/pmbs/pmbs.sock" ] || [ -S "/var/lib/pmbs/pmbs.sock" ]; then
    echo "OK (Socket exists)"
else
    echo "NOT FOUND"
fi

# 3. Showcase UI
echo -n "[Showcase UI :${VITE_PORT}] "
if curl -s "http://127.0.0.1:${VITE_PORT}" | grep -q "html"; then
    echo "OK (Serving HTML)"
else
    echo "OFFLINE"
fi
