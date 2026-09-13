#!/usr/bin/env python3
"""Submit one bounded, idempotent observation through the local manager API."""
import argparse
import json
import os
import socket
import sys
import time

SOCKET_PATH = "/run/podmesh-manager/control.sock"


def exchange(request):
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(2.0)
    client.connect(SOCKET_PATH)
    client.sendall(request)
    client.shutdown(socket.SHUT_WR)
    response = bytearray()
    while len(response) <= 32768:
        chunk = client.recv(4096)
        if not chunk:
            break
        response.extend(chunk)
    client.close()
    if len(response) > 32768:
        raise RuntimeError("control response exceeds bound")
    return json.loads(response)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--operation-id", required=True)
    parser.add_argument("--scope", required=True)
    parser.add_argument("--subject", required=True)
    parser.add_argument("--value", required=True)
    parser.add_argument("--timeout-seconds", type=float, default=10.0)
    args = parser.parse_args()
    if os.geteuid() != 0 or not (0 < args.timeout_seconds <= 30):
        return 2
    request = json.dumps({
        "operation": "append_observation",
        "operation_id": args.operation_id,
        "scope": args.scope,
        "subject": args.subject,
        "value": args.value,
    }, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode()
    deadline = time.monotonic() + args.timeout_seconds
    attempts = 0
    while time.monotonic() < deadline:
        attempts += 1
        try:
            response = exchange(request)
        except (OSError, ValueError, json.JSONDecodeError, RuntimeError):
            response = {"error": "append_observation_uncertain"}
        if response.get("response", {}).get("result") == "observed":
            print(json.dumps({
                "schema_version": "podmesh-manager-observation-injection/v1",
                "result": "observed",
                "attempts": attempts,
                "identical_request_retried": attempts > 1,
            }, sort_keys=True))
            return 0
        if response.get("error") not in ("append_observation_busy", "append_observation_uncertain"):
            print("observation refused by typed local API", file=sys.stderr)
            return 1
        time.sleep(0.05)
    print("observation outcome remained uncertain at the campaign deadline", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
