#!/usr/bin/env bash
# =============================================================================
# PODMESH v2.3.0 — Dual-Channel Debian 13 (Trixie) APT Repository Generator
# Hardened under Claude Fable 5.1 & GPT-6 Astra Counter-Reviews:
#  - Version unified to 2.3.0
#  - Collects all 6 fleet .deb packages (amd64 + all)
#  - Deterministic gzip -9nc (no embedded timestamp)
#  - Emits MD5Sum, SHA1, SHA256, and SHA512 (required by Debian 13 apt / sqv)
# =============================================================================
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="${ROOT_DIR}/dist"
APT_ROOT="${DIST_DIR}/apt-repos"
PKG_VERSION="2.3.0"

TARGET_CHANNEL="${1:-all}" # "dev", "prod", or "all"

echo "=================================================================="
echo "  PODMESH v2.3.0 — Debian 13 (Trixie) Dual-Channel APT Generator"
echo "  DEV Channel  : trixie-dev"
echo "  PROD Channel : trixie-stable"
echo "=================================================================="

build_channel() {
    local CHANNEL="$1"
    local SUITE="$2"
    local LABEL="$3"
    local CHANNEL_DIR="${APT_ROOT}/${CHANNEL}"
    local POOL_DIR="${CHANNEL_DIR}/pool/main"
    local BIN_DIR="${CHANNEL_DIR}/dists/${SUITE}/main/binary-amd64"
    local ALL_DIR="${CHANNEL_DIR}/dists/${SUITE}/main/binary-all"

    echo ""
    echo "--> [Channel ${CHANNEL^^}] Preparing APT repository (${SUITE})..."
    rm -rf "${CHANNEL_DIR}"
    mkdir -p "${POOL_DIR}" "${BIN_DIR}" "${ALL_DIR}"

    if [ ! -f "${DIST_DIR}/podmesh_${PKG_VERSION}_amd64.deb" ]; then
        bash "${ROOT_DIR}/packaging/build-all-debs.sh"
    fi

    for pattern in \
        "podmesh_${PKG_VERSION}_amd64.deb" \
        "podmesh-cli_${PKG_VERSION}_amd64.deb" \
        "podmesh-meshfs_${PKG_VERSION}_amd64.deb" \
        "podmesh-backup-server_${PKG_VERSION}_amd64.deb" \
        "podmesh-pve-plugin_${PKG_VERSION}_all.deb" \
        "podmesh-installer_${PKG_VERSION}_all.deb"; do
        if [ -f "${DIST_DIR}/${pattern}" ]; then
            cp -p "${DIST_DIR}/${pattern}" "${POOL_DIR}/"
        fi
    done

    echo "    Indexing Debian packages into Packages & Packages.gz..."
    (
        cd "${CHANNEL_DIR}"
        dpkg-scanpackages --multiversion pool/main /dev/null > "dists/${SUITE}/main/binary-amd64/Packages"
        cp -p "dists/${SUITE}/main/binary-amd64/Packages" "dists/${SUITE}/main/binary-all/Packages"
    )

    gzip -9nc "${BIN_DIR}/Packages" > "${BIN_DIR}/Packages.gz"
    gzip -9nc "${ALL_DIR}/Packages" > "${ALL_DIR}/Packages.gz"

    local RELEASE_FILE="${CHANNEL_DIR}/dists/${SUITE}/Release"
    cat << EOF > "${RELEASE_FILE}"
Origin: PodMesh
Label: ${LABEL}
Suite: ${SUITE}
Codename: ${SUITE}
Version: ${PKG_VERSION}
Architectures: amd64 all
Components: main
Description: Official PodMesh v${PKG_VERSION} Debian 13 (Trixie) Repository — Channel ${CHANNEL^^} (AI-First Pure Rust Infrastructure)
Date: $(LC_ALL=C date -Ru)
Valid-Until: $(LC_ALL=C date -Ru -d "+1 year")
EOF

    append_checksums() {
        local HASH_NAME="$1"
        local HASH_CMD="$2"

        echo "${HASH_NAME}:" >> "${RELEASE_FILE}"
        for rel_path in \
            "main/binary-amd64/Packages" \
            "main/binary-amd64/Packages.gz" \
            "main/binary-all/Packages" \
            "main/binary-all/Packages.gz"; do
            local file_path="${CHANNEL_DIR}/dists/${SUITE}/${rel_path}"
            if [ -f "${file_path}" ]; then
                local hash_val size
                hash_val=$("${HASH_CMD}" "${file_path}" | cut -d' ' -f1)
                size=$(stat -c%s "${file_path}")
                printf " %s %s %s\n" "${hash_val}" "${size}" "${rel_path}" >> "${RELEASE_FILE}"
            fi
        done
    }

    append_checksums "MD5Sum" "md5sum"
    append_checksums "SHA1" "sha1sum"
    append_checksums "SHA256" "sha256sum"
    append_checksums "SHA512" "sha512sum"

    if command -v gpg >/dev/null 2>&1; then
        local GPG_KEY=${GPG_SIGN_KEY:-}
        if [ -n "${GPG_KEY}" ]; then
            echo "    Signing Release with GPG key ${GPG_KEY} (SHA512)..."
            gpg --batch --yes --armor --digest-algo SHA512 --default-key "${GPG_KEY}" --detach-sign --output "${RELEASE_FILE}.gpg" "${RELEASE_FILE}"
            gpg --batch --yes --clearsign --digest-algo SHA512 --default-key "${GPG_KEY}" --output "${CHANNEL_DIR}/dists/${SUITE}/InRelease" "${RELEASE_FILE}"
        fi
    fi

    echo "    ✓ Channel ${CHANNEL^^} ready: ${CHANNEL_DIR}"
}

case "${TARGET_CHANNEL}" in
    dev) build_channel "dev" "trixie-dev" "PodMesh Development" ;;
    prod) build_channel "prod" "trixie-stable" "PodMesh Production" ;;
    all)
        build_channel "dev" "trixie-dev" "PodMesh Development"
        build_channel "prod" "trixie-stable" "PodMesh Production"
        ;;
    *)
        echo "Unknown channel: ${TARGET_CHANNEL}. Use 'dev', 'prod', or 'all'."
        exit 1
        ;;
esac
