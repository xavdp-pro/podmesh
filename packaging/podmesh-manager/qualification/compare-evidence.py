#!/usr/bin/env python3
"""Compare externally collected host evidence without contacting a host."""
import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

SCHEMA = "podmesh-manager-host-evidence/v5"
EXISTING_PACKAGES = ("podmesh", "podmesh-web-observer")
EXISTING_UNITS = ("podmesh.service", "podmesh-web-observer.service")
EXISTING_SOCKETS = ("/run/podmesh/api.sock", "/run/podmesh-web-observer/api.sock")
SHA256 = re.compile(r"^[a-f0-9]{64}$")
COMMITMENT = re.compile(r"^sha256:[a-f0-9]{64}$")
FINGERPRINT = re.compile(r"^[A-F0-9]{40}$")
ABSENT_ACCOUNT = {
    "name": "podmesh-manager", "present": False, "uid": None,
    "primary_gid": None, "primary_group": None, "home": None, "shell": None,
}
ABSENT_STATE_DIRECTORY = {
    "path": "/var/lib/podmesh-manager", "present": False,
    "file_type": None, "owner": None, "group": None, "mode": None,
    "entry_count": None,
}
CANDIDATE_BINDING_FIELDS = {
    "package", "version", "binary_sha256", "dpkg_verify",
    "regular_payload_files_commitment", "maintainer_scripts_commitment",
    "verification_commitment",
}


def is_sha256(value):
    return isinstance(value, str) and SHA256.fullmatch(value) is not None


def is_commitment(value):
    return isinstance(value, str) and COMMITMENT.fullmatch(value) is not None


def validate_manifest(path, value, key, allowed_names=None, require_nonempty=False):
    if not isinstance(value, list) or (require_nonempty and not value):
        raise ValueError(f"{path}: invalid {key} manifest")
    identity = "path" if key == "regular payload" else "name"
    expected_fields = {identity, "sha256"}
    identities = []
    for entry in value:
        if (not isinstance(entry, dict) or set(entry) != expected_fields or
                not isinstance(entry.get(identity), str) or not entry[identity] or
                not is_sha256(entry.get("sha256"))):
            raise ValueError(f"{path}: invalid {key} manifest entry")
        if identity == "path" and not entry[identity].startswith("/"):
            raise ValueError(f"{path}: invalid regular payload path")
        if allowed_names is not None and entry[identity] not in allowed_names:
            raise ValueError(f"{path}: invalid maintainer script name")
        identities.append(entry[identity])
    if identities != sorted(set(identities)):
        raise ValueError(f"{path}: non-canonical {key} manifest")


def manifest_commitment(value):
    canonical = json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n"
    return "sha256:" + hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def read_json(path):
    with Path(path).open(encoding="utf-8") as handle:
        return json.load(handle)


def validate_candidate_binding(path, binding, nullable):
    if binding is None and nullable:
        return
    if not isinstance(binding, dict) or set(binding) != CANDIDATE_BINDING_FIELDS:
        raise ValueError(f"{path}: invalid candidate binding shape")
    if (binding["package"] != "podmesh-manager" or
            not isinstance(binding["version"], str) or not binding["version"] or
            not is_sha256(binding["binary_sha256"]) or
            binding["dpkg_verify"] != "clean" or
            not is_commitment(binding["regular_payload_files_commitment"]) or
            not is_commitment(binding["maintainer_scripts_commitment"]) or
            not is_commitment(binding["verification_commitment"])):
        raise ValueError(f"{path}: invalid candidate binding values")


