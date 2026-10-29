#!/usr/bin/env bash
# =============================================================================
# PodMesh Core Orchestrator v2.4.1 — Debian 13 (Trixie) Package Builder
# Hardened under Claude Fable 5.1 & GPT-6 Astra Counter-Reviews:
#  - Version unified to 2.4.1
#  - Depends on libc6 (>= 2.36) and podmesh-cli (= 2.4.1) (no file collision)
#  - Ships all 5 core binaries: podmesh, podmeshd, podmesh-host-adapter,
#    podmesh-store-admin, podmesh-storage-migrate
#  - Enforces 0755 on ./ and OS directories, usrmerge /usr/lib/systemd/system/
#  - Reproducible SHA-256 build via SOURCE_DATE_EPOCH
# =============================================================================
set -euo pipefail
umask 0022

cd "$(dirname "$0")/.."
ROOT_DIR="$(pwd)"
version="2.4.1"

if [ ! -f target/release/podmeshd ] || [ ! -f target/release/podmesh-host-adapter ]; then
    PODMESH_PACKAGE_VERSION="$version" cargo build --release --workspace
fi

work=$(mktemp -d /tmp/build-podmesh.XXXXXX)
trap 'rm -rf "$work"' EXIT
chmod 0755 "$work"

mkdir -p -m 0755 "$work/DEBIAN" "$work/usr" "$work/usr/bin" "$work/usr/lib" \
  "$work/usr/lib/systemd" "$work/usr/lib/systemd/system" \
  "$work/usr/share" "$work/usr/share/doc" "$work/usr/share/doc/podmesh"

install -m 0755 \
  target/release/podmesh \
  target/release/podmeshd \
  target/release/podmesh-host-adapter \
  target/release/podmesh-store-admin \
  target/release/podmesh-storage-migrate \
  "$work/usr/bin/"

install -m 0644 \
  packaging/podmesh.service \
  packaging/podmesh-fence.service \
  packaging/podmesh-fence.timer \
  packaging/podmesh-restore.service \
  packaging/podmesh-decision-follow.service \
  packaging/podmesh-decision-follow.timer \
  "$work/usr/lib/systemd/system/"

ln -s podmesh-ctl "$work/usr/bin/podmesh-fence"
ln -s podmesh-ctl "$work/usr/bin/podmesh-restore"
ln -s podmesh-ctl "$work/usr/bin/podmesh-decision-follow"
ln -s podmesh-ctl "$work/usr/bin/podmesh-publisher-follow"

if [ -f docs/DECISION-FOLLOW.md ]; then
    install -m 0644 docs/DECISION-FOLLOW.md "$work/usr/share/doc/podmesh/"
fi
install -m 0644 AI_INSTALL_AND_CONFIG.md "$work/usr/share/doc/podmesh/"

INSTALLED_SIZE="$(du -sk "$work" | awk '{print $1}')"

cat > "$work/DEBIAN/control" <<CONTROL
Package: podmesh
Version: $version
Architecture: amd64
Section: admin
Priority: optional
Installed-Size: $INSTALLED_SIZE
Maintainer: Xavier DP <xavier@xavdp.pro>
Homepage: https://podmesh.xavdp.pro
Depends: libc6 (>= 2.36), libgcc-s1, podman, systemd, coreutils, podmesh-cli (= $version)
Recommends: podmesh-meshfs (>= $version), podmesh-backup-server (>= $version)
Description: PodMesh Sovereign Container & Universe Orchestrator (v2.4.1)
 Sovereign AI-First Podman lifecycle, replication, and high-availability engine
 with persistent host identity, observation journal, Ed25519 epoch fencing,
 and private MariaDB functional units (podmesh-node, podmesh-manager).
 Ships podmesh, podmeshd, podmesh-host-adapter, podmesh-store-admin, and
 podmesh-storage-migrate.
CONTROL
chmod 0644 "$work/DEBIAN/control"

cat > "$work/DEBIAN/postinst" <<'SCRIPT'
#!/bin/sh
set -e
if [ "$1" = configure ] && [ -d /run/systemd/system ]; then
 systemctl daemon-reload
 systemctl enable podmesh.service
 systemctl restart podmesh.service
fi
SCRIPT

cat > "$work/DEBIAN/prerm" <<'SCRIPT'
#!/bin/sh
set -e
if [ -d /run/systemd/system ]; then
 systemctl stop podmesh-fence.timer podmesh-decision-follow.timer 2>/dev/null || true
 systemctl stop podmesh.service 2>/dev/null || true
 if [ "$1" = remove ]; then
  systemctl disable podmesh-fence.timer podmesh-restore.service podmesh-decision-follow.timer 2>/dev/null || true
  systemctl disable podmesh.service 2>/dev/null || true
 fi
fi
SCRIPT

cat > "$work/DEBIAN/postrm" <<'SCRIPT'
#!/bin/sh
set -e
if [ -d /run/systemd/system ]; then systemctl daemon-reload 2>/dev/null || true; fi
SCRIPT

chmod 0755 "$work/DEBIAN/postinst" "$work/DEBIAN/prerm" "$work/DEBIAN/postrm"
find "$work" -type d -exec chmod 0755 {} +

if git rev-parse HEAD >/dev/null 2>&1; then
    export SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)"
else
    export SOURCE_DATE_EPOCH="1793100000"
fi
find "$work" -exec touch -h -d "@${SOURCE_DATE_EPOCH}" {} +

mkdir -p dist
DEB_OUT="dist/podmesh_${version}_amd64.deb"
dpkg-deb --root-owner-group -Zxz --build "$work" "$DEB_OUT"

ROOT_PERM="$(dpkg-deb -c "$DEB_OUT" | awk '$6 == "./" {print $1}')"
if [ "${ROOT_PERM}" != "drwxr-xr-x" ]; then
    echo "FATAL [Invariant Breach]: ${DEB_OUT} root dir is ${ROOT_PERM}" >&2
    exit 1
fi
echo "[PodMesh] Debian 13 package built & verified: ${DEB_OUT}"
