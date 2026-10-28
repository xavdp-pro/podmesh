#!/usr/bin/env bash
# =============================================================================
# PodMesh Unified CLI (podmesh-ctl) v2.3.0 — Debian 13 (Trixie) Package Builder
# Hardened under Claude Fable 5.1 & GPT-6 Astra Counter-Reviews:
#  - Enforces 0755 on ./ and OS directories
#  - Replaces: podmesh (<< 2.3.0) to prevent /usr/bin/podmesh-ctl collision
#  - Reproducible SHA-256 build via SOURCE_DATE_EPOCH
# =============================================================================
set -euo pipefail
umask 0022

PKG_NAME="podmesh-cli"
PKG_VERSION="2.3.0"
PKG_ARCH="amd64"
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BUILD_DIR="$(mktemp -d /tmp/build-${PKG_NAME}.XXXXXX)"
DIST_DIR="${ROOT_DIR}/dist"
RUST_BIN="${ROOT_DIR}/target/release/podmesh-ctl"

echo "=== [PodMesh-CLI] Packaging Debian 13 (Trixie): ${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb ==="

if [ ! -f "${RUST_BIN}" ]; then
    echo "Compiling release binary for podmesh-ctl v${PKG_VERSION}..."
    cargo build --release -p podmesh-cli --manifest-path "${ROOT_DIR}/Cargo.toml"
fi

trap 'rm -rf "${BUILD_DIR}"' EXIT

chmod 0755 "${BUILD_DIR}"
mkdir -p -m 0755 "${BUILD_DIR}/DEBIAN"
mkdir -p -m 0755 "${BUILD_DIR}/usr" "${BUILD_DIR}/usr/bin"
mkdir -p "${DIST_DIR}"

install -m 0755 "${RUST_BIN}" "${BUILD_DIR}/usr/bin/podmesh-ctl"

INSTALLED_SIZE="$(du -sk "${BUILD_DIR}" | awk '{print $1}')"

cat << EOF > "${BUILD_DIR}/DEBIAN/control"
Package: ${PKG_NAME}
Version: ${PKG_VERSION}
Section: admin
Priority: optional
Architecture: ${PKG_ARCH}
Installed-Size: ${INSTALLED_SIZE}
Depends: libc6 (>= 2.36)
Recommends: podmesh (>= 2.3.0) | podmesh-meshfs (>= 2.3.0)
Replaces: podmesh (<< 2.3.0)
Breaks: podmesh (<< 2.3.0)
Maintainer: Xavier DP <xavier@xavdp.pro>
Homepage: https://podmesh.xavdp.pro
Description: PodMesh Unified CLI (podmesh-ctl) — AI-First Orchestration, HA & Replication (v2.3.0)
 Native command line control tool for PodMesh nodes, warm standby failover,
 Ed25519 cryptographic takeover proof generation, universe replication,
 and cross-host disaster recovery. Outputs deterministic JSON for AI agents.
EOF
chmod 0644 "${BUILD_DIR}/DEBIAN/control"

find "${BUILD_DIR}" -type d -exec chmod 0755 {} +

if git -C "${ROOT_DIR}" rev-parse HEAD >/dev/null 2>&1; then
    export SOURCE_DATE_EPOCH="$(git -C "${ROOT_DIR}" log -1 --format=%ct)"
else
    export SOURCE_DATE_EPOCH="1793100000"
fi
find "${BUILD_DIR}" -exec touch -h -d "@${SOURCE_DATE_EPOCH}" {} +

DEB_OUT="${DIST_DIR}/${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb"
dpkg-deb --root-owner-group -Zxz --build "${BUILD_DIR}" "${DEB_OUT}"

ROOT_PERM="$(dpkg-deb -c "${DEB_OUT}" | awk '$6 == "./" {print $1}')"
if [ "${ROOT_PERM}" != "drwxr-xr-x" ]; then
    echo "FATAL [Invariant Breach]: ${DEB_OUT} root dir is ${ROOT_PERM}" >&2
    exit 1
fi

echo "[PodMesh-CLI] Debian 13 package built & verified: ${DEB_OUT}"
