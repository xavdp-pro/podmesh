#!/usr/bin/env bash
# =============================================================================
# PodMesh MeshFS v2.3.0 — Debian 13 (Trixie) Package Builder
# Hardened under Claude Fable 5.1 & GPT-6 Astra Counter-Reviews:
#  - Enforces 0755 on ./ and OS directories; 0700 on /etc/meshfs & /var/lib/meshfs
#  - Debian 13 usrmerge compliance (/usr/lib/systemd/system/meshfs.service)
#  - Ships both /usr/bin/meshfs (Pure Rust) and /usr/bin/meshfs-p2p
#  - Protects /etc/meshfs/config.json in DEBIAN/conffiles
#  - Clean FUSE unmount in prerm and reproducible build via SOURCE_DATE_EPOCH
# =============================================================================
set -euo pipefail
umask 0022

PKG_NAME="podmesh-meshfs"
PKG_VERSION="2.3.0"
PKG_ARCH="amd64"
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BUILD_DIR="$(mktemp -d /tmp/build-${PKG_NAME}.XXXXXX)"
DIST_DIR="${ROOT_DIR}/dist"
RUST_BIN="${ROOT_DIR}/target/release/meshfs"

echo "=== [MeshFS] Packaging Debian 13 (Trixie): ${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb ==="

if true; then
    echo "Compiling release binary for MeshFS v${PKG_VERSION}..."
    cargo build --release -p meshfs --manifest-path "${ROOT_DIR}/Cargo.toml"
fi

trap 'rm -rf "${BUILD_DIR}"' EXIT

chmod 0755 "${BUILD_DIR}"
mkdir -p -m 0755 "${BUILD_DIR}/DEBIAN"
mkdir -p -m 0755 "${BUILD_DIR}/usr" "${BUILD_DIR}/usr/bin" "${BUILD_DIR}/usr/lib" \
  "${BUILD_DIR}/usr/lib/systemd" "${BUILD_DIR}/usr/lib/systemd/system"
mkdir -p -m 0755 "${BUILD_DIR}/etc" "${BUILD_DIR}/var" "${BUILD_DIR}/var/lib"
mkdir -p -m 0700 "${BUILD_DIR}/etc/meshfs"
mkdir -p -m 0700 "${BUILD_DIR}/var/lib/meshfs"
mkdir -p -m 0700 "${BUILD_DIR}/var/lib/meshfs/chunks"
mkdir -p -m 0700 "${BUILD_DIR}/var/lib/meshfs/meta"
mkdir -p -m 0700 "${BUILD_DIR}/var/lib/meshfs/volumes"
mkdir -p "${DIST_DIR}"

# 1. Install binaries (/usr/bin/meshfs + /usr/bin/meshfs-p2p)
install -m 0755 "${RUST_BIN}" "${BUILD_DIR}/usr/bin/meshfs"
if [ -f "${ROOT_DIR}/bin/meshfs-p2p" ]; then
    install -m 0755 "${ROOT_DIR}/bin/meshfs-p2p" "${BUILD_DIR}/usr/bin/meshfs-p2p"
fi

# 2. Default configuration (/etc/meshfs/config.json)
cat << 'EOF' > "${BUILD_DIR}/etc/meshfs/config.json"
{
  "version": "2.3.0",
  "clusterName": "meshfs-cluster",
  "port": 7443,
  "dataDir": "/var/lib/meshfs",
  "chunkSizeMb": 4,
  "socket": "/var/lib/meshfs/meshfs.sock",
  "standalone": true,
  "requiresPodmesh": false
}
EOF
chmod 0600 "${BUILD_DIR}/etc/meshfs/config.json"

# 3. DEBIAN/conffiles
cat << 'EOF' > "${BUILD_DIR}/DEBIAN/conffiles"
/etc/meshfs/config.json
EOF
chmod 0644 "${BUILD_DIR}/DEBIAN/conffiles"

# 4. Debian 13 usrmerge systemd unit (/usr/lib/systemd/system/meshfs.service)
cat << 'EOF' > "${BUILD_DIR}/usr/lib/systemd/system/meshfs.service"
[Unit]
Description=PodMesh MeshFS Pure Rust 4MB CAS Distributed Filesystem Daemon (v2.3.0)
After=network-online.target local-fs.target
Wants=network-online.target

