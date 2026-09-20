#!/bin/bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/.." && pwd)
base=$(cd -- "$root/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

make_candidate() {
  local version=$1 hash=$2 prefix=$3
  jq -n --arg version "$version" --arg hash "$hash" --arg source "$prefix-source" '{
    schema_version:"podmesh-manager-candidate-verification/v2",verified_at_utc:"2026-09-13T10:00:00Z",
    package:"podmesh-manager",version:$version,architecture:"amd64",deb_sha256:$hash,binary_sha256:$hash,source_commit:$source,
    signed_metadata:{inrelease_signature:"verified-by-gpgv-and-pinned-fingerprint",signing_fingerprint:"0000000000000000000000000000000000000000",keyring_sha256:$hash,packages_path:"pool/Packages",packages_sha256:$hash,packages_size:123},
    regular_payload_files:[{path:"/usr/lib/podmesh-manager/podmesh-managerd",sha256:$hash}],maintainer_scripts:[]
  }' > "$work/$prefix-report.json"
  jq '{schema_version:"podmesh-manager-candidate-contract/v2",package,version,architecture,deb_sha256,binary_sha256,signing_fingerprint:.signed_metadata.signing_fingerprint,source_commit,expected_files:["/usr/lib/podmesh-manager/podmesh-managerd"],expected_regular_payload_files:.regular_payload_files,expected_maintainer_scripts:.maintainer_scripts}' \
    "$work/$prefix-report.json" > "$work/$prefix-contract.json"
}

make_candidate 0.1 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" old
make_candidate 0.2 "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff" new

binding() {
  local report=$1 payload scripts verification
  payload="sha256:$(jq -cS .regular_payload_files "$report" | sha256sum | awk '{print $1}')"
  scripts="sha256:$(jq -cS .maintainer_scripts "$report" | sha256sum | awk '{print $1}')"
  verification="sha256:$(sha256sum -- "$report" | awk '{print $1}')"
  jq -cn --arg version "$(jq -r .version "$report")" --arg binary "$(jq -r .binary_sha256 "$report")" --arg payload "$payload" --arg scripts "$scripts" --arg verification "$verification" \
    '{package:"podmesh-manager",version:$version,binary_sha256:$binary,regular_payload_files_commitment:$payload,maintainer_scripts_commitment:$scripts,dpkg_verify:"clean",verification_commitment:$verification}'
}

old_binding=$(binding "$work/old-report.json")
new_binding=$(binding "$work/new-report.json")

make_upgrade() {
  local alias=$1 stage=$2 version=$3 candidate_binding=$4 output=$5
  jq --arg alias "$alias" --arg version "$version" --argjson binding "$candidate_binding" '
    .stage="post-install" | .host_alias=$alias |
    .packages["podmesh-manager"]={status:"installed",version:$version} |
    .manager.config_present=true |
    .manager.state_directory.entry_count=1 |
    .manager.candidate_binding=$binding
  ' "$base/tests/fixtures/post-install.json" > "$work/host.json"
  jq -n --arg stage "$stage" --slurpfile host "$work/host.json" '{
    schema_version:"podmesh-manager-inactive-upgrade-evidence/v1",stage:$stage,host_evidence:$host[0],
    config_commitment:"sha256:1111111111111111111111111111111111111111111111111111111111111111",
    state_commitment:"sha256:2222222222222222222222222222222222222222222222222222222222222222"
  }' > "$output"
}

for alias in lab-a lab-b lab-c; do
  make_upgrade "$alias" pre-upgrade 0.1 "$old_binding" "$work/$alias-pre.json"
  make_upgrade "$alias" post-upgrade 0.2 "$new_binding" "$work/$alias-post.json"
done

python3 - "$root/evidence-schema.json" "$work/lab-a-pre.json" "$work/lab-a-post.json" <<'PY'
import json
import sys
from pathlib import Path
from jsonschema import Draft202012Validator, FormatChecker
schema = json.loads(Path(sys.argv[1]).read_text())
validator = Draft202012Validator(schema, format_checker=FormatChecker())
for path in sys.argv[2:]:
    errors = list(validator.iter_errors(json.loads(Path(path).read_text())))
    if errors:
        raise SystemExit(f"schema rejected {path}: {errors[0].message}")
PY

compare_one() {
  python3 "$root/compare-evidence.py" --phase upgrade --pre "$1" --post "$2" \
    --old-candidate-verification "$work/old-report.json" --old-contract "$work/old-contract.json" \
    --new-candidate-verification "$work/new-report.json" --new-contract "$work/new-contract.json"
}

