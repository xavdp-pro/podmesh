#!/usr/bin/env python3
"""Wait for the exact manager process and typed local control interface."""
import argparse
import json
import os
import socket
import subprocess
import sys
import time

SOCKET_PATH = "/run/podmesh-manager/control.sock"
BINARY_PATH = "/usr/lib/podmesh-manager/podmesh-managerd"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--timeout-seconds", type=float, default=30.0)
    args = parser.parse_args()
    if os.geteuid() != 0 or not (0 < args.timeout_seconds <= 30):
        return 2
    deadline = time.monotonic() + args.timeout_seconds
    last = "not ready"
    while time.monotonic() < deadline:
        try:
            result = subprocess.run(
                ["systemctl", "show", "podmesh-manager.service", "--no-page",
                 "-p", "ActiveState", "-p", "SubState", "-p", "MainPID"],
                check=True, capture_output=True, text=True,
            )
            state = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
            pid = int(state.get("MainPID", "0"))
            if state.get("ActiveState") != "active" or state.get("SubState") != "running" or pid <= 0:
                raise RuntimeError("unit is not active/running")
            if os.path.realpath(f"/proc/{pid}/exe") != BINARY_PATH:
                raise RuntimeError("MainPID executable differs from packaged manager")
            client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            client.settimeout(1.0)
            client.connect(SOCKET_PATH)
            client.sendall(b'{"operation":"status"}')
            client.shutdown(socket.SHUT_WR)
            response = bytearray()
            while len(response) <= 32768:
                chunk = client.recv(4096)
                if not chunk:
                    break
                response.extend(chunk)
            client.close()
            value = json.loads(response)
            if len(response) > 32768 or value.get("kind") != "resident_observation":
                raise RuntimeError("typed status response is invalid")
            print(json.dumps({
                "schema_version": "podmesh-manager-readiness/v1",
                "unit_active": True,
                "main_pid_executable_verified": True,
                "typed_status_verified": True,
            }, sort_keys=True))
            return 0
        except (OSError, ValueError, json.JSONDecodeError, RuntimeError, subprocess.SubprocessError) as error:
            last = str(error)
            time.sleep(0.05)
    print(f"manager readiness refused: {last}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