def validate_manager_evidence(path, manager, stage):
    required = {"process_count", "account", "config_present", "state_directory", "runtime_present", "candidate_binding"}
    if not isinstance(manager, dict) or set(manager) != required:
        raise ValueError(f"{path}: incomplete manager evidence")
    if type(manager["process_count"]) is not int or manager["process_count"] < 0:
        raise ValueError(f"{path}: invalid manager process count")
    if type(manager["config_present"]) is not bool or type(manager["runtime_present"]) is not bool:
        raise ValueError(f"{path}: invalid manager presence flags")

    account = manager["account"]
    account_fields = {"name", "present", "uid", "primary_gid", "primary_group", "home", "shell"}
    if not isinstance(account, dict) or set(account) != account_fields or account.get("name") != "podmesh-manager" or type(account.get("present")) is not bool:
        raise ValueError(f"{path}: invalid manager account evidence")
    if account["present"]:
        if (type(account["uid"]) is not int or account["uid"] < 0 or
                type(account["primary_gid"]) is not int or account["primary_gid"] < 0 or
                not all(isinstance(account[field], str) and account[field] for field in ("primary_group", "home", "shell"))):
            raise ValueError(f"{path}: incomplete manager account identity")
    elif account != ABSENT_ACCOUNT:
        raise ValueError(f"{path}: manager account absence is not explicit")

    state = manager["state_directory"]
    state_fields = {"path", "present", "file_type", "owner", "group", "mode", "entry_count"}
    if not isinstance(state, dict) or set(state) != state_fields or state.get("path") != "/var/lib/podmesh-manager" or type(state.get("present")) is not bool:
        raise ValueError(f"{path}: invalid manager state directory evidence")
    if state["present"]:
        if (not all(isinstance(state[field], str) and state[field] for field in ("file_type", "owner", "group")) or
                not isinstance(state["mode"], str) or len(state["mode"]) not in (3, 4) or
                any(character not in "01234567" for character in state["mode"]) or
                (state["entry_count"] is not None and
                 (type(state["entry_count"]) is not int or state["entry_count"] < 0))):
            raise ValueError(f"{path}: incomplete manager state directory metadata")
        if state["file_type"] == "directory" and type(state["entry_count"]) is not int:
            raise ValueError(f"{path}: directory entry count is missing")
        if state["file_type"] != "directory" and state["entry_count"] is not None:
            raise ValueError(f"{path}: non-directory has an entry count")
    elif state != ABSENT_STATE_DIRECTORY:
        raise ValueError(f"{path}: manager state directory absence is not explicit")
    validate_candidate_binding(path, manager["candidate_binding"], nullable=(stage == "pre-install"))


def validate_socket(path, socket_path, value):
    fields = {"path", "present", "file_type", "mode", "uid", "gid", "inode", "bytes"}
    if not isinstance(value, dict) or set(value) != fields or value.get("path") != socket_path or type(value.get("present")) is not bool:
        raise ValueError(f"{path}: incomplete socket reading for {socket_path}")
    metadata = ("file_type", "mode", "uid", "gid", "inode", "bytes")
    if value["present"]:
        if (not all(isinstance(value[field], str) and value[field] for field in ("file_type", "mode")) or
                not all(type(value[field]) is int and value[field] >= 0 for field in ("uid", "gid", "inode", "bytes"))):
            raise ValueError(f"{path}: incomplete socket reading for {socket_path}")
    elif any(value[field] is not None for field in metadata):
        raise ValueError(f"{path}: socket absence is not explicit for {socket_path}")


def validate_host_value(path, value):
    required = {"schema_version", "stage", "host_alias", "captured_at_utc", "boot_id_commitment", "packages", "services", "sockets", "podman_rootful", "manager"}
    if not isinstance(value, dict) or set(value) != required or value.get("schema_version") != SCHEMA:
        raise ValueError(f"{path}: invalid evidence schema")
    if value.get("stage") not in ("pre-install", "post-install"):
        raise ValueError(f"{path}: invalid evidence stage")
    if not isinstance(value.get("packages"), dict) or set(value["packages"]) != {*EXISTING_PACKAGES, "podmesh-manager"}:
        raise ValueError(f"{path}: invalid package evidence")
    for package in (*EXISTING_PACKAGES, "podmesh-manager"):
        package_value = value["packages"][package]
        if not isinstance(package_value, dict) or not (
            package_value == {"status": "absent", "version": None}
            or (set(package_value) == {"status", "version"} and package_value.get("status") == "installed" and isinstance(package_value.get("version"), str) and package_value["version"])
        ):
            raise ValueError(f"{path}: incomplete package reading for {package}")
    podman = value.get("podman_rootful")
    commitments = podman.get("commitments", {}) if isinstance(podman, dict) else {}
    if not isinstance(podman, dict) or set(podman) != {"projection_version", "commitments"} or podman.get("projection_version") != "v2" or set(commitments) != {"containers", "images", "volumes", "networks", "pods"} or not all(is_commitment(item) for item in commitments.values()):
        raise ValueError(f"{path}: invalid Podman commitment projection")
    validate_manager_evidence(path, value["manager"], value["stage"])
    if not is_commitment(value.get("boot_id_commitment")):
        raise ValueError(f"{path}: invalid boot commitment")
    if not isinstance(value.get("services"), dict) or set(value["services"]) != {*EXISTING_UNITS, "podmesh-manager.service"}:
        raise ValueError(f"{path}: invalid systemd evidence")
    for unit in (*EXISTING_UNITS, "podmesh-manager.service"):
        unit_value = value["services"][unit]
        required_unit = {"unit", "load_state", "active_state", "sub_state", "unit_file_state", "main_pid", "exec_main_pid", "invocation_id", "start_monotonic_usec", "n_restarts"}
        if (not isinstance(unit_value, dict) or set(unit_value) != required_unit or unit_value["unit"] != unit or
                any(not isinstance(unit_value[field], str) or unit_value[field] == "unknown" for field in ("load_state", "active_state", "sub_state", "unit_file_state")) or
                not all(type(unit_value[field]) is int and unit_value[field] >= 0 for field in ("main_pid", "exec_main_pid", "start_monotonic_usec", "n_restarts")) or
                (unit_value["invocation_id"] is not None and not is_commitment(unit_value["invocation_id"]))):
            raise ValueError(f"{path}: incomplete systemd reading for {unit}")
    if not isinstance(value.get("sockets"), dict) or set(value["sockets"]) != set(EXISTING_SOCKETS):
        raise ValueError(f"{path}: invalid socket evidence")
    for socket_path in EXISTING_SOCKETS:
        validate_socket(path, socket_path, value["sockets"][socket_path])
    return value


