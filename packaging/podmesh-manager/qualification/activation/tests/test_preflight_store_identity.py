#!/usr/bin/env python3
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
MODULE = ROOT / "preflight-store-identity.py"


def load_module():
    import importlib.util

    spec = importlib.util.spec_from_file_location("preflight_store_identity", MODULE)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class PreflightStoreIdentityTests(unittest.TestCase):
    def setUp(self):
        self.module = load_module()

    def test_fresh_store_passes_without_inspect(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = Path(tmp) / "config.json"
            state = Path(tmp) / "state"
            state.mkdir()
            config.write_text(
                json.dumps(
                    {
                        "network": {
                            "replica_id": "replica-a",
                            "database_path": str(state / "manager.sqlite"),
                            "manager": {
                                "logical_manager_id": "logical",
                                "replicas": [{"replica_id": "replica-a", "host_id": "host-a"}],
                                "grants": [],
                            },
                        }
                    }
                )
            )
            managerd = Path(tmp) / "podmesh-managerd"
            managerd.write_text("")
            argv = [
                "preflight-store-identity.py",
                "--config",
                str(config),
                "--state-dir",
                str(state),
                "--managerd",
                str(managerd),
            ]
            with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(sys, "argv", argv), mock.patch.object(
                os, "geteuid", return_value=0
            ), mock.patch.object(self.module, "inspect_store") as inspect:
                self.assertEqual(self.module.main(), 0)
                inspect.assert_not_called()

    def test_configured_store_is_inspected_without_a_sqlite_file(self):
        # A MariaDB store has no SQLite sentinel. Invalid profiles and explicit
        # overrides also must reach the resident's refusal instead of a false PASS.
        for profile_kind in ("mariadb", "malformed", "override"):
            with self.subTest(profile_kind=profile_kind), tempfile.TemporaryDirectory() as tmp:
                state = Path(tmp) / "state"
                state.mkdir()
                config = Path(tmp) / "config.json"
                config.write_text(json.dumps({"network": {
                    "replica_id": "replica-a",
                    "database_path": str(state / "manager.sqlite"),
                    "manager": {
                        "logical_manager_id": "logical",
                        "replicas": [{"replica_id": "replica-a", "host_id": "host-a"}],
                        "grants": [],
                    },
                }}))
                managerd = Path(tmp) / "podmesh-managerd"
                managerd.write_text("")
                environment = {}
                if profile_kind == "override":
                    environment["PODMESH_STORE_PROFILE"] = str(Path(tmp) / "external.json")
                else:
                    (state / "store.json").write_text(
                        '{"store":{"engine":"mariadb"}}' if profile_kind == "mariadb" else "{invalid"
                    )
                argv = ["preflight-store-identity.py", "--config", str(config),
                        "--state-dir", str(state), "--managerd", str(managerd)]
                responses = (
                    (subprocess.CompletedProcess([], 1, "", "resident refused: identity_mismatch"), 1),
                    (subprocess.CompletedProcess([], 0, json.dumps({
                        "replica_id": "replica-a", "logical_manager_id": "logical"
                    }), ""), 0),
                )
                for response, expected in responses:
                    with mock.patch.dict(os.environ, environment, clear=True), mock.patch.object(
                        sys, "argv", argv
                    ), mock.patch.object(os, "geteuid", return_value=0), mock.patch.object(
                        self.module, "inspect_store", return_value=response
                    ) as inspect, mock.patch("sys.stderr", new_callable=io.StringIO):
                        self.assertEqual(self.module.main(), expected)
                        inspect.assert_called_once_with(managerd, config, state)

    def test_identity_mismatch_refusal_message(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = Path(tmp) / "config.json"
            state = Path(tmp) / "state"
            state.mkdir()
            store = state / "manager.sqlite"
            store.write_text("x")
            config.write_text(
                json.dumps(
                    {
                        "network": {
                            "replica_id": "replica-a",
                            "database_path": str(store),
                            "manager": {
                                "logical_manager_id": "logical",
                                "replicas": [{"replica_id": "replica-a", "host_id": "host-a"}],
                                "grants": [],
                            },
                        }
                    }
                )
            )
            managerd = Path(tmp) / "podmesh-managerd"
            managerd.write_text("")
            argv = [
                "preflight-store-identity.py",
                "--config",
                str(config),
                "--state-dir",
                str(state),
                "--managerd",
                str(managerd),
            ]
            refused = subprocess.CompletedProcess(
                args=[],
                returncode=1,
                stdout="",
                stderr="resident refused: refused: identity_mismatch\n",
            )
            with mock.patch.object(sys, "argv", argv), mock.patch.object(
                os, "geteuid", return_value=0
            ), mock.patch.object(self.module, "inspect_store", return_value=refused), mock.patch(
                "sys.stderr", new_callable=io.StringIO
            ) as err:
                self.assertEqual(self.module.main(), 1)
                self.assertIn("identity_mismatch", err.getvalue())
                self.assertIn("replica-a", err.getvalue())


if __name__ == "__main__":
    unittest.main()
