#!/bin/bash
# Package an explicitly supplied, trusted build; never compile or deploy here.
set -euo pipefail
umask 022
if [ "$#" -ne 4 ]; then
  echo "Usage: SOURCE_DATE_EPOCH=<epoch> $0 <podmeshd-binary> <sha256> <version> <output-directory>" >&2
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
actual=$(sha256sum -- "$binary")
[ "${actual%% *}" = "$expected" ] || { echo "Binary checksum mismatch" >&2; exit 2; }
# This first package targets the qualified Linux amd64 laboratory only.
LC_ALL=C readelf -h "$binary" | grep -q 'Machine:.*Advanced Micro Devices X86-64' || { echo "An amd64 ELF binary is required" >&2; exit 2; }
source_dir=$(cd -- "$(dirname -- "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
chmod 755 "$work"
install -d "$work/DEBIAN" "$work/usr/lib/podmesh-web-observer" "$work/usr/lib/systemd/system" "$work/usr/share/doc/podmesh-web-observer"
install -m755 "$binary" "$work/usr/lib/podmesh-web-observer/podmeshd"
staged=$(sha256sum "$work/usr/lib/podmesh-web-observer/podmeshd")
[ "${staged%% *}" = "$expected" ] || { echo "Binary changed during packaging" >&2; exit 2; }
install -m644 "$source_dir/podmesh-web-observer.service" "$work/usr/lib/systemd/system/"
install -m644 "$source_dir/../../docs/WEB-OBSERVER-PACKAGING.md" "$work/usr/share/doc/podmesh-web-observer/README.md"
printf '%s  /usr/lib/podmesh-web-observer/podmeshd\n' "$expected" > "$work/usr/share/doc/podmesh-web-observer/binary.sha256"
cat > "$work/DEBIAN/control" <<CONTROL
Package: podmesh-web-observer
Version: $version
Architecture: amd64
Section: admin
Priority: optional
Maintainer: Xavier de Poorter <xavier@xavdp.pro>
Depends: libc6 (>= 2.39), libgcc-s1, podman, systemd, coreutils
Description: Experimental observation-only PodMesh explorer service
 Dedicated root-only Unix API for host and nested Podman observations.
 Uses a separate binary, socket, identity and journal from the lifecycle daemon.
 No TCP listener, migration runtime or automatic activation is included.
CONTROL
cat > "$work/DEBIAN/postinst" <<'SCRIPT'
#!/bin/sh
set -e
# Installation and upgrades never start or restart the observer automatically.
if [ "$1" = configure ] && [ -d /run/systemd/system ]; then
 systemctl daemon-reload
fi
SCRIPT
cat > "$work/DEBIAN/prerm" <<'SCRIPT'
#!/bin/sh
set -e
# Stop only this service on removal; an upgrade leaves activation to the operator.
if [ "$1" = remove ] && [ -d /run/systemd/system ]; then
 systemctl stop podmesh-web-observer.service
 systemctl disable podmesh-web-observer.service
fi
SCRIPT
cat > "$work/DEBIAN/postrm" <<'SCRIPT'
#!/bin/sh
set -e
# Preserve the observation identity and journal even on purge.
if [ -d /run/systemd/system ]; then systemctl daemon-reload; fi
SCRIPT
chmod 755 "$work/DEBIAN/postinst" "$work/DEBIAN/prerm" "$work/DEBIAN/postrm"
find "$work" -print0 | xargs -0 touch --no-dereference --date="@$SOURCE_DATE_EPOCH"
mkdir -p -- "$output"
dpkg-deb --root-owner-group -Zxz --uniform-compression --build "$work" "$output/podmesh-web-observer_${version}_amd64.deb"
