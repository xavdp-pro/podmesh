#!/usr/bin/env python3
"""CLI regressions for the fifth independent G2 review.

Each refusal starts from a valid positive fixture and changes one semantic
property.  Reason assertions make every surviving review mutant observable.
"""

from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path


TEST_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(TEST_DIR))

from g2_adversarial_fixture import STAGES, commitment, recount  # noqa: E402
from test_g2_adversarial import (  # noqa: E402
    ATTEMPT,
    COMPARATOR,
    NONCE,
    OPERATION,
    RA,
    RB,
    RC,
    RECEIPT,
    RECEIVER_SERVED,
    REQUEST,
    SENDER_PREPARED,
    ComparatorCase,
)


REPLAY_NONCE = commitment("nonce-retry")
REPLAY_REQUEST = commitment("rq-retry")
REPLAY_REPLY = commitment("rp-retry")


class ReviewFiveCase(ComparatorCase):
    def reason_text(self, report: dict) -> str:
        return json.dumps(
            {"error": report.get("error"), "failures": report.get("failures")},
            sort_keys=True,
        )

    def assert_refuses_for(self, expected: str) -> dict:
        report = self.assert_refuses()
        self.assertIn(expected, self.reason_text(report), report)
        return report

    def replay_row(self) -> dict:
        row = copy.deepcopy(RECEIVER_SERVED)
        row.update(
            nonce_commitment=REPLAY_NONCE,
            request_sha256_commitment=REPLAY_REQUEST,
            reply_sha256_commitment=REPLAY_REPLY,
            replayed=True,
        )
        return row

    def install_valid_replay(self) -> None:
        self.install_strand("post-cleanup")
        target = self.fixtures[("lab-b", "post-cleanup")]
        target["exchanges"].append(self.replay_row())
        recount(target)

    def install_joined_replay(self, label: str) -> tuple[dict, dict]:
        nonce = commitment(f"nonce-retry-{label}")
        request = commitment(f"rq-retry-{label}")
        reply = commitment(f"rp-retry-{label}")
        receiver = self.replay_row()
        receiver.update(
            nonce_commitment=nonce,
            request_sha256_commitment=request,
            reply_sha256_commitment=reply,
        )
        sender = copy.deepcopy(SENDER_PREPARED)
        sender.update(
            nonce_commitment=nonce,
            phases_reached=["outbound_exchange_completed", "outbound_request_prepared"],
            row_count=2,
            peer_commitment=RB,
            operation_commitment=OPERATION,
            request_sha256_commitment=request,
            reply_sha256_commitment=reply,
            remote_receipt_commitment=RECEIPT,
            request_frame_bytes=2735,
            reply_frame_bytes=626,
            reply_announced_body_bytes=622,
            outcomes=["accepted", "incomplete"],
            replayed=True,
        )
        self.fixtures[("lab-a", "post-cleanup")]["exchanges"].append(sender)
        self.fixtures[("lab-b", "post-cleanup")]["exchanges"].append(receiver)
        recount(self.fixtures[("lab-a", "post-cleanup")])
        recount(self.fixtures[("lab-b", "post-cleanup")])
        return sender, receiver

    def replay_receiver(self, host: str = "lab-b") -> dict:
        return next(
            row
            for row in self.fixtures[(host, "post-cleanup")]["exchanges"]
            if row["nonce_commitment"] == REPLAY_NONCE and row["direction"] == "inbound"
        )

    def install_valid_terminal(self, *, stage: str = "post-cleanup") -> tuple[dict, dict]:
        sender = copy.deepcopy(SENDER_PREPARED)
        sender.update(
            phases_reached=["outbound_exchange_completed", "outbound_request_prepared"],
            row_count=2,
            outcomes=["accepted", "incomplete"],
            reply_sha256_commitment=RECEIVER_SERVED["reply_sha256_commitment"],
            remote_receipt_commitment=RECEIPT,
            request_frame_bytes=2735,
            reply_frame_bytes=626,
            reply_announced_body_bytes=622,
            replayed=False,
        )
        receiver = copy.deepcopy(RECEIVER_SERVED)
        self.fixtures[("lab-a", stage)]["exchanges"].append(sender)
        self.fixtures[("lab-b", stage)]["exchanges"].append(receiver)
        self.fixtures[("lab-b", stage)]["inspection"]["imported_operation_commitments"] = [OPERATION]
        recount(self.fixtures[("lab-a", stage)])
        recount(self.fixtures[("lab-b", stage)])
        return sender, receiver

    def install_inbound_strand(self, stage: str, *, old: bool = False) -> tuple[dict, dict]:
        nonce = commitment("nonce-old-in") if old else NONCE
        operation = commitment("op-old-in") if old else OPERATION
        attempt = {
            "attempt_commitment": commitment("attempt-old-in") if old else commitment("attempt-recv"),
            "nonce_commitment": nonce,
            "nonce_authority": "peer-validated",
            "operation_commitment": operation,
            "direction": "inbound",
            "last_phase": "inbound_reply_prepared" if old else "inbound_import_committed",
        }
        row = copy.deepcopy(RECEIVER_SERVED)
        row.update(
            nonce_commitment=nonce,
            operation_commitment=operation,
            request_sha256_commitment=commitment("rq-old-in") if old else REQUEST,
            phases_reached=(
                ["inbound_import_committed", "inbound_reply_prepared", "inbound_request_observed"]
                if old
                else ["inbound_import_committed", "inbound_request_observed"]
            ),
            row_count=3 if old else 2,
            outcomes=["accepted", "incomplete"] if old else ["accepted"],
            local_receipt_commitment=commitment("receipt-old-in") if old else RECEIPT,
            reply_sha256_commitment=None,
            reply_frame_bytes=0,
            reply_announced_body_bytes=None,
            replayed=False,
        )
        capture = self.fixtures[("lab-b", stage)]
        capture["inspection"]["incomplete_attempts"].append(copy.deepcopy(attempt))
        capture["exchanges"].append(copy.deepcopy(row))
        capture["inspection"]["history_count"] = max(capture["inspection"]["history_count"], 1)
        capture["inspection"]["receipt_count"] = max(capture["inspection"]["receipt_count"], 1)
        capture["inspection"]["imported_operation_commitments"] = [operation]
        recount(capture)
        return attempt, row


