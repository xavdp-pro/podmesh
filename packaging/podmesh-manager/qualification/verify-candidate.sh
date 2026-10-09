#!/bin/bash
# Offline candidate verification. It never downloads, installs, or starts anything.
set -euo pipefail

usage() {
  echo "Usage: $0 --deb <candidate.deb> --contract <reviewed.json> --inrelease <InRelease> --keyring <trusted.gpg> --packages <Packages> --packages-path <signed/relative/Packages> --output <report.json>" >&2
  exit 2
}

deb=
contract=
inrelease=
keyring=
packages=
packages_path=
output=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --deb) deb=${2-}; shift 2 ;;
    --contract) contract=${2-}; shift 2 ;;
    --inrelease) inrelease=${2-}; shift 2 ;;
    --keyring) keyring=${2-}; shift 2 ;;
    --packages) packages=${2-}; shift 2 ;;
    --packages-path) packages_path=${2-}; shift 2 ;;
    --output) output=${2-}; shift 2 ;;
    *) usage ;;
  esac
done
for file in "$deb" "$contract" "$inrelease" "$keyring" "$packages"; do [ -f "$file" ] || { echo "Missing file: $file" >&2; exit 2; }; done
[ -n "$packages_path" ] && [ -n "$output" ] || usage
command -v jq >/dev/null || { echo "jq is required" >&2; exit 2; }
command -v gpgv >/dev/null || { echo "gpgv is required" >&2; exit 2; }
command -v realpath >/dev/null || { echo "realpath is required" >&2; exit 2; }

work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
jq -e '
  (keys | sort) == (["architecture","binary_sha256","deb_sha256","expected_files","expected_maintainer_scripts","expected_regular_payload_files","package","schema_version","signing_fingerprint","source_commit","version"] | sort) and
  .schema_version == "podmesh-manager-candidate-contract/v2" and
  (.package|type == "string") and (.version|type == "string") and (.architecture == "amd64") and
  (.deb_sha256|test("^[a-f0-9]{64}$")) and (.binary_sha256|test("^[a-f0-9]{64}$")) and
  (.signing_fingerprint|test("^[A-F0-9]{40}$")) and
  (.source_commit|type == "string") and
  (.expected_files|type == "array" and length > 0 and all(.[]; type == "string" and startswith("/")) and . == (sort | unique)) and
  (.expected_regular_payload_files|type == "array" and length > 0 and
    all(.[]; (keys | sort) == ["path","sha256"] and (.path|type == "string" and startswith("/")) and (.sha256|test("^[a-f0-9]{64}$"))) and
    (map(.path) == (map(.path) | sort | unique))) and
  (.expected_maintainer_scripts|type == "array" and
    all(.[]; (keys | sort) == ["name","sha256"] and (.name|test("^(config|postinst|postrm|preinst|prerm)$")) and (.sha256|test("^[a-f0-9]{64}$"))) and
    (map(.name) == (map(.name) | sort | unique)))
' "$contract" >/dev/null || { echo "Invalid reviewed candidate contract" >&2; exit 2; }

keyring=$(realpath -e -- "$keyring")
expected_fingerprint=$(jq -r .signing_fingerprint "$contract")
gpg_status="$work/gpgv.status"
release="$work/Release"
if ! gpgv --status-fd 3 --output "$release" --keyring "$keyring" "$inrelease" 3>"$gpg_status" >/dev/null; then
  echo "InRelease signature verification failed" >&2
  exit 2
fi
awk -v expected="$expected_fingerprint" '
  $1 != "[GNUPG:]" { next }
  $2 == "GOODSIG" { good_count++; good_signer=$3; next }
  $2 == "VALIDSIG" { valid_count++; signer_fingerprint=$3; primary_fingerprint=$12; next }
  $2 == "BADSIG" || $2 == "ERRSIG" || $2 == "EXPSIG" || $2 == "EXPKEYSIG" ||
  $2 == "REVKEYSIG" || $2 == "KEYEXPIRED" || $2 == "SIGEXPIRED" ||
  $2 == "KEYREVOKED" || $2 == "NO_PUBKEY" || $2 == "NODATA" ||
  $2 == "UNEXPECTED" || $2 == "TRUNCATED" || $2 == "BADARMOR" ||
  $2 == "ERROR" || $2 == "FAILURE" { rejected=1 }
  END {
    signer_keyid=substr(signer_fingerprint, length(signer_fingerprint)-15)
    good_matches_signer=(good_signer == signer_fingerprint || good_signer == signer_keyid)
    valid_matches_primary=(primary_fingerprint == expected)
    exit !(good_count == 1 && valid_count == 1 && !rejected && good_matches_signer && valid_matches_primary)
  }
' "$gpg_status" || { echo "InRelease signer does not match exactly one valid signature from the reviewed primary fingerprint" >&2; exit 2; }
packages_sha=$(sha256sum -- "$packages" | awk '{print $1}')
packages_size=$(wc -c < "$packages" | tr -d '[:space:]')
awk -v path="$packages_path" -v sha="$packages_sha" -v size="$packages_size" '
  $1 == "SHA256:" { section=1; next }
  section && /^[^[:space:]]/ { exit }
  section && $1 == sha && $2 == size && $3 == path { found=1 }
  END { exit found ? 0 : 1 }
' "$release" || { echo "Packages file hash is not covered by the verified Release metadata" >&2; exit 2; }

