#!/usr/bin/env python3
"""Validate the deliberately tiny manager2 network drop-in grammar."""
import argparse
import hashlib
import ipaddress
import json
import sys
from pathlib import Path


def fail(message):
    raise ValueError(message)


def validate(path):
    raw = Path(path).read_bytes()
    if b"\x00" in raw:
        fail("drop-in contains a NUL byte")
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValueError("drop-in is not UTF-8") from error
    section = None
    entries = []
    for number, line in enumerate(text.splitlines(), 1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or stripped.startswith(";"):
            continue
        if stripped.startswith("[") and stripped.endswith("]"):
            if stripped != "[Service]" or section is not None:
                fail(f"line {number}: only one [Service] section is permitted")
            section = "Service"
            continue
        if section != "Service" or "=" not in stripped:
            fail(f"line {number}: invalid drop-in syntax")
        key, value = stripped.split("=", 1)
        entries.append((key, value))
    if section != "Service":
        fail("drop-in lacks [Service]")
    expected = {
        "Environment": ["PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers"],
        "RestrictAddressFamilies": ["AF_UNIX AF_INET"],
    }
    actual = {}
    allows = []
    for key, value in entries:
        if key == "IPAddressAllow":
            allows.append(value)
        elif key in expected:
            actual.setdefault(key, []).append(value)
        else:
            fail(f"unsupported directive {key}")
    if actual != expected:
        fail("network mode or address-family limits differ from the reviewed grammar")
    if len(allows) != 2:
        fail("exactly two peer /32 allowances are required")
    addresses = []
    for value in allows:
        try:
            network = ipaddress.ip_network(value, strict=True)
        except ValueError as error:
            raise ValueError("peer allowance is not a canonical IPv4 /32") from error
        if network.version != 4 or network.prefixlen != 32:
            fail("peer allowance is not an IPv4 /32")
        addresses.append(str(network))
    if len(set(addresses)) != 2:
        fail("peer /32 allowances are duplicated")
    return {
        "sha256": hashlib.sha256(raw).hexdigest(),
        "network_mode": "authenticated-static-peers",
        "address_families": ["AF_UNIX", "AF_INET"],
        "peer_allow_count": 2,
        "peer_allow_prefix_length": 32,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dropin", required=True)
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()
    try:
        value = validate(args.dropin)
    except (OSError, ValueError) as error:
        print(f"drop-in refused: {error}", file=sys.stderr)
        return 2
    if not args.quiet:
        print(json.dumps(value, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