[Service]
Type=simple
User=root
Group=root
UMask=0077
StateDirectory=meshfs
StateDirectoryMode=0700
ConfigurationDirectory=meshfs
ConfigurationDirectoryMode=0700
ExecStart=/usr/bin/meshfs daemon --port 7443 --data-dir /var/lib/meshfs --socket /var/lib/meshfs/meshfs.sock
Restart=on-failure
RestartSec=2
LimitNOFILE=65536
NoNewPrivileges=yes
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
EOF
chmod 0644 "${BUILD_DIR}/usr/lib/systemd/system/meshfs.service"

INSTALLED_SIZE="$(du -sk "${BUILD_DIR}" | awk '{print $1}')"

# 5. DEBIAN/control
cat << EOF > "${BUILD_DIR}/DEBIAN/control"
Package: ${PKG_NAME}
Version: ${PKG_VERSION}
Section: admin
Priority: optional
Architecture: ${PKG_ARCH}
Installed-Size: ${INSTALLED_SIZE}
Depends: libc6 (>= 2.36), systemd, fuse3
Recommends: podmesh-pve-plugin (>= 2.3.0), python3
Maintainer: Xavier DP <xavier@xavdp.pro>
Homepage: https://meshfs.xavdp.pro
Description: PodMesh MeshFS — Pure Rust 4MB CAS Distributed Filesystem (v2.3.0)
 Ultra-lightweight distributed and peer-to-peer filesystem operating seamlessly
 from 1 standalone node up to N-node clusters. Features 4 MiB SHA-256 Content
 Addressable Storage (CAS), Merkle Copy-on-Write snapshots, Unix Domain Socket
 IPC, FUSE POSIX mounting, and Zero-Transit synergy with PodMesh Backup Server.
EOF
chmod 0644 "${BUILD_DIR}/DEBIAN/control"

# 6. DEBIAN/postinst
cat << 'EOF' > "${BUILD_DIR}/DEBIAN/postinst"
#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "configure" ]; then
    install -d -m 0700 -o root -g root /etc/meshfs /var/lib/meshfs /var/lib/meshfs/chunks /var/lib/meshfs/meta /var/lib/meshfs/volumes
    if [ -f /etc/meshfs/config.json ]; then
        chmod 0600 /etc/meshfs/config.json
        chown root:root /etc/meshfs/config.json
    fi
    if [ -f /etc/systemd/system/meshfs.service ] && [ ! -L /etc/systemd/system/meshfs.service ]; then
        rm -f /etc/systemd/system/meshfs.service
    fi
    if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
        systemctl daemon-reload || true
        systemctl enable meshfs.service || true
        systemctl restart meshfs.service || true
        echo "[MeshFS] Service meshfs.service (v2.3.0) enabled and started on :7443 + /var/lib/meshfs/meshfs.sock."
    fi
fi
exit 0
EOF
chmod 0755 "${BUILD_DIR}/DEBIAN/postinst"

# 7. DEBIAN/prerm
cat << 'EOF' > "${BUILD_DIR}/DEBIAN/prerm"
#!/usr/bin/env bash
set -e
if [ "${1:-}" = "remove" ]; then
    if command -v fusermount3 >/dev/null 2>&1; then
        fusermount3 -uz /mnt/meshfs 2>/dev/null || true
    fi
    if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
        systemctl stop meshfs.service 2>/dev/null || true
        systemctl disable meshfs.service 2>/dev/null || true
    fi
    rm -f /var/lib/meshfs/meshfs.sock 2>/dev/null || true
fi
exit 0
EOF
chmod 0755 "${BUILD_DIR}/DEBIAN/prerm"

# 8. DEBIAN/postrm (Storage Safety First: never delete /var/lib/meshfs CAS chunks)
cat << 'EOF' > "${BUILD_DIR}/DEBIAN/postrm"
#!/usr/bin/env bash
set -e
if [ "${1:-}" = "remove" ] || [ "${1:-}" = "purge" ]; then
    if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
        systemctl daemon-reload 2>/dev/null || true
    fi
fi
if [ "${1:-}" = "purge" ]; then
    rm -f /etc/meshfs/config.json 2>/dev/null || true
fi
exit 0
EOF
chmod 0755 "${BUILD_DIR}/DEBIAN/postrm"

# 9. Reproducible build timestamp & permission gate
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

sha256sum "${DEB_OUT}" > "${DIST_DIR}/SHA256SUMS"
echo "[MeshFS] Debian 13 package built & verified: ${DEB_OUT}"
cat "${DIST_DIR}/SHA256SUMS"
