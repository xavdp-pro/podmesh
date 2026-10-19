#!/usr/bin/env bash
set -euo pipefail

PKG_NAME="podmesh-backup-server"
PKG_VERSION="2.1.0"
PKG_ARCH="amd64"
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BUILD_DIR="$(mktemp -d /tmp/build-${PKG_NAME}.XXXXXX)"
DIST_DIR="${ROOT_DIR}/dist"
RUST_BIN="${ROOT_DIR}/target/release/pmbs"

echo "=== [PMBS] Packaging Debian: ${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb (Pure Rust Edition) ==="

if [ ! -f "${RUST_BIN}" ]; then
    echo "Compilation release de PMBS..."
    cargo build --release -p pmbs --manifest-path "${ROOT_DIR}/Cargo.toml"
fi

trap 'rm -rf "${BUILD_DIR}"' EXIT

mkdir -p "${BUILD_DIR}/DEBIAN"
mkdir -p "${BUILD_DIR}/usr/bin"
mkdir -p "${BUILD_DIR}/lib/systemd/system"
mkdir -p "${BUILD_DIR}/etc/pmbs"
mkdir -p "${BUILD_DIR}/var/lib/pmbs/datastores/datastore-default/chunks"
mkdir -p "${BUILD_DIR}/var/lib/pmbs/datastores/datastore-default/snapshots"
mkdir -p "${DIST_DIR}"

# 1. Copie du binaire natif autonome Rust
cp "${RUST_BIN}" "${BUILD_DIR}/usr/bin/pmbs"
chmod 755 "${BUILD_DIR}/usr/bin/pmbs"

# 2. Fichier de configuration par défaut
cat << 'EOF' > "${BUILD_DIR}/etc/pmbs/config.json"
{
  "port": 8007,
  "dataDir": "/var/lib/pmbs",
  "socket": "/var/lib/pmbs/pmbs.sock",
  "defaultDatastore": "datastore-default",
  "dedupEngine": "SHA-256 CAS 4MB (Pure Rust)",
  "op10VerifierReady": true,
  "remoteSyncEnabled": true
}
EOF

# 3. Service systemd
cat << 'EOF' > "${BUILD_DIR}/lib/systemd/system/pmbs.service"
[Unit]
Description=PodMesh Backup Server Daemon (Pure Rust PBS-like standalone)
After=network.target

[Service]
Type=simple
User=root
ExecStart=/usr/bin/pmbs daemon --port 8007 --data-dir /var/lib/pmbs --socket /var/lib/pmbs/pmbs.sock
Restart=always
RestartSec=3
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

# 4. DEBIAN/control
cat << EOF > "${BUILD_DIR}/DEBIAN/control"
Package: ${PKG_NAME}
Version: ${PKG_VERSION}
Section: admin
Priority: optional
Architecture: ${PKG_ARCH}
Depends: libc6 (>= 2.31)
Maintainer: Xavier <xavier@xavdp.pro>
Description: PodMesh Backup Server (PMBS) — Pure Rust Deduplicated Backup Server
 Proxmox Backup Server alternative for PodMesh and sovereign Linux servers.
 Features SHA-256 chunk deduplication, remote sync, Op 10 sandbox boot proof,
 Unix Domain Socket control IPC, and single-file restore.
 100% Native Pure Rust — Zero external runtime, < 8MB RAM.
 Can be installed on servers WITH or WITHOUT PodMesh.
EOF

# 5. DEBIAN/postinst
cat << 'EOF' > "${BUILD_DIR}/DEBIAN/postinst"
#!/usr/bin/env bash
set -e
if [ "$1" = "configure" ]; then
    systemctl daemon-reload || true
    systemctl enable pmbs.service || true
    systemctl restart pmbs.service || true
    echo "[PMBS] Service pmbs.service (Pure Rust) activé et démarré sur le port 8007."
fi
exit 0
EOF
chmod 755 "${BUILD_DIR}/DEBIAN/postinst"

# 6. DEBIAN/prerm
cat << 'EOF' > "${BUILD_DIR}/DEBIAN/prerm"
#!/usr/bin/env bash
set -e
if [ "$1" = "remove" ]; then
    systemctl stop pmbs.service 2>/dev/null || true
    systemctl disable pmbs.service 2>/dev/null || true
fi
exit 0
EOF
chmod 755 "${BUILD_DIR}/DEBIAN/prerm"

# 7. Construction du paquet .deb
dpkg-deb --root-owner-group --build "${BUILD_DIR}" "${DIST_DIR}/${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb"
echo "[PMBS] Paquet généré avec succès dans : ${DIST_DIR}/${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb"
