#!/usr/bin/env bash
# =============================================================================
# PodMesh Backup Server (PMBS) v2.3.0 — Debian 13 (Trixie) Package Builder
# Architected under Claude Fable 5.1 & GPT-6 Astra Counter-Review Invariants:
#  - Debian 13 Trixie usrmerge compliance (/usr/lib/systemd/system/pmbs.service)
#  - Zero-Touch CSPRNG 256-bit PMBS_API_TOKEN bootstrap in postinst (idempotent)
#  - Strict 0700/0600 POSIX permissions, UMask=0077, and 127.0.0.1 loopback bind
#  - DEBIAN/conffiles protection for /etc/pmbs/config.json
#  - Reproducible build via SOURCE_DATE_EPOCH
# =============================================================================
set -euo pipefail

PKG_NAME="podmesh-backup-server"
PKG_VERSION="2.3.0"
PKG_ARCH="amd64"
ROOT_DIR="$(cd "$(dirname "$0")/../.." ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)" pwd)"
BUILD_DIR="$(mktemp -d /tmp/build-${PKG_NAME}.XXXXXX)"
DIST_DIR="${ROOT_DIR}/dist"
RUST_BIN="${ROOT_DIR}/target/release/pmbs"

echo "=== [PMBS] Packaging Debian 13 (Trixie): ${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb ==="

if [ ! -f "${RUST_BIN}" ]; then
    echo "Compiling release binary for PMBS v${PKG_VERSION}..."
    cargo build --release --locked -p pmbs --manifest-path "${ROOT_DIR}/Cargo.toml"
fi

trap 'rm -rf "${BUILD_DIR}"' EXIT

# Create Debian 13 (usrmerge) directory hierarchy with strict permissions
chmod 0755 "${BUILD_DIR}"
mkdir -p -m 0755 "${BUILD_DIR}/DEBIAN"
mkdir -p -m 0755 "${BUILD_DIR}/usr" "${BUILD_DIR}/usr/bin" "${BUILD_DIR}/usr/lib" "${BUILD_DIR}/usr/lib/systemd" "${BUILD_DIR}/usr/lib/systemd/system"
mkdir -p -m 0755 "${BUILD_DIR}/etc" "${BUILD_DIR}/var" "${BUILD_DIR}/var/lib"
mkdir -p -m 0700 "${BUILD_DIR}/etc/pmbs"
mkdir -p -m 0700 "${BUILD_DIR}/var/lib/pmbs"
mkdir -p -m 0700 "${BUILD_DIR}/var/lib/pmbs/datastores"
mkdir -p -m 0700 "${BUILD_DIR}/var/lib/pmbs/datastores/datastore-default"
mkdir -p -m 0700 "${BUILD_DIR}/var/lib/pmbs/datastores/datastore-default/.chunks"
mkdir -p -m 0700 "${BUILD_DIR}/var/lib/pmbs/datastores/datastore-default/snapshots"
mkdir -p "${DIST_DIR}"

# 1. Copy compiled Pure Rust binary
install -m 0755 "${RUST_BIN}" "${BUILD_DIR}/usr/bin/pmbs"

# 2. Default static configuration file (protected via DEBIAN/conffiles)
cat << 'EOF' > "${BUILD_DIR}/etc/pmbs/config.json"
{
  "version": "2.3.0",
  "bind": "127.0.0.1",
  "port": 8007,
  "dataDir": "/var/lib/pmbs",
  "socket": "/var/lib/pmbs/pmbs.sock",
  "defaultDatastore": "datastore-default",
  "dedupEngine": "SHA-256 CAS 4MB (Pure Rust)",
  "op10VerifierReady": true,
  "remoteSyncEnabled": true
}
EOF
chmod 0600 "${BUILD_DIR}/etc/pmbs/config.json"

# 3. DEBIAN/conffiles (never include /etc/pmbs/pmbs.env here so upgrades preserve dynamic token)
cat << 'EOF' > "${BUILD_DIR}/DEBIAN/conffiles"
/etc/pmbs/config.json
EOF
chmod 0644 "${BUILD_DIR}/DEBIAN/conffiles"

