#!/bin/bash
# Assembly-only test: /usr/bin/true is an ELF fixture, never a deployable observer.
set -euo pipefail
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
source_dir=$(cd -- "$(dirname -- "$0")" && pwd)
fixture=/usr/bin/true
hash=$(sha256sum "$fixture")
export SOURCE_DATE_EPOCH=1700000000
"$source_dir/build-deb.sh" "$fixture" "${hash%% *}" '0.0.0~fixture' "$work/one"
"$source_dir/build-deb.sh" "$fixture" "${hash%% *}" '0.0.0~fixture' "$work/two"
package=podmesh-web-observer_0.0.0~fixture_amd64.deb
cmp "$work/one/$package" "$work/two/$package"
dpkg-deb -x "$work/one/$package" "$work/extracted"
dpkg-deb -e "$work/one/$package" "$work/control"
cmp "$fixture" "$work/extracted/usr/lib/podmesh-web-observer/podmeshd"
test ! -e "$work/extracted/usr/bin/podmeshd"
grep -q '^Environment=PODMESH_READ_ONLY=1$' "$work/extracted/usr/lib/systemd/system/podmesh-web-observer.service"
for script in postinst prerm postrm; do sh -n "$work/control/$script"; done
if "$source_dir/build-deb.sh" "$fixture" "$(printf '0%.0s' {1..64})" '0.0.0~fixture' "$work/rejected"; then
 echo 'Checksum mismatch was accepted' >&2; exit 1
fi
printf '%s\n' 'PASS: deterministic fixture assembly, isolated paths, read-only environment, script syntax and checksum rejection.'
