#!/bin/bash
set -euo pipefail

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
compare="$root/compare-three-hosts.sh"
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

bash -n "$compare"

python3 - "$work" <<'PY'
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])

def commitment(label):
    return "sha256:" + hashlib.sha256(label.encode()).hexdigest()

replicas = [commitment(f"replica-{i}") for i in range(3)]
hosts = [commitment(f"host-{i}") for i in range(3)]
listeners = [commitment(f"bind-{i}") for i in range(3)]
keys = {(0, 1): commitment("key-12"), (0, 2): commitment("key-13"), (1, 2): commitment("key-23")}
logical = commitment("logical-manager")
topology = commitment("topology")
fragment = "0" * 64

def observation(label):
    return {"status": "available-successful", "bytes": 1, "reason": None, "commitment": commitment(label)}

def services(index):
    return {
        "lifecycle": {"unit": "podmesh.service", "commitment": commitment(f"lifecycle-{index}")},
        "observer": {"unit": "podmesh-web-observer.service", "commitment": commitment(f"observer-{index}")},
    }

def host(index):
    peers = []
    for remote in range(3):
        if remote == index:
            continue
        pair = (min(index, remote), max(index, remote))
        peers.append({
            "replica_id_commitment": replicas[remote],
            "endpoint_commitment": listeners[remote],
            "shared_key_commitment": keys[pair],
        })
    return {
        "schema_version": "podmesh-manager-default-refusal-evidence/v1",
        "host_alias": ["lab-a", "lab-b", "lab-c"][index],
        "captured_at_utc": "2026-09-12T00:00:00Z",
        "configuration": {
            "metadata": {"path": "/etc/podmesh-manager/config.json", "present": True, "file_type": "regular file", "uid": 0, "gid": 104, "mode": "640", "bytes": 1},
            "commitments": {
                "document_commitment": commitment(f"document-{index}"),
                "logical_manager_commitment": logical,
                "local_replica_commitment": replicas[index],
                "local_host_commitment": hosts[index],
                "observation_writer_uid": 0,
                "topology_commitment": topology,
                "peer_count": 2,
                "peers": sorted(peers, key=lambda peer: peer["replica_id_commitment"]),
            },
        },
        "offline_validation": {
            "command": ["podmesh-managerd", "--validate-config"],
            "effective_uid": 103,
            "account_uid": 103,
            "configuration_valid": True,
            "runtime": {
                "created": True,
                "metadata": {"path": "/run/podmesh-manager", "present": True, "file_type": "directory", "uid": 103, "gid": 104, "mode": "700", "bytes": 1},
                "initial_entry_count": 0,
                "post_validation_entry_count": 0,
                "removed": True,
            },
        },
        "default_refusal": {
            "start_exit_status": 0,
            "pre_unit": {
                "load_state": "loaded", "active_state": "inactive", "sub_state": "dead",
                "unit_file_state": "disabled", "main_pid": 0, "exec_main_pid": 0,
                "invocation_id": None, "n_restarts": 0, "dropins": [],
                "fragment_sha256": fragment,
            },
            "post_unit": {
                "load_state": "loaded",
                "active_state": "failed",
                "sub_state": "failed",
                "unit_file_state": "disabled",
                "result": "exit-code",
                "exec_main_status": 1,
                "main_pid": 0,
                "exec_main_pid": 41,
                "n_restarts": 0,
                "dropins": [],
                "fragment_sha256": fragment,
                "invocation_id": commitment(f"invocation-{index}"),
            },
            "journal": {
                "selector": "_SYSTEMD_INVOCATION_ID",
                "invocation_commitment": commitment(f"invocation-{index}"),
                "transcript_commitment": commitment(f"journal-{index}"),
                "line_count": 1,
                "network_disabled_refusal_observed": True,
            },
        },
        "before": {
            "existing_services": services(index),
            "podman_containers_commitment": commitment("containers"),
            "routes": observation(f"routes-{index}"),
            "firewall": observation("firewall"),
        },
        "after": {
            "existing_services": services(index),
            "podman_containers_commitment": commitment("containers"),
            "routes": observation(f"routes-{index}"),
            "firewall": observation("firewall"),
            "manager": {
                "process_count": 0,
                "state_directory": {"path": "/var/lib/podmesh-manager", "present": True, "file_type": "directory", "uid": 103, "gid": 104, "mode": "750", "bytes": 1, "entry_count": 0},
                "control_socket": {"path": "/run/podmesh-manager/control.sock", "present": False, "file_type": None, "uid": None, "gid": None, "mode": None, "bytes": None},
                "listeners": {"status": "available-successful", "endpoint_commitment": listeners[index], "tcp_listener_count": 0, "udp_listener_count": 0, "snapshot_commitment": commitment(f"listeners-{index}")},
            },
        },
        "assertions": {
            "existing_services_unchanged": True,
            "podman_containers_unchanged": True,
            "routes_unchanged": True,
            "firewall_unchanged": True,
            "no_manager_process": True,
            "state_directory_empty": True,
            "control_socket_absent": True,
            "configured_endpoint_not_listening": True,
        },
    }

