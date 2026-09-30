#!/usr/bin/env python3
"""Provenance regressions on private inert ELF copies; no SQLite or MariaDB SQL."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import types
import unittest

HERE = Path(__file__).parent
spec = importlib.util.spec_from_file_location('cli_inputs_current', HERE / 'test_c02_cli_inputs.py')
current = importlib.util.module_from_spec(spec)
spec.loader.exec_module(current)
OPTIONS = None
PROOFS = []


class BinaryCustody(unittest.TestCase):
    def inert(self, label):
        path = OPTIONS.output / label
        # An inert executable is copied once; published product binaries are never altered.
        with open('/usr/bin/true', 'rb') as source:
            raw = source.read()
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o700)
        with os.fdopen(fd, 'wb') as stream:
            stream.write(raw)
        return path, hashlib.sha256(raw).hexdigest()

    def test_initial_wrong_hash_refuses_before_any_spawn(self):
        path, _ = self.inert('wrong-hash.elf')
        with self.assertRaisesRegex(RuntimeError, 'binary_initial_hash_or_ELF_mismatch'):
            current.BinaryPin(path, '0' * 64)
        PROOFS.append({'case': 'wrong_initial_hash', 'spawned': False, 'refused': True})

    def test_fifo_and_symlink_refuse_before_blocking_or_spawn(self):
        fifo = OPTIONS.output / 'fifo'
        os.mkfifo(fifo, 0o600)
        with self.assertRaisesRegex(RuntimeError, 'binary_not_private_regular_ELF'):
            current.BinaryPin(fifo, '0' * 64)
        path, expected = self.inert('symlink-target.elf')
        link = OPTIONS.output / 'symlink'
        link.symlink_to(path)
        with self.assertRaises(OSError):
            current.BinaryPin(link, expected)
        PROOFS.append({'case': 'nonregular_and_symlink', 'spawned': False, 'refused': True})

    def test_private_inert_drift_old_harness_launches_new_refuses(self):
        path, expected = self.inert('drift-comparison.elf')
        pin = current.BinaryPin(path, expected)
        try:
            # Appended inert bytes preserve ELF behavior but change provenance.
            with path.open('ab') as stream:
                stream.write(b'private-inert-provenance-regression\n')
            old.OPTIONS = types.SimpleNamespace(output=OPTIONS.output)
            old_result = old.child([path], 'old-harness-inert-drift')
            self.assertEqual(old_result['returncode'], 0)
            self.assertNotEqual(old.digest(path), expected)
            current.OPTIONS = types.SimpleNamespace(output=OPTIONS.output)
            current.PINS = {str(path.absolute()): pin}
            before = len(current.RECORDS)
            with self.assertRaisesRegex(RuntimeError, 'binary_identity_or_hash_drift'):
                current.child([path], 'new-harness-must-not-spawn')
            self.assertEqual(len(current.RECORDS), before)
            self.assertFalse((OPTIONS.output / 'new-harness-must-not-spawn.log').exists())
            PROOFS.append({'case': 'private_inert_drift', 'old_harness_launch': old_result,
                           'old_posthoc_sha256': old.digest(path), 'expected_sha256': expected,
                           'new_named_refusal': 'binary_identity_or_hash_drift', 'new_spawned': False})
        finally:
            current.PINS = {}
            pin.close()

    def test_same_bytes_path_replacement_refuses_identity_drift(self):
        path, expected = self.inert('replacement.elf')
        pin = current.BinaryPin(path, expected)
        try:
            path.rename(OPTIONS.output / 'original-retained.elf')
            replacement, _ = self.inert('replacement-new.elf')
            replacement.rename(path)
            with self.assertRaisesRegex(RuntimeError, 'binary_identity_or_hash_drift'):
                pin.verify('before_spawn')
            PROOFS.append({'case': 'same_bytes_replacement', 'spawned': False, 'refused': True})
        finally:
            pin.close()

    def test_end_check_refuses_after_private_drift(self):
        path, expected = self.inert('end-drift.elf')
        pin = current.BinaryPin(path, expected)
        try:
            pin.verify('before_spawn')
            with path.open('ab') as stream:
                stream.write(b'end-drift\n')
            with self.assertRaisesRegex(RuntimeError, 'binary_identity_or_hash_drift'):
                pin.verify('campaign_end')
            PROOFS.append({'case': 'campaign_end_drift', 'refused': True})
        finally:
            pin.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--old-harness', type=Path, required=True)
    parser.add_argument('--old-harness-sha256', required=True)
    parser.add_argument('--output', type=Path, required=True)
    OPTIONS = parser.parse_args()
    fd = os.open(OPTIONS.old_harness, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode) or before.st_size > 1048576:
            raise RuntimeError('old_harness_not_regular_or_bounded')
        raw = os.pread(fd, before.st_size + 1, 0)
        after = os.fstat(fd)
        if (current.BinaryPin.identity_of(before) != current.BinaryPin.identity_of(after)
                or hashlib.sha256(raw).hexdigest() != OPTIONS.old_harness_sha256):
            raise RuntimeError('old_harness_identity_or_hash_mismatch')
        old = types.ModuleType('old_cli_inputs_frozen')
        exec(compile(raw, str(OPTIONS.old_harness), 'exec'), old.__dict__)
    finally:
        os.close(fd)
    OPTIONS.output.mkdir(mode=0o700)
    result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(BinaryCustody))
    current.write_new(OPTIONS.output / 'receipt.json', {'format': 'podmesh-C02-CLI-binary-provenance-regressions/1',
                      'status': 'PASS' if result.wasSuccessful() else 'FAILED_NOT_PASS',
                      'old_harness_sha256': OPTIONS.old_harness_sha256,
                      'tests_run': result.testsRun, 'failed': len(result.failures), 'errors': len(result.errors),
                      'skipped': len(result.skipped), 'proofs': PROOFS,
                      'SQLite_SQL_executed': False, 'MariaDB_server_or_DSN_used': False,
                      'published_ELF_modified': False, 'scope': 'private_inert_ELF_cooperative_custody'})
    raise SystemExit(0 if result.wasSuccessful() else 1)
