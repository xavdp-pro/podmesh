#!/bin/bash
# G2 op 10 operator entry after boot on the qualification kernel (read-only until boot-restore).
set -euo pipefail
export LC_ALL=C

root=$(cd -- "$(dirname -- "$0")" && pwd)
qual_kernel_substr=${PODMESH_OP10_QUAL_KERNEL_SUBSTR:-6.8.12-36-pve}
lab_grub_window=records/slice-a-campaign-2026-10-07/op10-operator-grub-reboot-window.md
dry_run=0

usage() {
  cat >&2 <<EOF
Usage: $0 [--dry-run]

Requires root. After a one-shot GRUB boot to the qualification kernel, verifies
uname -r contains "$qual_kernel_substr" (override test substring with
PODMESH_OP10_QUAL_KERNEL_SUBSTR), then runs check-op10-vzcriu-kernel.sh.

--dry-run   kernel gate only; do not run criu check (agent / wrong-kernel probe).

On vzcriu gate PASS, prints the lab-recorded check-boot-restore.py command.
Wrong kernel: see podmesh-lab $lab_grub_window.
EOF
  exit 2
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run)
      dry_run=1
      shift
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

[ "$(id -u)" -eq 0 ] || { echo 'This gate must run as root (sudo).' >&2; exit 2; }
command -v uname >/dev/null || { echo 'uname is required' >&2; exit 2; }

kernel=$(uname -r)
printf 'kernel: %s\n' "$kernel"

if [[ "$kernel" != *"$qual_kernel_substr"* ]]; then
  echo "FAIL: op 10 post-reboot gate requires a running kernel containing \"$qual_kernel_substr\"." >&2
  echo "Current: $kernel — boot the qualification kernel before CRIU or boot-restore work." >&2
  echo "Operator GRUB one-shot window (no agent reboot): podmesh-lab $lab_grub_window" >&2
  exit 1
fi

if [ "$dry_run" -eq 1 ]; then
  echo "PASS (dry-run): qualification kernel substring \"$qual_kernel_substr\" present."
  exit 0
fi

"$root/check-op10-vzcriu-kernel.sh"

cat <<'EOF'

Next (operator only, disposable reboot window; gate OP10-REBOOT-APPROVED on record):
  export PODMESH_REBOOT_SSH=zaza@localhost
  export PODMESH_REBOOT_MANDATE=mandate:lab-standing-2026-09-17
  export PODMESH_SOCKET=/run/podmesh-dev-ha/api.sock
  export PODMESH_STATE_DIR=/var/lib/podmesh-dev-ha
  export PODMESH_UNIT=podmesh-dev-ha.service
  export PODMESH_RESTORE_UNIT=podmesh-dev-ha-restore.service
  cd /home/zaza/Bureau/NOW7/PODMESH/podmesh
  python3 -B tests/check-boot-restore.py | tee /home/zaza/Bureau/NOW7/PODMESH/podmesh-lab/records/slice-a-campaign-2026-10-07/op10-boot-restore.log
  echo $? > /home/zaza/Bureau/NOW7/PODMESH/podmesh-lab/records/slice-a-campaign-2026-10-07/op10-boot-restore.exit

Full copy-paste block: podmesh-lab records/slice-a-campaign-2026-10-07/op10-operator-grub-reboot-window.md §8.
EOF
