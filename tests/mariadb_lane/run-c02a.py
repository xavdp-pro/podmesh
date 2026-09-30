#!/usr/bin/env python3
"""Run only the explicit C02A fixture test; no lifecycle, reset, copy or cutover."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

TEST = "store::parity_tests::server::c02a_populated_node_scalar_parity_and_expected_f6_counterexamples"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--contract", type=Path, required=True)
    parser.add_argument("--evidence-dir", type=Path, required=True, help="New private directory; never overwrite old evidence")
    args = parser.parse_args()
    contract_bytes = args.contract.read_bytes()
    contract = json.loads(contract_bytes)
    if not (contract.get("task_id") == "A02R" and contract.get("status") == "ready"
            and contract.get("network") == "none" and contract.get("synthetic_only") is True
            and contract.get("C02A_execution_authorized") is True):
        parser.error("A02R ready/execution attestation is required; no connection attempted")
    # This is the lead's attestation and test-process scoping, not production OS isolation.
    dsn = contract.get("DSN")
    if not isinstance(dsn, str) or not dsn:
        parser.error("A02R fixture DSN missing; no connection attempted")
    args.evidence_dir.mkdir(mode=0o700, parents=False, exist_ok=False)
    os.chmod(args.evidence_dir, 0o700)
    root = Path(__file__).resolve().parents[2]
    sources = ["src/store/mod.rs", "src/store/parity_tests.rs", "fixtures/C/node-parity-v1.json", "tests/mariadb_lane/run-c02a.py"]
    hashes = {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in sources}
    env = os.environ.copy()
    for name in ("PODMESH_MARIADB_DSN", "PODMESH_STORE_PROFILE", "PODMESH_C02A_DSN", "PODMESH_C02A_CONTRACT", "PODMESH_C02A_REPORT"):
        env.pop(name, None)
    env.update(PODMESH_C02A_CONTRACT=str(args.contract.resolve()), PODMESH_C02A_DSN=dsn,
               PODMESH_C02A_REPORT=str((args.evidence_dir / "measurement.json").resolve()))
    command = ["cargo", "test", "--offline", "--locked", "--features", "mariadb", "--lib", TEST,
               "--", "--ignored", "--exact", "--nocapture"]
    started = time.monotonic()
    result = subprocess.run(command, cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    receipt = {
        "command": command, "exit_code": result.returncode, "elapsed_seconds": time.monotonic() - started,
        "source_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        "source_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=root)),
        "source_sha256": hashes, "source_unchanged": all(hashlib.sha256((root / name).read_bytes()).hexdigest() == value for name, value in hashes.items()),
        "contract_sha256": hashlib.sha256(contract_bytes).hexdigest(),
        "contract_unchanged": args.contract.read_bytes() == contract_bytes,
        "fixture_id": contract["fixture_id"], "fixture_database": contract["database"],
        "claim": "Independent scalar insert/read measurement, not data movement or runtime qualification",
    }
    for name, data in (("command.log", result.stdout), ("receipt.json", (json.dumps(receipt, indent=2) + "\n").encode()), ("A02R-contract.json", contract_bytes)):
        target = args.evidence_dir / name
        with target.open("xb") as handle:
            os.chmod(target, 0o600)
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
    print(json.dumps({"exit_code": result.returncode, "evidence_dir": str(args.evidence_dir.resolve())}))
    if not receipt["source_unchanged"] or not receipt["contract_unchanged"]:
        return 2
    return result.returncode


if __name__ == "__main__":
    sys.exit(main())
