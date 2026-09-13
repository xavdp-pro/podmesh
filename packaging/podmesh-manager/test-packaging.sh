#!/bin/bash
# Assembly-only test. The fixture is never a manager binary and is never installed.
set -euo pipefail
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
source_dir=$(cd -- "$(dirname -- "$0")" && pwd)
mkdir "$work/fixture"
install -m755 /usr/bin/true "$work/fixture/podmesh-managerd"
fixture="$work/fixture/podmesh-managerd"
hash=$(sha256sum "$fixture")
export SOURCE_DATE_EPOCH=1700000000
"$source_dir/build-deb.sh" "$fixture" "${hash%% *}" '0.0.0~fixture' "$work/one"
"$source_dir/build-deb.sh" "$fixture" "${hash%% *}" '0.0.0~fixture' "$work/two"
package=podmesh-manager_0.0.0~fixture_amd64.deb
cmp "$work/one/$package" "$work/two/$package"
dpkg-deb -x "$work/one/$package" "$work/extracted"
dpkg-deb -e "$work/one/$package" "$work/control"
cmp "$fixture" "$work/extracted/usr/lib/podmesh-manager/podmesh-managerd"
test ! -e "$work/extracted/usr/bin/podmeshd"
test ! -e "$work/extracted/usr/bin/podmesh"
test ! -e "$work/extracted/usr/lib/systemd/system/podmesh.service"
test ! -e "$work/extracted/usr/lib/systemd/system/podmesh-web-observer.service"
grep -q '^User=podmesh-manager$' "$work/extracted/usr/lib/systemd/system/podmesh-manager.service"
grep -q '^ConditionPathExists=/etc/podmesh-manager/config.json$' "$work/extracted/usr/lib/systemd/system/podmesh-manager.service"
grep -q '^Environment=PODMESH_MANAGER_NETWORK_MODE=disabled$' "$work/extracted/usr/lib/systemd/system/podmesh-manager.service"
grep -q '^RuntimeDirectoryMode=0700$' "$work/extracted/usr/lib/systemd/system/podmesh-manager.service"
grep -q '^Restart=no$' "$work/extracted/usr/lib/systemd/system/podmesh-manager.service"
grep -q '^IPAddressDeny=any$' "$work/extracted/usr/lib/systemd/system/podmesh-manager.service"
grep -q '^RestrictAddressFamilies=AF_UNIX$' "$work/extracted/usr/lib/systemd/system/podmesh-manager.service"
grep -q '^Package: podmesh-manager$' "$work/control/control"
python3 - "$work/extracted/usr/share/podmesh-manager/config.example.json" <<'PY'
import json, sys
config = json.load(open(sys.argv[1], encoding="utf-8"))
writer_uid = config["observation_writer_uid"]
assert type(writer_uid) is int and writer_uid == 0
assert config["control_socket"] == "/run/podmesh-manager/control.sock"
assert config["network"]["manager"]["grants"] == []
PY
test ! -e "$work/control/preinst"
cat > "$work/expected-postinst" <<'SCRIPT'
#!/bin/sh
set -e

if [ "$1" = configure ]; then
  if ! getent passwd podmesh-manager >/dev/null; then
    adduser --system --group --home /nonexistent --no-create-home \
      --shell /usr/sbin/nologin podmesh-manager
  fi
  install -d -m 0750 -o root -g podmesh-manager /etc/podmesh-manager
  install -d -m 0750 -o podmesh-manager -g podmesh-manager /var/lib/podmesh-manager
  if [ -d /run/systemd/system ]; then systemctl daemon-reload; fi
fi
SCRIPT
cat > "$work/expected-prerm" <<'SCRIPT'
#!/bin/sh
set -e

if [ "$1" = remove ] && [ -d /run/systemd/system ]; then
  systemctl stop podmesh-manager.service || true
  systemctl disable podmesh-manager.service || true
fi
SCRIPT
cat > "$work/expected-postrm" <<'SCRIPT'
#!/bin/sh
set -e

if [ -d /run/systemd/system ]; then systemctl daemon-reload; fi
SCRIPT
for script in postinst prerm postrm; do
  expected="$work/expected-$script"
  cmp "$expected" "$source_dir/$script"
  cmp "$expected" "$work/control/$script"
  sh -n "$source_dir/$script"
  sh -n "$work/control/$script"
  mutated="$work/mutated-$script"
  cp "$source_dir/$script" "$mutated"
  printf '\n: adversarial-extra-command\n' >> "$mutated"
  if cmp -s "$expected" "$mutated"; then
    echo "Policy oracle accepted an appended $script command" >&2
    exit 1
  fi
done
cmp "$source_dir/../../LICENSE" "$work/extracted/usr/share/doc/podmesh-manager/copyright"
cmp "$source_dir/../../NOTICE" "$work/extracted/usr/share/doc/podmesh-manager/NOTICE"
glibc_symbol=$(LC_ALL=C readelf --version-info "$fixture" | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -Vu | tail -n 1)
[ -n "$glibc_symbol" ] || { echo 'Fixture has no dynamic GLIBC symbol' >&2; exit 1; }
test "$(cat "$work/extracted/usr/share/doc/podmesh-manager/binary.glibc-max")" = "${glibc_symbol#GLIBC_}"
if "$source_dir/build-deb.sh" "$fixture" "$(printf '0%.0s' {1..64})" '0.0.0~fixture' "$work/rejected"; then
  echo 'Checksum mismatch was accepted' >&2
  exit 1
fi
mkdir "$work/newer-glibc"
install -m755 "$fixture" "$work/newer-glibc/podmesh-managerd"
replacement=GLIBC_2.99
sed -i "0,/$glibc_symbol/s//$replacement/" "$work/newer-glibc/podmesh-managerd"
newer_hash=$(sha256sum "$work/newer-glibc/podmesh-managerd")
if "$source_dir/build-deb.sh" "$work/newer-glibc/podmesh-managerd" "${newer_hash%% *}" '0.0.0~fixture' "$work/rejected-glibc"; then
  echo 'Unsupported GLIBC requirement was accepted' >&2
  exit 1
fi
printf '%s\n' 'PASS: deterministic assembly, isolated inactive network boundary, maintainer-script syntax, license/notice inclusion, GLIBC baseline and checksum rejection.'