for index in range(3):
    (root / f"lab-{index}.json").write_text(json.dumps(host(index), sort_keys=True), encoding="utf-8")
summary = {
    "schema_version": "podmesh-manager-corrected-default-refusal-qualification/v1",
    "result": "PASS",
    "host_count": 3,
    "host_results": [{"host_alias": alias, "result": "PASS"} for alias in ["lab-a", "lab-b", "lab-c"]],
    "failures": [],
    "package_version": "0.1.0~manager1+g5319ab150fc3",
    "source_commit": "5319ab150fc34abb0b1648be0c3a68350975cebb",
    "topology": {
        "distinct_local_hosts": True,
        "distinct_local_replicas": True,
        "one_equal_topology": True,
        "one_logical_manager": True,
        "peer_endpoint_bindings_match": True,
        "symmetric_distinct_pair_keys": True,
        "two_peers_per_host": True,
    },
    "unit_fragment_equal": True,
}
(root / "summary.json").write_text(json.dumps(summary, sort_keys=True), encoding="utf-8")
PY

args=(--summary "$work/summary.json" "$work/lab-0.json" "$work/lab-1.json" "$work/lab-2.json")
"$compare" "${args[@]}" > "$work/pass.json"
jq -e '.result=="PASS" and .host_aliases==["lab-a","lab-b","lab-c"] and .observation_writer_uid==0 and .topology.one_observation_writer_uid and .topology.peer_endpoint_bindings_match and .topology.symmetric_distinct_pair_keys and .private_values=="absent"' "$work/pass.json" >/dev/null
cp "$work/lab-0.json" "$work/lab-0-pristine.json"

expect_failure() {
  if "$compare" "${args[@]}" >/dev/null 2>"$work/error"; then
    echo "comparison accepted invalid refusal evidence" >&2
    exit 1
  fi
}

mutate() {
  cp "$work/lab-0-pristine.json" "$work/lab-0.json"
  python3 - "$work/lab-0.json" "$@" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
data = json.loads(path.read_text())
if sys.argv[2] == "unknown-route":
    data["after"]["routes"]["status"] = "unknown"
elif sys.argv[2] == "false-firewall":
    data["assertions"]["firewall_unchanged"] = False
elif sys.argv[2] == "dropin":
    data["default_refusal"]["post_unit"]["dropins"] = ["/run/systemd/system/podmesh-manager.service.d/network.conf"]
elif sys.argv[2] == "raw-endpoint":
    data["configuration"]["commitments"]["peers"][0]["endpoint"] = "10.0.0.1:9443"
elif sys.argv[2] == "bad-reciprocal":
    data["configuration"]["commitments"]["peers"][0]["endpoint_commitment"] = "sha256:" + "0" * 64
elif sys.argv[2] == "changed-route-commitment":
    data["after"]["routes"]["commitment"] = "sha256:" + "0" * 64
elif sys.argv[2] == "changed-service-commitment":
    data["after"]["existing_services"]["lifecycle"]["commitment"] = "sha256:" + "0" * 64
elif sys.argv[2] == "wrong-journal-invocation":
    data["default_refusal"]["journal"]["invocation_commitment"] = "sha256:" + "0" * 64
elif sys.argv[2] == "invalid-precondition":
    data["default_refusal"]["pre_unit"]["active_state"] = "active"
elif sys.argv[2] == "invalid-config-owner":
    data["configuration"]["metadata"]["uid"] = 103
elif sys.argv[2] == "invalid-runtime-owner":
    data["offline_validation"]["runtime"]["metadata"]["uid"] = 999
elif sys.argv[2] == "missing-observation-writer-uid":
    del data["configuration"]["commitments"]["observation_writer_uid"]
elif sys.argv[2] == "nonzero-observation-writer-uid":
    data["configuration"]["commitments"]["observation_writer_uid"] = 1000
elif sys.argv[2] == "tampered-observation-writer-uid":
    data["configuration"]["commitments"]["observation_writer_uid"] = "0"
elif sys.argv[2] == "one-host-grant-drift":
    data["configuration"]["commitments"]["topology_commitment"] = "sha256:" + "1" * 64
else:
    raise SystemExit("unknown mutation")
path.write_text(json.dumps(data), encoding="utf-8")
PY
}

mutate unknown-route
expect_failure
mutate false-firewall
expect_failure
mutate dropin
expect_failure
mutate raw-endpoint
expect_failure
mutate bad-reciprocal
expect_failure
mutate changed-route-commitment
expect_failure
mutate changed-service-commitment
expect_failure
mutate wrong-journal-invocation
expect_failure
mutate invalid-precondition
expect_failure
mutate invalid-config-owner
expect_failure
mutate invalid-runtime-owner
expect_failure
mutate missing-observation-writer-uid
expect_failure
mutate nonzero-observation-writer-uid
expect_failure
mutate tampered-observation-writer-uid
expect_failure
mutate one-host-grant-drift
expect_failure

printf '%s\n' 'PASS: three-host refusal comparison recomputes identity, canonical topology, writer UID, reciprocal endpoints, symmetric pair keys, privacy and fail-closed observations.'
