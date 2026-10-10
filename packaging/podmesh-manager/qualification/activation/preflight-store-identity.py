#!/usr/bin/env python3
"""Refuse activation when config.json identity disagrees with the canonical store.

Uses the same read-only ``--inspect-store`` path as capture-host.sh. A missing SQLite
file with no store profile is a fresh host and passes. A configured store must be
inspected even when the legacy SQLite path is absent. Any ``identity_mismatch`` refusal matches resident start
semantics: ``replica_id`` and serde-stable ``topology_json`` in the store must equal
``network.replica_id`` and ``network.manager`` replicas/grants from the installed config.
"""
import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

CONFIG_PATH = Path("/etc/podmesh-manager/config.json")
STATE_DIR = Path("/var/lib/podmesh-manager")
MANAGERD = Path("/usr/lib/podmesh-manager/podmesh-managerd")
TOPOLOGY_JQ = """
  .network as $network |
  ($network.manager.replicas | sort_by(.replica_id, .host_id)) as $replicas |
  ($network.manager.grants | sort_by(.scope, .owner_replica_id)) as $grants |
  {
    logical_manager_id: $network.manager.logical_manager_id,
    replica_id: $network.replica_id,
    topology: {replicas: $replicas, grants: $grants}
  }
"""


def usage():
    print(
        "Usage: preflight-store-identity.py [--config PATH] [--state-dir PATH] [--managerd PATH]",
        file=sys.stderr,
    )
    return 2


def jq_json(program, document):
    result = subprocess.run(
        ["jq", "-ce", program, document],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise ValueError(result.stderr.strip() or "jq failed")
    return json.loads(result.stdout)


def configured_identity(config_path):
    if not config_path.is_file() or config_path.is_symlink():
        raise ValueError("protected manager configuration is missing or symlinked")
    identity = jq_json(TOPOLOGY_JQ, str(config_path))
    store_path = jq_json(".network.database_path", str(config_path))
    if not isinstance(store_path, str) or not store_path.startswith("/"):
        raise ValueError("manager configuration does not name an absolute database path")
    return identity, Path(store_path)


def inspect_store(managerd, config_path, state_dir):
    command = [
        "runuser",
        "-u",
        "podmesh-manager",
        "--",
        str(managerd),
        "--inspect-store",
        "--config",
        str(config_path),
        "--state-dir",
        str(state_dir),
    ]
    return subprocess.run(command, capture_output=True, text=True, check=False)


def refuse_identity_mismatch(identity, detail):
    topology = json.dumps(identity["topology"], separators=(",", ":"), sort_keys=True)
    print(
        "activation preflight refused: identity_mismatch — installed config.json names "
        f"replica_id={identity['replica_id']} logical_manager_id={identity['logical_manager_id']} "
        f"topology={topology}; the canonical store identity row disagrees"
        + (f" ({detail})" if detail else "")
        + ". Align configuration with the migrated store or replace the store before "
        "activation (same refusal the resident applies at start).",
        file=sys.stderr,
    )


def main():
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--config", type=Path, default=CONFIG_PATH)
    parser.add_argument("--state-dir", type=Path, default=STATE_DIR)
    parser.add_argument("--managerd", type=Path, default=MANAGERD)
    args = parser.parse_args()
    if os.geteuid() != 0:
        return usage()
    for path in (args.config, args.managerd):
        if not path.is_file() or path.is_symlink():
            print(f"required file is missing or symlinked: {path}", file=sys.stderr)
            return 2
    if not args.state_dir.is_dir() or args.state_dir.is_symlink():
        print("manager state directory is missing or unsafe", file=sys.stderr)
        return 2
    try:
        identity, store_path = configured_identity(args.config)
    except ValueError as error:
        print(f"activation preflight refused: {error}", file=sys.stderr)
        return 2
    # The resident resolves the profile in state_dir (or PODMESH_STORE_PROFILE).
    # MariaDB deliberately has no SQLite file: absence is not an empty-store proof.
    # Delegate all configured profiles, including malformed ones, to the resident
    # rather than duplicating its engine/credential validation here.
    profile = args.state_dir / "store.json"
    profile_named = (
        "PODMESH_STORE_PROFILE" in os.environ
        or profile.exists()
        or profile.is_symlink()
    )
    if not profile_named and not store_path.exists() and not store_path.is_symlink():
        statedir = store_path.parent
        if not statedir.is_dir() or statedir.is_symlink():
            print("manager state directory is absent or is not a directory", file=sys.stderr)
            return 2
        return 0
    inspected = inspect_store(args.managerd, args.config, args.state_dir)
    combined = f"{inspected.stdout}\n{inspected.stderr}"
    if inspected.returncode != 0:
        if "identity_mismatch" in combined:
            refuse_identity_mismatch(identity, "read-only inspect-store refused")
            return 1
        detail = inspected.stderr.strip() or inspected.stdout.strip() or "inspect-store failed"
        print(f"activation preflight refused: {detail}", file=sys.stderr)
        return 1
    try:
        report = json.loads(inspected.stdout)
    except json.JSONDecodeError:
        print("activation preflight refused: inspect-store returned invalid JSON", file=sys.stderr)
        return 1
    for field in ("logical_manager_id", "replica_id"):
        if report.get(field) != identity[field]:
            refuse_identity_mismatch(
                identity,
                f"inspect-store reported {field}={report.get(field)!r}",
            )
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
