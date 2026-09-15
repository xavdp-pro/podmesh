#!/bin/bash
set -euo pipefail
# Package directories must not inherit a group-writable build umask.
umask 022
cd "$(dirname "$0")/.."
version=0.1.0~experimental5
# Reported by the capabilities operation, so an installed binary identifies its package.
PODMESH_PACKAGE_VERSION=$version cargo build --release --locked -j2
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
chmod 755 "$work"
mkdir -p "$work/DEBIAN" "$work/usr/bin" "$work/usr/lib/systemd/system"
install -m755 target/release/podmesh target/release/podmeshd "$work/usr/bin/"
install -m644 packaging/podmesh.service packaging/podmesh-fence.service packaging/podmesh-fence.timer "$work/usr/lib/systemd/system/"
install -m755 packaging/podmesh-fence "$work/usr/bin/"
cat > "$work/DEBIAN/control" <<CONTROL
Package: podmesh
Version: $version
Architecture: amd64
Section: admin
Priority: optional
Maintainer: Xavier de Poorter <xavier@xavdp.pro>
Depends: libc6 (>= 2.39), libgcc-s1, podman, systemd, coreutils
Description: Experimental local Podman lifecycle service
 Local root-only API and CLI with persistent host identity, observation
 journal and operation attempt history, operating on the default rootful
 Podman store. Experimental managed operations on network-disabled containers
 recorded by this host's journal: create from a local image ID; explicit start
 with an observed outcome; stop with a declared graceful timeout and declared
 escalation; delete of a stopped container; clone of a stopped container
 without volumes or bind mounts through a committed snapshot image.
 This version also carries the experimental migration operations, qualified in
 the laboratory for one workload shape between two identical hosts: source
 preflight and checkpoint with a durable reservation, transfer authorization,
 destination preflight and restore with imported ownership, completion, source
 retirement, and the recovery of a reservation that never left its host. They
 require the separately packaged podmesh-vzcriu runtime and its helper shim.
 Networking, volumes and high availability are not implemented in this version,
 and a reservation is not fencing. The self-fence timer (podmesh-fence.timer) is
 shipped disabled and runs nothing without the operator's mandate file.
CONTROL
cat > "$work/DEBIAN/postinst" <<'SCRIPT'
#!/bin/sh
set -e
if [ "$1" = configure ] && [ -d /run/systemd/system ]; then
 systemctl daemon-reload
 systemctl enable podmesh.service
 systemctl restart podmesh.service
 # podmesh-fence.timer is deliberately NOT enabled here: whether the host may fence itself on a
 # schedule is the operator's decision, taken by writing /etc/podmesh/fence-mandate and enabling it.
fi
SCRIPT
cat > "$work/DEBIAN/prerm" <<'SCRIPT'
#!/bin/sh
set -e
if [ -d /run/systemd/system ]; then
 systemctl stop podmesh-fence.timer 2>/dev/null || true
 systemctl stop podmesh.service
 if [ "$1" = remove ]; then systemctl disable podmesh-fence.timer 2>/dev/null || true; systemctl disable podmesh.service; fi
fi
SCRIPT
cat > "$work/DEBIAN/postrm" <<'SCRIPT'
#!/bin/sh
set -e
# Keep persistent identity and journal, including on purge, for explicit recovery.
if [ -d /run/systemd/system ]; then systemctl daemon-reload; fi
SCRIPT
chmod 755 "$work/DEBIAN/postinst" "$work/DEBIAN/prerm" "$work/DEBIAN/postrm"
mkdir -p dist
dpkg-deb --root-owner-group --build "$work" "dist/podmesh_${version}_amd64.deb"
