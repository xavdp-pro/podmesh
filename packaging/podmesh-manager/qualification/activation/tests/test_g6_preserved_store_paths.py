#!/usr/bin/env python3
"""Unit regression for the resident's direct-child boundary during G6 preservation.

Executes the campaign's actual jq transformation, without SSH, MariaDB or
activation. This proves configuration path rebinding, not dump restoration.
"""
import json
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class PreservedStorePaths(unittest.TestCase):
    def test_mariadb_inspection_config_stays_inside_temporary_state(self):
        source = (ROOT / "campaign/run-campaign.sh").read_text()
        maria = source.split('cp -p -- "\\$profile" "\\$d/store.json"', 1)[1]
        maria = maria.split('\nelse\n', 1)[0]
        command = next(line.strip() for line in maria.splitlines() if line.strip().startswith("jq "))
        program = re.search(r"'([^']*)'", command).group(1)
        # This command lives in an unquoted SSH heredoc. Its escaped dollar
        # reaches the remote shell as a literal dollar, then jq receives $p.
        program = program.replace("\\$", "$")
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp)
            original = {"network": {"database_path": "/var/lib/podmesh-manager/manager.sqlite",
                                    "replica_id": "replica-a", "manager": {"logical_manager_id": "logical"}}}
            config = state / "original.json"
            config.write_text(json.dumps(original))
            argv = ["jq"]
            if "--arg p" in command:
                argv += ["--arg", "p", str(state / "manager.sqlite")]
            result = subprocess.run(argv + [program, str(config)], capture_output=True, text=True, check=True)
            derived = json.loads(result.stdout)
            self.assertEqual(Path(derived["network"]["database_path"]).parent, state)
            expected = json.loads(json.dumps(original))
            expected["network"]["database_path"] = str(state / "manager.sqlite")
            self.assertEqual(derived, expected)
            self.assertEqual(json.loads(config.read_text()), original)
            self.assertFalse((state / "manager.sqlite").exists())


if __name__ == "__main__":
    unittest.main()
