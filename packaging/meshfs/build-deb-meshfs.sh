#!/usr/bin/env bash
set -euo pipefail

PKG_NAME="podmesh-meshfs"
PKG_VERSION="2.1.0"
PKG_ARCH="amd64"
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BUILD_DIR="$(mktemp -d /tmp/build-${PKG_NAME}.XXXXXX)"
DIST_DIR="${ROOT_DIR}/dist"
RUST_BIN="${ROOT_DIR}/target/release/meshfs"

echo "=== [MeshFS] Packaging Debian: ${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb (Pure Rust Edition) ==="

# 1. Vérification compilation
if [ ! -f "${RUST_BIN}" ]; then
    echo "Compilation release de MeshFS..."
    cargo build --release -p meshfs --manifest-path "${ROOT_DIR}/Cargo.toml"
fi

trap 'rm -rf "${BUILD_DIR}"' EXIT

mkdir -p "${BUILD_DIR}/DEBIAN"
mkdir -p "${BUILD_DIR}/usr/bin"
mkdir -p "${BUILD_DIR}/lib/systemd/system"
mkdir -p "${BUILD_DIR}/etc/meshfs"
mkdir -p "${BUILD_DIR}/var/lib/meshfs/chunks"
mkdir -p "${BUILD_DIR}/var/lib/meshfs/meta"
mkdir -p "${DIST_DIR}"

# 2. Copie du binaire natif autonome Rust
cp "${RUST_BIN}" "${BUILD_DIR}/usr/bin/meshfs"
chmod 755 "${BUILD_DIR}/usr/bin/meshfs"

# 3. Fichier de configuration par défaut
cat << 'EOF' > "${BUILD_DIR}/etc/meshfs/config.json"
{
  "clusterName": "meshfs-federation",
  "port": 7443,
  "dataDir": "/var/lib/meshfs",
  "chunkSizeMb": 4,
  "socket": "/var/lib/meshfs/meshfs.sock",
  "standalone": true,
  "requiresPodmesh": false
}
EOF

# 4. Service systemd natif avec socket Unix et restart automatique
cat << 'EOF' > "${BUILD_DIR}/lib/systemd/system/meshfs.service"
[Unit]
Description=PodMesh MeshFS Pure Rust Distributed Filesystem Daemon
After=network.target

[Service]
Type=simple
User=root
ExecStart=/usr/bin/meshfs daemon --port 7443 --data-dir /var/lib/meshfs --socket /var/lib/meshfs/meshfs.sock
Restart=always
RestartSec=3
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

# 5. DEBIAN/control
cat << EOF > "${BUILD_DIR}/DEBIAN/control"
Package: ${PKG_NAME}
Version: ${PKG_VERSION}
Section: admin
Priority: optional
Architecture: ${PKG_ARCH}
Depends: libc6 (>= 2.31)
Maintainer: Xavier <xavier@xavdp.pro>
Description: PodMesh MeshFS — Pure Rust Distributed Deduplicated Filesystem (CephFS alternative)
 Ultra-lightweight distributed filesystem operating seamlessly on 1 node up to
 N nodes. Features SHA-256 chunk deduplication, Merkle tree structure snapshots,
 Unix Domain Socket control IPC, kernel FICLONE zero-copy reflink, and native
 incremental backup to PodMesh Backup Server (PMBS).
 100% Native Pure Rust — Zero external runtime, < 8MB RAM.
 Can be installed on servers WITH or WITHOUT PodMesh.
EOF

# 6. Intégration du plugin Proxmox VE si présent
if [ -d "${ROOT_DIR}/packaging/proxmox" ]; then
    mkdir -p "${BUILD_DIR}/usr/share/meshfs/proxmox"
    cp -r "${ROOT_DIR}/packaging/proxmox/"* "${BUILD_DIR}/usr/share/meshfs/proxmox/"
fi

# 7. DEBIAN/postinst
cat << 'EOF' > "${BUILD_DIR}/DEBIAN/postinst"
#!/usr/bin/env bash
set -e
if [ "$1" = "configure" ]; then
    systemctl daemon-reload || true
    systemctl enable meshfs.service || true
    systemctl restart meshfs.service || true
    echo "[MeshFS] Service meshfs.service (Pure Rust) activé et démarré sur le port 7443."

    # Détection automatique de Proxmox VE
    if [ -d "/usr/share/perl5/PVE" ]; then
        echo "[MeshFS] Environnement Proxmox VE détecté ! Enregistrement du plugin PVE..."
        mkdir -p /usr/share/perl5/PVE/Storage/Custom
        rm -f /usr/share/perl5/PVE/Storage/MeshFSPlugin.pm 2>/dev/null || true
        cp /usr/share/meshfs/proxmox/MeshFSPlugin.pm /usr/share/perl5/PVE/Storage/Custom/
        chmod 644 /usr/share/perl5/PVE/Storage/Custom/MeshFSPlugin.pm

        if ! grep -q "PVE::Storage::Custom::MeshFSPlugin" /usr/share/perl5/PVE/Storage.pm 2>/dev/null; then
            echo "[MeshFS] Ajout du plugin dans /usr/share/perl5/PVE/Storage.pm..."
            sed -i '/use PVE::Storage::Plugin;/a use PVE::Storage::Custom::MeshFSPlugin;' /usr/share/perl5/PVE/Storage.pm || true
            sed -i '/PVE::Storage::Plugin->init();/a PVE::Storage::Custom::MeshFSPlugin->register();' /usr/share/perl5/PVE/Storage.pm || true
        fi
        systemctl restart pvedaemon pveproxy 2>/dev/null || true
        echo "[MeshFS] Plugin Proxmox VE enregistré et services PVE rechargés."
    fi
fi
exit 0
EOF
chmod 755 "${BUILD_DIR}/DEBIAN/postinst"

# 8. DEBIAN/prerm
cat << 'EOF' > "${BUILD_DIR}/DEBIAN/prerm"
#!/usr/bin/env bash
set -e
if [ "$1" = "remove" ]; then
    systemctl stop meshfs.service 2>/dev/null || true
    systemctl disable meshfs.service 2>/dev/null || true
fi
exit 0
EOF
chmod 755 "${BUILD_DIR}/DEBIAN/prerm"

# 9. Construction du paquet .deb
dpkg-deb --root-owner-group --build "${BUILD_DIR}" "${DIST_DIR}/${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb"
echo "[MeshFS] Paquet généré avec succès dans : ${DIST_DIR}/${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}.deb"
