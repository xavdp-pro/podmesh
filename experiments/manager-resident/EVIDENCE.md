# Resident replication evidence

Date: 2026-09-12. Scope: disposable loopback processes and SQLite stores.
Implementation: Codex GPT-6 Astra; inherited effort not independently exposed.
Claude Code Opus high counter-review found four important issues. All are corrected:
strict empty-struct control variants reject unknown fields; owned socket cleanup
precedes fallible joins; failed exchanges clear historical count differences; TCP
and Unix I/O retry EINTR without renewing the deadline. Claude's independent
closure review reported no remaining blocker or important finding for this
documented experimental scope.

| ID | Boundary | Test |
| --- | --- | --- |
| MR-01 | Periodic authenticated replication | Three simultaneous compiled children converge three independently created facts |
| MR-02 | Live partition/reconnect | Six TCP proxies isolate a still-running replica; two others append/exchange; reconnection catches up |
| MR-03 | Durable restart | SIGKILL, collect child exit, explicitly remove stale private socket, restart same store, verify history |
| MR-04 | Conflict visibility | Two owners claim one exclusive resource; all replicas retain both facts and report blocked resource; activation authority false |
| MR-05 | Authentication/atomicity | Wrong key yields no authenticated successes/imports; correctly signed valid-then-invalid batch reaches durable validator and commits zero facts |
| MR-06 | Admission/frame/shutdown | Thirty connections remain at two workers; excess counted; shutdown drains; oversize/trickle cannot mutate history |
| MR-07 | Config and duplicate instance | Invalid bounds/unknown fields refused; second resident with distinct free bind/socket but same DB lock cannot start |
| MR-08 | Error/control regressions | Extra fields on status/shutdown refused without process exit; store failure terminates and removes owned socket; partition yields null count delta after fresh local attempt |

Eight integration tests passed. Tests inspect durable state through independently
opened Store instances and the typed status interface, beyond process existence.
Transport tests additionally exercise the accepted-connection seam and absolute
frame deadline.

The interrupted-syscall retry branches were inspected and surrounding I/O gates
rerun; no deterministic signal/EINTR injection is claimed.

```sh
cargo test --locked --manifest-path experiments/manager-resident/Cargo.toml
cargo clippy --locked --all-targets --manifest-path experiments/manager-resident/Cargo.toml -- -D warnings
cargo fmt --manifest-path experiments/manager-resident/Cargo.toml -- --check
cargo test --locked --manifest-path experiments/manager-network/Cargo.toml
cargo clippy --locked --all-targets --manifest-path experiments/manager-network/Cargo.toml -- -D warnings
cargo fmt --manifest-path experiments/manager-network/Cargo.toml -- --check
```

No installed hosts, customer services, power failures, encrypted tunnels, DNS,
Podman activation or fencing were tested. Flood shutdown passes a 15-second test
deadline; that is not a guarantee under arbitrary storage/OS stalls. Exact causal
lag, retention, lost receipts, admission fairness and HA remain unproven.
