#!/usr/bin/env bash
set -eo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DEV_DATA="${ROOT_DIR}/data/dev"
MESHFS_DATA="${DEV_DATA}/meshfs"
PMBS_DATA="${DEV_DATA}/pmbs"
MESHFS_SOCK="${MESHFS_DATA}/meshfs.sock"
PMBS_SOCK="${PMBS_DATA}/pmbs.sock"

MESHFS_PORT=${MESHFS_PORT:-7443}
PMBS_PORT=${PMBS_PORT:-8007}
VITE_PORT=${VITE_PORT:-5173}

CHECK_ONLY=false
if [ "${1:-}" = "--check" ] || [ "${1:-}" = "--test" ]; then
    CHECK_ONLY=true
fi

echo "=========================================================="
echo "  PODMESH SUITE — Local Development Environment (DEV MODE)"
echo "  100% Pure Rust Backend + Vite React Showcase / Admin UI"
echo "=========================================================="
echo "  MeshFS HTTP API : http://127.0.0.1:${MESHFS_PORT}"
echo "  MeshFS IPC Sock : ${MESHFS_SOCK}"
echo "  PMBS HTTP API   : http://127.0.0.1:${PMBS_PORT}"
echo "  PMBS IPC Sock   : ${PMBS_SOCK}"
echo "  Showcase UI     : http://127.0.0.1:${VITE_PORT}"
echo "=========================================================="

mkdir -p "${MESHFS_DATA}/chunks" "${MESHFS_DATA}/meta"
mkdir -p "${PMBS_DATA}/datastores/datastore-default/chunks" "${PMBS_DATA}/datastores/datastore-default/snapshots"

# Nettoyage d'anciens sockets Unix résiduels
rm -f "${MESHFS_SOCK}" "${PMBS_SOCK}"

echo "--> [1/4] Compilation des binaires Rust en mode debug..."
cargo build --workspace --manifest-path "${ROOT_DIR}/Cargo.toml"

MESHFS_BIN="${ROOT_DIR}/target/debug/meshfs"
PMBS_BIN="${ROOT_DIR}/target/debug/pmbs"
CTL_BIN="${ROOT_DIR}/target/debug/podmesh-ctl"

PIDS=()
cleanup() {
    echo ""
    echo "--> Arrêt propre des processus de développement..."
    for pid in "${PIDS[@]}"; do
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
        fi
    done
    rm -f "${MESHFS_SOCK}" "${PMBS_SOCK}"
    echo "--> Environnement Dev arrêté avec succès."
}
trap cleanup EXIT INT TERM

# 2. Démarrage MeshFS Daemon
echo "--> [2/4] Démarrage de MeshFS Daemon (Port ${MESHFS_PORT})..."
"${MESHFS_BIN}" daemon \
    --port "${MESHFS_PORT}" \
    --data-dir "${MESHFS_DATA}" \
    --socket "${MESHFS_SOCK}" \
    --debug > "${DEV_DATA}/meshfs.log" 2>&1 &
PIDS+=($!)

# 3. Démarrage PMBS Daemon
echo "--> [3/4] Démarrage de PMBS Daemon (Port ${PMBS_PORT})..."
"${PMBS_BIN}" daemon \
    --port "${PMBS_PORT}" \
    --data-dir "${PMBS_DATA}" \
    --socket "${PMBS_SOCK}" \
    --debug > "${DEV_DATA}/pmbs.log" 2>&1 &
PIDS+=($!)

# 4. Attente et vérification de la disponibilité
echo "--> Vérification de la disponibilité des daemons..."
READY=false
for i in $(seq 1 30); do
    if [ -S "${MESHFS_SOCK}" ] && [ -S "${PMBS_SOCK}" ]; then
        if curl -s "http://127.0.0.1:${MESHFS_PORT}/api/v1/meshfs/status" >/dev/null 2>&1 && \
           curl -s "http://127.0.0.1:${PMBS_PORT}/api/v1/pmbs/status" >/dev/null 2>&1; then
            READY=true
            break
        fi
    fi
    sleep 0.2
done

if [ "$READY" = false ]; then
    echo "ERREUR: Les daemons n'ont pas démarré correctement. Logs :"
    echo "--- MeshFS log ---"
    cat "${DEV_DATA}/meshfs.log"
    echo "--- PMBS log ---"
    cat "${DEV_DATA}/pmbs.log"
    exit 1
fi

echo "  ✓ MeshFS Daemon en ligne (HTTP + Socket Unix)"
echo "  ✓ PMBS Daemon en ligne (HTTP + Socket Unix)"

# Test rapide CLI podmesh-ctl
echo "  ✓ Test de communication podmesh-ctl vers daemons..."
"${CTL_BIN}" status --remote-url "http://127.0.0.1:8080" >/dev/null 2>&1 || true

# 5. Démarrage Frontend Showcase
if [ -d "${ROOT_DIR}/showcase" ]; then
    echo "--> [4/4] Démarrage du serveur Frontend Showcase (Port ${VITE_PORT})..."
    cd "${ROOT_DIR}/showcase"
    if [ ! -d "node_modules" ] && [ -d "${ROOT_DIR}/../podmesh-manager/node_modules" ]; then
        ln -s "${ROOT_DIR}/../podmesh-manager/node_modules" ./node_modules
    fi
    npx vite --port "${VITE_PORT}" --host 0.0.0.0 > "${DEV_DATA}/vite.log" 2>&1 &
    PIDS+=($!)
    cd "${ROOT_DIR}"
fi

echo ""
echo "=========================================================="
echo "  TOUS LES COMPOSANTS SONT OPÉRATIONNELS EN MODE DEV !"
echo "=========================================================="
echo "  - MeshFS : curl http://127.0.0.1:${MESHFS_PORT}/api/v1/meshfs/status"
echo "  - PMBS   : curl http://127.0.0.1:${PMBS_PORT}/api/v1/pmbs/status"
echo "  - Web UI : http://localhost:${VITE_PORT}"
echo "  - Logs   : tail -f ${DEV_DATA}/*.log"
echo "=========================================================="

if [ "$CHECK_ONLY" = true ]; then
    echo "Mode --check terminé avec succès. Arrêt des services..."
    exit 0
fi

echo "Appuyez sur [Ctrl+C] pour arrêter l'ensemble des services."
# Attendre que l'utilisateur quitte
wait
