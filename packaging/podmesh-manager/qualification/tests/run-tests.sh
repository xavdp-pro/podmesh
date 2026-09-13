#!/bin/bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

python3 - "$root/evidence-schema.json" "$root/tests/fixtures"/*.json <<'PY'
import json
import sys
from copy import deepcopy
from pathlib import Path
from jsonschema import Draft202012Validator, FormatChecker
schema = json.loads(Path(sys.argv[1]).read_text())
validator = Draft202012Validator(schema, format_checker=FormatChecker())
fixtures = {}
for name in sys.argv[2:]:
    evidence = json.loads(Path(name).read_text())
    fixtures[Path(name).name] = evidence
    errors = sorted(validator.iter_errors(evidence), key=str)
    if errors:
        raise SystemExit(f"schema rejected {name}: {errors[0].message}")

negative_cases = []
missing_shell = deepcopy(fixtures["post-install.json"])
del missing_shell["manager"]["account"]["shell"]
negative_cases.append(("missing account shell", missing_shell))
invalid_mode = deepcopy(fixtures["post-install.json"])
invalid_mode["manager"]["state_directory"]["mode"] = "75A"
negative_cases.append(("invalid state directory mode", invalid_mode))
implicit_account_absence = deepcopy(fixtures["pre-install.json"])
implicit_account_absence["manager"]["account"]["uid"] = 994
negative_cases.append(("implicit account absence", implicit_account_absence))
implicit_state_absence = deepcopy(fixtures["pre-install.json"])
implicit_state_absence["manager"]["state_directory"]["owner"] = "podmesh-manager"
negative_cases.append(("implicit state directory absence", implicit_state_absence))
for description, evidence in negative_cases:
    if not list(validator.iter_errors(evidence)):
        raise SystemExit(f"schema accepted {description}")
PY

cat > "$work/candidate-contract.json" <<'JSON'
{"schema_version":"podmesh-manager-candidate-contract/v2","package":"podmesh-manager","version":"0.1","architecture":"amd64","deb_sha256":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","binary_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","signing_fingerprint":"0000000000000000000000000000000000000000","source_commit":"deadbeef","expected_files":["/usr/lib/podmesh-manager/podmesh-managerd"],"expected_regular_payload_files":[{"path":"/usr/lib/podmesh-manager/podmesh-managerd","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}],"expected_maintainer_scripts":[]}
JSON
cat > "$work/candidate-verification.json" <<'JSON'
{"schema_version":"podmesh-manager-candidate-verification/v2","verified_at_utc":"2026-09-12T10:01:00Z","package":"podmesh-manager","version":"0.1","architecture":"amd64","deb_sha256":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","binary_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","source_commit":"deadbeef","signed_metadata":{"inrelease_signature":"verified-by-gpgv-and-pinned-fingerprint","signing_fingerprint":"0000000000000000000000000000000000000000","keyring_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","packages_path":"pool/Packages","packages_sha256":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","packages_size":123},"regular_payload_files":[{"path":"/usr/lib/podmesh-manager/podmesh-managerd","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}],"maintainer_scripts":[]}
JSON
verification_commitment="sha256:$(sha256sum -- "$work/candidate-verification.json" | awk '{print $1}')"
make_post() {
  jq --arg commitment "$verification_commitment" '
    .stage="post-install" |
    .captured_at_utc="2026-09-12T10:05:00Z" |
    .packages["podmesh-manager"]={status:"installed",version:"0.1"} |
    .services["podmesh-manager.service"]={unit:"podmesh-manager.service",load_state:"loaded",active_state:"inactive",sub_state:"dead",unit_file_state:"disabled",main_pid:0,exec_main_pid:0,invocation_id:null,start_monotonic_usec:0,n_restarts:0} |
    .manager={process_count:0,account:{name:"podmesh-manager",present:true,uid:994,primary_gid:994,primary_group:"podmesh-manager",home:"/nonexistent",shell:"/usr/sbin/nologin"},config_present:false,state_directory:{path:"/var/lib/podmesh-manager",present:true,file_type:"directory",owner:"podmesh-manager",group:"podmesh-manager",mode:"750",entry_count:0},runtime_present:false,candidate_binding:{package:"podmesh-manager",version:"0.1",binary_sha256:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",regular_payload_files_commitment:"sha256:bb2a570ebff6353a995865f31438e72fcb94d8a4c96e71d8db0da3ef73ff66ac",maintainer_scripts_commitment:"sha256:37517e5f3dc66819f61f5a7bb8ace1921282415f10551d2defa5c3eb0985b570",dpkg_verify:"clean",verification_commitment:$commitment}}
  ' "$1" > "$2"
}
make_post "$root/tests/fixtures/pre-install.json" "$work/post-install.json"
make_post "$root/tests/fixtures/pre-install-lab-b.json" "$work/post-install-lab-b.json"
make_post "$root/tests/fixtures/pre-install-lab-c.json" "$work/post-install-lab-c.json"
compare_install() {
  python3 "$root/compare-evidence.py" --phase install --pre "$1" --post "$2" --candidate-verification "$work/candidate-verification.json" --contract "$work/candidate-contract.json"
}

compare_install "$root/tests/fixtures/pre-install.json" "$work/post-install.json" > "$work/install.json"
jq -e '.status == "PASS" and .failures == []' "$work/install.json" >/dev/null
python3 "$root/compare-evidence.py" --phase three-host --pre "$root/tests/fixtures/pre-install.json" "$root/tests/fixtures/pre-install-lab-b.json" "$root/tests/fixtures/pre-install-lab-c.json" --post "$work/post-install.json" "$work/post-install-lab-b.json" "$work/post-install-lab-c.json" --candidate-verification "$work/candidate-verification.json" --contract "$work/candidate-contract.json" > "$work/three-host.json"
jq -e '.status == "PASS"' "$work/three-host.json" >/dev/null

jq '
  .packages.podmesh={status:"absent",version:null} |
  .services["podmesh.service"]={unit:"podmesh.service",load_state:"not-found",active_state:"inactive",sub_state:"dead",unit_file_state:"",main_pid:0,exec_main_pid:0,invocation_id:null,start_monotonic_usec:0,n_restarts:0} |
  .sockets["/run/podmesh/api.sock"]={path:"/run/podmesh/api.sock",present:false,file_type:null,mode:null,uid:null,gid:null,inode:null,bytes:null}
' "$root/tests/fixtures/pre-install.json" > "$work/missing-existing-pre.json"
jq '
  .packages.podmesh={status:"absent",version:null} |
  .services["podmesh.service"]={unit:"podmesh.service",load_state:"not-found",active_state:"inactive",sub_state:"dead",unit_file_state:"",main_pid:0,exec_main_pid:0,invocation_id:null,start_monotonic_usec:0,n_restarts:0} |
  .sockets["/run/podmesh/api.sock"]={path:"/run/podmesh/api.sock",present:false,file_type:null,mode:null,uid:null,gid:null,inode:null,bytes:null}
' "$work/post-install.json" > "$work/missing-existing-post.json"
if compare_install "$work/missing-existing-pre.json" "$work/missing-existing-post.json" > "$work/missing-existing-report.json"; then
  echo "comparison accepted absent pre-existing lifecycle package, unit and socket" >&2
  exit 1
fi
jq -e '.failures | index("pre-install podmesh package is not installed") != null and index("pre-install podmesh.service is not loaded and active with one proven main PID") != null and index("pre-install /run/podmesh/api.sock is not a socket") != null' "$work/missing-existing-report.json" >/dev/null

jq '.services["podmesh-manager.service"].invocation_id="sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff" | .services["podmesh-manager.service"].start_monotonic_usec=42 | .services["podmesh-manager.service"].n_restarts=1' "$work/post-install.json" > "$work/manager-ran.json"
if compare_install "$root/tests/fixtures/pre-install.json" "$work/manager-ran.json" > "$work/manager-ran-report.json"; then
  echo "comparison accepted manager systemd execution evidence" >&2
  exit 1
fi
jq -e '.failures | index("manager unit has invocation, start or restart evidence") != null' "$work/manager-ran-report.json" >/dev/null

jq '.manager.state_directory.entry_count=1' "$work/post-install.json" > "$work/nonempty-state.json"
if compare_install "$root/tests/fixtures/pre-install.json" "$work/nonempty-state.json" > "$work/nonempty-state-report.json"; then
  echo "comparison accepted non-empty manager state" >&2
  exit 1
fi
jq -e '.failures | index("manager state directory is not empty after package-only installation") != null' "$work/nonempty-state-report.json" >/dev/null

jq 'del(.services["podmesh-manager.service"].main_pid)' "$work/post-install.json" > "$work/missing-pid.json"
if compare_install "$root/tests/fixtures/pre-install.json" "$work/missing-pid.json" > "$work/missing-pid-report.json"; then
  echo "comparison accepted a missing systemd PID reading" >&2
  exit 1
fi
jq -e '.status == "FAIL" and (.error | contains("incomplete systemd reading"))' "$work/missing-pid-report.json" >/dev/null

jq '.manager.config_present=true | .manager.runtime_present=true' "$work/post-install.json" > "$work/present-manager-paths.json"
if compare_install "$root/tests/fixtures/pre-install.json" "$work/present-manager-paths.json" > "$work/present-manager-paths-report.json"; then
  echo "comparison accepted present manager config/runtime paths" >&2
  exit 1
fi
jq -e '.failures | index("manager configuration exists after package-only installation") != null and index("manager runtime directory exists after package-only installation") != null' "$work/present-manager-paths-report.json" >/dev/null

jq '.manager.candidate_binding.verification_commitment="sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"' "$work/post-install.json" > "$work/wrong-binding.json"
if compare_install "$root/tests/fixtures/pre-install.json" "$work/wrong-binding.json" > "$work/wrong-binding-report.json"; then
  echo "comparison accepted a candidate binding unrelated to the supplied report" >&2
  exit 1
fi
jq -e '.failures | index("post-install candidate binding does not match the verified report and contract") != null' "$work/wrong-binding-report.json" >/dev/null

jq '.binary_sha256="ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"' "$work/candidate-contract.json" > "$work/wrong-contract.json"
if python3 "$root/compare-evidence.py" --phase install --pre "$root/tests/fixtures/pre-install.json" --post "$work/post-install.json" --candidate-verification "$work/candidate-verification.json" --contract "$work/wrong-contract.json" > "$work/wrong-contract-report.json"; then
  echo "comparison accepted a report that differs from the reviewed contract" >&2
  exit 1
fi
jq -e '.status == "FAIL" and (.error | contains("candidate report and contract differ for binary_sha256"))' "$work/wrong-contract-report.json" >/dev/null

jq '(.expected_regular_payload_files[0].sha256)="ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"' "$work/candidate-contract.json" > "$work/wrong-payload-contract.json"
if python3 "$root/compare-evidence.py" --phase install --pre "$root/tests/fixtures/pre-install.json" --post "$work/post-install.json" --candidate-verification "$work/candidate-verification.json" --contract "$work/wrong-payload-contract.json" > "$work/wrong-payload-contract-report.json"; then
  echo "comparison accepted a report with a different reviewed payload manifest" >&2
  exit 1
fi
jq -e '.status == "FAIL" and (.error | contains("candidate report and contract differ for regular payload files"))' "$work/wrong-payload-contract-report.json" >/dev/null

jq '.schema_version="podmesh-manager-candidate-verification/v1"' "$work/candidate-verification.json" > "$work/legacy-candidate-verification.json"
if python3 "$root/compare-evidence.py" --phase install --pre "$root/tests/fixtures/pre-install.json" --post "$work/post-install.json" --candidate-verification "$work/legacy-candidate-verification.json" --contract "$work/candidate-contract.json" > "$work/legacy-candidate-report.json"; then
  echo "comparison silently accepted the legacy candidate report schema" >&2
  exit 1
fi
jq -e '.status == "FAIL" and (.error | contains("invalid candidate verification report"))' "$work/legacy-candidate-report.json" >/dev/null

jq '.schema_version="podmesh-manager-host-evidence/v4"' "$work/post-install.json" > "$work/legacy-host-evidence.json"
if python3 "$root/compare-evidence.py" --phase install --pre "$root/tests/fixtures/pre-install.json" --post "$work/legacy-host-evidence.json" --candidate-verification "$work/candidate-verification.json" --contract "$work/candidate-contract.json" > "$work/legacy-host-report.json"; then
  echo "comparison silently accepted the legacy host evidence schema" >&2
  exit 1
fi
jq -e '.status == "FAIL" and (.error | contains("invalid evidence schema"))' "$work/legacy-host-report.json" >/dev/null

jq '.manager.candidate_binding.verification_commitment="sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"' "$work/post-install-lab-c.json" > "$work/different-candidate-lab-c.json"
if python3 "$root/compare-evidence.py" --phase three-host --pre "$root/tests/fixtures/pre-install.json" "$root/tests/fixtures/pre-install-lab-b.json" "$root/tests/fixtures/pre-install-lab-c.json" --post "$work/post-install.json" "$work/post-install-lab-b.json" "$work/different-candidate-lab-c.json" --candidate-verification "$work/candidate-verification.json" --contract "$work/candidate-contract.json" > "$work/different-candidate-report.json"; then
  echo "comparison accepted different candidate bindings across hosts" >&2
  exit 1
fi
jq -e '.failures | index("post-install hosts do not bind the same candidate") != null' "$work/different-candidate-report.json" >/dev/null
jq '.services["podmesh.service"].main_pid = 999' "$work/post-install.json" > "$work/changed-pid.json"
if compare_install "$root/tests/fixtures/pre-install.json" "$work/changed-pid.json" > "$work/changed-pid-report.json"; then
  echo "comparison accepted a lifecycle PID change" >&2
  exit 1
fi
jq -e '.failures | index("podmesh.service state or PID changed") != null' "$work/changed-pid-report.json" >/dev/null
jq '.podman_rootful.commitments.networks = "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"' "$work/post-install.json" > "$work/changed-podman.json"
if compare_install "$root/tests/fixtures/pre-install.json" "$work/changed-podman.json" > "$work/changed-podman-report.json"; then
  echo "comparison accepted a Podman commitment change" >&2
  exit 1
fi
jq -e '.failures | index("rootful Podman commitment changed") != null' "$work/changed-podman-report.json" >/dev/null
jq '.manager.account.shell = "/bin/bash"' "$work/post-install.json" > "$work/wrong-shell.json"
if compare_install "$root/tests/fixtures/pre-install.json" "$work/wrong-shell.json" > "$work/wrong-shell-report.json"; then
  echo "comparison accepted an interactive manager account shell" >&2
  exit 1
fi
jq -e '.failures | index("manager system account shell is not /usr/sbin/nologin") != null' "$work/wrong-shell-report.json" >/dev/null
jq '.manager.account.uid = 0 | .manager.account.primary_group = "users" | .manager.account.home = "/home/podmesh-manager"' "$work/post-install.json" > "$work/wrong-account-identity.json"
if compare_install "$root/tests/fixtures/pre-install.json" "$work/wrong-account-identity.json" > "$work/wrong-account-identity-report.json"; then
  echo "comparison accepted an incorrect manager account identity" >&2
  exit 1
fi
jq -e '.failures | index("manager system account has a root identity") != null and index("manager system account primary group is not podmesh-manager") != null and index("manager system account home is not /nonexistent") != null' "$work/wrong-account-identity-report.json" >/dev/null
jq '.manager.state_directory.owner = "root" | .manager.state_directory.group = "root" | .manager.state_directory.mode = "755"' "$work/post-install.json" > "$work/wrong-state-directory.json"
if compare_install "$root/tests/fixtures/pre-install.json" "$work/wrong-state-directory.json" > "$work/wrong-state-directory-report.json"; then
  echo "comparison accepted incorrect manager state directory metadata" >&2
  exit 1
fi
jq -e '.failures | index("manager state directory owner is not podmesh-manager") != null and index("manager state directory group is not podmesh-manager") != null and index("manager state directory mode is not 750") != null' "$work/wrong-state-directory-report.json" >/dev/null
jq '.manager.state_directory = {"path":"/var/lib/podmesh-manager","present":false,"file_type":null,"owner":null,"group":null,"mode":null,"entry_count":null}' "$work/post-install.json" > "$work/missing-state-directory.json"
if compare_install "$root/tests/fixtures/pre-install.json" "$work/missing-state-directory.json" > "$work/missing-state-directory-report.json"; then
  echo "comparison accepted a missing manager state directory" >&2
  exit 1
fi
jq -e '.failures | index("manager state directory is absent after package-only installation") != null' "$work/missing-state-directory-report.json" >/dev/null
jq '.manager.account = {"name":"podmesh-manager","present":true,"uid":994,"primary_gid":994,"primary_group":"podmesh-manager","home":"/nonexistent","shell":"/usr/sbin/nologin"}' "$root/tests/fixtures/pre-install.json" > "$work/preexisting-account.json"
if compare_install "$work/preexisting-account.json" "$work/post-install.json" > "$work/preexisting-account-report.json"; then
  echo "comparison accepted a pre-existing manager account" >&2
  exit 1
fi
jq -e '.failures | index("pre-install manager absence is not proven") != null' "$work/preexisting-account-report.json" >/dev/null
jq '.manager.state_directory = {"path":"/var/lib/podmesh-manager","present":true,"file_type":"directory","owner":"podmesh-manager","group":"podmesh-manager","mode":"750","entry_count":0}' "$root/tests/fixtures/pre-install.json" > "$work/preexisting-state-directory.json"
if compare_install "$work/preexisting-state-directory.json" "$work/post-install.json" > "$work/preexisting-state-directory-report.json"; then
  echo "comparison accepted a pre-existing manager state directory" >&2
  exit 1
fi
jq -e '.failures | index("pre-install manager absence is not proven") != null' "$work/preexisting-state-directory-report.json" >/dev/null
jq 'del(.manager.account.shell)' "$work/post-install.json" > "$work/missing-account-field.json"
if compare_install "$root/tests/fixtures/pre-install.json" "$work/missing-account-field.json" > "$work/missing-account-field-report.json"; then
  echo "comparison accepted missing manager account evidence" >&2
  exit 1
fi
jq -e '.status == "FAIL" and (.error | contains("invalid manager account evidence"))' "$work/missing-account-field-report.json" >/dev/null
jq 'del(.podman_rootful.commitments.pods)' "$root/tests/fixtures/pre-install.json" > "$work/malformed.json"
if python3 "$root/compare-evidence.py" --phase three-host --pre "$work/malformed.json" "$root/tests/fixtures/pre-install-lab-b.json" "$root/tests/fixtures/pre-install-lab-c.json" --post "$work/post-install.json" "$work/post-install-lab-b.json" "$work/post-install-lab-c.json" --candidate-verification "$work/candidate-verification.json" --contract "$work/candidate-contract.json" > "$work/malformed-report.json"; then
  echo "comparison accepted malformed evidence" >&2
  exit 1
fi
jq -e '.status == "FAIL" and (.error | contains("invalid Podman commitment projection"))' "$work/malformed-report.json" >/dev/null
jq -e '.schema_version == "podmesh-manager-candidate-contract/v2" and (.expected_files | all(.[]; startswith("/"))) and (.expected_regular_payload_files | all(.[]; (.path | startswith("/")) and (.sha256 | test("^replace-with-")))) and (.expected_maintainer_scripts | all(.[]; (.name | test("^(config|postinst|postrm|preinst|prerm)$")) and (.sha256 | test("^replace-with-")))) and (.deb_sha256 | test("^replace-with-")) and (.signing_fingerprint | test("^[A-F0-9]{40}$"))' "$root/candidate-contract.example.json" >/dev/null
hash=$(printf '%064d' 0)
cat > "$work/Release" <<EOF
SHA256:
 $hash 17 pool/Packages
MD5Sum:
 $hash 999 pool/Packages
EOF
awk -v path='pool/Packages' -v sha="$hash" -v size=17 '$1 == "SHA256:" { section=1; next } section && /^[^[:space:]]/ { exit } section && $1 == sha && $2 == size && $3 == path { found=1 } END { exit found ? 0 : 1 }' "$work/Release"
if awk -v path='pool/Packages' -v sha="$hash" -v size=999 '$1 == "SHA256:" { section=1; next } section && /^[^[:space:]]/ { exit } section && $1 == sha && $2 == size && $3 == path { found=1 } END { exit found ? 0 : 1 }' "$work/Release"; then
  echo "Release lookup accepted a size from a later section" >&2
  exit 1
fi
if grep -En 'podman info|manager_processes|--slurpfile.*networks' "$root/collect-host.sh"; then
  echo "collector retains a public raw-information surface" >&2
  exit 1
fi
bash -n "$root/collect-host.sh" "$root/verify-candidate.sh"
"$root/tests/collector-stub-tests.sh"
"$root/tests/verifier-tests.sh"
"$root/refusal/tests/run-tests.sh"
"$root/upgrade/tests/run-tests.sh"
printf '%s\n' '[{"name":"podman","id":"2f259bab93aa","driver":"bridge","network_interface":"podman0"}]' |
  jq -e -cS 'if type != "array" then error("invalid network inventory") else map({id:(.id//.Id//.ID//error("missing network id")),driver:(.driver//.Driver//"")})|sort_by(.id) end' |
  jq -e '.[0] == {"id":"2f259bab93aa","driver":"bridge"}' >/dev/null
if command -v shellcheck >/dev/null; then
  shellcheck "$root/collect-host.sh" "$root/verify-candidate.sh"
fi
git -C "$(cd -- "$root/../../.." && pwd)" diff --check
printf '%s\n' 'PASS: schema validation, fixture comparison including negative cases, collector stubs, contract parsing, public-surface check, shell syntax, optional shellcheck and diff check.'
