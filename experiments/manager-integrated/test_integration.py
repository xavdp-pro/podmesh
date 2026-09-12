"""External SQLite observations across real resident and gate processes."""

from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
import uuid

from integrated_lab import (ChannelState, CONTEXT, GateClient, Lab, Permit, Refused,
                            RESOURCE, control, receive, send, until)


def fresh_client_attempt(channel, state, reply):
    client = GateClient(channel, state)
    try:
        client.effect(Permit("test-authority", RESOURCE, 3, "r0", "test-instance", "test-grant"),
                      ("r0", "test-instance"), "next", 1)
        send(reply, {"unexpected_success": True})
    except Refused as error:
        send(reply, {"code": error.code, "committed": client.committed})


class GateProtocol(unittest.TestCase):
    def permit(self):
        return Permit("test-authority", RESOURCE, 3, "r0", "test-instance", "test-grant")

    def response(self, operation_id):
        return {"ok": True, "operation_id": operation_id, "authority_id": "test-authority",
                "epoch": 3, "resource": RESOURCE, "result": 17}

    def test_mismatched_binding_disables_channel_before_another_request(self):
        for key, wrong in (("operation_id", "old-operation"), ("authority_id", "foreign"),
                           ("epoch", 2), ("resource", "other-resource")):
            with self.subTest(key=key):
                local, remote = CONTEXT.Pipe()
                state = ChannelState.new()
                response = self.response("current-operation")
                response[key] = wrong
                send(remote, response)
                client = GateClient(local, state)
                try:
                    with self.assertRaises(Refused) as caught:
                        client.effect(self.permit(), ("r0", "test-instance"), "current-operation", 1)
                    self.assertEqual(caught.exception.code, "channel_unknown")
                    self.assertIsNone(client.committed)
                    self.assertTrue(state.uncertain.is_set())
                    self.assertEqual(receive(remote)["operation_id"], "current-operation")
                    # A fresh client sees shared poison and sends no new request.
                    with self.assertRaises(Refused):
                        GateClient(local, state).effect(self.permit(), ("r0", "test-instance"), "next", 1)
                    self.assertFalse(remote.poll(0.05))
                finally:
                    local.close()
                    remote.close()

    def test_timeout_late_reply_cannot_be_consumed_by_fresh_maker_client(self):
        local, remote = CONTEXT.Pipe()
        state = ChannelState.new()
        late_reply_allowed = threading.Event()
        received = []

        def delayed_gate():
            request = receive(remote)
            received.append(request["operation_id"])
            if late_reply_allowed.wait(2):
                send(remote, self.response(request["operation_id"]))

        thread = threading.Thread(target=delayed_gate)
        thread.start()
        try:
            client = GateClient(local, state, timeout=0.03)
            with self.assertRaises(Refused) as caught:
                client.effect(self.permit(), ("r0", "test-instance"), "timed-out", 1)
            self.assertEqual(caught.exception.code, "channel_unknown")
            self.assertIsNone(client.committed)
            late_reply_allowed.set()
            thread.join(3)
            self.assertFalse(thread.is_alive())
            self.assertTrue(local.poll(1), "the late reply must really be queued")
            observer, child_reply = CONTEXT.Pipe()
            worker = CONTEXT.Process(target=fresh_client_attempt, args=(local, state, child_reply))
            worker.start()
            try:
                result = receive(observer)
                worker.join(3)
                self.assertEqual(worker.exitcode, 0)
                self.assertEqual(result, {"code": "channel_unknown", "committed": None})
            finally:
                if worker.is_alive():
                    worker.kill()
                    worker.join(3)
                observer.close()
                child_reply.close()
            self.assertEqual(received, ["timed-out"])
            self.assertFalse(remote.poll(0.05), "the next operation was never sent")
            self.assertTrue(local.poll(0.05), "the stale reply was never consumed")
        finally:
            late_reply_allowed.set()
            thread.join(3)
            local.close()
            remote.close()


