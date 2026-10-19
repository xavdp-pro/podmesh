#!/usr/bin/env bash
# =============================================================================
# PodMesh MeshFS — Script d'installation 1-clic du Plugin Proxmox VE
# =============================================================================
# Ce script installe et active le plugin officiel MeshFS pour Proxmox VE :
#  1. Copie MeshFSPlugin.pm dans /usr/share/perl5/PVE/Storage/Custom/
#  2. Crée le répertoire cible /mnt/pve/<storage_id>
#  3. Déclare le stockage dans /etc/pve/storage.cfg
#  4. Recharge les démons Proxmox (pvedaemon, pveproxy, pvestatd)
#  5. Teste la connectivité et le statut du stockage
# =============================================================================

set -euo pipefail

# Couleurs de sortie
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_SOURCE="${SCRIPT_DIR}/MeshFSPlugin.pm"
TARGET_DIR="/usr/share/perl5/PVE/Storage/Custom"
TARGET_FILE="${TARGET_DIR}/MeshFSPlugin.pm"
LEGACY_TARGET="/usr/share/perl5/PVE/Storage/MeshFSPlugin.pm"

# Variables de configuration personnalisables (via options ou environnement)
STORAGE_ID="${STORAGE_ID:-meshfs}"
SERVER="${SERVER:-127.0.0.1}"
PORT="${PORT:-7443}"
VOLUME="${VOLUME:-default}"
STORAGE_PATH="${STORAGE_PATH:-/mnt/pve/${STORAGE_ID}}"
SHARED="${SHARED:-1}"
CONTENT="${CONTENT:-images,rootdir,backup,iso,vztmpl,snippets}"
DRY_RUN="${DRY_RUN:-0}"

usage() {
    cat << EOF
Utilisation: $0 [OPTIONS]

Options :
  -s, --storage-id ID     Nom de l'identifiant de stockage Proxmox (défaut: meshfs)
  -h, --server IP/HOST    Adresse du nœud MeshFS (défaut: 127.0.0.1)
  -p, --port PORT         Port de l'API REST MeshFS (défaut: 7443)
  -v, --volume VOL        Nom du volume MeshFS (défaut: default)
  -m, --path CHEMIN       Chemin local du montage (défaut: /mnt/pve/<storage-id>)
      --shared 0|1        Activer le stockage partagé cluster (défaut: 1)
      --content LISTE     Types de contenus autorisés (défaut: images,rootdir,backup,iso,vztmpl,snippets)
      --dry-run           Affiche les actions sans les exécuter
      --help              Affiche cette aide

Exemple :
  sudo $0 --storage-id meshfs-cluster --server 192.168.1.50 --port 7443 --shared 1
EOF
    exit 0
}

# Analyse des arguments CLI
while [[ $# -gt 0 ]]; do
    case "$1" in
        -s|--storage-id) STORAGE_ID="$2"; shift 2 ;;
        -h|--server) SERVER="$2"; shift 2 ;;
        -p|--port) PORT="$2"; shift 2 ;;
        -v|--volume) VOLUME="$2"; shift 2 ;;
        -m|--path) STORAGE_PATH="$2"; shift 2 ;;
        --shared) SHARED="$2"; shift 2 ;;
        --content) CONTENT="$2"; shift 2 ;;
        --dry-run) DRY_RUN=1; shift ;;
        --help) usage ;;
        *) echo -e "${RED}[ERREUR] Option inconnue: $1${NC}" >&2; usage ;;
    esac
done

echo -e "${BLUE}====================================================================${NC}"
echo -e "${CYAN}      PodMesh MeshFS — Déploiement Plugin Stockage Proxmox VE       ${NC}"
echo -e "${BLUE}====================================================================${NC}"
echo -e "  • Storage ID   : ${GREEN}${STORAGE_ID}${NC}"
echo -e "  • Serveur API  : ${GREEN}http://${SERVER}:${PORT}${NC}"
echo -e "  • Volume cible : ${GREEN}${VOLUME}${NC}"
echo -e "  • Chemin local : ${GREEN}${STORAGE_PATH}${NC}"
echo -e "  • Mode Partagé : ${GREEN}${SHARED}${NC}"
echo -e "  • Contenu      : ${GREEN}${CONTENT}${NC}"
echo -e "${BLUE}--------------------------------------------------------------------${NC}"

# 1. Vérification des privilèges root
if [[ $EUID -ne 0 ]] && [[ "$DRY_RUN" -eq 0 ]]; then
    echo -e "${RED}[ERREUR] Ce script doit être exécuté avec les privilèges root (sudo).${NC}" >&2
    exit 1
fi

# 2. Vérification de la présence de MeshFSPlugin.pm source
if [[ ! -f "$PLUGIN_SOURCE" ]]; then
    echo -e "${RED}[ERREUR] Source MeshFSPlugin.pm introuvable: ${PLUGIN_SOURCE}${NC}" >&2
    exit 1
fi

# 3. Vérification des dépendances Perl sous Debian/Proxmox
echo -e "${YELLOW}[1/5] Vérification des modules Perl système...${NC}"
MISSING_PKGS=()
if ! perl -MLWP::UserAgent -e 1 2>/dev/null; then
    MISSING_PKGS+=("libwww-perl")
fi
if ! perl -MJSON -e 1 2>/dev/null; then
    MISSING_PKGS+=("libjson-perl")
fi

