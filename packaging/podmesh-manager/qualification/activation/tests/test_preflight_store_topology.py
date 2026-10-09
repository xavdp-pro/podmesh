#!/usr/bin/env python3
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MODULE = ROOT / "preflight-store-topology.py"
FIXTURE_TOPO = (
    '{"logical_manager_id":"00000000-0000-4000-8000-000000000010",'
    '"replicas":{"00000000-0000-4000-8000-000000000001":'
    '{"replica_id":"00000000-0000-4000-8000-000000000001",'
    '"host_id":"00000000-0000-4000-8000-000000000101"}},'
    '"scope_owners":{"g6-field":"00000000-0000-4000-8000-000000000001"}}'
)
LEGACY_PLACEHOLDER = '{"lab":"g6-fixture-2026-10-07"}'


def load_module():
    import importlib.util

    spec = importlib.util.spec_from_file_location("preflight_store_topology", MODULE)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class PreflightStoreTopologyTests(unittest.TestCase):
    def setUp(self):
        self.module = load_module()

    def test_valid_topology_json_passes(self):
        argv = ["preflight-store-topology.py", "--topology-json", FIXTURE_TOPO]
        with mock_argv(argv):
            self.assertEqual(self.module.main(), 0)

    def test_legacy_placeholder_refused(self):
        argv = ["preflight-store-topology.py", "--topology-json", LEGACY_PLACEHOLDER]
        with mock_argv(argv), captured_stderr() as err:
            self.assertEqual(self.module.main(), 1)
            self.assertIn("not manager_ha::Topology shape", err.getvalue())

    def test_sqlite_identity_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            db = Path(tmp) / "manager.sqlite"
            connection = __import__("sqlite3").connect(db)
            connection.executescript(
                """
                CREATE TABLE identity (
                  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                  replica_id TEXT NOT NULL,
                  topology_json TEXT NOT NULL
                );
                """
            )
            connection.execute(
                "INSERT INTO identity (singleton, replica_id, topology_json) VALUES (1, ?, ?)",
                ("00000000-0000-4000-8000-000000000001", FIXTURE_TOPO),
            )
            connection.commit()
            connection.close()
            argv = ["preflight-store-topology.py", "--from-sqlite", str(db)]
            with mock_argv(argv):
                self.assertEqual(self.module.main(), 0)

    def test_config_mismatch_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = Path(tmp) / "config.json"
            config.write_text(
                json.dumps(
                    {
                        "network": {
                            "replica_id": "other-replica",
                            "manager": {
                                "logical_manager_id": "logical",
                                "replicas": [
                                    {"replica_id": "other-replica", "host_id": "host-a"}
                                ],
                                "grants": [],
                            },
                        }
                    }
                )
            )
            argv = [
                "preflight-store-topology.py",
                "--topology-json",
                FIXTURE_TOPO,
                "--config",
                str(config),
            ]
            with mock_argv(argv), captured_stderr() as err:
                self.assertEqual(self.module.main(), 1)
                self.assertIn("disagrees", err.getvalue())


class mock_argv:
    def __init__(self, argv):
        self.argv = argv

    def __enter__(self):
        import unittest.mock as mock

        self._patch = mock.patch.object(sys, "argv", self.argv)
        self._patch.start()
        return self

    def __exit__(self, *args):
        self._patch.stop()


class captured_stderr:
    def __enter__(self):
        import unittest.mock as mock

        self._patch = mock.patch("sys.stderr", new_callable=io.StringIO)
        self.err = self._patch.start()
        return self.err

    def __exit__(self, *args):
        self._patch.stop()


if __name__ == "__main__":
    unittest.main()