class Integration(unittest.TestCase):
    def test_partition_epoch_restart_crash_and_conflict_retention(self):
        # Keep raw files on both success and failure. No existing state is reused.
        base = Path(os.environ.get("PODMESH_INTEGRATED_EVIDENCE", tempfile.gettempdir()))
        directory = base / ("pm-integrated-" + uuid.uuid4().hex[:12])
        self.assertLess(len(str(directory / "r0.sock")), 101)
        lab = Lab(directory)
        print(f"\nRetained integration evidence: {directory}", flush=True)
        try:
            old = lab.transfer(0, 2)
            first_id = str(uuid.uuid4())
            self.assertTrue(lab.attempt(2, old, first_id)["ok"])
            lab.converged(4)
            # A copied permit does not bind to another maker or IPC principal.
            self.assertFalse(lab.attempt(0, old)["ok"])
            self.assertEqual(len(lab.effects()), 1)
            lab.partition(True)
            # Permit assignment stays r2 even though its manager peers are isolated.
            for i in range(3):
                lab.observe(i, "partition-local")
            until(lambda: len(lab.histories()[0]) == len(lab.histories()[1]) == 6)
            self.assertEqual(len(lab.histories()[2]), 5)
            self.assertTrue(all(child.poll() is None for child in lab.children.values()))
            for _ in range(2):
                self.assertFalse(lab.attempt(0, old)["ok"])
            # Explicit fixture authorization, not a partition detector, changes epoch.
            current = lab.transfer(1, 0)
            self.assertEqual(current["epoch"], 2)
            with self.assertRaises(Refused):
                GateClient(lab.channels[1][0], lab.channel_states[1]).effect(Permit(**current), lab.actors[0],
                                                     str(uuid.uuid4()), 1)
            lab.record("wrong_gate_channel_refused", channel_replica=1, claimed_replica=0)
            current_id = str(uuid.uuid4())
            self.assertTrue(lab.attempt(0, current, current_id)["ok"])
            self.assertFalse(lab.attempt(2, old)["ok"])
            self.assertFalse(lab.attempt(2, old, first_id)["ok"])
            # Concurrent post-transfer stale and current requests use distinct channels.
            with ThreadPoolExecutor(max_workers=2) as executor:
                for _ in range(4):
                    stale = executor.submit(lab.attempt, 2, old)
                    fresh = executor.submit(lab.attempt, 0, current)
                    self.assertFalse(stale.result()["ok"])
                    self.assertTrue(fresh.result()["ok"])
            # A fixture-cut gate path never falls back to a cached permit.
            before = len(lab.effects())
            self.assertFalse(lab.attempt(0, current, gate_reachable=False)["ok"])
            self.assertEqual(len(lab.effects()), before)
            lab.observe(0, "gate-unreachable-local")
            # Gate restart retains current authority and refuses old epoch.
            lab.stop_gate()
            lab.start_gate()
            self.assertFalse(lab.attempt(2, old)["ok"])
            self.assertTrue(lab.attempt(0, current, current_id)["ok"])
            self.assertEqual(len(lab.effects()), before)
            # Crash after external commit, before publishing a local result.
            crash_id = str(uuid.uuid4())
            self.assertTrue(lab.attempt(0, current, crash_id, crash_after_gate=True)["unknown"])
            self.assertEqual(len(lab.effects()), before + 1)
            self.assertTrue(lab.attempt(0, current, crash_id)["ok"])
            self.assertEqual(len(lab.effects()), before + 1)
            # Return of the live old resident does not restore its effect epoch.
            lab.partition(False)
            # 3 initial + 7 effects + 3 partition facts + 1 gate-offline fact.
            lab.converged(14)
            self.assertFalse(lab.attempt(2, old)["ok"])
            process = lab.children[2]
            process.kill()
            self.assertNotEqual(process.wait(5), 0)
            lab.record("injected_resident_kill", replica=2, pid=process.pid, exit_code=process.returncode)
            before_restart = len(lab.effects())
            self.assertFalse(lab.attempt(2, old)["ok"])
            self.assertEqual(len(lab.effects()), before_restart)
            (directory / "r2.sock").unlink()  # only after verified process exit
            lab.start(2)
            self.assertFalse(lab.attempt(2, old)["ok"])
            lab.converged(14)
            # Concurrent contradictory claims remain facts; priority cannot erase one.
            lab.partition(True)
            lab.observe(0, "claim-left", RESOURCE)
            lab.observe(2, "claim-right", RESOURCE)
            lab.partition(False)
            lab.converged(16)
            conflicts = []
            for i in range(3):
                inspection = control(directory / f"r{i}.sock")["inspection"]
                self.assertIn(RESOURCE, inspection["blocked_exclusive_resources"])
                conflicts.append(inspection["conflicts"])
            self.assertEqual(conflicts[0], conflicts[1])
            self.assertEqual(conflicts[1], conflicts[2])
            lab.write_json("external-conflicts.json", conflicts)
            self.assertEqual(lab.attempt(0, current)["code"], "conflict")
            self.assertEqual(len(lab.effects()), before + 1)
            for i in range(3):
                lab.observe(i, "after-conflict-local")
            lab.converged(19)
            # The independent gate log is the effect oracle, not successful replies.
            effects = lab.effects()
            self.assertEqual([effect["epoch"] for effect in effects], [1] + [2] * 6)
            self.assertEqual([effect["replica_id"] for effect in effects], ["r2"] + ["r0"] * 6)
            self.assertEqual([effect["result"] for effect in effects], list(range(1, 8)))
            self.assertEqual(len({effect["operation_id"] for effect in effects}), 7)
            facts = [json.loads(row[1]) for row in lab.histories()[0]]
            result_values = [json.loads(fact["value"]) for fact in facts
                             if fact["subject"].startswith("effect:")]
            self.assertEqual(len(result_values), len(effects))
            self.assertEqual(len({value["operation_id"] for value in result_values}), len(effects))
            self.assertEqual(
                sorted((value["operation_id"], value["epoch"], value["result"], value["authority_id"])
                       for value in result_values),
                sorted((effect["operation_id"], effect["epoch"], effect["result"], effect["authority_id"])
                       for effect in effects),
            )
            lab.record("acceptance_passed", effects=7, retained_facts_per_replica=19,
                       epoch_owners={"1": "r2", "2": "r0"}, conflict_retained=True)
        finally:
            lab.close()
        self.assertTrue((directory / "sha256.json").is_file())
        self.assertTrue(all(process.returncode == 0 for process in lab.children.values()))


if __name__ == "__main__":
    unittest.main(verbosity=2)