if [[ ${#MISSING_PKGS[@]} -gt 0 ]]; then
    echo -e "${YELLOW}  -> Installation des dépendances manquantes: ${MISSING_PKGS[*]}${NC}"
    if [[ "$DRY_RUN" -eq 0 ]]; then
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq && apt-get install -y -qq "${MISSING_PKGS[@]}"
    fi
fi
echo -e "${GREEN}  ✓ Dépendances Perl validées.${NC}"

# 4. Copie du plugin dans le répertoire Custom de Proxmox
echo -e "${YELLOW}[2/5] Installation du plugin Perl dans Proxmox VE...${NC}"
if [[ "$DRY_RUN" -eq 0 ]]; then
    mkdir -p "${TARGET_DIR}"
    cp -f "${PLUGIN_SOURCE}" "${TARGET_FILE}"
    chmod 644 "${TARGET_FILE}"

    # Lien symbolique dans PVE/Storage pour compatibilité double namespace
    ln -sf "${TARGET_FILE}" "${LEGACY_TARGET}" 2>/dev/null || true

    # Validation de la compilation Perl
    perl -I/usr/share/perl5 -c "${TARGET_FILE}"
fi
echo -e "${GREEN}  ✓ Plugin installé dans ${TARGET_FILE}.${NC}"

# 5. Création du point de montage et arborescence locale
echo -e "${YELLOW}[3/5] Préparation du point de montage local...${NC}"
if [[ "$DRY_RUN" -eq 0 ]]; then
    mkdir -p "${STORAGE_PATH}"
    mkdir -p "${STORAGE_PATH}/images"
    mkdir -p "${STORAGE_PATH}/dump"
    mkdir -p "${STORAGE_PATH}/template/iso"
    mkdir -p "${STORAGE_PATH}/template/cache"
    mkdir -p "${STORAGE_PATH}/snippets"
    chmod 755 "${STORAGE_PATH}"
fi
echo -e "${GREEN}  ✓ Dossiers de stockage initialisés sous ${STORAGE_PATH}.${NC}"

# 6. Déclaration dans /etc/pve/storage.cfg
echo -e "${YELLOW}[4/5] Configuration du stockage dans /etc/pve/storage.cfg...${NC}"
STORAGE_CFG="/etc/pve/storage.cfg"

if [[ -f "$STORAGE_CFG" ]] && [[ -r "$STORAGE_CFG" ]]; then
    if grep -q -E "^meshfs:\s*${STORAGE_ID}\b" "$STORAGE_CFG" 2>/dev/null; then
        echo -e "${CYAN}  ℹ Le stockage '${STORAGE_ID}' est déjà déclaré dans ${STORAGE_CFG}. Mise à jour des paramètres si besoin.${NC}"
    else
        echo -e "${YELLOW}  -> Ajout de la strophe meshfs:${STORAGE_ID} à ${STORAGE_CFG}...${NC}"
        if [[ "$DRY_RUN" -eq 0 ]]; then
            # Sauvegarde de précaution
            cp -a "$STORAGE_CFG" "${STORAGE_CFG}.bak.$(date +%s)"

            cat << CFG_EOF >> "$STORAGE_CFG"

meshfs: ${STORAGE_ID}
	path ${STORAGE_PATH}
	server ${SERVER}
	port ${PORT}
	volume ${VOLUME}
	content ${CONTENT}
	shared ${SHARED}
CFG_EOF
        fi
        echo -e "${GREEN}  ✓ Strophe ajoutée avec succès.${NC}"
    fi
else
    echo -e "${YELLOW}  ℹ ${STORAGE_CFG} non accessible ou non présent (mode dry-run ou standalone).${NC}"
fi

# 7. Rechargement des services Proxmox VE
echo -e "${YELLOW}[5/5] Rechargement des démons Proxmox VE...${NC}"
if [[ "$DRY_RUN" -eq 0 ]]; then
    for srv in pvedaemon pveproxy pvestatd; do
        if systemctl is-active --quiet "$srv" 2>/dev/null; then
            echo -e "  -> Rechargement de ${srv}..."
            systemctl reload-or-restart "$srv" || true
        fi
    done
fi
echo -e "${GREEN}  ✓ Services Proxmox rafraîchis.${NC}"

# 8. Test de connectivité vers MeshFS
echo -e "${BLUE}--------------------------------------------------------------------${NC}"
echo -e "${CYAN}Vérification de la santé du stockage MeshFS...${NC}"
HTTP_STATUS=$(curl -s -m 3 "http://${SERVER}:${PORT}/api/v1/meshfs/status" || echo "FAIL")
if [[ "$HTTP_STATUS" != "FAIL" ]] && echo "$HTTP_STATUS" | grep -q '"ok":\s*true'; then
    echo -e "${GREEN}[SUCCÈS] Démon MeshFS opérationnel sur http://${SERVER}:${PORT}.${NC}"
    if which pvesm >/dev/null 2>&1 && [[ -r /etc/pve/storage.cfg ]]; then
        echo -e "${CYAN}Statut Proxmox (pvesm status --storage ${STORAGE_ID}) :${NC}"
        pvesm status --storage "${STORAGE_ID}" || true
    fi
else
    echo -e "${YELLOW}[ATTENTION] Le démon MeshFS n'a pas répondu sur http://${SERVER}:${PORT}.${NC}"
    echo -e "${YELLOW}Vérifiez que le service meshfs est démarré : systemctl status meshfs${NC}"
fi

echo -e "${BLUE}====================================================================${NC}"
echo -e "${GREEN}Installation du Plugin Proxmox VE MeshFS terminée avec succès !${NC}"
echo -e "${BLUE}====================================================================${NC}"
exit 0
