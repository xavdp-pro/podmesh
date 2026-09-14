#!/usr/bin/env python3
"""Adversarial non-regression tests for the three-host G2 evidence gate.

The fixtures model the frozen candidate's actual receipt topology: an
authenticated-import receipt is local to the receiver.  Every negative test
changes one relevant property and requires a typed refusal from the real CLI.
"""

from __future__ import annotations

import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TEST_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(TEST_DIR))

from g2_adversarial_fixture import (  # noqa: E402
    ALIASES,
    STAGES,
    build_fixture,
    commitment,
    comparator_versions,
    recount,
    seal,
)


ACTIVATION_DIR = TEST_DIR.parent
COMPARATOR = Path(os.environ.get("PODMESH_G2_COMPARATOR", ACTIVATION_DIR / "compare-evidence.py"))

RA, RB, RC = (commitment("replica-" + host) for host in ALIASES)
NONCE = commitment("nonce-strand")
OPERATION = commitment("op-strand")
REQUEST = commitment("rq-strand")
RECEIPT = commitment("rcpt-strand")

ATTEMPT = {
    "attempt_commitment": commitment("attempt-strand"),
    "nonce_commitment": NONCE,
    "nonce_authority": "peer-validated",
    "operation_commitment": OPERATION,
    "direction": "outbound",
    "last_phase": "outbound_request_prepared",
}
SENDER_PREPARED = {
    "nonce_commitment": NONCE,
    "nonce_authority": "peer-validated",
    "joinable": True,
    "direction": "outbound",
    "phases_reached": ["outbound_request_prepared"],
    "row_count": 1,
    "peer_commitment": RB,
    "operation_commitment": OPERATION,
    "request_sha256_commitment": REQUEST,
    "reply_sha256_commitment": None,
    "local_receipt_commitment": None,
    "remote_receipt_commitment": None,
    "request_frame_bytes": 0,
    "reply_frame_bytes": 0,
    "request_announced_body_bytes": 2731,
    "reply_announced_body_bytes": None,
    "outcomes": ["incomplete"],
    "replayed": None,
}
RECEIVER_SERVED = {
    "nonce_commitment": NONCE,
    "nonce_authority": "peer-validated",
    "joinable": True,
    "direction": "inbound",
    "phases_reached": [
        "inbound_request_observed",
        "inbound_import_committed",
        "inbound_reply_prepared",
        "inbound_reply_write_observed",
    ],
    "row_count": 4,
    "peer_commitment": RA,
    "operation_commitment": OPERATION,
    "request_sha256_commitment": REQUEST,
    "reply_sha256_commitment": commitment("rp-strand"),
    "local_receipt_commitment": RECEIPT,
    "remote_receipt_commitment": None,
    "request_frame_bytes": 2735,
    "reply_frame_bytes": 626,
    "request_announced_body_bytes": 2731,
    "reply_announced_body_bytes": 622,
    "outcomes": ["accepted"],
    "replayed": False,
}


