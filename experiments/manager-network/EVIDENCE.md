# Stage N durable authenticated exchange evidence

Date: 2026-09-13. Scope: `experiments/manager-network/` only. Source baseline:
`e3251f2` on `codex/web-console`, plus the uncommitted fingerprints below.
Runtime: local Linux x86_64 workspace `/tmp/podmesh-web`; Rust 1.98.0.

No dependency, lockfile, Stage D, resident production, package, host, service,
route, DNS, WireGuard, commit, or push change was made by this Stage N work. A
concurrent Stage R owner changed only
`experiments/manager-resident/tests/processes.rs` so its consumer assertion checks
the Stage N signed refusal contract. Stage N changed only that consumer's copied
HMAC domain separator from literal `\\0` text to the actual NUL byte required by
R1. The test-only file is visible in the shared working tree and is not otherwise
owned by Stage N.

## Qualified local behavior

The network laboratory now uses the Stage D durable exchange API rather than an
ordinary laboratory import. The locally executed evidence supports these bounded
properties:

- a frame records actual prefix-inclusive transferred bytes, announced body
  size after exactly four prefix bytes, and a digest only for a complete received
  body;
- an intended write retains its body digest on partial failure and has no
  post-frame flush state that could turn a complete write into unavailability;
- one absolute deadline spans prefix and body, with `Interrupted` retries under
  the original deadline;
- zero, one, three, four, total-minus-one, total, oversized-prefix, and trickled
  deadline boundaries are exercised directly;
- every outbound attempt appends request preparation before connect/write;
  classified partial/malformed replies append exactly one terminal event, while
  total reply loss deliberately leaves the prepared attempt incomplete;
- accepted replies bind source, destination, wire operation, nonce, exact sent
  request digest, inserted and history counts, destination receipt ID/checksum,
  replay flag, and MAC;
- authenticated refusals bind source, destination, operation, nonce, complete
  request digest, one allowed typed reason, and MAC;
- the MAC domain uses an actual NUL separator byte;
- wrong keys, unknown peers, malformed JSON, truncation, and oversized frames
  create only unsigned malformed diagnostics and no durable decision authority;
- valid inbound requests record observation, then commit facts, receipt, and import
  audit in one Store transaction before recording signed-reply preparation and
  exact write outcome; post-write failure rollback is code reasoning only in this
  increment, not an executed Stage N claim;
- identical logical retry keeps its wire operation and snapshot while using a
  fresh nonce and local attempt; it returns the original destination receipt with
  `replayed: true`;
- changed snapshot content under the same wire operation receives an
  authenticated `operation_id_reused` refusal and imports no additional fact;
- an invalid later fact is rejected before durable writes, with no partial import,
  before the signed refusal;
- source terminal-audit failure after a signed success returns local uncertainty;
- source preparation, destination Store prevalidation, and destination reply
  preparation failures cannot return authenticated success; and
- `DurableError::{Refused, Corrupt, Storage, InvalidAudit}` is matched by variant,
  while only invalid request, policy violation, and operation reuse can become a
  signed peer-request refusal;
- the one-shot listener checks its absolute admission deadline before every
  accept and stops at eight connections. A queued connection is not accepted
  after the cutoff; accepted exchanges retain their own bounded I/O deadlines;
  fast bad connections do not end admission, while one stalled connection can
  exhaust the deadline and intentionally prevent queued admission; and
- non-signable pre-decision durable failure records a zero-byte close when the
  store is writable. Stage D requires `transport_unavailable` on that close while
  the returned local error retains the actual cause. Post-decision
  missing-receipt, signing, encoding, or preparation failure remains incomplete
  because Stage D forbids a close without a matching prepared reply. The original
  error is never masked.

The HMAC tamper matrix changes every conveyed success field: request digest,
receipt ID, checksum, replay flag, source, destination, operation, nonce,
inserted count, and history count. It also changes every signed-refusal field and
every top-level request field, including the snapshot. Every changed value fails
verification. A separate regression captures a valid success, changes the
snapshot while reusing both operation and nonce, and proves that the old success
cannot authenticate the changed request. Correctness does not depend on nonce
uniqueness, although normal retries use a fresh nonce.

## Three-process proof

The integration test starts destination executables with port zero. Each process
prints the actual address only after it owns the listener; the source then uses
that retained listener directly. No bind-drop-rebind reservation exists.

The source process begins with one retained observation. The first r2 process
commits it with its receipt and audit in one successful Store transaction, then
deliberately loses its reply after the committed audit. This exercises the
successful commit path, not rollback after a partial transaction write.
The source classifies any zero-byte unavailable reply read, including EOF,
timeout, or reset, as uncertainty and retains only its prepared attempt, with no
fabricated terminal reply event. A new r2 process
receives the same wire operation and snapshot with a fresh nonce, returns the
original signed receipt with `replayed: true`, and adds no fact. A third
destination process then catches up from the source.

The test invokes the external
`podmesh-manager-ha-lab --inspect-store DATABASE CONFIG REPLICA` process for all
three canonical stores. The observed results are:

