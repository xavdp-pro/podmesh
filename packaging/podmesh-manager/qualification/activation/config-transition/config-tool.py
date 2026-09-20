#!/usr/bin/env python3
"""Build and verify the exact three-grant manager2 configuration transition."""

import argparse
import copy
import json
import os
import sys
from pathlib import Path

MAX_BYTES = 1024 * 1024
ALIASES = ("lab-a", "lab-b", "lab-c")
SCOPES = {alias: f"g2/{alias}/observations" for alias in ALIASES}
TRANSITIONS = ("three-grants", "incoming-workers")
# The second exact transition. Live activation showed that a resident with one incoming worker drops every
# concurrent peer connection without a reply, and the peer keeps that exchange as a permanent uncertain
# attempt; with one outgoing worker per replica and two declared peers, a limit of two admits every peer.
INCOMING_WORKERS_FROM = 1
INCOMING_WORKERS_TO = 2


class Refusal(ValueError):
    pass


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise Refusal(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def load_json(path):
    raw = Path(path).read_bytes()
    if len(raw) > MAX_BYTES:
        raise Refusal(f"{path}: document exceeds 1 MiB")
    try:
        return json.loads(raw, object_pairs_hook=unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise Refusal(f"{path}: invalid JSON") from error


def mapping(path):
    document = load_json(path)
    if not isinstance(document, dict) or set(document) != {"schema_version", "aliases"}:
        raise Refusal("mapping must contain only schema_version and aliases")
    if document["schema_version"] != "podmesh-manager-alias-replica-map/v1":
        raise Refusal("unsupported mapping schema")
    aliases = document["aliases"]
    if not isinstance(aliases, dict) or set(aliases) != set(ALIASES):
        raise Refusal("mapping must contain exactly lab-a, lab-b and lab-c")
    if any(not isinstance(value, str) or not value for value in aliases.values()):
        raise Refusal("every alias must map to one nonempty replica identifier")
    if len(set(aliases.values())) != 3:
        raise Refusal("aliases must map to three distinct replica identifiers")
    return aliases


def manager(config):
    try:
        network = config["network"]
        result = network["manager"]
        replicas = result["replicas"]
        grants = result["grants"]
        local = network["replica_id"]
    except (KeyError, TypeError) as error:
        raise Refusal("configuration lacks the manager topology") from error
    if not isinstance(config, dict) or not isinstance(network, dict) or not isinstance(result, dict):
        raise Refusal("configuration topology must be JSON objects")
    if not isinstance(replicas, list) or not isinstance(grants, list) or not isinstance(local, str):
        raise Refusal("replicas and grants must be lists and replica_id must be a string")
    replica_ids = []
    for replica in replicas:
        if not isinstance(replica, dict) or not isinstance(replica.get("replica_id"), str):
            raise Refusal("every topology replica requires a string replica_id")
        replica_ids.append(replica["replica_id"])
    if len(replica_ids) != 3 or len(set(replica_ids)) != 3:
        raise Refusal("configuration must declare exactly three distinct replicas")
    return result, local, set(replica_ids)


def expected_grants(alias_map):
    return [
        {"scope": SCOPES[alias], "owner_replica_id": alias_map[alias]}
        for alias in ALIASES
    ]


def build(original, alias_map, host_alias, transition="three-grants"):
    topology, local, replica_ids = manager(original)
    if host_alias not in ALIASES or local != alias_map[host_alias]:
        raise Refusal("host alias does not map to the local replica")
    if replica_ids != set(alias_map.values()):
        raise Refusal("mapping replica set differs from the declared topology")
    candidate = copy.deepcopy(original)
    if transition == "three-grants":
        if topology["grants"] != []:
            raise Refusal("transition source must have exactly zero grants")
        candidate["network"]["manager"]["grants"] = expected_grants(alias_map)
    elif transition == "incoming-workers":
        if topology["grants"] != expected_grants(alias_map):
            raise Refusal("transition source must carry exactly the three reviewed grants")
        workers = original.get("incoming_workers")
        if type(workers) is not int or workers != INCOMING_WORKERS_FROM:
            raise Refusal(f"transition source must have incoming_workers exactly {INCOMING_WORKERS_FROM}")
        candidate["incoming_workers"] = INCOMING_WORKERS_TO
    else:
        raise Refusal("unsupported transition")
    return candidate


def verify_applied(original, applied, alias_map, host_alias, transition="three-grants"):
    expected = build(original, alias_map, host_alias, transition)
    if transition == "incoming-workers":
        # Python equality would accept the JSON float 2.0 for the integer 2; the manager's own parser would not.
        workers = applied.get("incoming_workers")
        if type(workers) is not int or workers != INCOMING_WORKERS_TO:
            raise Refusal(f"applied incoming_workers must be the integer {INCOMING_WORKERS_TO}")
    if applied != expected:
        raise Refusal("applied configuration differs from the exact additive transition")
    topology, local, replicas = manager(applied)
    shape = {
        "schema_version": "podmesh-manager-config-shape-verification/v1",
        "host_alias": host_alias,
        "local_replica_matches_mapping": local == alias_map[host_alias],
        "replica_count": len(replicas),
        "grant_count": len(topology["grants"]),
        "grant_scopes": [entry["scope"] for entry in topology["grants"]],
        "all_other_values_preserved": True,
    }
    if transition == "incoming-workers":
        shape.update({
            "schema_version": "podmesh-manager-config-shape-verification/v2",
            "transition_kind": transition,
            "changed_keys": ["incoming_workers"],
            "incoming_workers": {"from": INCOMING_WORKERS_FROM, "to": INCOMING_WORKERS_TO},
        })
    return shape


def write_json(path, value):
    target = Path(path)
    with target.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, sort_keys=True, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=("prepare", "verify"), required=True)
    parser.add_argument("--original", required=True)
    parser.add_argument("--mapping", required=True)
    parser.add_argument("--host-alias", choices=ALIASES, required=True)
    parser.add_argument("--candidate")
    parser.add_argument("--output")
    parser.add_argument("--transition", choices=TRANSITIONS, default="three-grants")
    args = parser.parse_args()
    try:
        original = load_json(args.original)
        alias_map = mapping(args.mapping)
        if args.mode == "prepare":
            if not args.output or args.candidate:
                raise Refusal("prepare requires --output and forbids --candidate")
            write_json(args.output, build(original, alias_map, args.host_alias, args.transition))
        else:
            if not args.candidate or args.output:
                raise Refusal("verify requires --candidate and forbids --output")
            print(json.dumps(verify_applied(
                original, load_json(args.candidate), alias_map, args.host_alias, args.transition
            ), sort_keys=True))
        return 0
    except (OSError, Refusal) as error:
        print(f"configuration transition refused: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