def load(path):
    return validate_host_value(path, read_json(path))


def load_candidate(report_path, contract_path):
    report = read_json(report_path)
    contract = read_json(contract_path)
    report_fields = {"schema_version", "verified_at_utc", "package", "version", "architecture", "deb_sha256", "binary_sha256", "source_commit", "signed_metadata", "regular_payload_files", "maintainer_scripts"}
    signed_fields = {"inrelease_signature", "signing_fingerprint", "keyring_sha256", "packages_path", "packages_sha256", "packages_size"}
    if not isinstance(report, dict) or set(report) != report_fields or report.get("schema_version") != "podmesh-manager-candidate-verification/v2":
        raise ValueError(f"{report_path}: invalid candidate verification report")
    signed = report.get("signed_metadata")
    if (report.get("package") != "podmesh-manager" or not isinstance(report.get("version"), str) or not report["version"] or
            report.get("architecture") != "amd64" or not is_sha256(report.get("deb_sha256")) or
            not is_sha256(report.get("binary_sha256")) or not isinstance(report.get("source_commit"), str) or not report["source_commit"] or
            not isinstance(report.get("verified_at_utc"), str) or not report["verified_at_utc"] or
            not isinstance(signed, dict) or set(signed) != signed_fields or signed.get("inrelease_signature") != "verified-by-gpgv-and-pinned-fingerprint" or
            not isinstance(signed.get("signing_fingerprint"), str) or FINGERPRINT.fullmatch(signed["signing_fingerprint"]) is None or
            not is_sha256(signed.get("keyring_sha256")) or not isinstance(signed.get("packages_path"), str) or not signed["packages_path"] or
            not is_sha256(signed.get("packages_sha256")) or type(signed.get("packages_size")) is not int or signed["packages_size"] < 0):
        raise ValueError(f"{report_path}: invalid candidate verification report")
    validate_manifest(report_path, report.get("regular_payload_files"), "regular payload", require_nonempty=True)
    validate_manifest(report_path, report.get("maintainer_scripts"), "maintainer script", allowed_names={"config", "postinst", "postrm", "preinst", "prerm"})
    contract_fields = {"schema_version", "package", "version", "architecture", "deb_sha256", "binary_sha256", "signing_fingerprint", "source_commit", "expected_files", "expected_regular_payload_files", "expected_maintainer_scripts"}
    if not isinstance(contract, dict) or set(contract) != contract_fields or contract.get("schema_version") != "podmesh-manager-candidate-contract/v2":
        raise ValueError(f"{contract_path}: invalid reviewed candidate contract")
    if (contract.get("package") != "podmesh-manager" or not isinstance(contract.get("version"), str) or not contract["version"] or
            contract.get("architecture") != "amd64" or not is_sha256(contract.get("deb_sha256")) or
            not is_sha256(contract.get("binary_sha256")) or not isinstance(contract.get("source_commit"), str) or not contract["source_commit"] or
            not isinstance(contract.get("signing_fingerprint"), str) or FINGERPRINT.fullmatch(contract["signing_fingerprint"]) is None or
            not isinstance(contract.get("expected_files"), list) or not contract["expected_files"] or
            not all(isinstance(item, str) and item.startswith("/") for item in contract["expected_files"]) or
            contract["expected_files"] != sorted(set(contract["expected_files"]))):
        raise ValueError(f"{contract_path}: invalid reviewed candidate contract")
    validate_manifest(contract_path, contract.get("expected_regular_payload_files"), "regular payload", require_nonempty=True)
    validate_manifest(contract_path, contract.get("expected_maintainer_scripts"), "maintainer script", allowed_names={"config", "postinst", "postrm", "preinst", "prerm"})
    comparisons = {
        "package": report["package"], "version": report["version"], "architecture": report["architecture"],
        "deb_sha256": report["deb_sha256"], "binary_sha256": report["binary_sha256"],
        "source_commit": report["source_commit"], "signing_fingerprint": signed["signing_fingerprint"],
    }
    for field, report_value in comparisons.items():
        if contract[field] != report_value:
            raise ValueError(f"candidate report and contract differ for {field}")
    if contract["expected_regular_payload_files"] != report["regular_payload_files"]:
        raise ValueError("candidate report and contract differ for regular payload files")
    if contract["expected_maintainer_scripts"] != report["maintainer_scripts"]:
        raise ValueError("candidate report and contract differ for maintainer scripts")
    report_commitment = "sha256:" + hashlib.sha256(Path(report_path).read_bytes()).hexdigest()
    return report, report_commitment