class ReplayMutationTests(ReviewFiveCase):
    REPLAY_REASON = "no receiver observed a complete replay carrying the durable receipt"

    def test_r2_replay_must_be_on_the_original_receiver(self) -> None:
        self.install_valid_replay()
        row = self.replay_receiver()
        source = self.fixtures[("lab-b", "post-cleanup")]
        source["exchanges"].remove(row)
        recount(source)
        target = self.fixtures[("lab-c", "post-cleanup")]
        target["exchanges"].append(row)
        recount(target)
        self.assert_refuses_for(self.REPLAY_REASON)

    def test_r3_replay_flag_is_required(self) -> None:
        self.install_valid_replay()
        self.replay_receiver()["replayed"] = False
        self.assert_refuses_for(self.REPLAY_REASON)

    def test_r5_replay_requires_the_import_phase(self) -> None:
        self.install_valid_replay()
        row = self.replay_receiver()
        row["phases_reached"] = [
            "inbound_request_observed",
            "inbound_reply_prepared",
            "inbound_reply_write_observed",
        ]
        row["row_count"] = 3
        recount(self.fixtures[("lab-b", "post-cleanup")])
        self.assert_refuses_for(self.REPLAY_REASON)

    def test_r6_replay_requires_exact_accepted_outcome(self) -> None:
        self.install_valid_replay()
        self.replay_receiver()["outcomes"] = ["authenticated_refusal"]
        self.assert_refuses_for(self.REPLAY_REASON)

    def test_r7_replay_binds_the_original_sender_peer(self) -> None:
        self.install_valid_replay()
        self.replay_receiver()["peer_commitment"] = RC
        self.assert_refuses_for(self.REPLAY_REASON)

    def test_r8_replay_binds_the_original_operation(self) -> None:
        self.install_valid_replay()
        self.replay_receiver()["operation_commitment"] = commitment("different-operation")
        self.assert_refuses_for(self.REPLAY_REASON)

    def test_r9_original_nonce_cannot_claim_to_be_its_own_replay(self) -> None:
        self.install_strand("post-cleanup")
        original = next(
            row
            for row in self.fixtures[("lab-b", "post-cleanup")]["exchanges"]
            if row["nonce_commitment"] == NONCE
        )
        original["replayed"] = True
        self.assert_refuses_for(self.REPLAY_REASON)

    def test_multiple_fully_joined_replays_account_for_one_strand(self) -> None:
        self.install_strand("post-cleanup")
        self.install_joined_replay("one")
        self.install_joined_replay("two")
        report = self.assert_passes()
        details = report["incomplete_attempt_accounting"]["accounted_detail"]
        self.assertEqual(details[0]["branch"], "replay", report)

    def test_fully_joined_replay_is_preferred_over_receiver_only_replay(self) -> None:
        self.install_valid_replay()
        self.install_joined_replay("joined-after-receiver-only")
        report = self.assert_passes()
        details = report["incomplete_attempt_accounting"]["accounted_detail"]
        self.assertEqual(details[0]["branch"], "replay", report)

    def test_mismatched_replay_sender_is_refused(self) -> None:
        self.install_strand("post-cleanup")
        sender, _ = self.install_joined_replay("mismatch")
        sender["peer_commitment"] = RC
        self.assert_refuses_for("replay sender does not bind the receiver replica")