| Replica | History | Audit events | Incomplete attempts | Unaudited authenticated imports | Integrity |
| --- | ---: | ---: | ---: | ---: | --- |
| r1 source | 1 | 5 | 1, ending at `outbound_request_prepared` | 0 | `ok` |
| r2 lost reply plus replay | 1 | 6 | 1, ending at `inbound_import_committed` | 0 | `ok` |
| r3 catch-up | 1 | 4 | 0 | 0 | `ok` |

All three inspections retain `survives-reply-loss` and return the same
manager-bound `logical_history_sha256`:
`96fd39c0393c8c621145688a91c9aa267cddadd7d51417eff0a87e435d96667c`.
The r1 store has two terminal outbound attempts and one intentionally incomplete
lost-reply attempt. The r2 store has a replayed terminal signed write and one
intentionally incomplete first attempt. The test reads no SQLite table directly.

## Executed verification

| Command | Result |
| --- | --- |
| `cargo test --locked --manifest-path experiments/manager-network/Cargo.toml` | Exit 0; 28 library tests and one three-process test passed |
| `cargo clippy --locked --manifest-path experiments/manager-network/Cargo.toml --all-targets --all-features -- -D warnings` | Exit 0; no warning |
| `cargo test --locked --manifest-path experiments/manager-ha/Cargo.toml` | Exit 0; 10 model and 69 process tests passed; one crash helper ignored and invoked by passing parent tests |
| `cargo clippy --locked --manifest-path experiments/manager-ha/Cargo.toml --all-targets --all-features -- -D warnings` | Exit 0; no warning |
| `cargo test --locked --manifest-path experiments/manager-resident/Cargo.toml` | Exit 0; 13 process tests passed, including the updated signed-refusal consumer assertion |
| `cargo clippy --locked --manifest-path experiments/manager-resident/Cargo.toml --all-targets --all-features -- -D warnings` | Exit 0; no warning |
| `cargo fmt --all --manifest-path experiments/manager-ha/Cargo.toml -- --check` | Exit 0 |
| `cargo fmt --all --manifest-path experiments/manager-network/Cargo.toml -- --check` | Exit 0 |
| `cargo fmt --all --manifest-path experiments/manager-resident/Cargo.toml -- --check` | Exit 0 |
| `git diff --check -- experiments/manager-network experiments/manager-resident/tests/processes.rs` | Exit 0 |

The Stage R owner updated its narrow consumer assertion to verify the signed
source, destination, operation, nonce, request digest, refusal reason, and MAC.
Stage N updated its copied domain separator to an actual NUL. The final sequential
rerun passed all 13 resident process tests.

## Review status

| Independent review finding | Disposition |
| --- | --- |
| IMPORTANT: success not tied to request | Fixed: success signs and verifies the exact sent body digest; captured-success substitution is rejected even with reused operation and nonce |
| IMPORTANT: source refusal reason lost | Fixed: `policy_violation` and `operation_id_reused` reach exact source terminal audit assertions |
| IMPORTANT: total reply loss marked malformed | Fixed: a zero-byte unavailable read is uncertainty and leaves only the prepared source attempt; partial and malformed replies retain terminal evidence |
| LOW: literal `\\0` MAC domain | Fixed to an actual NUL byte in network code and the resident consumer test |
| LOW: contradictory/stale comments | Fixed for frame sizing and `sync_to` error authority |
| LOW: unit-test port races | Fixed with owned listeners and readiness handoff; no bind-drop-rebind or readiness sleep remains |
| LOW: wrapper-only rollback injection | Corrected: the hook invokes the real Store, which rejects the conflicting audit ID during prevalidation before any transaction write; the similarly named current Stage D collision test proves the same pre-write behavior |
| LOW: production CLI fault seam | CLI use is bounded by the explicit `PODMESH_MANAGER_NETWORK_LAB_ENABLE_REPLY_LOSS=1` gate; the public method is accepted as a laboratory-only API needed by the normal-build process proof, and no installed package exposes this crate |
| LOW: missing close audits | Fixed where Stage D permits a pre-decision close; post-decision failures remain explicitly incomplete, and unwritable storage cannot be closed |
| LOW: resident refusal label | Kept as an explicit Stage R gate; no resident production source changed |
| LOW: first bad connection consumes listener | Bounded: fast bad connections do not end service, while one stalled connection can exhaust the deadline and intentionally prevent later queued admission; the cap remains eight and accepted exchanges have separate I/O budgets |
| LOW: wrong-key category mismatch | Fixed: wire diagnostic and durable audit both use malformed without authenticated authority |
| R2 IMPORTANT: rollback claim exceeds test | Corrected to Store prevalidation and fail-closed no-import for both Stage N and the current Stage D collision regression; post-write failure injection remains a Stage D-owner gate |
| R2 IMPORTANT: admission deadline misses queued accept | Fixed by checking time before every accept; a slow first exchange with a valid queued second connection proves the second is never admitted after cutoff |
| R2 LOW: zero-byte wording only names EOF | Corrected to every zero-byte unavailable read, including EOF, timeout, or reset |
| R2 LOW: early close loses durable cause | Documented: Stage D requires `transport_unavailable` in the terminal grammar; the returned local error retains its real cause |
| R2 LOW: public library fault method | Accepted and documented as laboratory-only; the CLI remains explicitly gated and the crate has no installed exposure |
| R2 LOW: captured-success test lacks source audit | Added an end-to-end `sync_to` regression requiring one malformed terminal and no remote receipt |
| R2 LOW: invalid fact described as rollback | Corrected to rejection before write with no partial import |
| R2 LOW: resident port helper | Retained as an explicit Stage R-owned test-harness gate; Stage N did not edit it |
| R3 IMPORTANT: Stage D rollback citation still overclaimed | Corrected: the Stage D test name suggests rollback, but its collision is rejected before transaction writes; rollback after writes is code reasoning from uncommitted transaction semantics, not executed evidence |
| R3 LOW: Linux accepted-socket behavior | Documented as a Linux-only qualification; BSD-family inherited nonblocking behavior remains unqualified |

