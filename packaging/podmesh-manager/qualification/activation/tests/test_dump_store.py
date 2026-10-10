#!/usr/bin/env python3
"""Client-call regressions using a recorder, never MariaDB restoration proof."""
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class DumpStore(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name)
        self.password = self.work / "passwd"
        self.password.write_text("recorder-only-test-credential\n")
        self.password.chmod(0o600)
        self.profile = self.work / "store.json"
        self.output = self.work / "manager.sql"
        self.record = self.work / "client-call.json"
        client = self.work / "mariadb-dump"
        client.write_text("""#!/usr/bin/env python3
import json,os,sys
from pathlib import Path
Path(os.environ['DUMP_RECORDER']).write_text(json.dumps({
    'argv':sys.argv[1:], 'password':os.environ.get('MYSQL_PWD')
}))
mode=os.environ.get('DUMP_RECORDER_MODE','ok')
if mode!='empty':
    sys.stdout.write('-- recorder SQL output\\n')
if mode=='fail':
    sys.stderr.write(os.environ['MYSQL_PWD'])
    sys.exit(7)
""")
        client.chmod(0o700)
        self.config = {"user": "podmesh-manager", "database": "podmesh-manager",
                       "host": "127.0.0.1", "port": 33063, "password_file": str(self.password)}

    def run_dump(self, mode="ok"):
        self.profile.write_text(json.dumps({"store": {"engine": "mariadb", "mariadb": self.config}}))
        environment = os.environ.copy()
        environment.update(PATH=str(self.work) + os.pathsep + environment["PATH"],
                           DUMP_RECORDER=str(self.record), DUMP_RECORDER_MODE=mode)
        return subprocess.run([sys.executable, str(ROOT / "dump-store.py"),
                               "--profile", str(self.profile), "--output", str(self.output)],
                              env=environment, capture_output=True, text=True, check=False)

    def assert_private_call(self):
        call = json.loads(self.record.read_text())
        self.assertEqual(call["password"], "recorder-only-test-credential")
        self.assertNotIn("recorder-only-test-credential", " ".join(call["argv"]))
        self.assertIn("--no-defaults", call["argv"])
        self.assertEqual(call["argv"][-2:], ["--", "podmesh-manager"])
        return call["argv"]

    def test_socket_wins_over_tcp_and_dump_is_private(self):
        self.config["socket"] = str(self.work / "private.sock")
        result = self.run_dump()
        self.assertEqual(result.returncode, 0, result.stderr)
        argv = self.assert_private_call()
        self.assertIn("--protocol=SOCKET", argv)
        self.assertIn("--socket=" + self.config["socket"], argv)
        self.assertFalse(any(arg.startswith(("--host=", "--port=")) for arg in argv))
        self.assertGreater(self.output.stat().st_size, 0)
        self.assertEqual(self.output.stat().st_mode & 0o777, 0o600)
        self.assertEqual(list(self.work.glob("*.part")), [])

    def test_tcp_uses_the_configured_instance(self):
        result = self.run_dump()
        self.assertEqual(result.returncode, 0, result.stderr)
        argv = self.assert_private_call()
        self.assertIn("--protocol=TCP", argv)
        self.assertIn("--host=127.0.0.1", argv)
        self.assertIn("--port=33063", argv)

    def test_dsn_socket_and_encoded_credential_are_resolved(self):
        self.config = {"dsn": "mysql://podmesh-manager:recorder-only-test-credential@127.0.0.1:33063/podmesh-manager?socket=%2Fprivate%2Fstore.sock"}
        result = self.run_dump()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--socket=/private/store.sock", self.assert_private_call())

    def test_failed_or_empty_client_keeps_prior_dump_and_removes_partial(self):
        for mode in ("fail", "empty"):
            with self.subTest(mode=mode):
                self.output.write_text("prior completed dump\n")
                result = self.run_dump(mode)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.output.read_text(), "prior completed dump\n")
                self.assertNotIn("recorder-only-test-credential", result.stdout + result.stderr)
                self.assertEqual(list(self.work.glob("*.part")), [])

    def test_invalid_or_shared_credential_refuses_before_client(self):
        self.password.chmod(0o644)
        result = self.run_dump()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.record.exists())
        self.assertFalse(self.output.exists())

    def test_failure_never_publishes_a_new_dump(self):
        result = self.run_dump("fail")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exit 7", result.stderr)
        self.assertFalse(self.output.exists())
        self.assertEqual(list(self.work.glob("*.part")), [])


if __name__ == "__main__":
    unittest.main()
