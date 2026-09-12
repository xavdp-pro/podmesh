# Collector completion review

Status: source and two-host laboratory runtime qualified; package installation qualification in progress. Not production or full HA acceptance.

## Problem and resulting behavior

A collector must not treat uncertainty as proof of absence or replay a potentially completed external effect. This candidate preserves durable per-occurrence history, writes uncertainty before delegated effects, reserves runtime reclaim allowances before delegation, and independently verifies resulting state. Known pre-effect refusals remain refusals rather than being counted as applied effects.

Process and disk observations distinguish errors from measured absence or zero. Failed restore containment requires exact operation and attempt ownership. Reclaim is explicitly authorized and must bind its signal to the verified process identity. Normal operation has no autonomous garbage-collection timer.

## Qualification gates

The previous frozen candidate passed 29 Rust tests, 13 Python tests and an 80-check two-host API suite. Those counts are historical evidence for that binary, not acceptance of subsequent changes. A separate review identified an attempt-versus-delivery metric ambiguity and the numeric-PID signalling race. Both must be resolved and exercised before this candidate is accepted.

The current follow-up also requires a real daemon interruption after a committed collector effect and before final completion, followed by verified recovery without duplicate effect. Editing a progress row is useful simulation but does not establish this runtime property.

## Scope

Terminal reservation classes 1 and 2 and failed destination restore class 3 are in scope. Artifact retention with evidence holds and failed local restore collection remain separate capabilities. Networked workloads, arbitrary volumes, HA, and replicated-manager coordination are not qualified by this collector suite.

A local root API authorization reference records provenance; it is not a general multi-tenant authorization system or protection against an administrator bypassing PodMesh.

## Deployment discipline

Keep the previous package and persistent identity/journal backup before upgrading a test host. A rollback to older code can lose enforcement of newer safety gates even if the SQLite file remains readable; rollback is not permission to use older lifecycle operations against held universes. Test API outcomes and externally observed runtime effects, not only service startup.

The deterministic interruption barrier is a test facility only, disabled by default. Its enabling configuration must not appear in the shipped service unit.

## Evidence and final verdict

Final versioned daemon SHA-256: `7eca0a6cd0c7a17787f4f7ecfee7cb74b61f042f66adaa08cd25938073ba1851`.
CLI SHA-256: `10f3cecd7f55ad1c8e618ce0f466ef5b193bc1f35167a25eb04572e478bcc8a7`.

Local qualification: 35 Rust tests, 16 Python safety tests, formatting and strict clippy passed. Independent static review accepted the pidfd path and deterministic interruption driver after correcting misleading barrier-error metadata.

Runtime run `1dc2993b-2529-4ad1-93cc-c7c70c49e173` exited 0 with 81 checks passed. The daemon was actually SIGKILLed after the class-2 domain effect and verification-pending row committed; external SQLite reads established this state before the kill. Systemd restarted a new PID running the same binary. Retry recovered one effect, with one history occurrence and tombstone; later replay repeated nothing.

Reclaim observed ten candidates, nine pidfd signal calls successfully delivered, one process already gone, no refusals and no remaining target processes. Memory continuity kept the same token and progressed from 5 before checkpoint to observed values 7 through 13 after restore. Independently compared pre-existing containers, images and volumes were unchanged on both hosts. Test observers finished and the source-only barrier configuration was removed after the run.

The built experimental6 package contains byte-identical executables. Package installation, publication and user acceptance are separate gates. This interruption test covers a committed class-2 state transition; it does not prove every crash position during delegated runtime reclaim, host power failure, or distributed HA.