compare_one "$work/lab-a-pre.json" "$work/lab-a-post.json" > "$work/pass.json"
jq -e '.status=="PASS" and .failures==[]' "$work/pass.json" >/dev/null

python3 "$root/compare-evidence.py" --phase three-host-upgrade \
  --pre "$work/lab-a-pre.json" "$work/lab-b-pre.json" "$work/lab-c-pre.json" \
  --post "$work/lab-a-post.json" "$work/lab-b-post.json" "$work/lab-c-post.json" \
  --old-candidate-verification "$work/old-report.json" --old-contract "$work/old-contract.json" \
  --new-candidate-verification "$work/new-report.json" --new-contract "$work/new-contract.json" > "$work/three-host.json"
jq -e '.status=="PASS" and .failures==[]' "$work/three-host.json" >/dev/null

reject_change() {
  local description=$1 filter=$2 expected=$3
  jq "$filter" "$work/lab-a-post.json" > "$work/changed.json"
  if compare_one "$work/lab-a-pre.json" "$work/changed.json" > "$work/rejected.json"; then
    echo "inactive-upgrade comparison accepted $description" >&2
    exit 1
  fi
  jq -e --arg expected "$expected" '.failures | index($expected) != null' "$work/rejected.json" >/dev/null
}

reject_change "a configuration mutation" '.config_commitment="sha256:3333333333333333333333333333333333333333333333333333333333333333"' "protected manager configuration changed during package upgrade"
reject_change "a state mutation" '.state_commitment="sha256:3333333333333333333333333333333333333333333333333333333333333333"' "manager state content changed during package upgrade"
reject_change "a lifecycle restart" '.host_evidence.services["podmesh.service"].main_pid=303' "podmesh.service state, PID, invocation or restart evidence changed during upgrade"
reject_change "a manager invocation" '.host_evidence.services["podmesh-manager.service"].invocation_id="sha256:3333333333333333333333333333333333333333333333333333333333333333"' "post-upgrade manager unit is not proven disabled, inactive and free of retained invocation, start-timestamp or restart records"
reject_change "a host reboot" '.host_evidence.boot_id_commitment="sha256:3333333333333333333333333333333333333333333333333333333333333333"' "host boot changed during upgrade"
reject_change "an account mutation" '.host_evidence.manager.account.uid=995' "manager account identity changed during upgrade"
reject_change "a Podman mutation" '.host_evidence.podman_rootful.commitments.containers="sha256:3333333333333333333333333333333333333333333333333333333333333333"' "rootful Podman commitment changed during upgrade"

jq '.host_evidence.manager.candidate_binding.verification_commitment="sha256:3333333333333333333333333333333333333333333333333333333333333333"' "$work/lab-a-pre.json" > "$work/wrong-old.json"
if compare_one "$work/wrong-old.json" "$work/lab-a-post.json" > "$work/wrong-old-result.json"; then
  echo "inactive-upgrade comparison accepted an unbound old candidate" >&2
  exit 1
fi
jq -e '.failures | index("pre-upgrade host does not bind the reviewed old candidate") != null' "$work/wrong-old-result.json" >/dev/null

# Debian ordering, including epochs and tildes, decides whether this is an
# upgrade. Different strings alone must never allow a downgrade.
python3 - "$root/compare-evidence.py" <<'PY'
import importlib.util
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("upgrade_comparator", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
assert module.is_strict_debian_upgrade("1:1.0~rc1-1", "1:1.0-1")
assert not module.is_strict_debian_upgrade("1:1.0-1", "1:1.0~rc1-1")
assert not module.is_strict_debian_upgrade("1.0-1", "1.0-1")
PY

if python3 "$root/compare-evidence.py" --phase three-host-upgrade \
  --pre "$work/lab-a-pre.json" "$work/lab-b-pre.json" "$work/lab-b-pre.json" \
  --post "$work/lab-a-post.json" "$work/lab-b-post.json" "$work/lab-c-post.json" \
  --old-candidate-verification "$work/old-report.json" --old-contract "$work/old-contract.json" \
  --new-candidate-verification "$work/new-report.json" --new-contract "$work/new-contract.json" > "$work/duplicate.json"; then
  echo "three-host inactive-upgrade comparison accepted duplicate aliases" >&2
  exit 1
fi
jq -e '.failures | index("host aliases are not distinct") != null' "$work/duplicate.json" >/dev/null

if command -v shellcheck >/dev/null; then
  shellcheck "$root/collect-host.sh"
fi
bash -n "$root/collect-host.sh"
printf '%s\n' 'PASS: inactive-upgrade schema, old/new candidate binding, three-host pairing and preservation failures.'
