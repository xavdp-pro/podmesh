#!/usr/bin/env python3
"""Refuse storage migrate when identity.topology_json is not canonical Topology JSON.

The manager journal stores ``topology_json`` as ``serde_json::to_string`` of
``manager_ha::Topology`` (``logical_manager_id``, ``replicas`` map, ``scope_owners`` map).
Legacy lab placeholders such as ``{"lab":"g6-fixture-..."}`` are not valid and will
fail resident open and converged capture identity checks.

Run on a workstation against ``--from-sqlite`` before ``podmesh-storage-migrate``; optional
``--config`` compares the row to config-derived topology (same contract as capture).
"""
import argparse
import json
import sqlite3
import sys
from pathlib import Path

TOPOLOGY_KEYS = frozenset({"logical_manager_id", "replicas", "scope_owners"})
REPLICA_KEYS = frozenset({"replica_id", "host_id"})


def usage():
    print(
        "Usage: preflight-store-topology.py --from-sqlite PATH [--config PATH]\n"
        "       preflight-store-topology.py --topology-json TEXT [--config PATH]",
        file=sys.stderr,
    )
    return 2


def scopes_overlap(left: str, right: str) -> bool:
    if left == right:
        return True
    if left.startswith(right) and left[len(right) :].startswith("/"):
        return True
    if right.startswith(left) and right[len(left) :].startswith("/"):
        return True
    return False


def serde_topology_string(topology: dict) -> str:
    """Match ``serde_json::to_string`` on ``manager_ha::Topology`` (struct field order)."""
    replicas_out = {}
    for replica_id in sorted(topology["replicas"]):
        replica = topology["replicas"][replica_id]
        replicas_out[replica_id] = {
            "replica_id": replica["replica_id"],
            "host_id": replica["host_id"],
        }
    ordered = {
        "logical_manager_id": topology["logical_manager_id"],
        "replicas": replicas_out,
        "scope_owners": dict(sorted(topology["scope_owners"].items())),
    }
    return json.dumps(ordered, separators=(",", ":"), sort_keys=False)


def topology_from_config(config_path: Path) -> tuple[str, dict]:
    try:
        config = json.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"protected manager configuration is not valid JSON ({error})")
    if config_path.is_symlink():
        raise ValueError("protected manager configuration is symlinked")
    try:
        network = config["network"]
        manager = network["manager"]
        replica_id = network["replica_id"]
        replicas = manager["replicas"]
        grants = manager["grants"]
        logical_manager_id = manager["logical_manager_id"]
    except (KeyError, TypeError) as error:
        raise ValueError("configuration lacks the manager topology") from error
    if not isinstance(replica_id, str) or not replica_id:
        raise ValueError("network.replica_id must be a non-empty string")
    built, _replica_id = build_topology(logical_manager_id, replicas, grants)
    return replica_id, built


def build_topology(logical_manager_id, replicas, grants):
    if not isinstance(logical_manager_id, str) or not logical_manager_id:
        raise ValueError("logical_manager_id must be a non-empty string")
    if not isinstance(replicas, list) or not replicas:
        raise ValueError("topology requires at least one replica")
    if not isinstance(grants, list):
        raise ValueError("topology grants must be a list")

    by_id = {}
    hosts = set()
    for replica in replicas:
        if not isinstance(replica, dict):
            raise ValueError("every topology replica must be a JSON object")
        replica_id = replica.get("replica_id")
        host_id = replica.get("host_id")
        if (
            not isinstance(replica_id, str)
            or not replica_id
            or not isinstance(host_id, str)
            or not host_id
        ):
            raise ValueError("replica and host identities must be non-empty strings")
        if replica_id in by_id:
            raise ValueError("replica and host identities must be non-empty and unique")
        if host_id in hosts:
            raise ValueError("replica and host identities must be non-empty and unique")
        extra = set(replica) - REPLICA_KEYS
        if extra:
            raise ValueError(f"topology replica has unknown fields: {sorted(extra)}")
        by_id[replica_id] = {"replica_id": replica_id, "host_id": host_id}
        hosts.add(host_id)

    scope_owners = {}
    for grant in grants:
        if not isinstance(grant, dict):
            raise ValueError("every scope grant must be a JSON object")
        scope = grant.get("scope")
        owner = grant.get("owner_replica_id")
        if not isinstance(scope, str) or not scope or not isinstance(owner, str) or not owner:
            raise ValueError("every scope must have exactly one known replica owner")
        if owner not in by_id:
            raise ValueError("every scope must have exactly one known replica owner")
        if any(scopes_overlap(existing, scope) for existing in scope_owners):
            raise ValueError("every scope must have exactly one known replica owner")
        extra = set(grant) - {"scope", "owner_replica_id"}
        if extra:
            raise ValueError(f"topology grant has unknown fields: {sorted(extra)}")
        scope_owners[scope] = owner

    replicas_out = {}
    for replica_id in sorted(by_id):
        replicas_out[replica_id] = by_id[replica_id]
    topology = {
        "logical_manager_id": logical_manager_id,
        "replicas": replicas_out,
        "scope_owners": dict(sorted(scope_owners.items())),
    }
    return topology, None