Codex performed the three required closeout views. Governance remains bounded:
diagnostics carry no authority, receipts prove only a local commit, and no effect
permit was added. Human/operator behavior remains explicit through typed local,
authenticated-refusal, and unsigned-diagnostic outcomes. Runtime review covered
partial transfers, timeouts, interruption, reply loss, replay, changed-content
reuse, persistence failures, audit ordering, external inspection, and port
reservation concurrency.

An early independent finding identified an ambiguous flush failure after a
complete frame. The implementation removed transport flush and added a writer
that panics if framing invokes it. Claude R1 then found three important issues:
success lacked exact request binding, source refusal audits lost their typed
reason, and total reply loss was falsely closed as malformed. All three are fixed
with direct regressions. Its relevant low findings are also resolved: actual-NUL
domain separation, corrected comments, race-free listener ownership, real Store
prevalidation coverage, an
explicit CLI fault gate, honest close/incomplete semantics, aligned wrong-key
diagnostics, and bounded unauthenticated admission. Production
resident refusal labelling remains a Stage R gate and was not changed here. The
R2 corrected the original rollback claim and found the missing deadline check
before queued accepts. R3 then identified that the replacement Stage D citation
still overclaimed: despite its name,
`authenticated_import_rolls_back_facts_and_receipt_when_audit_refuses` also
rejects its audit collision during prevalidation, before transaction writes.
Both current collision tests prove fail-closed prevalidation and no write. The
Store code uses an uncommitted SQLite transaction on later errors, which supports
rollback by code reasoning, but no executed regression injects a failure after a
fact or receipt write. That post-write regression remains an open Stage D-owner
gate. R3 also bounded the listener claim and recorded the Linux accepted-socket
assumption. The resident harness bind-and-drop port helper remains a Stage R gate.
Claude Code's revision-4 confirmation found no blocker or important defect after
these wording fixes. It also identified and this evidence now closes the last low
wording ambiguity between successful one-transaction commits and unexecuted
post-write rollback. Closeout verdict: COHERENT for the bounded manager-network
laboratory claims and its executed Stage D and resident regressions.

## Strongest supported claim

On local Linux x86_64, Stage N is coded and locally tested as a bounded durable
authenticated TCP exchange laboratory. It proves receipt-bearing success,
authenticated typed refusal, authority-free diagnostics, exact frame evidence,
durable attempt sequencing, lost-reply replay, and externally inspected
three-process convergence against Stage D.

This evidence does not claim Stage R, complete G2, packaging, installation on
hosts, encrypted transport, production key lifecycle, dynamic membership,
incremental replication, quota/retention, takeover, fencing, split-brain
prevention, DNS, Podman effects, or manager high availability. It is not deployed
or user-accepted.

## Source fingerprints

| Path relative to repository root | SHA-256 |
| --- | --- |
| `experiments/manager-network/Cargo.toml` | `43fd367029d99395cfa68f0b3d4c1e5aa199adfeae51da1abf30473bc51aaa1e` |
| `experiments/manager-network/Cargo.lock` | `93b37a21b6617d17e5d3d1d11126e343d2bb9be2f4f3e492b673d8835e1c89bc` |
| `experiments/manager-network/src/lib.rs` | `684fef32e0614b0f32127e3a0a7f7e87d5a5a57512df2dd77791f549a3aaf4e5` |
| `experiments/manager-network/src/main.rs` | `24848247e274afc7911b0f340bcc16354b68caa66a3cbaa7be93385bc5b4fc56` |
| `experiments/manager-network/tests/three_processes.rs` | `1e653297645db12c3e168cbf725b81fdb7e7ff8450070778f16c1357d29e76b6` |
| `experiments/manager-network/README.md` | `801c6cc2311594f3e20c64d87c71806cbcf448ae6766221824d130b22bdc15e3` |

The concurrent Stage R consumer test fingerprint is
`12bd94bac25d12988b5c3c2a1c1b59c8927b4d4cf4baa20e795615e672bd9d4b` for
`experiments/manager-resident/tests/processes.rs`.