class ComparatorCase(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="podmesh-g2-adversarial-")
        self.directory = Path(self.temporary.name)
        self.fixtures = build_fixture(COMPARATOR)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_comparator(self) -> tuple[int, dict]:
        seal(self.directory, self.fixtures)
        command = [str(COMPARATOR), "--phase", "three-host"]
        for flag, stage in (
            ("--pre", STAGES[0]),
            ("--active-baseline", STAGES[1]),
            ("--converged", STAGES[2]),
            ("--cleanup", STAGES[3]),
        ):
            command.extend([flag, *(str(self.directory / f"{host}-{stage}.json") for host in ALIASES)])
        completed = subprocess.run(command, text=True, capture_output=True, check=False)
        self.assertTrue(completed.stdout.strip(), f"comparator crashed without a report: {completed.stderr}")
        try:
            report = json.loads(completed.stdout)
        except json.JSONDecodeError as error:
            self.fail(f"comparator emitted non-JSON output: {error}: {completed.stdout!r}")
        return completed.returncode, report

    def assert_passes(self) -> dict:
        code, report = self.run_comparator()
        self.assertEqual((code, report.get("status")), (0, "PASS"), report)
        return report

    def assert_refuses(self) -> dict:
        code, report = self.run_comparator()
        self.assertNotEqual(code, 0, report)
        self.assertEqual(report.get("status"), "FAIL", report)
        return report

    def install_strand(self, stage: str, *, receiver: bool = True) -> None:
        sender = self.fixtures[("lab-a", stage)]
        sender["inspection"]["incomplete_attempts"] = [copy.deepcopy(ATTEMPT)]
        sender["exchanges"].append(copy.deepcopy(SENDER_PREPARED))
        recount(sender)
        if receiver:
            target = self.fixtures[("lab-b", stage)]
            target["exchanges"].append(copy.deepcopy(RECEIVER_SERVED))
            target["inspection"]["imported_operation_commitments"] = [OPERATION]
            recount(target)

    def install_replay(self, *, valid: bool) -> None:
        retry_nonce = commitment("nonce-retry")
        request = commitment("rq-retry")
        reply = commitment("rp-retry")
        receiver = copy.deepcopy(RECEIVER_SERVED)
        receiver.update(
            nonce_commitment=retry_nonce,
            request_sha256_commitment=request,
            reply_sha256_commitment=reply,
            replayed=True,
        )
        if not valid:
            receiver.update(
                phases_reached=["inbound_request_observed", "inbound_refusal_recorded"],
                row_count=2,
                outcomes=["authenticated_refusal"],
                reply_sha256_commitment=None,
                local_receipt_commitment=commitment("different-receipt"),
                reply_frame_bytes=0,
                reply_announced_body_bytes=None,
            )
        target = self.fixtures[("lab-b", "post-cleanup")]
        target["exchanges"].append(receiver)
        recount(target)

        sender = copy.deepcopy(SENDER_PREPARED)
        sender.update(
            nonce_commitment=retry_nonce,
            request_sha256_commitment=request,
            phases_reached=["outbound_exchange_completed", "outbound_request_prepared"],
            row_count=2,
            outcomes=["accepted", "incomplete"] if valid else ["authenticated_refusal", "incomplete"],
            reply_sha256_commitment=reply if valid else None,
            remote_receipt_commitment=RECEIPT if valid else None,
            request_frame_bytes=2735,
            reply_frame_bytes=626 if valid else 0,
            reply_announced_body_bytes=622 if valid else None,
            replayed=True if valid else False,
        )
        source = self.fixtures[("lab-a", "post-cleanup")]
        source["exchanges"].append(sender)
        recount(source)


class G2SchemaPinningTests(unittest.TestCase):
    def test_new_evidence_shapes_have_distinct_versions(self) -> None:
        evidence, result, inspection = comparator_versions(COMPARATOR)
        self.assertEqual(evidence, "podmesh-manager-live-activation-evidence/v3")
        self.assertEqual(result, "podmesh-manager-live-activation-comparison/v3")
        self.assertEqual(inspection, 4)


