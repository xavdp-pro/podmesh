#!/usr/bin/env bash
# =============================================================================
# PODMESH v2.4.1 — Unified Debian 13 (Trixie) Fleet Package Builder & Gate
# =============================================================================
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="${ROOT_DIR}/dist"
mkdir -p "${DIST_DIR}"

echo "=================================================================="
echo "  PODMESH v2.4.1 — Debian 13 (Trixie) Fleet Builder & Audit Gate"
echo "=================================================================="

# 1. Compile Rust workspace
echo "--> [1/7] Ensuring full Rust workspace release build..."
PODMESH_PACKAGE_VERSION="2.4.1" cargo build --release --workspace --manifest-path "${ROOT_DIR}/Cargo.toml"

# 2. Build podmesh_2.4.1_amd64.deb
echo "--> [2/7] Building podmesh_2.4.1_amd64.deb..."
bash "${ROOT_DIR}/packaging/build-deb.sh"

# 3. Build podmesh-cli_2.4.1_amd64.deb
echo "--> [3/7] Building podmesh-cli_2.4.1_amd64.deb..."
bash "${ROOT_DIR}/packaging/podmesh-cli/build-deb-cli.sh"

# 4. Build podmesh-meshfs_2.4.1_amd64.deb
echo "--> [4/7] Building podmesh-meshfs_2.4.1_amd64.deb..."
bash "${ROOT_DIR}/packaging/meshfs/build-deb-meshfs.sh"

# 5. Build podmesh-backup-server_2.4.1_amd64.deb
echo "--> [5/7] Building podmesh-backup-server_2.4.1_amd64.deb..."
bash "${ROOT_DIR}/packaging/pmbs/build-deb-pmbs.sh"

# 6. Build satellite packages (podmesh-pve-plugin & podmesh-installer) if present alongside
PVE_REPO="${ROOT_DIR}/../podmesh-pve-plugin"
if [ -x "${PVE_REPO}/packaging/build-deb-pve-plugin.sh" ]; then
    echo "--> [6/7] Building podmesh-pve-plugin_2.4.1_all.deb..."
    bash "${PVE_REPO}/packaging/build-deb-pve-plugin.sh"
    cp -p "${PVE_REPO}/dist/podmesh-pve-plugin_2.4.1_all.deb" "${DIST_DIR}/"
fi

INST_REPO="${ROOT_DIR}/../podmesh-installer"
if [ -x "${INST_REPO}/packaging/build-deb-installer.sh" ]; then
    echo "--> [7/7] Building podmesh-installer_2.4.1_all.deb..."
    bash "${INST_REPO}/packaging/build-deb-installer.sh"
    cp -p "${INST_REPO}/dist/podmesh-installer_2.4.1_all.deb" "${DIST_DIR}/"
fi

echo ""
echo "=================================================================="
echo "  CLAUDE FABLE 5.1 & GPT-6 ASTRA FLEET INVARIANCE GATE (v2.4.1)"
echo "=================================================================="
for deb in "${DIST_DIR}"/*_2.4.1_*.deb; do
    ROOT_MODE="$(dpkg-deb -c "${deb}" | awk '$6 == "./" {print $1}')"
    [ "${ROOT_MODE}" = "drwxr-xr-x" ] || { echo "FAIL root mode on ${deb}: ${ROOT_MODE}"; exit 1; }

    if dpkg-deb -c "${deb}" | awk '{print $6}' | grep -Eq '^\./(lib|sbin)/'; then
        echo "FAIL legacy non-usrmerge path in ${deb}"; exit 1
    fi

    VER="$(dpkg-deb -f "${deb}" Version)"
    [ "${VER}" = "2.4.1" ] || { echo "FAIL version on ${deb}: ${VER}"; exit 1; }
    echo "  [PASS] $(basename "${deb}") — Version=${VER}, Root=${ROOT_MODE}, usrmerge=OK"
done

sha256sum "${DIST_DIR}"/*_2.4.1_*.deb > "${DIST_DIR}/SHA256SUMS"
sha512sum "${DIST_DIR}"/*_2.4.1_*.deb > "${DIST_DIR}/SHA512SUMS"
echo ""
cat "${DIST_DIR}/SHA256SUMS"
echo "=================================================================="
