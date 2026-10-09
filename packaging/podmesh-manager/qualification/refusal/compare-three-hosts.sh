#!/bin/bash
set -euo pipefail

# Compare three corrected default-refusal records without loading private salt or
# configuration. It recomputes cross-host relations from salted commitments.
if [ "${1-}" != "--summary" ] || [ "$#" -ne 5 ]; then
  echo "Usage: $0 --summary qualification-summary.json host-a.json host-b.json host-c.json" >&2
  exit 2
fi

exec python3 - "$2" "$3" "$4" "$5" <<'PY'
import json
import re
import sys
from pathlib import Path

SUMMARY, *HOSTS = map(Path, sys.argv[1:])
COMMITMENT = re.compile(r"^sha256:[0-9a-f]{64}$")
PLAIN_SHA256 = re.compile(r"^[0-9a-f]{64}$")
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
PACKAGE_VERSION = re.compile(r"^[A-Za-z0-9][A-Za-z0-9.+~:-]{0,127}$")
SENSITIVE_KEYS = {
    "endpoint", "bind", "shared_key_hex", "logical_manager_id",
    "local_replica_id", "local_host_id", "host_id", "replica_id",
}


def fail(message):
    print(f"FAIL: {message}", file=sys.stderr)
    raise SystemExit(1)


def load(path):
    try:
        with path.open(encoding="utf-8") as stream:
            return json.load(stream)
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot load evidence {path}: {error}")


def get(obj, path):
    current = obj
    for key in path.split("."):
        if not isinstance(current, dict) or key not in current:
            fail(f"missing evidence field {path}")
        current = current[key]
    if current is None:
        fail(f"null evidence field {path}")
    return current


def commitment(value, path):
    if not isinstance(value, str) or not COMMITMENT.fullmatch(value):
        fail(f"{path} is not a salted SHA-256 commitment")
    return value


def boolean(value, path):
    if value is not True:
        fail(f"{path} is not proven true")


def uid(value, path):
    if type(value) is not int or value < 0:
        fail(f"{path} is not a non-negative integer UID")
    return value


