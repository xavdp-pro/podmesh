#!/usr/bin/env python3
"""Real offline CLI input regressions; no DSN, MariaDB, keys or runtime servers.

All inputs and outputs belong to a new retained proof directory. The optional old
executable is run only against a FIFO without a writer; only that own child is
killed/reaped on the expected regression timeout. Synthetic SQLite DDL/inserts,
snapshot and inspect execute locally. No MariaDB SQL or DSN is used. Binary pins
are cooperative evidence custody, not protection against a malicious operator.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import time
import unittest

OPTIONS = None
RECORDS = []
PINS = {}
PIN_CHECKS = []


class BinaryPin:
    """Read/hash one regular ELF FD, retain it and execute that same object."""
    def __init__(self, path, expected_sha256):
        self.path = Path(path).absolute()
        self.expected = expected_sha256
        self.fd = os.open(self.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
        try:
            before = os.fstat(self.fd)
            if (not stat.S_ISREG(before.st_mode) or stat.S_IMODE(before.st_mode) not in (0o500, 0o700)
                    or before.st_uid != os.geteuid() or not 0 < before.st_size <= 134217728):
                raise RuntimeError("binary_not_private_regular_ELF")
            raw = os.pread(self.fd, before.st_size + 1, 0)
            self.identity = self.identity_of(before)
            if not raw.startswith(b"\x7fELF") or hashlib.sha256(raw).hexdigest() != self.expected:
                raise RuntimeError("binary_initial_hash_or_ELF_mismatch")
            self.verify("initial")
        except BaseException:
            os.close(self.fd)
            raise

    @staticmethod
    def identity_of(info):
        return (info.st_dev, info.st_ino, info.st_size, info.st_mode, info.st_uid, info.st_gid,
                info.st_mtime_ns, info.st_ctime_ns, info.st_nlink)

    def verify(self, phase):
        try:
            before = os.fstat(self.fd)
            named = os.stat(self.path, follow_symlinks=False)
            if self.identity_of(before) != self.identity or self.identity_of(named) != self.identity:
                raise RuntimeError("binary_identity_or_hash_drift")
            raw = os.pread(self.fd, before.st_size + 1, 0)
            if (hashlib.sha256(raw).hexdigest() != self.expected
                    or self.identity_of(os.fstat(self.fd)) != self.identity
                    or self.identity_of(os.stat(self.path, follow_symlinks=False)) != self.identity):
                raise RuntimeError("binary_identity_or_hash_drift")
        except OSError as error:
            raise RuntimeError("binary_identity_or_hash_drift") from error
        record = {"phase": phase, "path": str(self.path), "sha256": self.expected,
                  "identity": list(self.identity), "same_fd_and_named_identity": True}
        PIN_CHECKS.append(record)
        return record

    def receipt(self):
        return {"path": str(self.path), "expected_sha256": self.expected, "identity": list(self.identity),
                "execution": "held_regular_ELF_fd", "scope": "cooperative_local_custody"}

    def close(self):
        os.close(self.fd)


def verify_pins(phase):
    for pin in PINS.values():
        pin.verify(phase)


def worker_executable(pid, pin):
    try:
        actual = os.stat(f"/proc/{pid}/exe")
    except (FileNotFoundError, ProcessLookupError):
        return {"status": "UNAVAILABLE_CHILD_ALREADY_EXITED", "proof": "execution_uses_held_ELF_fd"}
    if (actual.st_dev, actual.st_ino) != pin.identity[:2]:
        raise RuntimeError("worker_executable_identity_mismatch")
    return {"status": "OBSERVED", "dev_inode": [actual.st_dev, actual.st_ino], "matches_held_ELF": True}


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
    verify_pins("before_spawn:" + label)
    pin = PINS[str(Path(command[0]).absolute())]
    log = OPTIONS.output / (label + ".log")
    fd = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    started = time.monotonic()
    with os.fdopen(fd, "wb") as stream:
        process = subprocess.Popen(command, executable=f"/proc/self/fd/{pin.fd}", pass_fds=(pin.fd,),
                                   stdout=stream, stderr=subprocess.STDOUT, close_fds=True)
        original_birth = birth(process.pid)
        timed_out = False
        try:
            observed_executable = worker_executable(process.pid, pin)
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
    verify_pins("after_spawn:" + label)
    record = {"binary_pin": pin.receipt(), "worker_executable": observed_executable,
              "command": [str(value) for value in command], "returncode": code,
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
    parser.add_argument("--binary-sha256", required=True)
    parser.add_argument("--regression-binary", type=Path, required=True)
    parser.add_argument("--regression-binary-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    OPTIONS = parser.parse_args()
    os.mkdir(OPTIONS.output, 0o700)  # preserve an existing proof directory by refusing reuse
    result = None
    refusal = None
    try:
        for path, expected in ((OPTIONS.binary, OPTIONS.binary_sha256),
                               (OPTIONS.regression_binary, OPTIONS.regression_binary_sha256)):
            if len(expected) != 64 or any(c not in "0123456789abcdef" for c in expected):
                raise RuntimeError("binary_expected_sha256_invalid")
            key = str(path.absolute())
            if key in PINS:
                raise RuntimeError("binary_roles_must_have_distinct_paths")
            PINS[key] = BinaryPin(path, expected)
        verify_pins("before_any_test")
        result = unittest.TextTestRunner(verbosity=2).run(
            unittest.defaultTestLoader.loadTestsFromTestCase(OfflineInputGuards))
        verify_pins("campaign_end")
    except (RuntimeError, OSError) as error:
        refusal = str(error)
    finally:
        write_new(OPTIONS.output / "receipt.json", {"format": "podmesh-C02-offline-CLI-input-regressions/2",
                  "status": "PASS" if refusal is None and result is not None and result.wasSuccessful() else "FAILED_NOT_PASS",
                  "refusal": refusal, "binary_pins": [pin.receipt() for pin in PINS.values()],
                  "pin_checks": PIN_CHECKS,
                  "tests_run": 0 if result is None else result.testsRun,
                  "failed": 0 if result is None else len(result.failures),
                  "errors": 0 if result is None else len(result.errors),
                  "skipped": 0 if result is None else len(result.skipped), "records": RECORDS,
                  "synthetic_SQLite_DDL_and_inserts_executed": any(
                      r["command"][1] == "fixture-create" and r["returncode"] == 0 for r in RECORDS),
                  "synthetic_SQLite_snapshot_and_inspect_executed": any(
                      r["command"][1] in ("snapshot", "inspect") for r in RECORDS),
                  "MariaDB_server_or_DSN_used": False, "operational_paths_touched": False,
                  "real_keys_created": False, "evidence_retained": True})
        for pin in PINS.values():
            pin.close()
    raise SystemExit(0 if refusal is None and result is not None and result.wasSuccessful() else 1)