class TerminalMutationTests(ReviewFiveCase):
    PREFIX = "an accepted terminal attempt is unaccounted for"

    def test_t3_terminal_join_binds_both_peers(self) -> None:
        _, receiver = self.install_valid_terminal()
        receiver["peer_commitment"] = RC
        self.assert_refuses_for("does not bind both declared peers")

    def test_t4_terminal_join_binds_reply_digest(self) -> None:
        sender, _ = self.install_valid_terminal()
        sender["reply_sha256_commitment"] = commitment("different-reply")
        self.assert_refuses_for("does not bind reply_sha256_commitment")

    def test_t5_terminal_join_requires_exactly_one_receiver(self) -> None:
        self.install_valid_terminal()
        duplicate = copy.deepcopy(RECEIVER_SERVED)
        duplicate["peer_commitment"] = RA
        target = self.fixtures[("lab-c", "post-cleanup")]
        target["exchanges"].append(duplicate)
        recount(target)
        self.assert_refuses_for("exactly one receiver-side row")

    def test_t6_terminal_join_requires_an_accepted_import(self) -> None:
        _, receiver = self.install_valid_terminal()
        receiver["outcomes"] = ["authenticated_refusal"]
        self.assert_refuses_for("has no accepted receiver import")


class DebtAndWindowMutationTests(ReviewFiveCase):
    def install_outbound_debt(self, stages=STAGES) -> None:
        for stage in stages:
            self.install_strand(stage, receiver=False)

    def test_d2_debt_peer_anchor_must_exist_before_activation(self) -> None:
        self.install_outbound_debt()
        target = self.fixtures[("lab-b", "post-cleanup")]
        target["exchanges"].append(copy.deepcopy(RECEIVER_SERVED))
        recount(target)
        self.assert_refuses_for("pre-existing debt has a peer row that was absent before activation")

    def test_d3_pre_activation_manager_process_must_be_zero(self) -> None:
        self.fixtures[("lab-a", "pre-activation")]["manager_process"]["count"] = 1
        self.assert_refuses_for("pre-activation manager is not disabled and inactive")

    def test_d4_pre_activation_inspection_is_mandatory(self) -> None:
        capture = self.fixtures[("lab-a", "pre-activation")]
        capture["inspection"] = None
        capture["exchanges"] = None
        self.assert_refuses_for("pre-activation inspection is absent or not bound")

    def test_x1_v2_evidence_is_explicitly_refused(self) -> None:
        self.fixtures[("lab-a", "converged")]["schema_version"] = "podmesh-manager-live-activation-evidence/v2"
        self.assert_refuses_for("unsupported evidence schema")


class InboundAttemptTests(ReviewFiveCase):
    def test_n1_inbound_attempt_may_be_in_flight_at_converged(self) -> None:
        self.install_strand("converged", receiver=False)
        self.install_inbound_strand("converged")
        self.install_valid_terminal()
        self.assert_passes()

    def test_n1_inbound_debt_is_retained_and_reported(self) -> None:
        for stage in STAGES:
            self.install_inbound_strand(stage, old=True)
        report = self.assert_passes()
        accounting = report["incomplete_attempt_accounting"]
        self.assertEqual(accounting["preexisting_incomplete_attempts"], 1, report)
        self.assertEqual(accounting["preexisting_detail"][0]["last_phase"], "inbound_reply_prepared", report)

    def test_n1_new_inbound_attempt_at_cleanup_is_refused_with_its_reason(self) -> None:
        self.install_inbound_strand("post-cleanup")
        self.assert_refuses_for("inbound incomplete attempt has no receiver-side join defined")


