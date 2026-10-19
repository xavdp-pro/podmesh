#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="${ROOT_DIR}/dist"
APT_ROOT="${DIST_DIR}/apt-repos"

TARGET_CHANNEL="${1:-all}" # "dev", "prod", or "all"

echo "=========================================================="
echo "  PODMESH — Générateur de Repositories Debian APT (2 Canaux)"
echo "  Canal DEV  : trixie-dev    (Développement & Tests)"
echo "  Canal PROD : trixie-stable (Production & Stabilité)"
echo "=========================================================="

build_channel() {
    local CHANNEL="$1"        # "dev" ou "prod"
    local SUITE="$2"          # "trixie-dev" ou "trixie-stable"
    local LABEL="$3"          # "PodMesh Development" ou "PodMesh Production"
    local CHANNEL_DIR="${APT_ROOT}/${CHANNEL}"
    local POOL_DIR="${CHANNEL_DIR}/pool/main"
    local BIN_DIR="${CHANNEL_DIR}/dists/${SUITE}/main/binary-amd64"

    echo ""
    echo "--> [Canal ${CHANNEL^^}] Préparation du repository (${SUITE})..."
    rm -rf "${CHANNEL_DIR}"
    mkdir -p "${POOL_DIR}" "${BIN_DIR}"

    # Copie des paquets .deb correspondants
    if [ "${CHANNEL}" = "dev" ]; then
        # En mode DEV, on copie tous les paquets générés (y compris expérimentaux)
        cp "${DIST_DIR}"/*.deb "${POOL_DIR}/" 2>/dev/null || {
            echo "Avertissement: Aucun paquet trouvé dans ${DIST_DIR}. Lancement de la génération..."
            bash "${ROOT_DIR}/packaging/build-all-debs.sh"
            cp "${DIST_DIR}"/*.deb "${POOL_DIR}/"
        }
    else
        # En mode PROD, on intègre les versions qualifiées
        cp "${DIST_DIR}"/podmesh-meshfs_*.deb "${POOL_DIR}/" 2>/dev/null || true
        cp "${DIST_DIR}"/podmesh-backup-server_*.deb "${POOL_DIR}/" 2>/dev/null || true
        cp "${DIST_DIR}"/podmesh-cli_*.deb "${POOL_DIR}/" 2>/dev/null || true
        cp "${DIST_DIR}"/podmesh_*.deb "${POOL_DIR}/" 2>/dev/null || true
    fi

    # 1. Indexation Packages et Packages.gz via dpkg-scanpackages
    echo "    Indexation des paquets Debian dans ${BIN_DIR}/Packages..."
    (
        cd "${CHANNEL_DIR}"
        dpkg-scanpackages --multiversion pool/main /dev/null > "dists/${SUITE}/main/binary-amd64/Packages"
    )

    gzip -9c "${BIN_DIR}/Packages" > "${BIN_DIR}/Packages.gz"

    # 2. Génération du fichier Release conforme aux spécifications Debian
    local RELEASE_FILE="${CHANNEL_DIR}/dists/${SUITE}/Release"
    echo "    Génération des métadonnées cryptographiques dans Release..."

    cat << EOF > "${RELEASE_FILE}"
Origin: PodMesh
Label: ${LABEL}
Suite: ${SUITE}
Codename: ${SUITE}
Version: 2.1.0
Architectures: amd64
Components: main
Description: Dépôt officiel Debian PodMesh — Canal ${CHANNEL^^} (100% Pure Rust, Zero-Python Storage)
Date: $(date -Ru)
Valid-Until: $(date -Ru -d "+1 year")
EOF

    # Calcul des checksums pour Packages et Packages.gz
    append_checksums() {
        local HASH_NAME="$1" # MD5Sum, SHA1, SHA256
        local HASH_CMD="$2"  # md5sum, sha1sum, sha256sum

        echo "${HASH_NAME}:" >> "${RELEASE_FILE}"
        for rel_path in "main/binary-amd64/Packages" "main/binary-amd64/Packages.gz"; do
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

    # 3. Signature GPG si clé disponible
    if command -v gpg >/dev/null 2>&1; then
        local GPG_KEY=${GPG_SIGN_KEY:-}
        if [ -n "${GPG_KEY}" ]; then
            echo "    Signature cryptographique GPG avec la clé ${GPG_KEY}..."
            gpg --batch --yes --armor --default-key "${GPG_KEY}" --detach-sign --output "${RELEASE_FILE}.gpg" "${RELEASE_FILE}"
            gpg --batch --yes --clearsign --default-key "${GPG_KEY}" --output "${CHANNEL_DIR}/dists/${SUITE}/InRelease" "${RELEASE_FILE}"
        fi
    fi

    echo "    ✓ Canal ${CHANNEL^^} prêt : ${CHANNEL_DIR}"
}

case "${TARGET_CHANNEL}" in
    dev)
        build_channel "dev" "trixie-dev" "PodMesh Development"
        ;;
    prod)
        build_channel "prod" "trixie-stable" "PodMesh Production"
        ;;
    all)
        build_channel "dev" "trixie-dev" "PodMesh Development"
        build_channel "prod" "trixie-stable" "PodMesh Production"
        ;;
    *)
        echo "Canal inconnu : ${TARGET_CHANNEL}. Utiliser 'dev', 'prod' ou 'all'."
        exit 1
        ;;
esac

echo ""
echo "=========================================================="
echo "  RÉSUMÉ DES CANAUX APT GÉNÉRÉS DANS : ${APT_ROOT}"
echo "=========================================================="
tree -L 4 "${APT_ROOT}" 2>/dev/null || find "${APT_ROOT}" -maxdepth 4 -print
echo ""
echo "Configurations sources.list prêtes pour les clients :"
echo "--- Mode DEV (/etc/apt/sources.list.d/podmesh-dev.list) ---"
echo "deb [signed-by=/etc/apt/keyrings/xavdp.gpg] https://deb.xavdp.pro/dev trixie-dev main"
echo ""
echo "--- Mode PROD (/etc/apt/sources.list.d/podmesh-prod.list) ---"
echo "deb [signed-by=/etc/apt/keyrings/xavdp.gpg] https://deb.xavdp.pro/prod trixie-stable main"
echo "=========================================================="
