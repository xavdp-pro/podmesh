"""Adversarial local acceptance tests; no physical hosts or network devices."""

import dataclasses
from collections import Counter
import json
import multiprocessing
from pathlib import Path
import random
import select
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import unittest

from fencing_lab import Authority, Maker, MAX_EFFECTS, MAX_EPOCH, MAX_RESOURCES, Permit, Refused


def transfer_worker(path, expected, replica, ready, start, output):
    gate, announced = None, False
    try:
        gate = Authority(Path(path))
        ready.put(("ready",))
        announced = True
        if not start.wait(5):
            raise RuntimeError("transfer start deadline")
        permit = gate.transfer("service", expected, replica, "instance-" + replica)
        output.put(("accepted", permit.encode()))
    except Exception as error:
        message = ("refused", error.code) if isinstance(error, Refused) else ("unexpected_error", type(error).__name__, str(error))
        if not announced:
            ready.put(message)
        output.put(message)
    finally:
        if gate is not None:
            gate.close()


def write_worker(path, wire, prefix, transferred, output):
    gate = None
    try:
        gate = Authority(Path(path))
        permit = Permit.decode(wire)
        gate.effect(permit, (permit.replica_id, permit.instance_id), prefix + "-initial", 1)
        output.put(("initial-effect-committed",))
        if not transferred.wait(5):
            raise RuntimeError("committed transfer deadline")
        accepted = refused = 0
        for index in range(32):
            try:
                gate.effect(permit, (permit.replica_id, permit.instance_id), f"{prefix}-{index}", 1)
                accepted += 1
            except Refused as error:
                if error.code != "refused" or "stale" not in str(error):
                    raise
                refused += 1
        output.put(("post-transfer", accepted, refused))
    except Exception as error:
        output.put(("unexpected_error", type(error).__name__, str(error)))
    finally:
        if gate is not None:
            gate.close()


class FencingTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="podmesh-fencing-")
        self.root = Path(self.directory.name)
        self.gate_path = self.root / "gate.sqlite"
        self.gate = Authority(self.gate_path, create=True)
        self.gate.declare("service")
        self.makers = {}
        for name in ("r1", "r2", "r3"):
            self.makers[name] = Maker(self.root / f"{name}.sqlite", self.gate.authority_id,
                                     name, "instance-" + name, create=True)

    def tearDown(self):
        for maker in self.makers.values():
            maker.close()
        self.gate.close()
        self.directory.cleanup()

    def grant(self, replica, expected):
        return self.gate.transfer("service", expected, replica, "instance-" + replica)

    def act(self, replica, permit, operation="op", gate=True):
        return self.makers[replica].effect(self.gate if gate else None, permit, operation, 1)

    def audit(self):
        # Independent connection, not the producer's successful response.
        with sqlite3.connect(self.gate_path) as reader:
            self.assertEqual(reader.execute("PRAGMA integrity_check").fetchone()[0], "ok")
            rows = reader.execute("SELECT resource,epoch,replica_id,instance_id,delta,result FROM effects ORDER BY sequence").fetchall()
            owners, epochs, counters = {}, {}, {}
            for resource, version, replica, instance, delta, result in rows:
                key = (resource, version)
                self.assertEqual(owners.setdefault(key, (replica, instance)), (replica, instance))
                self.assertGreaterEqual(version, epochs.get(resource, 0))
                epochs[resource] = version
                counters[resource] = counters.get(resource, 0) + delta
                self.assertEqual(result, counters[resource])
            for resource, counter in reader.execute("SELECT resource,counter FROM resources"):
                self.assertEqual(counter, counters.get(resource, 0))
            return rows

    def test_partition_without_gate_cannot_create_exclusive_effect(self):
        first = self.grant("r1", 0)
        self.assertEqual(self.act("r1", first), 1)
        for replica in self.makers:
            with self.assertRaises(Refused):
                self.act(replica, first, "partition", gate=False)
        self.assertEqual(len(self.audit()), 1)
        self.assertEqual(self.gate.inspect("service")["epoch"], 1)

    def test_delayed_old_manager_cannot_act_after_explicit_transfer(self):
        old = self.grant("r1", 0)
        self.act("r1", old, "first")
        new = self.grant("r2", 1)
        self.assertEqual(self.act("r2", new, "second"), 2)
        # r1 never received the new epoch, so its local cache cannot save us.
        with self.assertRaisesRegex(Refused, "stale"):
            self.act("r1", old, "delayed")
        self.assertEqual(len(self.audit()), 2)

    def test_priority_has_no_activation_or_truth_authority(self):
        high_id = self.grant("r3", 0)
        self.act("r3", high_id, "latest")
        # A low-ID coordinator may coordinate, but cannot borrow r3's grant.
        with self.assertRaises(Refused):
            self.act("r1", high_id, "priority")
        self.assertEqual(self.gate.inspect("service")["replica_id"], "r3")
        self.assertEqual(len(self.audit()), 1)

    def test_permit_roundtrip_tampering_actor_and_instance_refused(self):
        permit = self.grant("r1", 0)
        self.assertEqual(Permit.decode(permit.encode()), permit)
        for forged in (dataclasses.replace(permit, epoch=2),
                       dataclasses.replace(permit, grant_id="fabricated"),
                       dataclasses.replace(permit, resource="other"),
                       dataclasses.replace(permit, authority_id="other"),
                       dataclasses.replace(permit, instance_id="replacement")):
            with self.assertRaises(Refused):
                self.gate.effect(forged, (forged.replica_id, forged.instance_id), "bad", 1)
        with self.assertRaises(Refused):
            self.gate.effect(permit, ("r2", "instance-r2"), "stolen", 1)
        self.assertEqual(self.audit(), [])

    def test_restart_preserves_gate_and_maker_epoch(self):
        old = self.grant("r1", 0)
        self.act("r1", old, "old")
        new = self.grant("r1", 1)
        self.act("r1", new, "new")
        authority_id = self.gate.authority_id
        self.gate.close()
        self.gate = Authority(self.gate_path)
        self.assertEqual(self.gate.authority_id, authority_id)
        self.makers["r1"].close()
        self.makers["r1"] = Maker(self.root / "r1.sqlite", authority_id, "r1", "instance-r1")
        with self.assertRaisesRegex(Refused, "previously superseded"):
            self.act("r1", old, "stale")
        self.assertEqual(self.act("r1", new, "after-restart"), 3)
        self.audit()

    def test_restored_stale_maker_cannot_bypass_current_gate(self):
        old = self.grant("r1", 0)
        self.act("r1", old, "old")
        self.makers["r1"].close()
        shutil.copyfile(self.root / "r1.sqlite", self.root / "old-maker.sqlite")
        self.makers["r1"] = Maker(self.root / "r1.sqlite", self.gate.authority_id, "r1", "instance-r1")
        new = self.grant("r2", 1)
        self.act("r2", new, "new")
        with context_maker(self.root / "old-maker.sqlite", self.gate.authority_id, "r1", "instance-r1") as stale:
            with self.assertRaises(Refused):
                stale.effect(self.gate, old, "stale-restored", 1)
        self.assertEqual(len(self.audit()), 2)

    def test_missing_store_and_new_foreign_authority_require_recovery(self):
        with self.assertRaisesRegex(Refused, "recovery"):
            Authority(self.root / "absent.sqlite")
        self.assertFalse((self.root / "absent.sqlite").exists())
        foreign = Authority(self.root / "foreign.sqlite", create=True)
        try:
            foreign.declare("service")
            permit = foreign.transfer("service", 0, "r1", "instance-r1")
            with self.assertRaisesRegex(Refused, "binding"):
                self.act("r1", permit)
        finally:
            foreign.close()
        self.assertEqual(self.audit(), [])

    def test_counterexample_cloned_gate_is_not_safe_replication(self):
        old = self.grant("r1", 0)
        self.gate.close()
        shutil.copyfile(self.gate_path, self.root / "cloned-gate.sqlite")
        self.gate = Authority(self.gate_path)
        new = self.grant("r2", 1)
        cloned = Authority(self.root / "cloned-gate.sqlite")
        try:
            # Deliberate negative control: copying the authority database creates
            # two gates that can both accept. Never deploy this topology.
            self.assertEqual(self.act("r2", new, "current-gate"), 1)
            self.assertEqual(cloned.effect(old, ("r1", "instance-r1"), "cloned-gate", 1), 1)
            self.assertEqual(cloned.authority_id, self.gate.authority_id)
            self.assertNotEqual(cloned.inspect("service")["replica_id"],
                                self.gate.inspect("service")["replica_id"])
        finally:
            cloned.close()
        self.assertEqual(len(self.audit()), 1)

    def test_revoke_is_durable_and_has_no_timeout_takeover(self):
        old = self.grant("r1", 0)
        self.gate.revoke("service", 1)
        with self.assertRaises(Refused):
            self.act("r1", old)
        self.assertIsNone(self.gate.inspect("service")["replica_id"])
        with self.assertRaises(Refused):
            self.grant("r2", 1)
        new = self.grant("r2", 2)
        self.act("r2", new)
        self.audit()

    def test_operation_retry_does_not_duplicate_and_changed_retry_refused(self):
        permit = self.grant("r1", 0)
        self.assertEqual(self.act("r1", permit), 1)
        self.assertEqual(self.act("r1", permit), 1)
        with self.assertRaisesRegex(Refused, "incompatible"):
            self.makers["r1"].effect(self.gate, permit, "op", 2)
        newer = self.grant("r1", 1)
        with self.assertRaisesRegex(Refused, "incompatible"):
            self.act("r1", newer)
        self.assertEqual(len(self.audit()), 1)

    def test_concurrent_transfer_has_exactly_one_winner(self):
        self.grant("r1", 0)
        context = multiprocessing.get_context("spawn")
        ready, output, start = context.Queue(), context.Queue(), context.Event()
        workers = [context.Process(target=transfer_worker, args=(str(self.gate_path), 1, name, ready, start, output))
                   for name in ("r2", "r3")]
        try:
            for worker in workers:
                worker.start()
            for _ in workers:
                self.assertEqual(ready.get(timeout=10), ("ready",))
            start.set()
            results = [output.get(timeout=10) for _ in workers]
            self.assertEqual(sorted(item[0] for item in results), ["accepted", "refused"])
            for worker in workers:
                worker.join(10)
                self.assertEqual(worker.exitcode, 0)
        finally:
            for worker in workers:
                if worker.is_alive():
                    worker.kill()
                worker.join()
        self.assertEqual(self.gate.inspect("service")["epoch"], 2)
        self.audit()

    def test_concurrent_delayed_effects_serialize_with_takeover(self):
        old = self.grant("r1", 0)
        context = multiprocessing.get_context("spawn")
        transferred, output = context.Event(), context.Queue()
        worker = context.Process(target=write_worker, args=(str(self.gate_path), old.encode(), "old", transferred, output))
        worker.start()
        try:
            self.assertEqual(output.get(timeout=10), ("initial-effect-committed",))
            new = self.grant("r2", 1)
            self.act("r2", new, "new-initial")
            transferred.set()
            for index in range(32):
                self.act("r2", new, f"new-{index}")
            self.assertEqual(output.get(timeout=10), ("post-transfer", 0, 32))
            worker.join(10)
            self.assertEqual(worker.exitcode, 0)
        finally:
            if worker.is_alive():
                worker.kill()
            worker.join()
        rows = self.audit()
        self.assertEqual(len(rows), 34)
        self.assertEqual([row[1] for row in rows], [1] + [2] * 33)

    def test_kill_after_gate_commit_before_maker_commit_retries_once(self):
        permit = self.grant("r1", 0)
        helper = """
import sys,time
from pathlib import Path
from fencing_lab import Authority,Maker,Permit
gate=Authority(Path(sys.argv[1]))
p=Permit.decode(sys.argv[3].encode())
maker=Maker(Path(sys.argv[2]),p.authority_id,p.replica_id,p.instance_id)
maker.db.execute('BEGIN IMMEDIATE')
gate.effect(p,(p.replica_id,p.instance_id),'crash-operation',1)
print('GATE_COMMITTED_MAKER_UNCOMMITTED',flush=True)
time.sleep(30)
"""
        child = subprocess.Popen([sys.executable, "-c", helper, str(self.gate_path),
                                  str(self.root / "r1.sqlite"), permit.encode().decode()],
                                 cwd=Path(__file__).parent, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            ready, _, _ = select.select([child.stdout], [], [], 10)
            self.assertTrue(ready, "child did not reach bounded commit marker")
            self.assertEqual(child.stdout.readline().strip(), "GATE_COMMITTED_MAKER_UNCOMMITTED")
            child.kill()
            child.wait(timeout=5)
            self.assertLess(child.returncode, 0)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=5)
            child.stdout.close()
            child.stderr.close()
        self.assertEqual(len(self.audit()), 1)
        self.assertEqual(self.act("r1", permit, "crash-operation"), 1)
        self.assertEqual(len(self.audit()), 1)

    def test_failed_sql_write_has_no_effect_or_receipt(self):
        permit = self.grant("r1", 0)
        self.gate.db.execute("CREATE TRIGGER fail_update BEFORE UPDATE OF counter ON resources BEGIN SELECT RAISE(ABORT,'injected'); END")
        with self.assertRaisesRegex(Refused, "storage_fault"):
            self.act("r1", permit)
        self.assertEqual(self.audit(), [])
        self.gate.db.execute("DROP TRIGGER fail_update")
        self.assertEqual(self.act("r1", permit), 1)
        self.audit()

    def test_storage_busy_is_typed_without_implicit_retry(self):
        permit = self.grant("r1", 0)
        for path in (self.gate_path, self.root / "r1.sqlite"):
            with sqlite3.connect(path) as blocker:
                blocker.execute("BEGIN IMMEDIATE")
                with self.assertRaises(Refused) as caught:
                    self.act("r1", permit, "blocked")
                self.assertEqual(caught.exception.code, "storage_busy")
                blocker.rollback()
            self.assertEqual(self.audit(), [])
        # The caller explicitly retries only after releasing both locks.
        self.assertEqual(self.act("r1", permit, "blocked"), 1)
        self.assertEqual(len(self.audit()), 1)

    def test_store_role_version_corruption_and_open_errors_are_typed(self):
        with self.assertRaises(Refused) as caught:
            Authority(self.root / "r1.sqlite")
        self.assertEqual(caught.exception.code, "storage_schema")
        with self.assertRaises(Refused) as caught:
            Maker(self.gate_path, self.gate.authority_id, "r1", "instance-r1")
        self.assertEqual(caught.exception.code, "storage_schema")
        corrupt = self.root / "corrupt.sqlite"
        corrupt.write_bytes(b"not a sqlite store")
        with self.assertRaises(Refused) as caught:
            Authority(corrupt)
        self.assertEqual(caught.exception.code, "storage_schema")
        with self.assertRaises(Refused) as caught:
            Authority(self.root / "missing-parent" / "gate.sqlite", create=True)
        self.assertEqual(caught.exception.code, "storage_io")
        legacy = self.root / "legacy.sqlite"
        with sqlite3.connect(legacy) as fixture:
            fixture.execute("PRAGMA user_version=1")
        with self.assertRaises(Refused) as caught:
            Authority(legacy)
        self.assertEqual(caught.exception.code, "storage_schema")
        self.assertEqual(self.audit(), [])

    def test_broken_schema_read_is_typed_and_does_not_auto_rebuild(self):
        broken = Authority(self.root / "broken.sqlite", create=True)
        try:
            broken.db.execute("DROP TABLE effects")
            broken.db.execute("DROP TABLE resources")
            with self.assertRaises(Refused) as caught:
                broken.inspect("service")
            self.assertEqual(caught.exception.code, "storage_schema")
            self.assertNotIn("SELECT", str(caught.exception))
            self.assertIsNone(broken.db.execute("SELECT name FROM sqlite_master WHERE name='resources'").fetchone())
        finally:
            broken.close()

    def test_workers_report_unexpected_and_initialization_errors(self):
        permit = self.grant("r1", 0)
        context = multiprocessing.get_context("spawn")
        for mode in ("transfer-unexpected", "write-unexpected", "open-refusal"):
            ready, output, start = context.Queue(), context.Queue(), context.Event()
            start.set()
            if mode == "write-unexpected":
                worker = context.Process(target=write_worker, args=(str(self.gate_path), permit.encode(), 123, start, output))
            else:
                path = self.gate_path if mode != "open-refusal" else self.root / "missing.sqlite"
                worker = context.Process(target=transfer_worker,
                                         args=(str(path), 1, 123, ready, start, output))
            worker.start()
            try:
                if mode != "write-unexpected":
                    announced = ready.get(timeout=10)
                    self.assertEqual(announced[0], "refused" if mode == "open-refusal" else "ready")
                result = output.get(timeout=10)
                self.assertEqual(result[0], "refused" if mode == "open-refusal" else "unexpected_error")
                if mode != "open-refusal":
                    self.assertEqual(result[1], "TypeError")
                worker.join(10)
                self.assertEqual(worker.exitcode, 0)
            finally:
                if worker.is_alive():
                    worker.kill()
                worker.join()
        self.assertEqual(self.audit(), [])

    def test_quotas_overflow_and_malformed_permits_fail_closed(self):
        permit = self.grant("r1", 0)
        malformed = [b"[]", b"{" * 4096, b"x" * 4097, b"\xff", b"{}"]
        data = dataclasses.asdict(permit)
        for field, value in (("epoch", True), ("epoch", MAX_EPOCH + 1), ("resource", "../escape")):
            malformed.append(json.dumps(dict(data, **{field: value})).encode())
        malformed.append(permit.encode()[:-1] + b',"epoch":1}')
        for wire in malformed:
            with self.assertRaises(Refused):
                Permit.decode(wire)
        with self.assertRaises(Refused):
            self.gate.effect(permit, ("r1", "instance-r1"), "bool", True)
        self.gate.db.execute("UPDATE resources SET epoch=? WHERE resource='service'", (MAX_EPOCH,))
        self.gate.db.commit()
        with self.assertRaisesRegex(Refused, "exhausted"):
            self.grant("r1", MAX_EPOCH)
        for index in range(MAX_RESOURCES - 1):
            self.gate.declare(f"other-{index}")
        with self.assertRaisesRegex(Refused, "quota"):
            self.gate.declare("overflow")
        self.assertEqual(self.audit(), [])

    def test_effect_quota_never_evicts_and_verified_retry_still_works(self):
        permit = self.grant("r1", 0)
        for index in range(MAX_EFFECTS):
            self.gate.effect(permit, ("r1", "instance-r1"), f"op-{index}", 0)
        with self.assertRaisesRegex(Refused, "quota"):
            self.act("r1", permit, "overflow")
        self.assertEqual(self.gate.effect(permit, ("r1", "instance-r1"), "op-0", 0), 0)
        self.assertEqual(len(self.audit()), MAX_EFFECTS)

    def test_independent_scopes_do_not_share_one_owner(self):
        first = self.grant("r1", 0)
        self.gate.declare("independent")
        second = self.gate.transfer("independent", 0, "r2", "instance-r2")
        self.act("r1", first, "scope-one")
        self.act("r2", second, "scope-two")
        self.assertEqual(len(self.audit()), 2)

    def test_seeded_three_replica_partition_reconnect_stress(self):
        rng = random.Random(20260912)
        permits = [self.grant("r1", 0)]
        accepted = refused = transfers = stale_attempts = 0
        for index in range(600):
            if rng.randrange(10) == 0:
                permits.append(self.grant(rng.choice(tuple(self.makers)), permits[-1].epoch))
                transfers += 1
            use_stale = len(permits) > 1 and rng.randrange(4) == 0
            permit = rng.choice(permits[:-1]) if use_stale else permits[-1]
            stale_attempts += int(use_stale)
            connected = rng.randrange(4) != 0
            try:
                self.act(permit.replica_id, permit, f"stress-{index}", gate=connected)
                accepted += 1
            except Refused:
                refused += 1
            if index % 100 == 0:
                self.gate.close()
                self.gate = Authority(self.gate_path)
        self.assertGreaterEqual(accepted, 200)
        self.assertGreaterEqual(refused, 100)
        self.assertGreaterEqual(stale_attempts, 50)
        self.assertGreaterEqual(transfers, 20)
        rows = self.audit()
        self.assertEqual(len(rows), accepted)
        per_epoch = Counter(row[1] for row in rows)
        self.assertGreaterEqual(sum(count >= 2 for count in per_epoch.values()), 10)
        print(f"STRESS seed=20260912 attempts=600 accepted={accepted} refused={refused} transfers={transfers} stale_attempts={stale_attempts} multi_effect_epochs={sum(count >= 2 for count in per_epoch.values())}")


class context_maker:
    def __init__(self, *args):
        self.maker = Maker(*args)

    def __enter__(self):
        return self.maker

    def __exit__(self, *_):
        self.maker.close()


if __name__ == "__main__":
    unittest.main(verbosity=2)