class DebtRetirementTests(ReviewFiveCase):
    def install_debt_until_converged(self) -> None:
        for stage in STAGES[:3]:
            self.install_strand(stage, receiver=False)

    def completed(self, outcomes: list[str]) -> dict:
        row = copy.deepcopy(SENDER_PREPARED)
        accepted = "accepted" in outcomes
        row.update(
            phases_reached=["outbound_exchange_completed", "outbound_request_prepared"],
            row_count=2,
            outcomes=outcomes,
            reply_sha256_commitment=commitment("debt-reply") if accepted else None,
            remote_receipt_commitment=commitment("debt-receipt") if accepted else None,
            request_frame_bytes=2735,
            reply_frame_bytes=626 if accepted else 0,
            reply_announced_body_bytes=622 if accepted else None,
            replayed=False,
        )
        return row

    def test_n3_debt_cannot_retire_as_sender_only_accepted(self) -> None:
        self.install_debt_until_converged()
        cleanup = self.fixtures[("lab-a", "post-cleanup")]
        cleanup["exchanges"].append(self.completed(["accepted", "incomplete"]))
        recount(cleanup)
        self.assert_refuses_for("pre-existing debt")

    def test_n3_debt_cannot_retire_as_sender_only_unavailable(self) -> None:
        self.install_debt_until_converged()
        cleanup = self.fixtures[("lab-a", "post-cleanup")]
        cleanup["exchanges"].append(self.completed(["incomplete", "unavailable"]))
        recount(cleanup)
        self.assert_refuses_for("pre-existing debt")


class ReportingAndReachabilityTests(ReviewFiveCase):
    def test_n4_receiver_only_replay_is_marked_receiver_asserted(self) -> None:
        self.install_strand("post-cleanup")
        target = self.fixtures[("lab-b", "post-cleanup")]
        target["exchanges"].append(self.replay_row())
        recount(target)
        report = self.assert_passes()
        details = report["incomplete_attempt_accounting"]["accounted_detail"]
        self.assertEqual(details[0]["branch"], "receiver_asserted", report)

    def test_n4_unmatched_inbound_accepted_row_is_reported(self) -> None:
        target = self.fixtures[("lab-b", "post-cleanup")]
        target["exchanges"].append(copy.deepcopy(RECEIVER_SERVED))
        recount(target)
        report = self.assert_passes()
        unmatched = report["unmatched_inbound_rows"]
        self.assertEqual(unmatched["count"], 1, report)
        self.assertEqual(len(unmatched["detail"]), 1, report)

    def test_n5_impossible_every_replica_receipt_branch_does_not_account(self) -> None:
        self.install_strand("post-cleanup")
        for host in ("lab-a", "lab-b", "lab-c"):
            self.fixtures[(host, "post-cleanup")]["inspection"]["imported_operation_commitments"] = [OPERATION]
        self.assert_refuses_for("no receiver observed a complete replay carrying the durable receipt")


class CaptureAndIdentityTests(ReviewFiveCase):
    def test_n6_pre_activation_capture_rechecks_unit_and_process_after_inspection(self) -> None:
        source = (TEST_DIR.parent / "capture-host.sh").read_text()
        inspection_at = source.index("inspect=$(inspection)")
        seal_at = source.index("jq -nS", inspection_at)
        after_inspection = source[inspection_at:seal_at]
        self.assertIn("unit podmesh-manager.service", after_inspection)
        self.assertIn("manager_process", after_inspection)
        self.assertIn("changed during inspection", after_inspection)

    def test_n7_exchange_identity_is_frozen_across_stages(self) -> None:
        self.install_valid_terminal(stage="converged")
        self.install_valid_terminal(stage="post-cleanup")
        for host in ("lab-a", "lab-b"):
            row = next(
                value
                for value in self.fixtures[(host, "post-cleanup")]["exchanges"]
                if value["nonce_commitment"] == NONCE
            )
            row["operation_commitment"] = commitment("rewritten-operation")
        self.assert_refuses_for("exchange operation_commitment changed")

    def test_n7_outbound_fold_is_capped_at_two_rows(self) -> None:
        sender, _ = self.install_valid_terminal()
        sender["row_count"] = 3
        sender["outcomes"] = ["accepted", "incomplete", "malformed"]
        recount(self.fixtures[("lab-a", "post-cleanup")])
        self.assert_refuses_for("outbound exchange cannot collapse more than two audit rows")


if __name__ == "__main__":
    unittest.main(verbosity=2)
