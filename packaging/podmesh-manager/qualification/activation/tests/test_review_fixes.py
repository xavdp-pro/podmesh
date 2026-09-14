#!/usr/bin/env python3
"""Focused regressions for the fourth independent G2 review."""
import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("g2_compare", ROOT / "compare-evidence.py")
G2 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(G2)

C = lambda digit: "sha256:" + digit * 64


def attempt(last_phase="outbound_request_prepared"):
    return {"attempt_commitment": C("1"), "nonce_commitment": C("2"),
            "nonce_authority": "peer-validated", "operation_commitment": C("3"),
            "direction": "outbound", "last_phase": last_phase}


def exchange(direction="outbound", nonce=None, phases=None, outcomes=None):
    return {"nonce_commitment": nonce or C("2"), "nonce_authority": "peer-validated",
            "joinable": True, "direction": direction,
            "phases_reached": phases or ["outbound_request_prepared"],
            "row_count": len(phases or ["outbound_request_prepared"]),
            "peer_commitment": C("5"), "operation_commitment": C("3"),
            "request_sha256_commitment": C("6"), "reply_sha256_commitment": None,
            "local_receipt_commitment": None, "remote_receipt_commitment": None,
            "request_frame_bytes": 0, "reply_frame_bytes": 0,
            "request_announced_body_bytes": 10, "reply_announced_body_bytes": None,
            "outcomes": outcomes or ["incomplete"], "replayed": None}


def inspection(attempts=None):
    attempts = attempts or []
    return {"store_present": True, "incomplete_attempts": attempts,
            "history_count": 1, "receipt_count": 0, "audit_event_count": 1}


class ReviewFixes(unittest.TestCase):
    def test_attempt_vocabulary_is_closed(self):
        with self.assertRaisesRegex(ValueError, "unknown phase"):
            G2.validate_attempt(attempt("outbound_request_superseded"), "attempt")
        with self.assertRaisesRegex(ValueError, "terminal phase"):
            G2.validate_attempt(attempt("outbound_exchange_completed"), "attempt")

    def test_exchange_vocabulary_is_closed(self):
        row = exchange(phases=["outbound_request_prepared", "outbound_request_superseded"])
        with self.assertRaisesRegex(ValueError, "unknown phase"):
            G2.validate_exchanges([row], "rows")
        row = exchange(outcomes=["invented"])
        with self.assertRaisesRegex(ValueError, "unknown outcome"):
            G2.validate_exchanges([row], "rows")

    def test_stage_history_cannot_disappear(self):
        old={"inspection":inspection([attempt()]), "exchanges":[exchange()]}
        new={"inspection":inspection(), "exchanges":[]}
        failures=G2.stage_monotonicity(old,new,"baseline","cleanup")
        self.assertTrue(any("nonce disappeared" in failure for failure in failures))
        self.assertTrue(any("attempt disappeared" in failure for failure in failures))

    def test_accepted_terminal_needs_receiver(self):
        sender=exchange(phases=["outbound_request_prepared","outbound_exchange_completed"],
                        outcomes=["accepted","incomplete"])
        sender["reply_sha256_commitment"]=C("7")
        sender["remote_receipt_commitment"]=C("8")
        cleanups=[{"inspection":{"replica_commitment":C("4")}},
                  {"inspection":{"replica_commitment":C("5")}},
                  {"inspection":{"replica_commitment":C("9")}}]
        reason=G2.accepted_terminal_failure(0,sender,{C("2"):[(0,sender)]},cleanups,4,4)
        self.assertIn("exactly one receiver",reason)

    def test_accepted_terminal_is_cross_host_joined(self):
        sender=exchange(phases=["outbound_request_prepared","outbound_exchange_completed"],
                        outcomes=["accepted","incomplete"])
        sender.update(reply_sha256_commitment=C("7"), remote_receipt_commitment=C("8"))
        receiver=exchange("inbound", phases=["inbound_request_observed","inbound_import_committed",
                                              "inbound_reply_prepared","inbound_reply_write_observed"],
                          outcomes=["accepted"])
        receiver.update(peer_commitment=C("4"), local_receipt_commitment=C("8"),
                        request_frame_bytes=14, reply_frame_bytes=14,
                        reply_announced_body_bytes=10, reply_sha256_commitment=C("7"), replayed=False)
        cleanups=[{"inspection":{"replica_commitment":C("4")}},
                  {"inspection":{"replica_commitment":C("5")}},
                  {"inspection":{"replica_commitment":C("9")}}]
        rows={C("2"):[(0,sender),(1,receiver)]}
        self.assertIsNone(G2.accepted_terminal_failure(0,sender,rows,cleanups,4,4))
        sender["remote_receipt_commitment"]=C("a")
        self.assertIn("receiver receipt",G2.accepted_terminal_failure(0,sender,rows,cleanups,4,4))

    def test_replay_must_return_the_original_receipt(self):
        sender=exchange()
        receiver=exchange("inbound", phases=["inbound_request_observed","inbound_import_committed",
                                              "inbound_reply_prepared","inbound_reply_write_observed"],
                          outcomes=["accepted"])
        receiver.update(peer_commitment=C("4"), local_receipt_commitment=C("8"),
                        request_frame_bytes=14, reply_frame_bytes=14,
                        reply_announced_body_bytes=10, reply_sha256_commitment=C("7"), replayed=False)
        retry=dict(receiver, nonce_commitment=C("a"), local_receipt_commitment=C("b"), replayed=True)
        cleanups=[]
        for replica, operations in ((C("4"),[]),(C("5"),[C("3")]),(C("9"),[])):
            cleanups.append({"inspection":{"replica_commitment":replica,"store_present":True,
                                             "imported_operation_commitments":operations}})
        rows={C("2"):[(0,sender),(1,receiver)], C("a"):[(1,retry)]}
        reason,branch=G2.classify(0,attempt(),rows,cleanups,4,4)
        self.assertIsNone(branch)
        self.assertIn("durable receipt",reason)
        retry["local_receipt_commitment"]=C("8")
        reason,branch=G2.classify(0,attempt(),rows,cleanups,4,4)
        self.assertIsNone(reason)
        self.assertEqual(branch,"replay")

    def test_campaign_debt_comes_from_stopped_pre_activation_capture(self):
        pres=[]; bases=[]; cleanups=[]
        for index,replica in enumerate((C("4"),C("5"),C("9"))):
            pre_i={"store_present":True,"incomplete_attempts":[]}
            cleanup_i={"store_present":True,"incomplete_attempts":[attempt()] if index==0 else [],
                       "unaudited_import_receipt_count":0,"replica_commitment":replica,
                       "imported_operation_commitments":[]}
            pres.append({"inspection":pre_i,"exchanges":[]})
            base_i=dict(pre_i)
            if index==0: base_i["incomplete_attempts"]=[attempt()]
            bases.append({"inspection":base_i,"exchanges":[exchange()] if index==0 else []})
            cleanups.append({"inspection":cleanup_i,"exchanges":[exchange()] if index==0 else []})
        result,failures=G2.account_attempts(pres,bases,cleanups,cleanups)
        self.assertEqual(result["preexisting_incomplete_attempts"],0)
        self.assertEqual(result["new_incomplete_attempts"],1)
        self.assertEqual(result["unaccounted_incomplete_attempts"],1)
        self.assertIn("undecided_conditions",result)
        self.assertEqual(result["trust_model"],"collector-honest; cross-host joins only")
        self.assertTrue(failures)


if __name__ == "__main__":
    unittest.main()
