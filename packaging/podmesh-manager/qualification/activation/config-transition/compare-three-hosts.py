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


def refuse(message):
    raise ValueError(message)


def load(path):
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
    if value.get("schema_version") != "podmesh-manager-config-transition-evidence/v1":
        refuse(f"{path}: unsupported evidence schema")
    expected_top = {"schema_version", "result", "action", "host_alias", "candidate", "transition", "preconditions", "offline_validation", "rollback_permitted_only_while", "private_values", "claims_not_made"}
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
    transition = value.get("transition", {})
    if set(transition) != {"source_config_commitment", "applied_config_commitment", "protected_backup_commitment", "mapping_commitment", "activation_markers_commitment", "grant_count", "grants", "all_other_values_preserved", "local_replica_matches_mapping"}:
        refuse(f"{path}: unexpected or missing transition field")
    grants = transition.get("grants")
    if transition.get("grant_count") != 3 or not isinstance(grants, list) or len(grants) != 3:
        refuse(f"{path}: expected exactly three grants")
    if [grant.get("scope") for grant in grants] != SCOPES:
        refuse(f"{path}: grant scopes or order differ")
    if any(not isinstance(grant, dict) or set(grant) != {"scope", "owner_replica_commitment"} for grant in grants):
        refuse(f"{path}: unexpected or missing grant field")
    if any(not COMMITMENT.fullmatch(str(grant.get("owner_replica_commitment", ""))) for grant in grants):
        refuse(f"{path}: invalid owner commitment")
    for key in ("source_config_commitment", "applied_config_commitment", "protected_backup_commitment", "mapping_commitment", "activation_markers_commitment"):
        if not COMMITMENT.fullmatch(str(transition.get(key, ""))):
            refuse(f"{path}: invalid {key}")
    if transition.get("all_other_values_preserved") is not True or transition.get("local_replica_matches_mapping") is not True:
        refuse(f"{path}: transition preservation or local mapping is unproven")
    preconditions = value.get("preconditions", {})
    required_preconditions = (
        "manager_disabled", "manager_inactive", "no_manager_process",
        "no_control_socket", "no_configured_port_listener", "state_directory_empty",
    )
    if any(preconditions.get(name) is not True for name in required_preconditions):
        refuse(f"{path}: a required precondition is unproven")
    if set(preconditions) != set(required_preconditions):
        refuse(f"{path}: unexpected or missing precondition field")
    validation = value.get("offline_validation", {})
    if validation != {"candidate_valid": True, "durable_store_checked": False, "final_path_valid": True, "network_started": False}:
        refuse(f"{path}: offline validation boundary differs")
    if value.get("rollback_permitted_only_while") != {"state_directory_empty": True, "activation_markers_unchanged": True}:
        refuse(f"{path}: rollback boundary differs")
    claims = value.get("claims_not_made")
    expected_claims = ["activation", "availability", "convergence", "DNS", "fencing", "high availability", "replication", "takeover"]
    if claims != expected_claims:
        refuse(f"{path}: claim boundary differs")
    return value


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("evidence", nargs=3)
    args = parser.parse_args()
    try:
        records = [load(path) for path in args.evidence]
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
        print(json.dumps(output, sort_keys=True, indent=2))
        return 0
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(f"three-host transition comparison refused: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