def parse_topology_json(raw: str) -> dict:
    if not isinstance(raw, str) or not raw:
        raise ValueError("topology_json is empty")
    try:
        document = json.loads(raw)
    except json.JSONDecodeError as error:
        raise ValueError(f"topology_json is not valid JSON ({error})") from error
    if not isinstance(document, dict):
        raise ValueError("topology_json must be a JSON object")
    extra = set(document) - TOPOLOGY_KEYS
    missing = TOPOLOGY_KEYS - set(document)
    if missing or extra:
        raise ValueError(
            "topology_json is not manager_ha::Topology shape "
            f"(expected keys {sorted(TOPOLOGY_KEYS)}; "
            f"missing={sorted(missing) or None}; unknown={sorted(extra) or None})"
        )
    logical_manager_id = document["logical_manager_id"]
    replicas = document["replicas"]
    grants = document["scope_owners"]
    if not isinstance(logical_manager_id, str):
        raise ValueError("logical_manager_id must be a string")
    replica_list = []
    if not isinstance(replicas, dict) or not replicas:
        raise ValueError("topology replicas must be a non-empty object map")
    for key, replica in sorted(replicas.items()):
        if not isinstance(replica, dict):
            raise ValueError("topology replicas must map replica_id to replica objects")
        if replica.get("replica_id") != key:
            raise ValueError("topology replica map keys must match inner replica_id")
        replica_list.append(replica)
    grant_list = []
    if not isinstance(grants, dict):
        raise ValueError("topology scope_owners must be an object map")
    for scope, owner in sorted(grants.items()):
        grant_list.append({"scope": scope, "owner_replica_id": owner})
    built, _ = build_topology(logical_manager_id, replica_list, grant_list)
    return built


def read_identity_sqlite(path: Path) -> tuple[str, str]:
    if not path.is_file() or path.is_symlink():
        raise ValueError("sqlite database path is missing or symlinked")
    try:
        connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    except sqlite3.Error as error:
        raise ValueError(f"cannot open sqlite database ({error})") from error
    try:
        row = connection.execute(
            "SELECT replica_id, topology_json FROM identity WHERE singleton=1"
        ).fetchone()
    except sqlite3.Error as error:
        raise ValueError(f"identity table is missing or unreadable ({error})") from error
    finally:
        connection.close()
    if row is None:
        raise ValueError("identity row is absent (store not initialized)")
    replica_id, topology_json = row
    if not isinstance(replica_id, str) or not isinstance(topology_json, str):
        raise ValueError("identity row has unexpected types")
    return replica_id, topology_json


def refuse(message: str) -> int:
    print(f"storage preflight refused: {message}", file=sys.stderr)
    return 1


def main():
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--from-sqlite", type=Path)
    parser.add_argument("--topology-json")
    parser.add_argument("--config", type=Path)
    args = parser.parse_args()
    if bool(args.from_sqlite) == bool(args.topology_json):
        return usage()
    try:
        if args.from_sqlite:
            replica_id, topology_raw = read_identity_sqlite(args.from_sqlite)
        else:
            replica_id = None
            topology_raw = args.topology_json
        topology = parse_topology_json(topology_raw)
    except ValueError as error:
        return refuse(str(error))
    canonical = serde_topology_string(topology)
    if topology_raw != canonical:
        return refuse(
            "topology_json is valid Topology content but not serde-stable "
            f"(stored length {len(topology_raw)}; canonical length {len(canonical)}). "
            "Rewrite identity.topology_json with the canonical string before migrate."
        )
    if args.config:
        try:
            expected_replica_id, expected_topology = topology_from_config(args.config)
            expected_raw = serde_topology_string(expected_topology)
        except ValueError as error:
            return refuse(str(error))
        if replica_id is not None and replica_id != expected_replica_id:
            return refuse(
                f"identity replica_id={replica_id} disagrees with "
                f"network.replica_id={expected_replica_id} from configuration"
            )
        if canonical != expected_raw:
            return refuse(
                "topology_json disagrees with topology derived from configuration "
                f"(store={canonical}; config={expected_raw})"
            )
    return 0


if __name__ == "__main__":
    sys.exit(main())
