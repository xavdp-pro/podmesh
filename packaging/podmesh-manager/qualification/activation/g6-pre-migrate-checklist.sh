#!/bin/bash
# G6 operator gate: canonical store topology + config/store identity before migrate.
set -euo pipefail
export LC_ALL=C

root=$(cd -- "$(dirname -- "$0")" && pwd)
from_sqlite=
config=
state_dir=
managerd=

usage() {
  cat >&2 <<EOF
Usage: $0 --from-sqlite PATH [--config PATH] [--state-dir PATH] [--managerd PATH]

Runs (in order, first failure stops):
  preflight-store-topology.py --from-sqlite PATH [--config PATH]
  preflight-store-identity.py [--config PATH] [--state-dir PATH] [--managerd PATH]

Requires root on the target host (identity uses inspect-store). Run before
podmesh-storage-migrate --role manager. See docs/MANAGER-DEPLOYMENT.md.
EOF
  exit 2
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --from-sqlite)
      from_sqlite=$2
      shift 2
      ;;
    --config)
      config=$2
      shift 2
      ;;
    --state-dir)
      state_dir=$2
      shift 2
      ;;
    --managerd)
      managerd=$2
      shift 2
      ;;
    -h|--help)
      usage
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      ;;
  esac
done

[ -n "$from_sqlite" ] || usage
[ -f "$from_sqlite" ] || {
  echo "FAIL: --from-sqlite is not a regular file: $from_sqlite" >&2
  exit 1
}
[ "$(id -u)" -eq 0 ] || {
  echo 'FAIL: run as root (sudo); store identity preflight uses inspect-store.' >&2
  exit 2
}

topo_args=(--from-sqlite "$from_sqlite")
identity_args=()
if [ -n "$config" ]; then
  topo_args+=(--config "$config")
  identity_args+=(--config "$config")
fi
[ -n "$state_dir" ] && identity_args+=(--state-dir "$state_dir")
[ -n "$managerd" ] && identity_args+=(--managerd "$managerd")

printf 'g6-pre-migrate: preflight-store-topology.py'
printf ' %q' "${topo_args[@]}"
printf '\n'
if ! python3 "$root/preflight-store-topology.py" "${topo_args[@]}"; then
  echo 'FAIL: store topology preflight refused — fix identity.topology_json before migrate.' >&2
  exit 1
fi

printf 'g6-pre-migrate: preflight-store-identity.py'
if [ "${#identity_args[@]}" -eq 0 ]; then
  printf ' (default paths)\n'
else
  printf ' %q' "${identity_args[@]}"
  printf '\n'
fi
if ! python3 "$root/preflight-store-identity.py" "${identity_args[@]}"; then
  echo 'FAIL: store identity preflight refused — align config.json with the store before migrate/activation.' >&2
  exit 1
fi

printf '%s\n' 'PASS: G6 pre-migrate store prefights.'
