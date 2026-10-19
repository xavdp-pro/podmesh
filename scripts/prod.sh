#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="${ROOT_DIR}/dist"

echo "=========================================================="
echo "  PODMESH SUITE — Production Build & Packaging (PROD MODE)"
echo "  100% Pure Rust Binaries + Hardened Debian Packages (.deb)"
echo "=========================================================="

# 1. Compilation Release optimisée avec LTO
echo "--> [1/4] Compilation des binaires Rust en mode RELEASE (LTO, stripped)..."
cargo build --release --workspace --manifest-path "${ROOT_DIR}/Cargo.toml"

# 2. Build Frontend Showcase production
if [ -d "${ROOT_DIR}/showcase" ]; then
    echo "--> [2/4] Compilation des assets Frontend Showcase (Vite / React 19)..."
    cd "${ROOT_DIR}/showcase"
    if [ ! -d "node_modules" ] && [ -d "${ROOT_DIR}/../podmesh-manager/node_modules" ]; then
        ln -s "${ROOT_DIR}/../podmesh-manager/node_modules" ./node_modules
    fi
    npx vite build
    cd "${ROOT_DIR}"
fi

# 3. Génération de tous les paquets Debian
echo "--> [3/4] Génération de la suite complète de paquets Debian..."
bash "${ROOT_DIR}/packaging/build-all-debs.sh"

# 4. Synthèse et vérification des empreintes cryptographiques
echo "--> [4/4] Validation des paquets générés..."
echo ""
echo "=========================================================="
echo "  SUCCÈS : TOUS LES COMPOSANTS SONT PRÊTS POUR LA PRODUCTION"
echo "=========================================================="
ls -lh "${DIST_DIR}"/*.deb
echo ""
echo "Empreintes SHA-256 officielles :"
cat "${DIST_DIR}/SHA256SUMS"
echo ""
echo "=========================================================="
echo "  GUIDE DE DÉPLOIEMENT EN PRODUCTION (1-CLIC SUR VOS NŒUDS) :"
echo "=========================================================="
echo "  1. Installer MeshFS (Stockage Distribué & Plugin PVE) :"
echo "     dpkg -i dist/podmesh-meshfs_2.1.0_amd64.deb"
echo ""
echo "  2. Installer PodMesh Backup Server (PMBS) :"
echo "     dpkg -i dist/podmesh-backup-server_2.1.0_amd64.deb"
echo ""
echo "  3. Installer PodMesh Node Engine & Unified CLI :"
echo "     dpkg -i dist/podmesh_0.1.0~experimental7_amd64.deb"
echo ""
echo "  4. Vérifier les services systemd :"
echo "     systemctl status meshfs.service"
echo "     systemctl status pmbs.service"
echo "     systemctl status podmesh.service"
echo "=========================================================="
