#!/usr/bin/env python3
"""Compare externally collected evidence for an inactive manager package upgrade."""
import argparse
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

UPGRADE_SCHEMA = "podmesh-manager-inactive-upgrade-evidence/v1"
COMMITMENT_FIELDS = ("config_commitment", "state_commitment")


def load_base_module():
    path = Path(__file__).resolve().parent.parent / "compare-evidence.py"
    spec = importlib.util.spec_from_file_location("podmesh_manager_base_comparator", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


BASE = load_base_module()


def load_upgrade(path):
    value = BASE.read_json(path)
    required = {"schema_version", "stage", "host_evidence", *COMMITMENT_FIELDS}
    if not isinstance(value, dict) or set(value) != required or value.get("schema_version") != UPGRADE_SCHEMA:
        raise ValueError(f"{path}: invalid inactive-upgrade evidence schema")
    if value.get("stage") not in ("pre-upgrade", "post-upgrade"):
        raise ValueError(f"{path}: invalid inactive-upgrade stage")
    for field in COMMITMENT_FIELDS:
        if not BASE.is_commitment(value.get(field)):
            raise ValueError(f"{path}: invalid {field}")
    BASE.validate_host_value(f"{path}:host_evidence", value.get("host_evidence"))
    if value["host_evidence"]["stage"] != "post-install":
        raise ValueError(f"{path}: embedded host evidence is not candidate-bound")
    return value


def expected_binding(report, report_commitment):
    return {
        "package": report["package"],
        "version": report["version"],
        "binary_sha256": report["binary_sha256"],
        "dpkg_verify": "clean",
        "regular_payload_files_commitment": BASE.manifest_commitment(report["regular_payload_files"]),
        "maintainer_scripts_commitment": BASE.manifest_commitment(report["maintainer_scripts"]),
        "verification_commitment": report_commitment,
    }


def is_strict_debian_upgrade(old_version, new_version):
    try:
        result = subprocess.run(
            ["dpkg", "--compare-versions", old_version, "lt", new_version],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
    except OSError as error:
        raise ValueError(f"cannot execute dpkg version comparison: {error}") from error
    if result.returncode not in (0, 1):
        detail = result.stderr.strip() or f"exit status {result.returncode}"
        raise ValueError(f"dpkg rejected candidate version ordering: {detail}")
    return result.returncode == 0


def inactive_manager_failures(label, host):
    failures = []
    unit = host["services"]["podmesh-manager.service"]
    if (unit["load_state"] != "loaded" or unit["active_state"] != "inactive" or
            unit["sub_state"] != "dead" or unit["unit_file_state"] != "disabled" or
            unit["main_pid"] != 0 or unit["exec_main_pid"] != 0 or
            unit["invocation_id"] is not None or unit["start_monotonic_usec"] != 0 or
            unit["n_restarts"] != 0):
        failures.append(
            f"{label} manager unit is not proven disabled, inactive and free of retained "
            "invocation, start-timestamp or restart records"
        )
    manager = host["manager"]
    if manager["process_count"] != 0:
        failures.append(f"{label} manager process exists")
    if manager["runtime_present"]:
        failures.append(f"{label} manager runtime directory exists")
    if not manager["config_present"]:
        failures.append(f"{label} protected manager configuration is absent")
    if not manager["state_directory"]["present"] or manager["state_directory"]["file_type"] != "directory":
        failures.append(f"{label} manager state directory is not a directory")
    if not manager["account"]["present"] or manager["account"]["uid"] == 0 or manager["account"]["primary_gid"] == 0:
        failures.append(f"{label} manager account is absent or privileged")
    return failures


def upgrade_comparison(pre, post, old_report, old_commitment, new_report, new_commitment):
    failures = []
    before = pre["host_evidence"]
    after = post["host_evidence"]
    if pre["stage"] != "pre-upgrade" or post["stage"] != "post-upgrade":
        failures.append("stage pair must be pre-upgrade then post-upgrade")
    if before["host_alias"] != after["host_alias"]:
        failures.append("host aliases differ")
    if not is_strict_debian_upgrade(old_report["version"], new_report["version"]):
        failures.append("new candidate version is not later under Debian version ordering")
    if before["manager"]["candidate_binding"] != expected_binding(old_report, old_commitment):
        failures.append("pre-upgrade host does not bind the reviewed old candidate")
    if after["manager"]["candidate_binding"] != expected_binding(new_report, new_commitment):
        failures.append("post-upgrade host does not bind the reviewed new candidate")
    if before["packages"]["podmesh-manager"] != {"status": "installed", "version": old_report["version"]}:
        failures.append("pre-upgrade installed manager version differs from the old candidate")
    if after["packages"]["podmesh-manager"] != {"status": "installed", "version": new_report["version"]}:
        failures.append("post-upgrade installed manager version differs from the new candidate")
    failures.extend(inactive_manager_failures("pre-upgrade", before))
    failures.extend(inactive_manager_failures("post-upgrade", after))
    if before["boot_id_commitment"] != after["boot_id_commitment"]:
        failures.append("host boot changed during upgrade")
    for package in BASE.EXISTING_PACKAGES:
        if before["packages"][package] != after["packages"][package]:
            failures.append(f"{package} version or status changed during upgrade")
    for unit in BASE.EXISTING_UNITS:
        if before["services"][unit] != after["services"][unit]:
            failures.append(f"{unit} state, PID, invocation or restart evidence changed during upgrade")
    for socket_path in BASE.EXISTING_SOCKETS:
        if before["sockets"][socket_path] != after["sockets"][socket_path]:
            failures.append(f"{socket_path} metadata changed during upgrade")
    if before["podman_rootful"] != after["podman_rootful"]:
        failures.append("rootful Podman commitment changed during upgrade")
    if before["services"]["podmesh-manager.service"] != after["services"]["podmesh-manager.service"]:
        failures.append("manager unit state or invocation evidence changed during upgrade")
    if before["manager"]["account"] != after["manager"]["account"]:
        failures.append("manager account identity changed during upgrade")
    if before["manager"]["state_directory"] != after["manager"]["state_directory"]:
        failures.append("manager state directory metadata changed during upgrade")
    if pre["config_commitment"] != post["config_commitment"]:
        failures.append("protected manager configuration changed during package upgrade")
    if pre["state_commitment"] != post["state_commitment"]:
        failures.append("manager state content changed during package upgrade")
    return failures


def three_host_comparison(pre_values, post_values, old_report, old_commitment, new_report, new_commitment):
    failures = []
    if len(pre_values) != 3 or len(post_values) != 3:
        failures.append("three-host-upgrade requires exactly three pre-upgrade and three post-upgrade files")
    pre_by_alias = {value["host_evidence"]["host_alias"]: value for value in pre_values}
    post_by_alias = {value["host_evidence"]["host_alias"]: value for value in post_values}
    if len(pre_by_alias) != len(pre_values) or len(post_by_alias) != len(post_values):
        failures.append("host aliases are not distinct")
    if set(pre_by_alias) != set(post_by_alias):
        failures.append("pre-upgrade and post-upgrade host aliases differ")
    for alias in sorted(set(pre_by_alias) & set(post_by_alias)):
        for finding in upgrade_comparison(pre_by_alias[alias], post_by_alias[alias], old_report, old_commitment, new_report, new_commitment):
            failures.append(f"{alias}: {finding}")
    return failures


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--phase", required=True, choices=("upgrade", "three-host-upgrade"))
    parser.add_argument("--pre", required=True, nargs="+")
    parser.add_argument("--post", required=True, nargs="+")
    parser.add_argument("--old-candidate-verification", required=True)
    parser.add_argument("--old-contract", required=True)
    parser.add_argument("--new-candidate-verification", required=True)
    parser.add_argument("--new-contract", required=True)
    args = parser.parse_args()
    try:
        pre = [load_upgrade(path) for path in args.pre]
        post = [load_upgrade(path) for path in args.post]
        old_report, old_commitment = BASE.load_candidate(args.old_candidate_verification, args.old_contract)
        new_report, new_commitment = BASE.load_candidate(args.new_candidate_verification, args.new_contract)
        if args.phase == "upgrade":
            if len(pre) != 1 or len(post) != 1:
                parser.error("upgrade requires exactly one --pre and one --post")
            failures = upgrade_comparison(pre[0], post[0], old_report, old_commitment, new_report, new_commitment)
            subject = pre[0]["host_evidence"]["host_alias"]
        else:
            failures = three_host_comparison(pre, post, old_report, old_commitment, new_report, new_commitment)
            subject = ",".join(value["host_evidence"]["host_alias"] for value in pre)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(json.dumps({"status": "FAIL", "error": str(error)}, sort_keys=True))
        return 2
    output = {
        "schema_version": "podmesh-manager-inactive-upgrade-comparison/v1",
        "phase": args.phase,
        "subject": subject,
        "status": "PASS" if not failures else "FAIL",
        "failures": failures,
    }
    print(json.dumps(output, sort_keys=True))
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
