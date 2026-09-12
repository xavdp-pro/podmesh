# Counter-review provider preference

Operator instruction, 2026-09-12: use Claude Code for counter-reviews by default,
including subsequent lots. Codex remains responsible for coordination, critical
verification of findings and final integration.

Use the operator's existing authenticated Claude subscription session. Confirm
availability and authentication before reporting that a review has started. Do not
switch to paid API credentials. Announce the requested model and reasoning effort.

Provide the exact source revision, relevant intent and boundaries, changed code,
recorded test evidence and remaining unproven claims. Distinguish supplied evidence
from tests actually executed by the reviewer. Keep review access proportionate to
the lot: a supplied-source review can run with tools disabled.

If Claude is unavailable, report that fact and keep independent authorized work
moving. Do not silently label another Codex agent as a Claude counter-review.
Another Codex review may supplement review, but does not satisfy this preference.
This routing instruction does not change runtime authority or production scope.
