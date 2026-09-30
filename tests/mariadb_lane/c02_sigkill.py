#!/usr/bin/env python3
"""A-issued synthetic fixture only. SIGKILL own snapshot/copier, retain evidence, resume twice.

No database administration, DSN discovery, reset, cleanup, dump or restore is performed.
The lead supplies every contract and owns resource lifecycle. A controller success proves
only this explicitly selected interruption case, not all C02 or physical power loss.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import time


def read_regular(path, limit, modes):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        opened, named = os.fstat(stream.fileno()), Path(path).lstat()
        if (not stat.S_ISREG(opened.st_mode) or not stat.S_ISREG(named.st_mode)
                or (opened.st_dev, opened.st_ino) != (named.st_dev, named.st_ino)
                or stat.S_IMODE(opened.st_mode) not in modes or opened.st_size > limit):
            raise ValueError("private_regular_input_required")
        data = stream.read(limit + 1)
        if len(data) != opened.st_size:
            raise ValueError("private_input_drift_or_limit")
        return data


def sha(path):
    # Binary may be 0700; control files must be exactly 0600 or 0400.
    return hashlib.sha256(read_regular(path, 268435456, (0o700, 0o600, 0o400))).hexdigest()


def load(path):
    value = json.loads(read_regular(path, 8388608, (0o600, 0o400)))
    if not isinstance(value, dict):
        raise ValueError("private_json_object_required")
    return value


def required(value, key, kind):
    item = value.get(key)
    if (not isinstance(item, kind) or isinstance(item, bool) and kind is int
            or not item or kind is int and item <= 0):
        raise ValueError("required_field_missing_or_invalid:" + key)
    return item


BINDINGS = ("database", "dsn", "target_resource_identity", "migration_id", "source_manifest_path",
            "source_manifest_sha256", "resource_caps", "SQL_hashes", "binary_sha256", "source_commit",
            "writer_lock", "canonical_contract_sha256", "resource_caps_contract_sha256",
            "server_version", "current_user", "session_sql_mode", "database_charset", "database_collation")
PROVENANCE = ("migration_id", "source_commit", "binary_sha256", "source_manifest_sha256", "SQL_hashes",
              "resource_caps", "canonical_contract_sha256", "resource_caps_contract_sha256")


def validate_contract(value, roles):
    if (value.get("schema") != "podmesh-c02-fixture/1" or value.get("status") != "ready"
            or value.get("server_execution_authorized") is not True or value.get("role") not in roles):
        raise ValueError("fixture_not_authorized_or_wrong_role")
    for key in BINDINGS:
        required(value, key, dict if key in ("target_resource_identity", "resource_caps", "SQL_hashes") else str)
    for key in ("container_id", "volume_name", "image_id"):
        required(value["target_resource_identity"], key, str)
    for key in ("maximum_campaign_seconds", "connect_timeout_ms", "lock_wait_timeout_seconds", "statement_timeout_ms"):
        required(value, key, int)
    for key in ("total_original_bundle_bytes", "total_snapshot_bytes", "total_input_scalar_bytes", "total_rows",
                "per_table_rows", "per_table_scalar_bytes", "per_value_bytes", "estimated_peak_memory_bytes"):
        required(value["resource_caps"], key, int)


def pinned_receipt(contract, key, format):
    spec = required(contract, key, dict)
    path, pinned = required(spec, "path", str), required(spec, "sha256", str)
    receipt = load(path)
    if sha(path) != pinned or receipt.get("format") != format or receipt.get("status") != "PASS":
        raise ValueError("prerequisite_receipt_invalid:" + key)
    for binding in PROVENANCE:
        if receipt.get(binding) != contract[binding]:
            raise ValueError("prerequisite_binding_changed:" + binding)
    required(receipt, "plan_sha256", str)
    tables = required(receipt, "tables", list)
    if len(tables) != 38:
        raise ValueError("prerequisite_38table_manifest_missing:" + key)
    return receipt


def validate_inputs(args):
    first = load(args.contract)
    validate_contract(first, ("copy",) if args.operation == "copy" else ("capability", "copy", "copy_resume"))
    required(first, "killpoint", str)
    event = Path(required(first, "kill_event_path", str))
    if event.exists() or event.is_symlink():
        raise ValueError("kill_event_reused")
    resumes = [load(path) for path in args.resume_contract]
    if len(resumes) != 2:
        raise ValueError("exactly_two_resume_contracts_required")
    # Controller intentionally requires copy_resume for both source and data resumes.
    # Rust source snapshot mode accepts more roles, but no role is inferred here.
    for resume in resumes:
        validate_contract(resume, ("copy_resume",))
        if resume.get("killpoint"):
            raise ValueError("explicit_no_fault_resume_contract_required")
        for key in BINDINGS:
            if resume[key] != first[key]:
                raise ValueError("resume_contract_binding_changed:" + key)
    if sha(args.binary) != first["binary_sha256"]:
        raise ValueError("binary_pin_mismatch")
    if args.operation == "copy":
        for contract in (first, *resumes):
            for key in ("capability_receipt", "seeded_restore_receipt"):
                if required(contract, key, dict) != required(first, key, dict):
                    raise ValueError("resume_prerequisite_spec_changed:" + key)
            capability = pinned_receipt(contract, "capability_receipt", "podmesh-c02-capability/1")
            seeded = pinned_receipt(contract, "seeded_restore_receipt", "podmesh-c02-restoration/1")
            if seeded.get("restoration_kind") != "seeded_restore":
                raise ValueError("seeded_restore_role_missing")
            for key in ("plan_sha256", "tables"):
                if seeded[key] != capability[key]:
                    raise ValueError("prerequisite_plan_or_tables_changed:" + key)
    # Snapshot campaigns do not consume any DB receipt. Their checked source manifest,
    # bundle and cooperative leases are validated by the actual Rust child before copying.
    return first, resumes


def write_new(path, value):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as out:
        json.dump(value, out, indent=2)
        out.write("\n")
        out.flush()
        os.fsync(out.fileno())
    directory = os.open(Path(path).parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def spawn(binary, mode, contract, snapshot, seal, output, log):
    args = [str(binary), mode, "--contract", str(contract), "--output", str(output)]
    if mode != "snapshot-campaign":
        args.extend(["--snapshot", str(snapshot), "--seal", str(seal)])
    fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    stream = os.fdopen(fd, "wb")
    process = subprocess.Popen(args, stdout=stream, stderr=subprocess.STDOUT, close_fds=True)
    return process, stream, args


def close_log(stream):
    stream.flush()
    os.fsync(stream.fileno())
    stream.close()


def run(args):
    # Every proof directory is new, private and retained on failure.
    os.mkdir(args.output, 0o700)
    args.created_output = True
    first, resumes = validate_inputs(args)
    evidence = []
    process, stream, command = spawn(args.binary, args.operation, args.contract, args.snapshot,
                                     args.seal, args.output / ("killed-snapshot" if args.operation == "snapshot-campaign" else "killed-attempt.json"),
                                     args.output / "killed-attempt.log")
    deadline = time.monotonic() + first["maximum_campaign_seconds"]
    try:
        event_path = Path(first["kill_event_path"])
        while not event_path.exists():
            if process.poll() is not None:
                raise ValueError("copier_exited_before_requested_SIGKILL")
            if time.monotonic() >= deadline:
                raise TimeoutError("killpoint_observation_timeout")
            time.sleep(0.05)
        # Atomic create does not imply finished JSON write; retry bounded while own child lives.
        while True:
            try:
                event = load(event_path)
                break
            except json.JSONDecodeError:
                if time.monotonic() >= deadline or process.poll() is not None:
                    raise ValueError("incomplete_kill_event")
                time.sleep(0.02)
        if event.get("pid") != process.pid or event.get("point") != first["killpoint"]:
            raise ValueError("kill_event_does_not_belong_to_child")
        if event.get("contract_sha256") != sha(args.contract):
            raise ValueError("kill_event_contract_mismatch")
        os.kill(process.pid, signal.SIGKILL)
        code = process.wait(timeout=10)
        if code != -signal.SIGKILL:
            raise ValueError("child_not_terminated_by_SIGKILL")
        evidence.append({"command": command, "returncode": code, "signal": "SIGKILL",
                         "event": str(event_path), "event_sha256": sha(event_path)})
    finally:
        # A controller failure terminates only its own child; the DB and every artifact stay.
        if process.poll() is None:
            os.kill(process.pid, signal.SIGKILL)
            process.wait(timeout=10)
        close_log(stream)
    results = []
    for index, contract_path in enumerate(args.resume_contract, 1):
        snapshot_phase = args.operation == "snapshot-campaign"
        output = args.output / (f"resume-{index}-snapshot" if snapshot_phase else f"resume-{index}.json")
        log = args.output / f"resume-{index}.log"
        process, stream, command = spawn(args.binary, "snapshot-campaign" if snapshot_phase else "resume", contract_path, args.snapshot,
                                         args.seal, output, log)
        try:
            code = process.wait(timeout=resumes[index-1]["maximum_campaign_seconds"] + 10)
        finally:
            if process.poll() is None:
                os.kill(process.pid, signal.SIGKILL)
                process.wait(timeout=10)
            close_log(stream)
        evidence.append({"command": command, "returncode": code, "log_sha256": sha(log)})
        if code:
            raise ValueError("resume_failed_all_artifacts_retained")
        if snapshot_phase:
            result = json.loads(log.read_text())
            if result.get("status") != "PRIVATE_SNAPSHOT_CREATED_NOT_MIGRATED":
                raise ValueError("snapshot_resume_not_completed")
            sealed = load(output / "seal.json")
            for key in ("source_bundle_before", "source_bundle_after", "snapshot_sha256", "source_manifest_sha256"):
                required(sealed, key, str if key.endswith("sha256") else dict)
            if sealed["source_bundle_before"] != sealed["source_bundle_after"]:
                raise ValueError("original_bundle_changed")
            if sha(output / "snapshot.sqlite") != sealed["snapshot_sha256"]:
                raise ValueError("snapshot_seal_mismatch")
            results.append(sealed)
            continue
        result = load(output)
        if result.get("status") != "PASS" or result.get("serving") is not False:
            raise ValueError("resume_not_verified_nonserving")
        required(result, "verification", dict)
        for key in ("verification_root", "plan_sha256", "snapshot_sha256"):
            required(result, key, str)
        if not isinstance(result.get("table_copy"), list):
            raise ValueError("required_field_missing_or_invalid:table_copy")
        if len(result.get("tables", [])) != 38 or result["verification"].get("typed_full_vectors_compared") is not True:
            raise ValueError("all38_typed_verification_missing")
        results.append(result)
    stable_keys = ("snapshot_sha256", "source_bundle_before", "source_manifest_sha256") if args.operation == "snapshot-campaign" else ("verification_root", "plan_sha256", "snapshot_sha256", "tables")
    for key in stable_keys:
        if results[0][key] != results[1][key]:
            raise ValueError("second_resume_changed_verified_state")
    if args.operation != "snapshot-campaign" and results[1]["table_copy"]:
        raise ValueError("second_resume_performed_new_copy")
    write_new(args.output / "controller-receipt.json", {
        "format": "podmesh-c02-SIGKILL-case/1", "status": "PASS",
        "scope": "ONE_ACTUAL_SYNTHETIC_SIGKILL_CASE_NOT_FULL_C02",
        "operation": args.operation, "point": first["killpoint"], "evidence": evidence,
        "verification_root": results[1].get("verification_root"),
        "snapshot_sha256": results[1]["snapshot_sha256"],
        "source_commit": first["source_commit"], "binary_sha256": sha(args.binary),
        "contracts": {str(path): sha(path) for path in [args.contract, *args.resume_contract]},
        "database": first["database"], "serving": False,
        "cleanup_performed": False, "physical_power_loss_proven": False,
    })


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "contract", "snapshot", "seal", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--operation", choices=("copy", "snapshot-campaign"), required=True)
    parser.add_argument("--resume-contract", type=Path, action="append", required=True)
    options = parser.parse_args()
    if len(options.resume_contract) != 2:
        parser.error("exactly two A-issued resume contracts are required")
    options.created_output = False
    try:
        run(options)
    except Exception as error:
        if options.created_output:
            write_new(options.output / "controller-failure.json", {
                "status": "FAILED_NOT_A_PASS", "reason": str(error),
                "evidence_retained": True, "database_cleanup_performed": False,
            })
        raise
