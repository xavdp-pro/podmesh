#!/usr/bin/env bash
set -euo pipefail

PKG_NAME="podmesh-cli"
PKG_VERSION="2.1.0"
PKG_ARCH="amd64"
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BUILD_DIR="$(mktemp -d /tmp/build-${PKG_NAME}.XXXXXX)"
DIST_DIR="${ROOT_DIR}/dist"
RUST_BIN="${ROOT_DIR}/target/release/podmesh-ctl"

echo "=== [PodMesh-CLI] Packaging Debian: ${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb (Pure Rust Edition) ==="

if [ ! -f "${RUST_BIN}" ]; then
    echo "Compilation release de podmesh-ctl..."
    cargo build --release -p podmesh-cli --manifest-path "${ROOT_DIR}/Cargo.toml"
fi

trap 'rm -rf "${BUILD_DIR}"' EXIT

mkdir -p "${BUILD_DIR}/DEBIAN"
mkdir -p "${BUILD_DIR}/usr/bin"
mkdir -p "${BUILD_DIR}/etc/podmesh"
mkdir -p "${DIST_DIR}"

# 1. Copie du binaire podmesh-ctl
cp "${RUST_BIN}" "${BUILD_DIR}/usr/bin/podmesh-ctl"
chmod 755 "${BUILD_DIR}/usr/bin/podmesh-ctl"

# 2. DEBIAN/control
cat << EOF > "${BUILD_DIR}/DEBIAN/control"
Package: ${PKG_NAME}
Version: ${PKG_VERSION}
Section: admin
Priority: optional
Architecture: ${PKG_ARCH}
Depends: libc6 (>= 2.31)
Maintainer: Xavier <xavier@xavdp.pro>
Description: PodMesh Unified CLI (podmesh-ctl) — Orchestration, HA Failover & Replication
 Native command line control tool for PodMesh nodes, warm standby failover,
 Ed25519 cryptographic takeover proof generation, universe replication,
 and cross-host disaster recovery.
 100% Native Pure Rust — Zero external runtime, ultra-fast execution.
EOF

# 3. Construction du paquet .deb
dpkg-deb --root-owner-group --build "${BUILD_DIR}" "${DIST_DIR}/${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb"
echo "[PodMesh-CLI] Paquet généré avec succès dans : ${DIST_DIR}/${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb"
