#!/usr/bin/env python3
"""Request and prove the manager's bounded application-level shutdown."""
import argparse
import json
import os
import socket
import subprocess
import sys
import time

SOCKET_PATH = "/run/podmesh-manager/control.sock"
REQUEST = b'{"operation":"shutdown"}'
EXPECTED = {"shutdown_requested": True}


def unit_state():
    result = subprocess.run(
        ["systemctl", "show", "podmesh-manager.service", "--no-page",
         "-p", "ActiveState", "-p", "SubState", "-p", "MainPID",
         "-p", "Result", "-p", "ExecMainCode", "-p", "ExecMainStatus"],
        check=True, capture_output=True, text=True,
    )
    return dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--timeout-seconds", type=float, default=30.0)
    args = parser.parse_args()
    if os.geteuid() != 0 or not (0 < args.timeout_seconds <= 30):
        print("graceful shutdown requires root and a timeout in (0, 30]", file=sys.stderr)
        return 2
    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.settimeout(2.0)
        client.connect(SOCKET_PATH)
        client.sendall(REQUEST)
        client.shutdown(socket.SHUT_WR)
        response = bytearray()
        while len(response) <= 32768:
            chunk = client.recv(4096)
            if not chunk:
                break
            response.extend(chunk)
        client.close()
        if len(response) > 32768 or json.loads(response) != EXPECTED:
            raise RuntimeError("manager did not acknowledge the typed shutdown request")
        deadline = time.monotonic() + args.timeout_seconds
        while time.monotonic() < deadline:
            state = unit_state()
            clean = (
                state.get("ActiveState"), state.get("SubState"), state.get("MainPID"),
                state.get("Result"), state.get("ExecMainStatus"),
            ) == ("inactive", "dead", "0", "success", "0") and state.get("ExecMainCode") in {
                "exited", "0", "1",
            }
            if clean and not os.path.lexists(SOCKET_PATH):
                print(json.dumps({
                    "schema_version": "podmesh-manager-graceful-shutdown/v1",
                    "typed_request_acknowledged": True,
                    "process_exited_successfully": True,
                    "service_inactive": True,
                    "control_socket_absent": True,
                    "forced_signal_used": False,
                }, sort_keys=True))
                return 0
            time.sleep(0.05)
        raise RuntimeError("manager did not complete bounded graceful shutdown")
    except (OSError, ValueError, json.JSONDecodeError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"graceful shutdown refused: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
