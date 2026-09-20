#!/usr/bin/env python3
"""Offline unit tests for the bounded activation control helpers."""
import importlib.util
import io
import json
import pathlib
import sys
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


graceful = load("graceful_shutdown", "graceful-shutdown.py")
ready = load("wait_ready", "wait-ready.py")


class FakeSocket:
    def __init__(self, response):
        self.parts = [response, b""]
        self.request = b""

    def settimeout(self, _value):
        pass

    def connect(self, path):
        if path != "/run/podmesh-manager/control.sock":
            raise OSError("wrong socket")

    def sendall(self, value):
        self.request += value

    def shutdown(self, _direction):
        pass

    def recv(self, _size):
        return self.parts.pop(0)

    def close(self):
        pass


class GracefulShutdownTests(unittest.TestCase):
    @mock.patch.object(graceful.os, "geteuid", return_value=0)
    @mock.patch.object(graceful.os.path, "lexists", return_value=False)
    def test_exact_typed_shutdown_and_clean_exit(self, _lexists, _uid):
        client = FakeSocket(b'{"shutdown_requested":true}')
        state = {
            "ActiveState": "inactive", "SubState": "dead", "MainPID": "0",
            "Result": "success", "ExecMainCode": "exited", "ExecMainStatus": "0",
        }
        with (
            mock.patch.object(graceful.socket, "socket", return_value=client),
            mock.patch.object(graceful, "unit_state", return_value=state),
            mock.patch.object(sys, "argv", ["graceful-shutdown.py"]),
            mock.patch("sys.stdout", new_callable=io.StringIO) as output,
        ):
            self.assertEqual(graceful.main(), 0)
        self.assertEqual(client.request, b'{"operation":"shutdown"}')
        self.assertFalse(json.loads(output.getvalue())["forced_signal_used"])

    @mock.patch.object(graceful.os, "geteuid", return_value=0)
    def test_malformed_acknowledgement_is_refused(self, _uid):
        with (
            mock.patch.object(graceful.socket, "socket", return_value=FakeSocket(b'{"ok":true}')),
            mock.patch.object(sys, "argv", ["graceful-shutdown.py"]),
        ):
            self.assertEqual(graceful.main(), 1)

    @mock.patch.object(graceful.os, "geteuid", return_value=0)
    @mock.patch.object(graceful.os.path, "lexists", return_value=True)
    def test_timeout_does_not_claim_clean_shutdown(self, _lexists, _uid):
        clock = iter([0.0, 0.0, 31.0])
        with (
            mock.patch.object(graceful.socket, "socket", return_value=FakeSocket(b'{"shutdown_requested":true}')),
            mock.patch.object(graceful, "unit_state", return_value={"ActiveState": "active"}),
            mock.patch.object(graceful.time, "monotonic", side_effect=lambda: next(clock)),
            mock.patch.object(sys, "argv", ["graceful-shutdown.py"]),
        ):
            self.assertEqual(graceful.main(), 1)


class ReadinessTests(unittest.TestCase):
    @mock.patch.object(ready.os, "geteuid", return_value=0)
    @mock.patch.object(ready.os.path, "realpath", return_value="/usr/lib/podmesh-manager/podmesh-managerd")
    def test_typed_status_proves_readiness(self, _realpath, _uid):
        completed = mock.Mock(stdout="ActiveState=active\nSubState=running\nMainPID=123\n")
        client = FakeSocket(b'{"kind":"resident_observation"}')
        with (
            mock.patch.object(ready.subprocess, "run", return_value=completed),
            mock.patch.object(ready.socket, "socket", return_value=client),
            mock.patch.object(sys, "argv", ["wait-ready.py"]),
        ):
            self.assertEqual(ready.main(), 0)
        self.assertEqual(client.request, b'{"operation":"status"}')

    @mock.patch.object(ready.os, "geteuid", return_value=0)
    @mock.patch.object(ready.os.path, "realpath", return_value="/tmp/wrong")
    def test_wrong_main_pid_executable_times_out(self, _realpath, _uid):
        completed = mock.Mock(stdout="ActiveState=active\nSubState=running\nMainPID=123\n")
        clock = iter([0.0, 0.0, 31.0])
        with (
            mock.patch.object(ready.subprocess, "run", return_value=completed),
            mock.patch.object(ready.time, "monotonic", side_effect=lambda: next(clock)),
            mock.patch.object(sys, "argv", ["wait-ready.py"]),
        ):
            self.assertEqual(ready.main(), 1)

    @mock.patch.object(ready.os, "geteuid", return_value=0)
    @mock.patch.object(ready.os.path, "realpath", return_value="/usr/lib/podmesh-manager/podmesh-managerd")
    def test_invalid_status_response_times_out(self, _realpath, _uid):
        completed = mock.Mock(stdout="ActiveState=active\nSubState=running\nMainPID=123\n")
        clock = iter([0.0, 0.0, 31.0])
        with (
            mock.patch.object(ready.subprocess, "run", return_value=completed),
            mock.patch.object(ready.socket, "socket", return_value=FakeSocket(b'{"error":"status_unavailable"}')),
            mock.patch.object(ready.time, "monotonic", side_effect=lambda: next(clock)),
            mock.patch.object(sys, "argv", ["wait-ready.py"]),
        ):
            self.assertEqual(ready.main(), 1)


if __name__ == "__main__":
    unittest.main()