def manager_absent(value):
    unit = value["services"]["podmesh-manager.service"]
    return (
        value["packages"]["podmesh-manager"] == {"status": "absent", "version": None}
        and unit["load_state"] == "not-found" and unit["active_state"] == "inactive"
        and unit["main_pid"] == 0 and unit["exec_main_pid"] == 0
        and unit["invocation_id"] is None and unit["start_monotonic_usec"] == 0 and unit["n_restarts"] == 0
        and value["manager"]["process_count"] == 0 and value["manager"]["account"] == ABSENT_ACCOUNT
        and not value["manager"]["config_present"] and value["manager"]["state_directory"] == ABSENT_STATE_DIRECTORY
        and not value["manager"]["runtime_present"]
    )


def preexisting_services_present(value):
    failures = []
    for package in EXISTING_PACKAGES:
        if value["packages"][package]["status"] != "installed":
            failures.append(f"pre-install {package} package is not installed")
    for unit in EXISTING_UNITS:
        state = value["services"][unit]
        if state["load_state"] != "loaded" or state["active_state"] != "active" or state["main_pid"] <= 0 or state["main_pid"] != state["exec_main_pid"]:
            failures.append(f"pre-install {unit} is not loaded and active with one proven main PID")
    for socket_path in EXISTING_SOCKETS:
        state = value["sockets"][socket_path]
        if not state["present"] or state["file_type"] != "socket":
            failures.append(f"pre-install {socket_path} is not a socket")
    return failures


