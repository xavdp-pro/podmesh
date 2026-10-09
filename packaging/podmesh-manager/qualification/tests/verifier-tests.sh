#!/bin/bash
# End-to-end offline candidate verifier test. It creates only throwaway keys and fixtures.
set -euo pipefail
export LC_ALL=C

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
packaging=$(cd -- "$root/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

for command in dpkg-deb gpg gpgv jq readelf realpath sha256sum; do
  command -v "$command" >/dev/null || { echo "Missing test dependency: $command" >&2; exit 2; }
done

export GNUPGHOME="$work/gnupg"
mkdir -m 700 "$GNUPGHOME"
gpg --batch --quiet --passphrase '' --quick-generate-key \
  'PodMesh verifier fixture <verifier@example.invalid>' rsa2048 sign 0
fingerprint=$(gpg --batch --with-colons --list-keys | awk -F: '$1 == "fpr" { print $10; exit }')
[[ "$fingerprint" =~ ^[A-F0-9]{40}$ ]] || { echo "Fixture signing fingerprint is invalid" >&2; exit 1; }
gpg --batch --quiet --export "$fingerprint" > "$work/trusted.gpg"

mkdir "$work/fixture"
install -m755 /usr/bin/true "$work/fixture/podmesh-managerd"
binary="$work/fixture/podmesh-managerd"
binary_sha=$(sha256sum -- "$binary" | awk '{print $1}')
export SOURCE_DATE_EPOCH=1700000000
version='0.0.0~verifier-fixture'
"$packaging/build-deb.sh" "$binary" "$binary_sha" "$version" "$work/package"
deb="$work/package/podmesh-manager_${version}_amd64.deb"
deb_sha=$(sha256sum -- "$deb" | awk '{print $1}')
deb_size=$(wc -c < "$deb" | tr -d '[:space:]')
packages_path='dists/fixture/main/binary-amd64/Packages'

write_packages() {
  local stanza_sha=$1
  local destination=$2
  cat > "$destination" <<EOF
Package: podmesh-manager
Version: $version
Architecture: amd64
Filename: pool/main/p/podmesh-manager/$(basename -- "$deb")
Size: $deb_size
SHA256: $stanza_sha
Description: PodMesh verifier fixture
EOF
}

sign_release() {
  local packages_file=$1
  local destination=$2
  local packages_sha packages_size
  packages_sha=$(sha256sum -- "$packages_file" | awk '{print $1}')
  packages_size=$(wc -c < "$packages_file" | tr -d '[:space:]')
  cat > "$work/Release" <<EOF
Origin: PodMesh verifier fixture
Suite: fixture
SHA256:
 $packages_sha $packages_size $packages_path
EOF
  gpg --batch --quiet --yes --local-user "$fingerprint" --digest-algo SHA256 \
    --clearsign --output "$destination" "$work/Release"
}

expect_rejection() {
  local description=$1
  shift
  if "$root/verify-candidate.sh" "$@" >/dev/null 2>"$work/rejection.stderr"; then
    echo "Verifier accepted $description" >&2
    exit 1
  fi
}

write_packages "$deb_sha" "$work/Packages"
sign_release "$work/Packages" "$work/InRelease"
dpkg-deb -c "$deb" | awk '{print $6}' | sed 's#^\./#/#' | LC_ALL=C sort -u > "$work/expected-files"
dpkg-deb -x "$deb" "$work/extracted"
dpkg-deb -e "$deb" "$work/control"
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
  if [ -f "$work/control/$script_name" ] && [ ! -L "$work/control/$script_name" ]; then
    script_sha=$(sha256sum -- "$work/control/$script_name" | awk '{print $1}')
    jq --arg name "$script_name" --arg sha256 "$script_sha" \
      '. + [{name:$name,sha256:$sha256}]' "$work/maintainer-scripts.json" > "$work/manifest.next"
    mv -- "$work/manifest.next" "$work/maintainer-scripts.json"
  fi
done
jq -n \
  --arg package podmesh-manager \
  --arg version "$version" \
  --arg architecture amd64 \
  --arg deb_sha256 "$deb_sha" \
  --arg binary_sha256 "$binary_sha" \
  --arg signing_fingerprint "$fingerprint" \
  --arg source_commit verifier-test-fixture \
  --rawfile expected_files "$work/expected-files" \
  --slurpfile expected_regular_payload_files "$work/regular-payload-files.json" \
  --slurpfile expected_maintainer_scripts "$work/maintainer-scripts.json" '
    {
      schema_version:"podmesh-manager-candidate-contract/v2",
      package:$package,
      version:$version,
      architecture:$architecture,
      deb_sha256:$deb_sha256,
      binary_sha256:$binary_sha256,
      signing_fingerprint:$signing_fingerprint,
      source_commit:$source_commit,
      expected_files:($expected_files | split("\n") | map(select(length > 0))),
      expected_regular_payload_files:$expected_regular_payload_files[0],
      expected_maintainer_scripts:$expected_maintainer_scripts[0]
    }
  ' > "$work/contract.json"

common=(
  --deb "$deb"
  --inrelease "$work/InRelease"
  --keyring "$work/trusted.gpg"
  --packages "$work/Packages"
  --packages-path "$packages_path"
)
"$root/verify-candidate.sh" "${common[@]}" \
  --contract "$work/contract.json" --output "$work/verification.json"
jq -e \
  --arg fingerprint "$fingerprint" \
  --arg deb_sha "$deb_sha" \
  '.schema_version == "podmesh-manager-candidate-verification/v2" and
   .deb_sha256 == $deb_sha and
   .signed_metadata.signing_fingerprint == $fingerprint and
   .signed_metadata.inrelease_signature == "verified-by-gpgv-and-pinned-fingerprint" and
   .regular_payload_files == $payload[0] and
   .maintainer_scripts == $scripts[0]' \
  --slurpfile payload "$work/regular-payload-files.json" \
  --slurpfile scripts "$work/maintainer-scripts.json" \
  "$work/verification.json" >/dev/null
sha256sum --check "$work/verification.json.sha256" >/dev/null

gpg --batch --quiet --passphrase '' --quick-generate-key \
  'PodMesh verifier second fixture <verifier-second@example.invalid>' rsa2048 sign 0
second_fingerprint=$(gpg --batch --with-colons --list-keys \
  'PodMesh verifier second fixture' | awk -F: '$1 == "fpr" { print $10; exit }')
gpg --batch --quiet --export "$fingerprint" "$second_fingerprint" > "$work/two-signers.gpg"
gpg --batch --quiet --yes --local-user "$fingerprint" --local-user "$second_fingerprint" \
  --digest-algo SHA256 --clearsign --output "$work/two-signatures-InRelease" "$work/Release"
expect_rejection 'two otherwise valid InRelease signatures' \
  --deb "$deb" --contract "$work/contract.json" --inrelease "$work/two-signatures-InRelease" \
  --keyring "$work/two-signers.gpg" --packages "$work/Packages" \
  --packages-path "$packages_path" --output "$work/two-signatures-report.json"

jq '.signing_fingerprint = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"' \
  "$work/contract.json" > "$work/wrong-fingerprint.json"
expect_rejection 'a wrong reviewed signing fingerprint' "${common[@]}" \
  --contract "$work/wrong-fingerprint.json" --output "$work/wrong-fingerprint-report.json"

cp "$work/Packages" "$work/wrong-hash-Packages"
printf '\n' >> "$work/wrong-hash-Packages"
expect_rejection 'a Packages file whose hash is absent from the signed Release' \
  --deb "$deb" --contract "$work/contract.json" --inrelease "$work/InRelease" \
  --keyring "$work/trusted.gpg" --packages "$work/wrong-hash-Packages" \
  --packages-path "$packages_path" --output "$work/wrong-packages-hash-report.json"

wrong_stanza_sha=$(printf '0%.0s' {1..64})
write_packages "$wrong_stanza_sha" "$work/wrong-stanza-Packages"
sign_release "$work/wrong-stanza-Packages" "$work/wrong-stanza-InRelease"
expect_rejection 'a signed Packages stanza with the wrong candidate hash' \
  --deb "$deb" --contract "$work/contract.json" --inrelease "$work/wrong-stanza-InRelease" \
  --keyring "$work/trusted.gpg" --packages "$work/wrong-stanza-Packages" \
  --packages-path "$packages_path" --output "$work/wrong-stanza-report.json"

extra_candidate_file='/usr/share/podmesh-manager/config.example.json'
jq -e --arg path "$extra_candidate_file" '.expected_files | index($path) != null' \
  "$work/contract.json" >/dev/null
jq --arg path "$extra_candidate_file" '.expected_files |= map(select(. != $path))' \
  "$work/contract.json" > "$work/missing-file.json"
expect_rejection 'a candidate containing a file absent from the reviewed contract' "${common[@]}" \
  --contract "$work/missing-file.json" --output "$work/extra-file-report.json"

jq '(.expected_regular_payload_files[] | select(.path == "/usr/lib/systemd/system/podmesh-manager.service") | .sha256) = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"' \
  "$work/contract.json" > "$work/changed-payload-hash.json"
expect_rejection 'a changed regular payload hash' "${common[@]}" \
  --contract "$work/changed-payload-hash.json" --output "$work/changed-payload-hash-report.json"

jq '(.expected_maintainer_scripts[] | select(.name == "postinst") | .sha256) = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"' \
  "$work/contract.json" > "$work/changed-postinst-hash.json"
expect_rejection 'a changed postinst hash' "${common[@]}" \
  --contract "$work/changed-postinst-hash.json" --output "$work/changed-postinst-hash-report.json"

bash -n "$root/verify-candidate.sh" "$0"
printf '%s\n' 'PASS: signed candidate verification plus signature, repository metadata, file-list, payload-hash and maintainer-script rejection.'