class G2AdversarialTests(ComparatorCase):
    def test_control_fixture_passes(self) -> None:
        self.assert_passes()

    def test_f1_attempt_created_after_stopped_manager_capture_is_not_baseline_debt(self) -> None:
        for stage in ("active-baseline", "converged", "post-cleanup"):
            self.install_strand(stage, receiver=False)
        report = self.assert_refuses()
        self.assertNotEqual(
            report.get("incomplete_attempt_accounting", {}).get("preexisting_incomplete_attempts"),
            1,
            report,
        )

    def test_f2_unknown_exchange_phase_is_refused(self) -> None:
        row = copy.deepcopy(SENDER_PREPARED)
        row.update(
            phases_reached=["outbound_request_prepared", "outbound_request_superseded"],
            row_count=2,
        )
        capture = self.fixtures[("lab-a", "post-cleanup")]
        capture["exchanges"].append(row)
        recount(capture)
        self.assert_refuses()

    def test_f2_unknown_attempt_last_phase_is_refused(self) -> None:
        for stage in ("pre-activation", "active-baseline", "converged", "post-cleanup"):
            self.install_strand(stage, receiver=False)
            self.fixtures[("lab-a", stage)]["inspection"]["incomplete_attempts"][0]["last_phase"] = "no_such_phase"
        self.assert_refuses()

    def test_f2_phase_from_the_wrong_direction_is_refused(self) -> None:
        row = copy.deepcopy(SENDER_PREPARED)
        row["phases_reached"] = ["inbound_request_observed"]
        capture = self.fixtures[("lab-a", "post-cleanup")]
        capture["exchanges"].append(row)
        recount(capture)
        self.assert_refuses()

    def test_f2_unknown_outcome_is_refused(self) -> None:
        row = copy.deepcopy(SENDER_PREPARED)
        row["outcomes"] = ["eventually_ok"]
        capture = self.fixtures[("lab-a", "post-cleanup")]
        capture["exchanges"].append(row)
        recount(capture)
        self.assert_refuses()

    def test_f3_genuine_replay_with_the_original_receipt_accounts_for_the_strand(self) -> None:
        self.install_strand("post-cleanup")
        self.install_replay(valid=True)
        report = self.assert_passes()
        accounting = report["incomplete_attempt_accounting"]
        self.assertEqual(accounting["accounted_incomplete_attempts"], 1)

    def test_f3_refused_retry_with_an_unrelated_receipt_cannot_account_for_the_strand(self) -> None:
        self.install_strand("post-cleanup")
        self.install_replay(valid=False)
        self.assert_refuses()

    def test_f4_receiver_only_receipt_topology_is_exercised(self) -> None:
        self.install_strand("post-cleanup")
        self.install_replay(valid=True)
        for host in ("lab-a", "lab-c"):
            self.assertNotIn(OPERATION, self.fixtures[(host, "post-cleanup")]["inspection"]["imported_operation_commitments"])
        self.assertIn(OPERATION, self.fixtures[("lab-b", "post-cleanup")]["inspection"]["imported_operation_commitments"])
        self.assert_passes()

    def test_f4_receiver_only_receipt_without_replay_is_not_silently_accounted(self) -> None:
        self.install_strand("post-cleanup")
        self.assert_refuses()

    def test_f5_accepted_terminal_sender_claim_requires_receiver_evidence(self) -> None:
        self.install_strand("converged", receiver=False)
        completed = copy.deepcopy(SENDER_PREPARED)
        completed.update(
            phases_reached=["outbound_exchange_completed", "outbound_request_prepared"],
            row_count=2,
            outcomes=["accepted", "incomplete"],
            reply_sha256_commitment=commitment("invented-reply"),
            remote_receipt_commitment=commitment("invented-receipt"),
            request_frame_bytes=2735,
            reply_frame_bytes=626,
            reply_announced_body_bytes=622,
            replayed=False,
        )
        cleanup = self.fixtures[("lab-a", "post-cleanup")]
        cleanup["exchanges"].append(completed)
        recount(cleanup)
        self.assert_refuses()

    def test_f5_matching_terminal_sender_and_receiver_evidence_passes(self) -> None:
        retry_nonce = commitment("terminal-nonce")
        request = commitment("terminal-request")
        reply = commitment("terminal-reply")
        receipt = commitment("terminal-receipt")
        sender = copy.deepcopy(SENDER_PREPARED)
        sender.update(
            nonce_commitment=retry_nonce,
            request_sha256_commitment=request,
            phases_reached=["outbound_exchange_completed", "outbound_request_prepared"],
            row_count=2,
            outcomes=["accepted", "incomplete"],
            reply_sha256_commitment=reply,
            remote_receipt_commitment=receipt,
            request_frame_bytes=2735,
            reply_frame_bytes=626,
            reply_announced_body_bytes=622,
            replayed=False,
        )
        receiver = copy.deepcopy(RECEIVER_SERVED)
        receiver.update(
            nonce_commitment=retry_nonce,
            request_sha256_commitment=request,
            reply_sha256_commitment=reply,
            local_receipt_commitment=receipt,
        )
        self.fixtures[("lab-a", "post-cleanup")]["exchanges"].append(sender)
        self.fixtures[("lab-b", "post-cleanup")]["exchanges"].append(receiver)
        recount(self.fixtures[("lab-a", "post-cleanup")])
        recount(self.fixtures[("lab-b", "post-cleanup")])
        self.assert_passes()

    def test_f6_nonce_and_attempt_cannot_disappear_between_stages(self) -> None:
        self.install_strand("converged", receiver=False)
        # Keep aggregate counts non-decreasing so only the missing nonce/attempt
        # transition can reject this mutation.
        converged = self.fixtures[("lab-a", "converged")]
        converged["exchanges"] = [converged["exchanges"][0], copy.deepcopy(SENDER_PREPARED)]
        recount(converged)
        self.assert_refuses()

    def test_f6_store_counts_cannot_regress_between_stages(self) -> None:
        baseline = self.fixtures[("lab-a", "active-baseline")]
        baseline["inspection"]["history_count"] = 50
        baseline["inspection"]["receipt_count"] = 50
        self.assert_refuses()

    def test_f6_phase_set_and_row_count_are_monotonic(self) -> None:
        for stage in ("active-baseline", "converged"):
            self.install_strand(stage, receiver=False)
        converged = self.fixtures[("lab-a", "converged")]
        row = next(row for row in converged["exchanges"] if row["nonce_commitment"] == NONCE)
        row.update(
            phases_reached=["outbound_exchange_completed", "outbound_request_prepared"],
            row_count=2,
            outcomes=["accepted", "incomplete"],
            reply_sha256_commitment=commitment("phase-reply"),
            remote_receipt_commitment=commitment("phase-receipt"),
            request_frame_bytes=2735,
            reply_frame_bytes=626,
            reply_announced_body_bytes=622,
            replayed=False,
        )
        converged["inspection"]["incomplete_attempts"] = []
        converged["exchanges"] = [converged["exchanges"][0], row]
        recount(converged)
        # Cleanup regresses this nonce to its single prepared row.
        self.install_strand("post-cleanup", receiver=False)
        self.assert_refuses()


if __name__ == "__main__":
    unittest.main(verbosity=2)
