#!/usr/bin/env python3
"""Tests for comparator checks that no suite exercised.

Found by mutation rather than by reading: each refusal below was rewritten to a no-op in a
throwaway copy of `compare-evidence.py` while every suite in this directory stayed green.
A check nothing exercises is a check nobody has -- it can be deleted, weakened or broken by
an unrelated edit and the suites will report success.

The method matters as much as the result. An earlier pass of this measurement was wrong
because the mutation did not apply: the check had been renamed, `sed` matched nothing, and
the green suites were reported as proof that the check was unexercised. Every mutation must
be confirmed to have changed the file before its result means anything.
"""
from __future__ import annotations

import copy
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from g2_adversarial_fixture import STAGES, recount  # noqa: E402
from test_g2_review5_regressions import ReviewFiveCase  # noqa: E402


class UnexercisedCheckTests(ReviewFiveCase):
    """Checks that mutation showed no suite exercises, and what each turned out to be."""

    def test_exchange_direction_may_not_change_between_stages(self) -> None:
        """One wire nonce is one exchange, and an exchange has one direction.

        A nonce published as outbound at one stage and inbound at another is two different
        events wearing one identity. Direction is the field that decides which side of a join
        a row belongs to, so a row free to switch sides between stages is a row that can be
        counted as either.

        The case is built on a plain exchange row rather than on an attempt's, because every
        mutation of an attempt's own row is caught earlier by the phase and corroboration
        rules -- which is worth knowing, and is why this check was reachable by nothing.
        """
        import copy as _copy
        from g2_adversarial_fixture import commitment as _c
        from test_g2_adversarial import RECEIVER_SERVED, SENDER_PREPARED
        # A TERMINAL outbound row, so the non-terminal converse rule does not claim it first.
        outbound = _copy.deepcopy(SENDER_PREPARED)
        outbound.update(
            nonce_commitment=_c("nonce-side-swap"),
            operation_commitment=_c("op-side-swap"),
            request_sha256_commitment=_c("rq-side-swap"),
            reply_sha256_commitment=_c("rp-side-swap"),
            remote_receipt_commitment=_c("rcpt-side-swap"),
            phases_reached=["outbound_request_prepared", "outbound_exchange_completed"],
            row_count=2,
            request_frame_bytes=2735,
            reply_frame_bytes=626,
            reply_announced_body_bytes=622,
            outcomes=["accepted"],
            replayed=False,
        )
        inbound = _copy.deepcopy(RECEIVER_SERVED)
        inbound.update(
            nonce_commitment=outbound["nonce_commitment"],
            operation_commitment=outbound["operation_commitment"],
            request_sha256_commitment=outbound["request_sha256_commitment"],
            request_announced_body_bytes=outbound["request_announced_body_bytes"],
            peer_commitment=outbound["peer_commitment"],
        )
        for stage, row in (("pre-activation", outbound), ("active-baseline", outbound),
                           ("converged", outbound), ("post-cleanup", inbound)):
            capture = self.fixtures[("lab-a", stage)]
            capture["exchanges"].append(_copy.deepcopy(row))
            recount(capture)
        self.assert_refuses_for("exchange direction changed")

    # The other two checks mutation found unheld are DEFENCE IN DEPTH, not gaps, and the
    # difference is worth recording because it changes what should be done about them.
    #
    #   compare-evidence.py  "an incomplete attempt last_phase is not the highest
    #                         corroborating phase"
    #   compare-evidence.py  "pre-existing debt attempt changed at <stage>"
    #
    # Both survive deletion with every suite green. But no evidence reaches them: every
    # mutation that would, is refused first by a stronger rule. Raising an attempt's
    # last_phase to a terminal one is refused by "an incomplete attempt cannot have a
    # terminal phase"; advancing its corroborating row is refused by "corroborated by a
    # terminal exchange row"; and for an outbound debt attempt every remaining mutable field
    # is already compared across stages by the exchange-row rules. The two tests below were
    # written against them and each was refused for the earlier reason instead, which is the
    # evidence for this note.
    #
    # So they are a second line behind a first that holds, not an untested safeguard. They
    # are worth keeping -- the first line could be narrowed by a later edit -- and they are
    # not worth a test that would only assert the first line firing.

    def test_an_incomplete_attempt_may_not_claim_a_terminal_phase(self) -> None:
        """The first line in front of the debt-change check, pinned."""
        for stage in STAGES:
            self.install_strand(stage, receiver=False)
        later = self.fixtures[("lab-a", "post-cleanup")]
        later["inspection"]["incomplete_attempts"][0]["last_phase"] = "outbound_exchange_completed"
        self.assert_refuses_for("an incomplete attempt cannot have a terminal phase")

    def test_an_incomplete_attempt_may_not_be_corroborated_by_a_terminal_row(self) -> None:
        """The first line in front of the highest-phase check, pinned."""
        for stage in STAGES[1:]:
            self.install_strand(stage, receiver=False)
        sender = self.fixtures[("lab-a", "post-cleanup")]
        nonce = sender["inspection"]["incomplete_attempts"][0]["nonce_commitment"]
        row = next(r for r in sender["exchanges"] if r["nonce_commitment"] == nonce)
        row["phases_reached"] = ["outbound_request_prepared", "outbound_exchange_completed"]
        row["row_count"] = 2
        recount(sender)
        self.assert_refuses_for("corroborated by a terminal exchange row")


if __name__ == "__main__":
    unittest.main(verbosity=1)