def install_comparison(pre, post, report, report_commitment):
    failures = []
    if pre["stage"] != "pre-install" or post["stage"] != "post-install": failures.append("stage pair must be pre-install then post-install")
    if pre["host_alias"] != post["host_alias"]: failures.append("host aliases differ")
    if pre["boot_id_commitment"] != post["boot_id_commitment"]: failures.append("host boot changed between captures")
    if not manager_absent(pre): failures.append("pre-install manager absence is not proven")
    if pre["manager"]["candidate_binding"] is not None: failures.append("pre-install candidate binding exists")
    failures.extend(preexisting_services_present(pre))
    for package in EXISTING_PACKAGES:
        if pre["packages"][package] != post["packages"][package]: failures.append(f"{package} version/status changed")
    for unit in EXISTING_UNITS:
        if pre["services"][unit] != post["services"][unit]: failures.append(f"{unit} state or PID changed")
    for socket_path in EXISTING_SOCKETS:
        if pre["sockets"][socket_path] != post["sockets"][socket_path]: failures.append(f"{socket_path} metadata changed")
    if pre["podman_rootful"] != post["podman_rootful"]: failures.append("rootful Podman commitment changed")
    manager_unit = post["services"]["podmesh-manager.service"]
    if post["packages"]["podmesh-manager"].get("status") != "installed": failures.append("manager package is not installed after operator installation")
    if manager_unit["load_state"] != "loaded" or manager_unit["active_state"] != "inactive" or manager_unit["main_pid"] != 0 or manager_unit["exec_main_pid"] != 0: failures.append("manager unit is active, unloaded or has a PID")
    if manager_unit["unit_file_state"] != "disabled": failures.append("manager unit is not disabled")
    if manager_unit["invocation_id"] is not None or manager_unit["start_monotonic_usec"] != 0 or manager_unit["n_restarts"] != 0: failures.append("manager unit has invocation, start or restart evidence")
    if post["manager"]["process_count"] != 0: failures.append("manager process exists after package-only installation")
    account = post["manager"]["account"]
    if not account["present"]: failures.append("manager system account is absent after package-only installation")
    else:
        if account["uid"] == 0 or account["primary_gid"] == 0: failures.append("manager system account has a root identity")
        if account["primary_group"] != "podmesh-manager": failures.append("manager system account primary group is not podmesh-manager")
        if account["home"] != "/nonexistent": failures.append("manager system account home is not /nonexistent")
        if account["shell"] != "/usr/sbin/nologin": failures.append("manager system account shell is not /usr/sbin/nologin")
    if post["manager"]["config_present"]: failures.append("manager configuration exists after package-only installation")
    state = post["manager"]["state_directory"]
    if not state["present"]: failures.append("manager state directory is absent after package-only installation")
    else:
        if state["file_type"] != "directory": failures.append("manager state path is not a directory")
        if state["owner"] != "podmesh-manager": failures.append("manager state directory owner is not podmesh-manager")
        if state["group"] != "podmesh-manager": failures.append("manager state directory group is not podmesh-manager")
        if state["mode"] != "750": failures.append("manager state directory mode is not 750")
        if state["entry_count"] != 0: failures.append("manager state directory is not empty after package-only installation")
    if post["manager"]["runtime_present"]: failures.append("manager runtime directory exists after package-only installation")
    binding = post["manager"]["candidate_binding"]
    expected_binding = {
        "package": report["package"], "version": report["version"],
        "binary_sha256": report["binary_sha256"], "dpkg_verify": "clean",
        "regular_payload_files_commitment": manifest_commitment(report["regular_payload_files"]),
        "maintainer_scripts_commitment": manifest_commitment(report["maintainer_scripts"]),
        "verification_commitment": report_commitment,
    }
    if binding != expected_binding or post["packages"]["podmesh-manager"].get("version") != report["version"]:
        failures.append("post-install candidate binding does not match the verified report and contract")
    return failures


def three_host_comparison(pre_values, post_values, report, report_commitment):
    failures = []
    if len(pre_values) != 3 or len(post_values) != 3: failures.append("three-host phase requires exactly three pre-install and three post-install evidence files")
    pre_by_alias = {value["host_alias"]: value for value in pre_values}
    post_by_alias = {value["host_alias"]: value for value in post_values}
    if len(pre_by_alias) != len(pre_values) or len(post_by_alias) != len(post_values): failures.append("host aliases are not distinct")
    if set(pre_by_alias) != set(post_by_alias): failures.append("pre-install and post-install host aliases differ")
    for alias in sorted(set(pre_by_alias) & set(post_by_alias)):
        for finding in install_comparison(pre_by_alias[alias], post_by_alias[alias], report, report_commitment):
            failures.append(f"{alias}: {finding}")
    bindings = [value["manager"]["candidate_binding"] for value in post_values]
    if bindings and any(binding != bindings[0] for binding in bindings[1:]): failures.append("post-install hosts do not bind the same candidate")
    return failures


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--phase", required=True, choices=("install", "three-host"))
    parser.add_argument("--pre", required=True, nargs="+")
    parser.add_argument("--post", required=True, nargs="+")
    parser.add_argument("--candidate-verification", required=True)
    parser.add_argument("--contract", required=True)
    args = parser.parse_args()
    try:
        pre = [load(path) for path in args.pre]
        post = [load(path) for path in args.post]
        report, report_commitment = load_candidate(args.candidate_verification, args.contract)
        if args.phase == "install":
            if len(pre) != 1 or len(post) != 1: parser.error("install requires exactly one --pre and one --post")
            failures = install_comparison(pre[0], post[0], report, report_commitment)
            subject = pre[0]["host_alias"]
        else:
            failures = three_host_comparison(pre, post, report, report_commitment)
            subject = ",".join(value["host_alias"] for value in pre)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(json.dumps({"status": "FAIL", "error": str(error)}, sort_keys=True))
        return 2
    report_output = {"schema_version": "podmesh-manager-evidence-comparison/v3", "phase": args.phase, "subject": subject, "status": "PASS" if not failures else "FAIL", "failures": failures}
    print(json.dumps(report_output, sort_keys=True))
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
