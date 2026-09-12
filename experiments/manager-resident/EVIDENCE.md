# Resident replication evidence

## Package-candidate CLI increment

At commit `13e7051f687775125fcef10290de6a148c2643f5`, thirteen process
integration tests pass, including strict CLI errors,
installed-facing `--version`, explicit network opt-in, exact declared directories,
symlink/hardlink/traversal refusals, group-readable state layout and offline
validation. An already bound configured TCP address does not prevent offline
validation; no database, resident lock or socket appears. Existing non-SQLite
bytes remain unchanged because validation does not open the durable store.
Legacy runtime tests explicitly set `authenticated-static-peers`.

A separate disposable local proof ran the binary as effective UID 1000 against
a root-owned `0640` config with group 1000, state `0750`, runtime `0700`, mode
disabled and the configured TCP address already held. Exit was 0, response was
`configuration_valid=true`, `durable_store_checked=false`, `network_started=false`;
both state/runtime directories remained empty. Only that temporary config's
ownership was changed with sudo; no service or package configuration was changed.

Transport static validation was extracted as pure `ConfigurationFile::validate`;
its focused regression confirms no store/listener opening. Claude Code Opus high
found that the integrated laboratory still invoked the legacy positional form
without the now-required explicit network mode. That caller was corrected and the
integrated suite rerun. Independent closure of the remaining CLI boundary is
complete: the source review found no remaining blocker or important issue in the
CLI, path boundary, offline validation, network refusal or legacy compatibility.
It identified deployment-procedure defects outside this source boundary; the
installation qualification review tracks their correction separately.

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

Eight integration tests passed at that increment. Tests inspect durable state through independently
opened Store instances and the typed status interface, beyond process existence.
Transport tests additionally exercise the accepted-connection seam and absolute
frame deadline.

The interrupted-syscall retry branches were inspected and surrounding I/O gates
rerun; no deterministic signal/EINTR injection is claimed. After replacing four
release-and-rebind ephemeral-port test patterns with retained listeners, Codex
reran the complete resident suite, strict Clippy and formatting at the commit
above. The recorded run completed with 13 passed and no failure.

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
