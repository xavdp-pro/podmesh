#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
echo "=========================================================="
echo "  PODMESH SUITE — Production Debian Packages Builder"
echo "  100% Pure Rust — Zero external runtime, Sovereign Footprint"
echo "=========================================================="

mkdir -p "${ROOT_DIR}/dist"

# 1. Build release binaries across the whole cargo workspace first
echo "--> [1/4] Compiling full Rust workspace in release mode..."
cargo build --release --workspace --manifest-path "${ROOT_DIR}/Cargo.toml"

# 2. Package core podmesh
echo "--> [2/4] Packaging Core PodMesh & Daemon..."
bash "${ROOT_DIR}/packaging/build-deb.sh"

# 3. Package MeshFS Pure Rust
echo "--> [3/4] Packaging MeshFS & Proxmox VE Plugin..."
bash "${ROOT_DIR}/packaging/meshfs/build-deb-meshfs.sh"

# 4. Package PMBS Pure Rust
echo "--> [4/4] Packaging PodMesh Backup Server (PMBS)..."
bash "${ROOT_DIR}/packaging/pmbs/build-deb-pmbs.sh"

# 5. Package Standalone CLI
echo "--> [Extra] Packaging Standalone podmesh-ctl..."
bash "${ROOT_DIR}/packaging/podmesh-cli/build-deb-cli.sh"

echo ""
echo "=========================================================="
echo "  BUILD SUMMARY — All .deb packages generated in dist/"
echo "=========================================================="
ls -lh "${ROOT_DIR}/dist"/*.deb
sha256sum "${ROOT_DIR}/dist"/*.deb > "${ROOT_DIR}/dist/SHA256SUMS"
cat "${ROOT_DIR}/dist/SHA256SUMS"
echo "=========================================================="