# 4. Debian 13 usrmerge systemd service (/usr/lib/systemd/system/pmbs.service)
cat << 'EOF' > "${BUILD_DIR}/usr/lib/systemd/system/pmbs.service"
[Unit]
Description=PodMesh Backup Server Daemon (Pure Rust 4MB CAS — AI-First v2.3.0)
After=network-online.target local-fs.target
Wants=network-online.target

[Service]
Type=simple
User=root
Group=root
UMask=0077
StateDirectory=pmbs
StateDirectoryMode=0700
ConfigurationDirectory=pmbs
ConfigurationDirectoryMode=0700
EnvironmentFile=/etc/pmbs/pmbs.env
ExecStart=/usr/bin/pmbs daemon --bind ${PMBS_BIND_ADDR} --port ${PMBS_PORT} --data-dir ${PMBS_DATA_DIR} --socket ${PMBS_SOCKET}
Restart=on-failure
RestartSec=2
LimitNOFILE=65536
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=full

[Install]
WantedBy=multi-user.target
EOF
chmod 0644 "${BUILD_DIR}/usr/lib/systemd/system/pmbs.service"

# 5. Compute Installed-Size in KiB
INSTALLED_SIZE="$(du -sk "${BUILD_DIR}" | awk '{print $1}')"

# 6. DEBIAN/control
cat << EOF > "${BUILD_DIR}/DEBIAN/control"
Package: ${PKG_NAME}
Version: ${PKG_VERSION}
Section: admin
Priority: optional
Architecture: ${PKG_ARCH}
Installed-Size: ${INSTALLED_SIZE}
Depends: libc6 (>= 2.36), systemd
Maintainer: Xavier DP <xavier@xavdp.pro>
Homepage: https://pmbs.xavdp.pro
Description: PodMesh Backup Server (PMBS) — Pure Rust 4MB CAS Deduplicated Backup Server
 High-performance, ultra-frugal (< 15 MB RAM) deduplicated backup server for
 Debian 13 (Trixie), Proxmox VE 9.x, and PodMesh clusters.
 Features 4 MiB SHA-256 Content-Addressable Storage (CAS) shared with MeshFS,
 atomic Merkle-sealed snapshot manifests, Op 10 referenced-chunk audits,
 staging-verified restoration, and 100% AI-First Unix Socket + REST control.
EOF
chmod 0644 "${BUILD_DIR}/DEBIAN/control"

# 7. DEBIAN/postinst (Zero-Touch CSPRNG 256-bit Bootstrap + Idempotent Permissions)
cat << 'EOF' > "${BUILD_DIR}/DEBIAN/postinst"
#!/usr/bin/env bash
set -euo pipefail

ENV_DIR="/etc/pmbs"
ENV_FILE="${ENV_DIR}/pmbs.env"
DATA_DIR="/var/lib/pmbs"

