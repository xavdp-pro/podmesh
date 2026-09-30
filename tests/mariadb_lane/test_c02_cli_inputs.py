#!/usr/bin/env python3
"""Real offline CLI input regressions; no DSN, MariaDB, keys or runtime servers.

All inputs and outputs belong to a new retained proof directory. The optional old
executable is run only against a FIFO without a writer; only that own child is
killed/reaped on the expected regression timeout. No database SQL is executed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time
import unittest

OPTIONS = None
RECORDS = []


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_new(path, value):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def birth(pid):
    try:
        text = Path(f"/proc/{pid}/stat").read_text()
        return int(text[text.rfind(")") + 2:].split()[19])
    except (OSError, ValueError, IndexError):
        return None


def child(command, label, timeout=3):
    log = OPTIONS.output / (label + ".log")
    fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    started = time.monotonic()
    with os.fdopen(fd, "wb") as stream:
        process = subprocess.Popen(command, stdout=stream, stderr=subprocess.STDOUT, close_fds=True)
        original_birth = birth(process.pid)
        timed_out = False
        try:
            try:
                code = process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                timed_out = True
                process.kill()  # this Popen's own child only
                code = process.wait(timeout=5)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            stream.flush()
            os.fsync(stream.fileno())
    record = {"command": [str(value) for value in command], "returncode": code,
              "timed_out": timed_out, "pid": process.pid, "birth": original_birth,
              "own_child_reaped": process.poll() is not None,
              "original_child_absent": birth(process.pid) is None or birth(process.pid) != original_birth,
              "elapsed_seconds": time.monotonic() - started, "log": str(log),
              "log_sha256": digest(log), "server_connected": False}
    RECORDS.append(record)
    return record


class OfflineInputGuards(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fixture = OPTIONS.output / "fixture"
        cls.snapshot = OPTIONS.output / "snapshot"
        result = child([OPTIONS.binary, "fixture-create", "--output", cls.fixture], "prepare-fixture")
        if result["returncode"] != 0 or result["timed_out"]:
            raise RuntimeError("synthetic_fixture_creation_failed")
        cls.manifest = cls.fixture / "source-manifest.json"
        result = child([OPTIONS.binary, "snapshot", "--source-manifest", cls.manifest,
                        "--output", cls.snapshot], "prepare-snapshot")
        if result["returncode"] != 0 or result["timed_out"]:
            raise RuntimeError("private_snapshot_creation_failed")
        cls.seal = cls.snapshot / "seal.json"

    def variant(self, original, kind, label):
        path = OPTIONS.output / label
        if kind == "fifo":
            os.mkfifo(path, 0o600)  # no writer is ever opened
        elif kind == "symlink":
            path.symlink_to(original)
        elif kind == "directory":
            path.mkdir(mode=0o700)
        else:
            path.write_bytes(original.read_bytes())
            path.chmod(0o400 if kind == "readonly" else 0o700)
        return path

    def command(self, mode, manifest, seal, output, binary=None):
        command = [OPTIONS.binary if binary is None else binary, mode, "--source-manifest", manifest]
        if mode == "inspect":
            command.extend(["--snapshot", self.snapshot / "snapshot.sqlite", "--seal", seal])
        return command + ["--output", output]

    def assert_refusal(self, mode, manifest, seal, label):
        output = OPTIONS.output / (label + "-output")
        snapshot_children = set(self.snapshot.iterdir())
        record = child(self.command(mode, manifest, seal, output), label)
        self.assertFalse(record["timed_out"], "regularity must be checked before any blocking read")
        self.assertEqual(record["returncode"], 2)
        self.assertTrue(record["own_child_reaped"] and record["original_child_absent"])
        response = json.loads(Path(record["log"]).read_text())
        self.assertEqual(response["status"], "REFUSED")
        self.assertIn(response["reason"], ("private_input_open_failed", "private_input_identity_mode_or_limit"))
        self.assertFalse(output.exists(), "invalid input may not publish output")
        self.assertEqual(set(self.snapshot.iterdir()), snapshot_children, "no private reader may be published")

    def test_snapshot_and_inspect_refuse_manifest_fifo_without_writer(self):
        for mode in ("snapshot", "inspect"):
            label = mode + "-manifest-fifo"
            with self.subTest(mode=mode):
                self.assert_refusal(mode, self.variant(self.manifest, "fifo", label + "-input"), self.seal, label)

    def test_inspect_refuses_seal_fifo_without_writer(self):
        label = "inspect-seal-fifo"
        self.assert_refusal("inspect", self.manifest, self.variant(self.seal, "fifo", label + "-input"), label)

    def test_manifest_executable_symlink_and_directory_refuse_before_publication(self):
        for mode in ("snapshot", "inspect"):
            for kind in ("executable", "symlink", "directory"):
                label = mode + "-manifest-" + kind
                with self.subTest(mode=mode, kind=kind):
                    self.assert_refusal(mode, self.variant(self.manifest, kind, label + "-input"), self.seal, label)

    def test_seal_executable_and_symlink_refuse_before_publication(self):
        for kind in ("executable", "symlink"):
            label = "inspect-seal-" + kind
            with self.subTest(kind=kind):
                self.assert_refusal("inspect", self.manifest, self.variant(self.seal, kind, label + "-input"), label)

    def test_private_readonly_400_manifest_and_seal_preserve_valid_offline_behavior(self):
        manifest = self.variant(self.manifest, "readonly", "readonly-manifest.json")
        seal = self.variant(self.seal, "readonly", "readonly-seal.json")
        for mode in ("snapshot", "inspect"):
            output = OPTIONS.output / ("readonly-" + mode)
            record = child(self.command(mode, manifest, seal, output), "readonly-" + mode)
            self.assertFalse(record["timed_out"])
            self.assertEqual(record["returncode"], 0)
            self.assertTrue(output.exists())

    def test_old_frozen_reader_fails_the_same_actual_fifo_refusals(self):
        if OPTIONS.regression_binary is None:
            self.skipTest("explicit old ELF not provided: regression comparison not executed")
        for mode, field in (("snapshot", "manifest"), ("inspect", "manifest"), ("inspect", "seal")):
            label = "old-" + mode + "-" + field + "-fifo"
            source = self.variant(self.manifest if field == "manifest" else self.seal, "fifo", label + "-input")
            output = OPTIONS.output / (label + "-output")
            manifest, seal = (source, self.seal) if field == "manifest" else (self.manifest, source)
            record = child(self.command(mode, manifest, seal, output, OPTIONS.regression_binary), label, timeout=1.5)
            self.assertTrue(record["timed_out"], "old vulnerable reader should fail the fast-refusal regression")
            self.assertEqual(record["returncode"], -signal.SIGKILL)
            self.assertTrue(record["own_child_reaped"] and record["original_child_absent"])
            self.assertFalse(output.exists())
            record["classification"] = "EXPECTED_OLD_SOURCE_REGRESSION_NOT_NEW_PASS"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--regression-binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    OPTIONS = parser.parse_args()
    os.mkdir(OPTIONS.output, 0o700)  # preserve an existing proof directory by refusing reuse
    result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(OfflineInputGuards))
    write_new(OPTIONS.output / "receipt.json", {"format": "podmesh-C02-offline-CLI-input-regressions/1",
              "status": "PASS" if result.wasSuccessful() else "FAILED_NOT_PASS",
              "binary": {"path": str(OPTIONS.binary), "sha256": digest(OPTIONS.binary)},
              "regression_binary": None if OPTIONS.regression_binary is None else {
                  "path": str(OPTIONS.regression_binary), "sha256": digest(OPTIONS.regression_binary)},
              "tests_run": result.testsRun, "failed": len(result.failures), "errors": len(result.errors),
              "skipped": len(result.skipped), "records": RECORDS, "server_or_DB_used": False,
              "operational_paths_touched": False, "real_keys_created": False, "evidence_retained": True})
    raise SystemExit(0 if result.wasSuccessful() else 1)
