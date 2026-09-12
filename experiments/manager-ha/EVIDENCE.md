# Durable process API qualification

Date: 2026-09-12. Scope: `experiments/manager-ha/` only.
Source baseline: `1733a78` on `codex/web-console`, plus the uncommitted source
hashes below. Runtime: local Linux development workspace `/tmp/podmesh-web`.
Compiler: `rustc 1.98.0 (88d9e12ae 2026-08-18)`; Cargo 1.98.0.
No host deployment, package, commit, push or service change was performed.

## Executed acceptance

The authoritative capability inventory remains MH-01 through MH-15 in README.md.
All rows were revisited against the implemented process boundary and these tests.
Scope inputs read: INTENT.md, the control-services implementation brief, network
and placement decisions, Rust implementation plan, delivery checklist availability
section, and the registry experiment README/stress/next-network milestone. The
registry event format and storage implementation are external dependencies for a
future integration; this increment does not claim to integrate them.

| Command | Result |
| --- | --- |
| `cargo fmt --manifest-path experiments/manager-ha/Cargo.toml -- --check` | Exit 0 |
| `cargo clippy --locked --manifest-path experiments/manager-ha/Cargo.toml --all-targets -- -D warnings` | Exit 0, no warnings |
| `cargo test --locked --manifest-path experiments/manager-ha/Cargo.toml` | Exit 0; 10 existing model tests and 17 process acceptance tests passed |
| `git diff --check -- experiments/manager-ha` | Exit 0 |

The ignored `crash_worker` helper is deliberately invoked twice as a child by the
passing crash acceptance test. Each child reaches a bounded checkpoint and is
killed and reaped. It is not an unexecuted acceptance scenario.

Process tests run the actual built JSON executable with independent files and
stdio pipes. They check replies and exit codes, and inspect rows and integrity
through separate SQLite connections. Temporary fixture directories are removed
by the harness. The tests assert persisted identity, exact logical fact replay,
operation receipt counts, rollback, monotonic sequences, convergence, retained
conflicts and per-history eligibility. They do not infer completion from a health
endpoint. There is no real service effect to observe or Logger correlation.

The malformed-protocol scenario initially exposed ignored unknown fields on
Serde internally tagged unit variants. Replacing the two unit variants with
empty struct variants makes the existing deny-unknown-fields constraint effective;
the targeted regression and complete suite subsequently passed.

## Three review passes

1. Governance: replication remains observation exchange within predeclared static
   scope. Supplied topology never enrolls a new replica. Imported data and
   reconciliation results grant no runtime authority. No OS or governor contract
   was changed.
2. Human operation: README includes executable configuration/request examples,
   explicit process exit behavior, historical-retry semantics, feature evidence,
   and recovery limits. The interface reports eligibility in supplied history,
   not a running or active service. Human acceptance is pending.
3. Runtime/adversarial: actual child-process boundaries, simultaneous writers,
   crash windows, stale offline backups, wrong identity, corrupted event checksums,
   malformed/oversize requests, missing predecessors, forks and exclusive conflicts
   were exercised. No authentication, fencing, host loss or power-loss qualification
   is inferred. Local operation receipts lost in stale-backup restoration are not
   recovered from peer facts; recovery epochs and receipt policy remain required.

Constructing-agent self-review: coherent for the bounded local experiment.
Independent counter-review: the coordinating agent relayed two findings from
Claude Code Opus at high effort: imported revision overflow and unchecked durable
receipts. Both findings were independently reproduced in the process harness,
corrected and covered by the regressions below. Claude Code Opus at medium effort
then reran the 10 model and 17 process tests, strict Clippy, formatting and source
fingerprint checks. Its closure review reported no remaining blocker or important
finding for this documented experimental scope.
Overall distributed-manager / production-HA verdict remains OPEN.

## Independent review corrections

Both primary regression tests failed against the initial implementation and passed
after correction:

- `maximum_imported_revision_is_quarantined_without_panicking_after_restart` imports
  a cyclic predecessor fixture containing `u64::MAX`, inspects it through separate
  executable processes, verifies stable quarantine and blocks the restored owner's
  attempt to extend that history. Both local and reducer successor arithmetic now
  use `checked_add`.
- `corrupted_receipt_response_cannot_produce_a_false_replay` changes a persisted
  response after disabling a fixture trigger. The former implementation replayed
  the false response; the corrected store refuses replay, inspection and another
  write without adding facts.

Three further process tests verify the checksum binds operation ID and request
against fabricated rows, unambiguous framing with escaped strings and verified
retries, and refusal of the old schema version without rewriting its state.

Schema v2 adds a mandatory receipt checksum. All receipts are verified before each
request, in the same transaction used for state and replay. The domain-separated
JSON tuple binds operation ID, stored request JSON and stored response JSON. It
provides corruption detection only: an administrator who recomputes checksums is
not authenticated by this mechanism. Schema v1 is rejected; no automatic migration
invents integrity evidence for its old unchecked receipts.

Final commands listed above were rerun after these corrections: 27 acceptance
tests passed (10 model, 17 process), with the crash helper explicitly invoked by
its parent as before. No dependencies or lockfile changes were needed for this
correction. Source fingerprints below identify the corrected files.

## Source fingerprints

| Path relative to this experiment | SHA-256 |
| --- | --- |
| `Cargo.toml` | `6dd587e350de4ba196983c053502e5436e0160460b3d7e6011de7d8480bf05d3` |
| `Cargo.lock` | `68b37364176d8c038ce48913980e1d53c5d8377c43b8ed97381334ba55d6ee4e` |
| `src/lib.rs` | `45be5c39be8c95494b15cff946ced84a4c1dd3d20e4fc713262f9aa4c0802103` |
| `src/durable.rs` | `21fe536101de7365e38e635ec890f172c2c11b69f2865201848113cad8a1b5fc` |
| `src/main.rs` | `7f5c411138c9234550aa7be94c18dc355ad91dd1be6926f29a4b46cacfe31ab0` |
| `tests/laboratory.rs` | `11f974698e7ca18ba12531896fd36ccf7e521cc60380f9f6f3ef2082a97ed198` |
| `tests/process.rs` | `cb7a79c8882915a3f828d2a4664d910a9209cbe95a4163afb87a821f0c92c91c` |
