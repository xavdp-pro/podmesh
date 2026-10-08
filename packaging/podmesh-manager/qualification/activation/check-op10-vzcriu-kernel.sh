#!/bin/bash
# G2 op 10 operator gate: kernel + packaged podmesh-vzcriu criu check (read-only).
set -euo pipefail
export LC_ALL=C

criu=/opt/podmesh-vzcriu-kit/bin/criu
kit_path=/opt/podmesh-vzcriu-kit/bin:/usr/sbin:/usr/bin:/sbin:/bin
known_bad_kernel=7.0.14-6-pve

usage() {
  echo "Usage: $0" >&2
  echo "Run as root on the controller host before check-boot-restore.py (see docs/PREPARE-A-HOST.md)." >&2
  exit 2
}

[ "$#" -eq 0 ] || usage
[ "$(id -u)" -eq 0 ] || { echo 'This gate must run as root (sudo).' >&2; exit 2; }
command -v uname >/dev/null || { echo 'uname is required' >&2; exit 2; }

kernel=$(uname -r)
printf 'kernel: %s\n' "$kernel"

if [ ! -x "$criu" ]; then
  echo "FAIL: packaged CRIU not found at $criu — install podmesh-vzcriu from the signed repository." >&2
  echo 'See docs/PREPARE-A-HOST.md (boot-restore acceptance) and podmesh-lab op10-operator-next-steps.md.' >&2
  exit 1
fi

criu_rc=0
if ! env PATH="$kit_path" "$criu" check; then
  criu_rc=$?
fi

if [ "$kernel" = "$known_bad_kernel" ] || [ "$criu_rc" -ne 0 ]; then
  echo 'FAIL: op 10 live capture is blocked until kit criu check passes on this kernel.' >&2
  if [ "$kernel" = "$known_bad_kernel" ]; then
    echo "Known pairing issue: Proxmox kernel $known_bad_kernel + podmesh-vzcriu CRIU 3.15 (vDSO / kerndat)." >&2
    echo 'Operator: boot an installed alternative kernel (lab used 6.8.12-36-pve), re-run this script, then check-boot-restore.py.' >&2
  elif [ "$criu_rc" -ne 0 ]; then
    echo "kit criu check exited $criu_rc (PATH=$kit_path)." >&2
  fi
  echo 'Product: docs/PREPARE-A-HOST.md — lab: op10-pve-vdso-research-2026-10-08.md, op10-p3-kernel-inventory-2026-10-08.md, op10-operator-next-steps.md.' >&2
  exit 1
fi

echo 'PASS: kernel and packaged criu check are qualified for op 10 boot-restore on this host.'