def walk_public_shape(value, path="evidence"):
    if isinstance(value, dict):
        for key, child in value.items():
            if key in SENSITIVE_KEYS:
                fail(f"raw private field {path}.{key} is present")
            if key == "commitment" or key.endswith("_commitment"):
                commitment(child, f"{path}.{key}")
            walk_public_shape(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            walk_public_shape(child, f"{path}[{index}]")


summary = load(SUMMARY)
walk_public_shape(summary, str(SUMMARY))
if get(summary, "schema_version") != "podmesh-manager-corrected-default-refusal-qualification/v1":
    fail("unsupported qualification summary schema")
if get(summary, "result") != "PASS":
    fail("qualification summary is not PASS")
if get(summary, "host_count") != 3 or get(summary, "failures") != []:
    fail("qualification summary is incomplete or contains failures")
package_version = get(summary, "package_version")
if not isinstance(package_version, str) or not PACKAGE_VERSION.fullmatch(package_version):
    fail("qualification summary has no valid package version")
source_commit = get(summary, "source_commit")
if not isinstance(source_commit, str) or not SOURCE_COMMIT.fullmatch(source_commit):
    fail("qualification summary has no valid source commit")
summary_topology = get(summary, "topology")
for field in (
    "distinct_local_hosts", "distinct_local_replicas", "one_equal_topology",
    "one_logical_manager", "peer_endpoint_bindings_match",
    "symmetric_distinct_pair_keys", "two_peers_per_host", "zero_grants",
):
    boolean(summary_topology.get(field), f"summary.topology.{field}")
boolean(get(summary, "unit_fragment_equal"), "summary.unit_fragment_equal")
summary_results = get(summary, "host_results")
if not isinstance(summary_results, list) or len(summary_results) != 3:
    fail("summary host results are incomplete")
if any(
    not isinstance(item, dict) or set(item) != {"host_alias", "result"}
    or item.get("result") != "PASS" for item in summary_results
):
    fail("summary contains a non-PASS host result")
summary_aliases = [item["host_alias"] for item in summary_results]
if len(set(summary_aliases)) != 3:
    fail("summary host aliases are not distinct")

hosts = [load(path) for path in HOSTS]
for host, path in zip(hosts, HOSTS):
    walk_public_shape(host, str(path))


def required_host(host, path):
    if get(host, "schema_version") != "podmesh-manager-default-refusal-evidence/v1":
        fail(f"{path}: unsupported host evidence schema")
    alias = get(host, "host_alias")
    if not isinstance(alias, str) or alias not in summary_aliases:
        fail(f"{path}: host alias does not match the summary")

    config = get(host, "configuration.commitments")
    for field in (
        "document_commitment", "logical_manager_commitment",
        "local_replica_commitment", "local_host_commitment", "topology_commitment",
    ):
        commitment(get(config, field), f"{path}: configuration.commitments.{field}")
    observation_writer_uid = uid(
        get(config, "observation_writer_uid"),
        f"{path}: configuration.commitments.observation_writer_uid",
    )
    grant_count = get(config, "grant_count")
    if type(grant_count) is not int or grant_count != 0:
        fail(f"{path}: manager grants are not proven absent")
    if get(config, "peer_count") != 2:
        fail(f"{path}: peer count is not exactly two")
    peers = get(config, "peers")
    if not isinstance(peers, list) or len(peers) != 2:
        fail(f"{path}: peer commitments are incomplete")
    seen_peers = set()
    for index, peer in enumerate(peers):
        expected = {"replica_id_commitment", "endpoint_commitment", "shared_key_commitment"}
        if not isinstance(peer, dict) or set(peer) != expected:
            fail(f"{path}: peer {index} has an unsafe or incomplete shape")
        peer_id = commitment(peer["replica_id_commitment"], f"{path}: peer {index} replica")
        if peer_id in seen_peers:
            fail(f"{path}: duplicate peer commitment")
        seen_peers.add(peer_id)
        commitment(peer["endpoint_commitment"], f"{path}: peer {index} endpoint")
        commitment(peer["shared_key_commitment"], f"{path}: peer {index} key")

    validation = get(host, "offline_validation")
    boolean(get(validation, "configuration_valid"), f"{path}: offline validation")
    if get(validation, "effective_uid") != get(validation, "account_uid"):
        fail(f"{path}: offline validation identity is not proven")
    runtime = get(validation, "runtime")
    boolean(get(runtime, "created"), f"{path}: runtime creation")
    boolean(get(runtime, "removed"), f"{path}: runtime removal")
    if get(runtime, "initial_entry_count") != 0 or get(runtime, "post_validation_entry_count") != 0:
        fail(f"{path}: offline runtime was not empty")
    if get(runtime, "metadata.mode") != "700":
        fail(f"{path}: offline runtime mode is not 0700")
    if get(runtime, "metadata.uid") != get(validation, "account_uid"):
        fail(f"{path}: offline runtime owner differs from the manager account")
    config_metadata = get(host, "configuration.metadata")
    if (get(config_metadata, "present") is not True
            or get(config_metadata, "file_type") != "regular file"
            or get(config_metadata, "uid") != 0
            or get(config_metadata, "mode") != "640"):
        fail(f"{path}: protected configuration metadata is invalid")

    pre_unit = get(host, "default_refusal.pre_unit")
    post_unit = get(host, "default_refusal.post_unit")
    if get(pre_unit, "dropins") != [] or get(post_unit, "dropins") != []:
        fail(f"{path}: a systemd drop-in is present")
    if (get(pre_unit, "load_state") != "loaded"
            or get(pre_unit, "active_state") != "inactive"
            or get(pre_unit, "sub_state") != "dead"
            or get(pre_unit, "unit_file_state") != "disabled"
            or get(pre_unit, "main_pid") != 0
            or get(pre_unit, "exec_main_pid") != 0
            or pre_unit.get("invocation_id") is not None
            or get(pre_unit, "n_restarts") != 0):
        fail(f"{path}: default-disabled precondition is invalid")
    if get(post_unit, "active_state") != "failed" or get(post_unit, "result") != "exit-code":
        fail(f"{path}: default refusal state is not failed closed")
    if (get(post_unit, "load_state") != "loaded"
            or get(post_unit, "sub_state") != "failed"
            or get(post_unit, "unit_file_state") != "disabled"):
        fail(f"{path}: default refusal unit state is incomplete")
    if get(post_unit, "exec_main_status") != 1 or get(post_unit, "main_pid") != 0:
        fail(f"{path}: default refusal exit proof is incomplete")
    if get(post_unit, "exec_main_pid") <= 0 or get(post_unit, "n_restarts") != 0:
        fail(f"{path}: retained execution or restart proof is invalid")
    fragment = get(post_unit, "fragment_sha256")
    if not isinstance(fragment, str) or not PLAIN_SHA256.fullmatch(fragment):
        fail(f"{path}: unit fragment is not a SHA-256 hash")
    invocation = commitment(get(post_unit, "invocation_id"), f"{path}: invocation")
    journal = get(host, "default_refusal.journal")
    if commitment(get(journal, "invocation_commitment"), f"{path}: journal invocation") != invocation:
        fail(f"{path}: refusal journal is not bound to the unit invocation")
    boolean(get(journal, "network_disabled_refusal_observed"), f"{path}: refusal journal")

    for side in ("before", "after"):
        for network in ("routes", "firewall"):
            observed = get(host, f"{side}.{network}")
            if get(observed, "status") != "available-successful":
                fail(f"{path}: {side}.{network} is unknown or unsuccessful")
            commitment(get(observed, "commitment"), f"{path}: {side}.{network}.commitment")
    for network in ("routes", "firewall"):
        if get(host, f"before.{network}.commitment") != get(host, f"after.{network}.commitment"):
            fail(f"{path}: {network} commitments changed")
    if get(host, "before.existing_services") != get(host, "after.existing_services"):
        fail(f"{path}: existing service commitments changed")
    if get(host, "before.podman_containers_commitment") != get(host, "after.podman_containers_commitment"):
        fail(f"{path}: Podman commitments changed")
    for assertion in (
        "existing_services_unchanged", "podman_containers_unchanged", "routes_unchanged",
        "firewall_unchanged", "no_manager_process", "state_directory_empty",
        "control_socket_absent", "configured_endpoint_not_listening",
    ):
        boolean(get(host, f"assertions.{assertion}"), f"{path}: assertions.{assertion}")
    if get(host, "after.manager.process_count") != 0 or get(host, "after.manager.state_directory.entry_count") != 0:
        fail(f"{path}: manager runtime is not empty")
    if get(host, "after.manager.control_socket.present") is not False:
        fail(f"{path}: manager control socket remains present")
    listeners = get(host, "after.manager.listeners")
    if get(listeners, "status") != "available-successful" or get(listeners, "tcp_listener_count") != 0 or get(listeners, "udp_listener_count") != 0:
        fail(f"{path}: configured listeners are not proven absent")
    commitment(get(listeners, "endpoint_commitment"), f"{path}: listener endpoint")
    commitment(get(host, "before.podman_containers_commitment"), f"{path}: before Podman")
    commitment(get(host, "after.podman_containers_commitment"), f"{path}: after Podman")
    return alias, config, peers, post_unit, listeners, observation_writer_uid, grant_count


records = [required_host(host, path) for host, path in zip(hosts, HOSTS)]
aliases = [record[0] for record in records]
if len(set(aliases)) != 3 or set(aliases) != set(summary_aliases):
    fail("host aliases are not a complete distinct three-host set")
logical = [record[1]["logical_manager_commitment"] for record in records]
topologies = [record[1]["topology_commitment"] for record in records]
fragments = [record[3]["fragment_sha256"] for record in records]
observation_writer_uids = [record[5] for record in records]
grant_counts = [record[6] for record in records]
if len(set(logical)) != 1:
    fail("logical manager commitments differ")
if len(set(topologies)) != 1:
    fail("topology commitments differ")
if len(set(fragments)) != 1:
    fail("unit fragment commitments differ")
if len(set(observation_writer_uids)) != 1:
    fail("observation writer UIDs differ")
if observation_writer_uids[0] != 0:
    fail("observation writer UID is not exactly zero")
if grant_counts != [0, 0, 0]:
    fail("manager grants are not exactly zero on every host")
if len({record[1]["local_replica_commitment"] for record in records}) != 3:
    fail("local replica commitments are not distinct")
if len({record[1]["local_host_commitment"] for record in records}) != 3:
    fail("local host commitments are not distinct")

by_replica = {record[1]["local_replica_commitment"]: record for record in records}
if len(by_replica) != 3:
    fail("local replica commitment map is incomplete")
directed_keys = {}
for _, config, peers, _, _, _, _ in records:
    local_id = config["local_replica_commitment"]
    for peer in peers:
        remote_id = peer["replica_id_commitment"]
        if remote_id not in by_replica or remote_id == local_id:
            fail("peer does not identify exactly one other host")
        remote_listeners = by_replica[remote_id][4]
        if peer["endpoint_commitment"] != remote_listeners["endpoint_commitment"]:
            fail("peer endpoint does not match the remote bind commitment")
        directed_keys[(local_id, remote_id)] = peer["shared_key_commitment"]

pair_keys = []
for left, right in directed_keys:
    if left >= right:
        continue
    key = directed_keys[(left, right)]
    if directed_keys.get((right, left)) != key:
        fail("pair key is not symmetric")
    pair_keys.append(key)
if len(pair_keys) != 3 or len(set(pair_keys)) != 3:
    fail("pair keys are not exactly three symmetric distinct commitments")

print(json.dumps({
    "result": "PASS",
    "host_aliases": sorted(aliases),
    "package_version": package_version,
    "source_commit": source_commit,
    "schema_version": "podmesh-manager-default-refusal-evidence/v1",
    "unit_fragment_sha256": fragments[0],
    "topology": {
        "one_logical_manager": True,
        "one_equal_topology": True,
        "distinct_local_replicas": True,
        "distinct_local_hosts": True,
        "two_peers_per_host": True,
        "peer_endpoint_bindings_match": True,
        "symmetric_distinct_pair_keys": True,
        "one_observation_writer_uid": True,
        "zero_grants": True,
    },
    "observation_writer_uid": observation_writer_uids[0],
    "grant_count": 0,
    "private_values": "absent",
}, sort_keys=True))
PY
