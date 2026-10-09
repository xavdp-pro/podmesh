#!/usr/bin/env python3
"""Compare three secret-free configuration-transition evidence documents."""

import argparse
import hashlib
import json
import re
import sys

ALIASES = ("lab-a", "lab-b", "lab-c")
SCOPES = [f"g2/{alias}/observations" for alias in ALIASES]
COMMITMENT = re.compile(r"^sha256:[0-9a-f]{64}$")
TRANSITIONS = ("three-grants", "incoming-workers")
INCOMING_WORKERS = {"from": 1, "to": 2}
SCHEMAS = {"three-grants": "podmesh-manager-config-transition-evidence/v1", "incoming-workers": "podmesh-manager-config-transition-evidence/v2"}
REQUIRED_PRECONDITIONS = (
    "manager_disabled", "manager_inactive", "no_manager_process",
    "no_control_socket", "no_configured_port_listener",
)


def refuse(message):
    raise ValueError(message)


def load(path, transition):
    with open(path, "rb") as stream:
        raw = stream.read()
    sidecar = f"{path}.sha256"
    with open(sidecar, encoding="ascii") as stream:
        fields = stream.read().strip().split()
    if len(fields) != 2 or not re.fullmatch(r"[0-9a-f]{64}", fields[0]) or fields[1] not in {path, path.rsplit("/", 1)[-1]}:
        refuse(f"{path}: invalid evidence checksum sidecar")
    if hashlib.sha256(raw).hexdigest() != fields[0]:
        refuse(f"{path}: evidence checksum mismatch")
    value = json.loads(raw)
    if value.get("schema_version") != SCHEMAS[transition]:
        refuse(f"{path}: unsupported evidence schema")
    expected_top = {"schema_version", "result", "action", "host_alias", "candidate", "transition", "preconditions", "offline_validation", "rollback_permitted_only_while", "private_values", "claims_not_made"}
    if transition == "incoming-workers":
        expected_top.add("transition_kind")
        if value.get("transition_kind") != transition:
            refuse(f"{path}: transition kind differs")
    if set(value) != expected_top:
        refuse(f"{path}: unexpected or missing top-level field")
    if value.get("result") != "PASS" or value.get("action") != "applied":
        refuse(f"{path}: evidence is not a successful apply")
    if value.get("private_values") != "absent":
        refuse(f"{path}: private-value boundary is missing")
    candidate = value.get("candidate", {})
    if set(candidate) != {"package", "version", "binary_sha256", "report_commitment", "dpkg_verify"}:
        refuse(f"{path}: unexpected or missing candidate field")
    if candidate.get("package") != "podmesh-manager" or not isinstance(candidate.get("version"), str) or not candidate["version"]:
        refuse(f"{path}: candidate package binding is invalid")
    if not re.fullmatch(r"[0-9a-f]{64}", str(candidate.get("binary_sha256", ""))) or not COMMITMENT.fullmatch(str(candidate.get("report_commitment", ""))) or candidate.get("dpkg_verify") != "clean":
        refuse(f"{path}: candidate verification binding is invalid")
    record = value.get("transition", {})
    expected_transition = {"source_config_commitment", "applied_config_commitment", "protected_backup_commitment", "mapping_commitment", "activation_markers_commitment", "grant_count", "grants", "all_other_values_preserved", "local_replica_matches_mapping"}
    if transition == "incoming-workers":
        expected_transition |= {"changed_keys", "incoming_workers", "state_listing_commitment"}
    if set(record) != expected_transition:
        refuse(f"{path}: unexpected or missing transition field")
    if transition == "incoming-workers":
        if record.get("changed_keys") != ["incoming_workers"]:
            refuse(f"{path}: the operational transition must change exactly incoming_workers")
        if record.get("incoming_workers") != INCOMING_WORKERS:
            refuse(f"{path}: incoming_workers transition differs from the exact reviewed values")
        if not COMMITMENT.fullmatch(str(record.get("state_listing_commitment", ""))):
            refuse(f"{path}: invalid state_listing_commitment")
    grants = record.get("grants")
    if record.get("grant_count") != 3 or not isinstance(grants, list) or len(grants) != 3:
        refuse(f"{path}: expected exactly three grants")
    if [grant.get("scope") for grant in grants] != SCOPES:
        refuse(f"{path}: grant scopes or order differ")
    if any(not isinstance(grant, dict) or set(grant) != {"scope", "owner_replica_commitment"} for grant in grants):
        refuse(f"{path}: unexpected or missing grant field")
    if any(not COMMITMENT.fullmatch(str(grant.get("owner_replica_commitment", ""))) for grant in grants):
        refuse(f"{path}: invalid owner commitment")
    for key in ("source_config_commitment", "applied_config_commitment", "protected_backup_commitment", "mapping_commitment", "activation_markers_commitment"):
        if not COMMITMENT.fullmatch(str(record.get(key, ""))):
            refuse(f"{path}: invalid {key}")
    if record.get("all_other_values_preserved") is not True or record.get("local_replica_matches_mapping") is not True:
        refuse(f"{path}: transition preservation or local mapping is unproven")
    preconditions = value.get("preconditions", {})
    if transition == "three-grants":
        required_preconditions = REQUIRED_PRECONDITIONS + ("state_directory_empty",)
        expected_preconditions = set(required_preconditions)
    else:
        # The listing assertion is required; what it found is disclosed, not required.
        required_preconditions = REQUIRED_PRECONDITIONS + ("state_directory_listing_unchanged",)
        expected_preconditions = set(required_preconditions) | {"durable_state_present", "activation_marker_present"}
        if any(type(preconditions.get(name)) is not bool for name in ("durable_state_present", "activation_marker_present")):
            refuse(f"{path}: disclosed durable-state and boundary-marker presence must be booleans")
    if any(preconditions.get(name) is not True for name in required_preconditions):
        refuse(f"{path}: a required precondition is unproven")
    if set(preconditions) != expected_preconditions:
        refuse(f"{path}: unexpected or missing precondition field")
    validation = value.get("offline_validation", {})
    if validation != {"candidate_valid": True, "durable_store_checked": False, "final_path_valid": True, "network_started": False}:
        refuse(f"{path}: offline validation boundary differs")
    expected_rollback = {"state_directory_empty": True, "activation_markers_unchanged": True} if transition == "three-grants" else {"activation_markers_unchanged": True}
    if value.get("rollback_permitted_only_while") != expected_rollback:
        refuse(f"{path}: rollback boundary differs")
    claims = value.get("claims_not_made")
    expected_claims = ["activation", "availability", "convergence", "DNS", "fencing", "high availability", "replication", "takeover"]
    if claims != expected_claims:
        refuse(f"{path}: claim boundary differs")
    return value


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("evidence", nargs=3)
    parser.add_argument("--transition", choices=TRANSITIONS, default="three-grants")
    args = parser.parse_args()
    try:
        records = [load(path, args.transition) for path in args.evidence]
        if sorted(record["host_alias"] for record in records) != list(ALIASES):
            refuse("evidence must contain exactly lab-a, lab-b and lab-c")
        mappings = {record["transition"]["mapping_commitment"] for record in records}
        if len(mappings) != 1:
            refuse("hosts did not use the same private alias mapping")
        grant_sets = {
            json.dumps(record["transition"]["grants"], sort_keys=True, separators=(",", ":"))
            for record in records
        }
        if len(grant_sets) != 1:
            refuse("scope-owner commitments differ across hosts")
        candidates = {(record["candidate"]["version"], record["candidate"]["binary_sha256"], record["candidate"]["report_commitment"]) for record in records}
        if len(candidates) != 1:
            refuse("hosts did not use the same qualified package candidate")
        output = {
            "schema_version": "podmesh-manager-config-transition-comparison/v1",
            "result": "PASS",
            "host_aliases": list(ALIASES),
            "host_count": 3,
            "grant_count": 3,
            "grant_scopes": SCOPES,
            "same_private_mapping_committed": True,
            "same_scope_owner_bindings_committed": True,
            "same_qualified_candidate": True,
            "all_other_values_preserved": True,
            "all_hosts_inactive_and_empty_before_transition": True,
            "offline_validation_only": True,
            "private_values": "absent",
            "claims_not_made": [
                "activation", "availability", "convergence", "DNS", "fencing",
                "high availability", "replication", "takeover",
            ],
        }
        if args.transition == "incoming-workers":
            by_alias = {record["host_alias"]: record["preconditions"] for record in records}
            output.update({
                "schema_version": "podmesh-manager-config-transition-comparison/v2",
                "transition_kind": args.transition,
                "changed_keys": ["incoming_workers"],
                "incoming_workers": INCOMING_WORKERS,
                "all_hosts_inactive_before_transition": True,
                "durable_state_present": {alias: by_alias[alias]["durable_state_present"] for alias in ALIASES},
                "activation_marker_present": {alias: by_alias[alias]["activation_marker_present"] for alias in ALIASES},
            })
            del output["all_hosts_inactive_and_empty_before_transition"]
        print(json.dumps(output, sort_keys=True, indent=2))
        return 0
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(f"three-host transition comparison refused: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