if [ "${1:-}" = "configure" ]; then
    install -d -m 0700 -o root -g root "${ENV_DIR}"
    install -d -m 0700 -o root -g root "${DATA_DIR}"
    install -d -m 0700 -o root -g root "${DATA_DIR}/datastores"
    install -d -m 0700 -o root -g root "${DATA_DIR}/datastores/datastore-default"
    install -d -m 0700 -o root -g root "${DATA_DIR}/datastores/datastore-default/.chunks"
    install -d -m 0700 -o root -g root "${DATA_DIR}/datastores/datastore-default/snapshots"

    # Idempotent CSPRNG 256-bit token bootstrap: generate only if missing or invalid
    if [ ! -f "${ENV_FILE}" ] || ! grep -Eq '^PMBS_API_TOKEN=[!-~]{32,256}$' "${ENV_FILE}" 2>/dev/null; then
        (
            umask 0077
            TOKEN="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
            TMP_ENV="$(mktemp "${ENV_DIR}/.pmbs.env.XXXXXX")"
            chmod 0600 "${TMP_ENV}"
            chown root:root "${TMP_ENV}"
            cat <<EOT > "${TMP_ENV}"
# Generated automatically by podmesh-backup-server postinst (Zero-Touch AI-First Bootstrap)
PMBS_API_TOKEN=${TOKEN}
PMBS_BIND_ADDR=127.0.0.1
PMBS_PORT=8007
PMBS_DATA_DIR=/var/lib/pmbs
PMBS_SOCKET=/var/lib/pmbs/pmbs.sock
RUST_LOG=info
EOT
            mv -f "${TMP_ENV}" "${ENV_FILE}"
        )
        echo "[PMBS] Generated new 256-bit CSPRNG PMBS_API_TOKEN in ${ENV_FILE} (0600)."
    else
        chmod 0600 "${ENV_FILE}"
        chown root:root "${ENV_FILE}"
        # Ensure new v2.3.0 env keys exist without clobbering PMBS_API_TOKEN
        grep -q '^PMBS_BIND_ADDR=' "${ENV_FILE}" || echo "PMBS_BIND_ADDR=127.0.0.1" >> "${ENV_FILE}"
        grep -q '^PMBS_PORT=' "${ENV_FILE}" || echo "PMBS_PORT=8007" >> "${ENV_FILE}"
        grep -q '^PMBS_DATA_DIR=' "${ENV_FILE}" || echo "PMBS_DATA_DIR=/var/lib/pmbs" >> "${ENV_FILE}"
        grep -q '^PMBS_SOCKET=' "${ENV_FILE}" || echo "PMBS_SOCKET=/var/lib/pmbs/pmbs.sock" >> "${ENV_FILE}"
    fi

    if [ -f "${ENV_DIR}/config.json" ]; then
        chmod 0600 "${ENV_DIR}/config.json"
        chown root:root "${ENV_DIR}/config.json"
    fi

    # Clean up any legacy non-usrmerge unit override if present
    if [ -f /etc/systemd/system/pmbs.service ] && [ ! -L /etc/systemd/system/pmbs.service ]; then
        rm -f /etc/systemd/system/pmbs.service
    fi

    if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
        systemctl daemon-reload || true
        systemctl enable pmbs.service || true
        systemctl restart pmbs.service || true
        echo "[PMBS] Service pmbs.service (v2.3.0) enabled and started on 127.0.0.1:8007 + /var/lib/pmbs/pmbs.sock."
    fi
fi
exit 0
EOF
chmod 0755 "${BUILD_DIR}/DEBIAN/postinst"

# 8. DEBIAN/prerm
cat << 'EOF' > "${BUILD_DIR}/DEBIAN/prerm"
#!/usr/bin/env bash
set -e
if [ "${1:-}" = "remove" ]; then
    if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
        systemctl stop pmbs.service 2>/dev/null || true
        systemctl disable pmbs.service 2>/dev/null || true
    fi
    rm -f /var/lib/pmbs/pmbs.sock 2>/dev/null || true
fi
exit 0
EOF
chmod 0755 "${BUILD_DIR}/DEBIAN/prerm"

# 9. DEBIAN/postrm (Storage Safety First: never destroy /var/lib/pmbs datastores on remove/purge)
cat << 'EOF' > "${BUILD_DIR}/DEBIAN/postrm"
#!/usr/bin/env bash
set -e
if [ "${1:-}" = "remove" ] || [ "${1:-}" = "purge" ]; then
    if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
        systemctl daemon-reload 2>/dev/null || true
    fi
fi
if [ "${1:-}" = "purge" ]; then
    rm -f /etc/pmbs/pmbs.env /etc/pmbs/config.json 2>/dev/null || true
fi
exit 0
EOF
chmod 0755 "${BUILD_DIR}/DEBIAN/postrm"

# 10. Normalize timestamps for reproducible SHA-256 build
if git -C "${ROOT_DIR}" rev-parse HEAD >/dev/null 2>&1; then
    export SOURCE_DATE_EPOCH="$(git -C "${ROOT_DIR}" log -1 --format=%ct)"
else
    export SOURCE_DATE_EPOCH="1793100000"
fi
find "${BUILD_DIR}" -exec touch -h -d "@${SOURCE_DATE_EPOCH}" {} +

# 11. Build .deb package
DEB_OUT="${DIST_DIR}/${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb"
dpkg-deb --root-owner-group -Zxz --build "${BUILD_DIR}" "${DEB_OUT}"
sha256sum "${DEB_OUT}" > "${DIST_DIR}/SHA256SUMS"

echo "[PMBS] Debian 13 package built successfully: ${DEB_OUT}"
cat "${DIST_DIR}/SHA256SUMS"