expected_package=$(jq -r .package "$contract")
expected_version=$(jq -r .version "$contract")
expected_architecture=$(jq -r .architecture "$contract")
expected_deb_sha=$(jq -r .deb_sha256 "$contract")
expected_binary_sha=$(jq -r .binary_sha256 "$contract")
actual_deb_sha=$(sha256sum -- "$deb" | awk '{print $1}')
[ "$actual_deb_sha" = "$expected_deb_sha" ] || { echo "Candidate .deb SHA-256 differs from reviewed contract" >&2; exit 2; }

actual_package=$(dpkg-deb -f "$deb" Package)
actual_version=$(dpkg-deb -f "$deb" Version)
actual_architecture=$(dpkg-deb -f "$deb" Architecture)
[ "$actual_package" = "$expected_package" ] && [ "$actual_version" = "$expected_version" ] && [ "$actual_architecture" = "$expected_architecture" ] || { echo "Candidate package identity differs from reviewed contract" >&2; exit 2; }

stanza_sha=$(awk -v package="$actual_package" -v version="$actual_version" -v architecture="$actual_architecture" '
  BEGIN { RS=""; FS="\n" }
  { p=v=a=s=""; for (i=1; i<=NF; i++) { if ($i ~ /^Package: /) p=substr($i,10); if ($i ~ /^Version: /) v=substr($i,10); if ($i ~ /^Architecture: /) a=substr($i,15); if ($i ~ /^SHA256: /) s=substr($i,9) } if (p==package && v==version && a==architecture) { print s; exit } }
' "$packages")
[ -n "$stanza_sha" ] && [ "$stanza_sha" = "$actual_deb_sha" ] || { echo "Signed Packages entry does not match candidate .deb" >&2; exit 2; }

dpkg-deb -x "$deb" "$work/extracted"
dpkg-deb -e "$deb" "$work/control"
binary="$work/extracted/usr/lib/podmesh-manager/podmesh-managerd"
[ -f "$binary" ] || { echo "Candidate lacks manager binary" >&2; exit 2; }
actual_binary_sha=$(sha256sum -- "$binary" | awk '{print $1}')
[ "$actual_binary_sha" = "$expected_binary_sha" ] || { echo "Embedded binary SHA-256 differs from reviewed contract" >&2; exit 2; }
dpkg-deb -c "$deb" | awk '{print $6}' | sed 's#^\./#/#' | LC_ALL=C sort -u > "$work/actual-files"
jq -r '.expected_files[]' "$contract" | LC_ALL=C sort -u > "$work/expected-files"
cmp "$work/expected-files" "$work/actual-files" || { echo "Candidate file list differs from reviewed contract" >&2; exit 2; }

printf '%s\n' '[]' > "$work/regular-payload-files.json"
while IFS= read -r -d '' payload_file; do
  payload_path=/${payload_file#"$work/extracted/"}
  payload_sha=$(sha256sum -- "$payload_file" | awk '{print $1}')
  jq --arg path "$payload_path" --arg sha256 "$payload_sha" \
    '. + [{path:$path,sha256:$sha256}]' "$work/regular-payload-files.json" > "$work/manifest.next"
  mv -- "$work/manifest.next" "$work/regular-payload-files.json"
done < <(find "$work/extracted" -type f -print0 | LC_ALL=C sort -z)

printf '%s\n' '[]' > "$work/maintainer-scripts.json"
for script_name in config postinst postrm preinst prerm; do
  script_path="$work/control/$script_name"
  if [ -e "$script_path" ] || [ -L "$script_path" ]; then
    [ -f "$script_path" ] && [ ! -L "$script_path" ] || { echo "Candidate maintainer script is not a regular file: $script_name" >&2; exit 2; }
    script_sha=$(sha256sum -- "$script_path" | awk '{print $1}')
    jq --arg name "$script_name" --arg sha256 "$script_sha" \
      '. + [{name:$name,sha256:$sha256}]' "$work/maintainer-scripts.json" > "$work/manifest.next"
    mv -- "$work/manifest.next" "$work/maintainer-scripts.json"
  fi
done

jq -e --slurpfile payload "$work/regular-payload-files.json" --slurpfile scripts "$work/maintainer-scripts.json" \
  '.expected_regular_payload_files == $payload[0] and .expected_maintainer_scripts == $scripts[0]' \
  "$contract" >/dev/null || { echo "Candidate content hashes differ from reviewed contract" >&2; exit 2; }

mkdir -p -- "$(dirname -- "$output")"
jq -n --arg verified_at_utc "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --arg package "$actual_package" --arg version "$actual_version" --arg architecture "$actual_architecture" --arg deb_sha256 "$actual_deb_sha" --arg binary_sha256 "$actual_binary_sha" --arg source_commit "$(jq -r .source_commit "$contract")" --arg packages_sha256 "$packages_sha" --arg packages_size "$packages_size" --arg packages_path "$packages_path" --arg signing_fingerprint "$expected_fingerprint" --arg keyring_sha256 "$(sha256sum -- "$keyring" | awk '{print $1}')" --slurpfile payload "$work/regular-payload-files.json" --slurpfile scripts "$work/maintainer-scripts.json" '
  {schema_version:"podmesh-manager-candidate-verification/v2",verified_at_utc:$verified_at_utc,package:$package,version:$version,architecture:$architecture,deb_sha256:$deb_sha256,binary_sha256:$binary_sha256,source_commit:$source_commit,signed_metadata:{inrelease_signature:"verified-by-gpgv-and-pinned-fingerprint",signing_fingerprint:$signing_fingerprint,keyring_sha256:$keyring_sha256,packages_path:$packages_path,packages_sha256:$packages_sha256,packages_size:($packages_size|tonumber)},regular_payload_files:$payload[0],maintainer_scripts:$scripts[0]}
' | jq -S . > "$output"
sha256sum -- "$output" > "$output.sha256"
