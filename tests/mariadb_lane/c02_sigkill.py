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
import subprocess
import time


def sha(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def load(path):
    return json.loads(Path(path).read_text())


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
    first = load(args.contract)
    resumes = [load(path) for path in args.resume_contract]
    if first.get("status") != "ready" or first.get("server_execution_authorized") is not True:
        raise ValueError("fixture_not_authorized")
    if not first.get("killpoint") or Path(first["kill_event_path"]).exists():
        raise ValueError("killpoint_missing_or_event_reused")
    for resume in resumes:
        if resume.get("role") != "copy_resume" or resume.get("killpoint"):
            raise ValueError("explicit_no_fault_resume_contract_required")
        for key in ("database", "target_resource_identity", "migration_id", "source_manifest_sha256",
                    "resource_caps", "SQL_hashes", "binary_sha256", "source_commit", "writer_lock"):
            if resume.get(key) != first.get(key):
                raise ValueError("resume_contract_binding_changed:" + key)
    if sha(args.binary) != first.get("binary_sha256"):
        raise ValueError("binary_pin_mismatch")
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
            if sealed["source_bundle_before"] != sealed["source_bundle_after"]:
                raise ValueError("original_bundle_changed")
            if sha(output / "snapshot.sqlite") != sealed["snapshot_sha256"]:
                raise ValueError("snapshot_seal_mismatch")
            results.append(sealed)
            continue
        result = load(output)
        if result.get("status") != "PASS" or result.get("serving") is not False:
            raise ValueError("resume_not_verified_nonserving")
        if len(result.get("tables", [])) != 38 or not result["verification"]["typed_full_vectors_compared"]:
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
