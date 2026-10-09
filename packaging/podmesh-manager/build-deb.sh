#!/bin/bash
# Assemble a separately supplied manager binary. This script never compiles, starts,
# publishes or deploys the manager.
set -euo pipefail
umask 022

if [ "$#" -ne 4 ]; then
  echo "Usage: SOURCE_DATE_EPOCH=<epoch> $0 <podmesh-managerd-binary> <sha256> <version> <output-directory>" >&2
  exit 2
fi

binary=$(realpath -- "$1")
expected=$2
version=$3
output=$4
: "${SOURCE_DATE_EPOCH:?Set SOURCE_DATE_EPOCH to the source commit timestamp}"
[[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] || { echo "Invalid SOURCE_DATE_EPOCH" >&2; exit 2; }
[[ "$expected" =~ ^[a-f0-9]{64}$ ]] || { echo "Expected lowercase SHA-256 required" >&2; exit 2; }
dpkg --validate-version "$version"
[ -f "$binary" ] && [ -x "$binary" ] || { echo "Executable binary required" >&2; exit 2; }
[ "$(basename -- "$binary")" = podmesh-managerd ] || { echo "Binary must be named podmesh-managerd" >&2; exit 2; }
actual=$(sha256sum -- "$binary")
[ "${actual%% *}" = "$expected" ] || { echo "Binary checksum mismatch" >&2; exit 2; }
LC_ALL=C readelf -h "$binary" | grep -q 'Machine:.*Advanced Micro Devices X86-64' || { echo "An amd64 ELF binary is required" >&2; exit 2; }
glibc_versions=$(LC_ALL=C readelf --version-info "$binary" 2>/dev/null | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -Vu || true)
glibc_max=none
if [ -n "$glibc_versions" ]; then
  glibc_max=$(printf '%s\n' "$glibc_versions" | tail -n 1 | sed 's/^GLIBC_//')
  glibc_major=${glibc_max%%.*}
  glibc_minor=${glibc_max#*.}
  if [ "$glibc_major" -gt 2 ] || { [ "$glibc_major" -eq 2 ] && [ "$glibc_minor" -gt 39 ]; }; then
    echo "Binary requires GLIBC_$glibc_max, exceeding the declared libc6 baseline 2.39" >&2
    exit 2
  fi
fi

source_dir=$(cd -- "$(dirname -- "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
chmod 755 "$work"
install -d "$work/DEBIAN" "$work/usr/lib/podmesh-manager" "$work/usr/lib/systemd/system" \
  "$work/usr/share/doc/podmesh-manager" "$work/usr/share/podmesh-manager"
install -m755 "$binary" "$work/usr/lib/podmesh-manager/podmesh-managerd"
staged=$(sha256sum "$work/usr/lib/podmesh-manager/podmesh-managerd")
[ "${staged%% *}" = "$expected" ] || { echo "Binary changed during packaging" >&2; exit 2; }
install -m644 "$source_dir/podmesh-manager.service" "$work/usr/lib/systemd/system/"
install -m644 "$source_dir/config.example.json" "$work/usr/share/podmesh-manager/config.example.json"
install -m644 "$source_dir/../../docs/MANAGER-DEPLOYMENT.md" "$work/usr/share/doc/podmesh-manager/MANAGER-DEPLOYMENT.md"
printf '%s  /usr/lib/podmesh-manager/podmesh-managerd\n' "$expected" > "$work/usr/share/doc/podmesh-manager/binary.sha256"
printf '%s\n' "$glibc_max" > "$work/usr/share/doc/podmesh-manager/binary.glibc-max"
install -m644 "$source_dir/../../LICENSE" "$work/usr/share/doc/podmesh-manager/copyright"
install -m644 "$source_dir/../../NOTICE" "$work/usr/share/doc/podmesh-manager/NOTICE"

cat > "$work/DEBIAN/control" <<CONTROL
Package: podmesh-manager
Version: $version
Architecture: amd64
Section: admin
Priority: optional
Maintainer: Xavier de Poorter <xavier@xavdp.pro>
Depends: adduser, libc6 (>= 2.39), libgcc-s1, systemd
Description: Experimental resident PodMesh control-services manager
 Separate manager process with an independent Unix/runtime path, system user,
 state directory and configuration boundary. It does not replace, restart or
 depend on the PodMesh lifecycle daemon, its observer, Podman or CRIU.
 Network replication remains disabled until an explicitly qualified manager
 binary and operator-provided configuration are installed.
CONTROL

install -m755 "$source_dir/postinst" "$work/DEBIAN/postinst"
install -m755 "$source_dir/prerm" "$work/DEBIAN/prerm"
install -m755 "$source_dir/postrm" "$work/DEBIAN/postrm"

find "$work" -print0 | xargs -0 touch --no-dereference --date="@$SOURCE_DATE_EPOCH"
mkdir -p -- "$output"
dpkg-deb --root-owner-group -Zxz --uniform-compression --build "$work" "$output/podmesh-manager_${version}_amd64.deb"
