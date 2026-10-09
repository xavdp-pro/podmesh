#!/usr/bin/env python3
"""Fail-closed shape check for campaign-plan.json (public example and operator copies)."""
import json
import re
import sys
from pathlib import Path

HOSTS = ("lab-a", "lab-b", "lab-c")
OP_ID = re.compile(r"^op-[a-z0-9]{10,64}$")


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    if len(sys.argv) != 2:
        fail(f"usage: {Path(sys.argv[0]).name} <campaign-plan.json>")
    path = Path(sys.argv[1])
    try:
        plan = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as problem:
        fail(f"{path}: not valid JSON ({problem})")
    if not isinstance(plan, dict):
        fail(f"{path}: root must be an object")

    for key in ("schema", "campaign", "host_order", "operation_ids", "observation_value", "convergence_wait", "does_not_qualify"):
        if key not in plan:
            fail(f"{path}: missing required key {key!r}")

    if plan["schema"] != "podmesh-manager-live-activation-campaign-plan/v1":
        fail(f"{path}: unsupported schema {plan['schema']!r}")

    if not isinstance(plan["campaign"], str) or not plan["campaign"].strip():
        fail(f"{path}: campaign must be a non-empty string")
    if "example" not in plan["campaign"].lower() and "test" not in plan["campaign"].lower():
        fail(f"{path}: public plans must name themselves example or test campaigns")

    order = plan["host_order"]
    if order != list(HOSTS):
        fail(f"{path}: host_order must be exactly {list(HOSTS)}")

    op_ids = plan["operation_ids"]
    if not isinstance(op_ids, dict):
        fail(f"{path}: operation_ids must be an object")
    for host in HOSTS:
        op = op_ids.get(host)
        if not isinstance(op, str) or not OP_ID.match(op):
            fail(f"{path}: operation_ids[{host!r}] must match {OP_ID.pattern}")
        if op_ids.get(host) == op_ids.get("lab-a") and host != "lab-a":
            fail(f"{path}: operation_ids must be distinct per host")

    value = plan["observation_value"]
    if not isinstance(value, str) or not value.strip():
        fail(f"{path}: observation_value must be a non-empty string")

    wait = plan["convergence_wait"]
    if not isinstance(wait, dict):
        fail(f"{path}: convergence_wait must be an object")
    for key in ("max_polls", "poll_seconds"):
        if key not in wait or not isinstance(wait[key], int) or wait[key] <= 0:
            fail(f"{path}: convergence_wait.{key} must be a positive integer")

    dnq = plan["does_not_qualify"]
    if not isinstance(dnq, list) or not dnq or not all(isinstance(x, str) and x.strip() for x in dnq):
        fail(f"{path}: does_not_qualify must be a non-empty list of strings")

    print(json.dumps({"status": "PASS", "campaign": plan["campaign"], "hosts": list(HOSTS)}, sort_keys=True))


if __name__ == "__main__":
    main()
