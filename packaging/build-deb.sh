#!/bin/bash
set -euo pipefail
# Package directories must not inherit a group-writable build umask.
umask 022
cd "$(dirname "$0")/.."
version=0.1.0~experimental7
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
Description: Experimental local Podman lifecycle and manager engine
 Local root-only API and CLI with persistent host identity, observation
 journal and operation attempt history, operating on the default rootful
 Podman store. Carries lifecycle, clone, migration (M1–M3), garbage collection
 and recovery points (M4–M5 on main), managed universe networking with an
 effects ledger, secrets outside images, activation leases with fence and
 preview, and Ed25519-verified takeover proofs for publisher binding.
 Manager-universe high availability is lab-qualified on an isolated transient
 service only; this package does not by itself install or qualify production HA.
 Migration still requires the separately packaged podmesh-vzcriu runtime.
 Volumes, join/leave occupied hosts, Backup Server and control-services
 universe are out of scope. The self-fence timer (podmesh-fence.timer) is
 shipped disabled and runs nothing without the operator's mandate file.
 See docs/EXPERIMENTAL-SCOPE.md for the experimental boundary.
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
