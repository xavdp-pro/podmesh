# Manager2 live activation preparation review

Status: implementation and offline qualification complete; real-host activation
not yet executed.

## Scope

This increment prepares the first authenticated three-host manager2 campaign for
non-exclusive observations. It adds an exact zero-to-three-grant configuration
transition, a shared transition/activation lock, a persistent first-activation
boundary, typed readiness and shutdown, four-stage evidence collection, and
strict three-host comparison.

The campaign may prove durable observation exchange and canonical convergence.
It cannot prove high availability, takeover, fencing, DNS, routing, Podman
effects, automatic failover or production readiness.

## Candidate binding

- Source commit: `ff77b1f946e82af421de2ccdbe06d8cd45b70c33`
- Package: `0.1.0~manager2+gff77b1f946e8`
- Binary SHA-256:
  `cbd5020a37d2b6b2ab94ee41a7ea9d2f50d0da36c6fb972f795c8a1fe3660128`
- Debian SHA-256:
  `4e475a2421302b5a3c1c6583853f9e04515076cbd00f430e805ecb9a3ecefc8d`

Every host-side transition and capture rechecks the installed version, exact
binary hash and clean `dpkg --verify` result against the candidate-verification
document.

## Corrected failure boundaries

The review loop corrected these blocking conditions before host mutation:

1. Transition and activation now hold the same root-only host lock.
2. The rollback window is durably sealed before first activation.
3. The persistent activation marker binds the host alias, package version and
   exact binary hash.
4. A crash leaving the temporary systemd drop-in has a hash-bound cleanup path.
5. A crash between ledger and checksum-sidecar replacement has a strict,
   root-owned recovery path.
6. A failed `systemctl start` records `start-failed` and can only produce
   `RECOVERED_NOT_QUALIFIED`, never activation evidence.
7. Cleanup remains resumable when an interruption occurs immediately after a
   hash-bound drop-in removal.
8. Every comparison input must pass its adjacent SHA-256 sidecar before JSON is
   parsed.

## Verification

Codex independently ran:

- exact transition and refusal tests;
- a Bubblewrap integration test covering apply, interrupted prepared recovery,
  idempotent apply, rollback, rollback evidence repair and permanent sealing;
- six typed readiness/shutdown helper tests;
- the synthetic four-stage activation comparator and its negative cases;
- the complete existing manager package qualification suite;
- shell syntax, Python compilation and `git diff --check`.

Claude Code was authenticated through the existing Max subscription. Long Opus
and high-effort review attempts expired without a report, so they are not
counted as evidence. Focused Claude Sonnet low-effort reviews produced actionable
NO-GO findings for the activation state machine; Codex verified and corrected
each finding. The final Claude closure returned GO for disposable laboratory
activation. A separate focused review returned GO for the configuration
transition.

## Remaining proof

The scripts are preparation, not live evidence. The next proof must execute the
reviewed sequence on all three disposable hosts, collect all four stages, prove
canonical convergence after one owned observation per replica, shut every
resident down through the typed control operation, and prove unchanged unrelated
services, containers, routes and firewall state.

