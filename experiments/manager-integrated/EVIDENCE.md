# Integrated resident and epoch-gate evidence

Date: 2026-09-12. Scope: `experiments/manager-integrated/` only. No commit,
deployment, installed service change, DNS effect or Podman invocation.

## Executed run

The final composed test ran after rebuilding both participating Rust binaries:

- Source base: `ae5de42f0d31020076f74925af023a8c9d77fbfc`, with the current review
  corrections and integration work. `run.json` records dirty status; `sha256.json`
  identifies the exact binaries, source files, lockfiles, configs and closed stores.
- Run ID: `7b560d03-b0a5-4f7d-962e-91c6d63dba6c`.
- Retained raw directory: `/tmp/pm-integrated-f1eeb2c34f3b`, private mode `0700`.
  Raw files contain disposable local credentials and are not public artifacts.
- Platform: Linux `6.8.12-43-pve`; Python `3.11.2`.
- Command: `python3 -m unittest -v test_integration` from this directory.
- Result: 3 tests passed in 6.877 seconds, exit code 0: two adversarial gate-protocol
  regressions and the composed scenario with 24 Maker process attempts. This is not a
  stress campaign or a host-failure scenario.

This run supersedes the earlier evidence that predated the resident/network review
fixes. Relevant SHA-256 identities from its retained manifest:

| Participating artifact | SHA-256 |
| --- | --- |
| Resident source `src/lib.rs` | `fb82b57616c37ab04b543947fa20adaa07c038410aab13836d114189e95ac16e` |
| Network source `src/lib.rs` | `8d061497c50ff3b8b5e47e78d8a12cbde3ef76d61a2bfbccce4f2caea89ef025` |
| Rebuilt resident executable | `8d94d6c2ae872f88c46a55f6570eae222d10e34e62b5b81da258ad9c198011fc` |
| `integrated_lab.py` | `c330870e45c536a40d7b2c04a11f58c87674ef4059925826304384998a2a744b` |
| `test_integration.py` | `058a81d50bc37968e28e991cb5ecb446d04c559addb211bbf0c12f0603215d9c` |

The later package-candidate CLI made runtime network selection explicit. A
compatibility rerun after updating this harness to set
`PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers` passed the same three
tests in 6.282 seconds. Its run ID is
`4f34ef37-d6d1-4076-a315-bb6a197f20bd`, retained under
`/tmp/pm-integrated-c8ead6e68775`. The manifest records the current CLI source as
`44f82bb6d5eb0a89ebf33922d7126324d55ac3820cf94efc4e3e7e222798c5c9`,
resident source as
`4c34356c658a15e59375e7b6015727f1cb199eedfc8bc0d57560c7ed42aacb75`
and network source as
`071bef1d6c1f7086455034b56464239cdbd2c8e9baf43229c6e31d40df115afa`.
This rerun preserves the laboratory scope; it does not turn the package candidate
into installed-service or host evidence.

## Independently observed result

| Observation | Result |
| --- | --- |
| Resident processes | Three simultaneous compiled residents; one killed old replica and a fresh restarted process |
| Gate process | Independent process; clean restart preserves the current authority database |
| Exclusive effects | Seven unique persisted receipts and counter updates |
| Epoch 1 | One effect, replica r2 |
| Epoch 2 | Six effects, replica r0; no later epoch-1 effect |
| Counter sequence | Exactly 1 through 7 in persisted gate sequence order |
| Stale owner | Refused during partition, after reconnect, after gate restart and after old-resident restart |
| Receipt retry | Same current operation returns its result without another counter update |
| Wrong principal | r0's valid permit presented on r1's pinned gate channel is refused |
| Reply mismatch | Wrong operation, authority, epoch or resource produces an unknown outcome and disables the actor channel |
| Late reply after timeout | A valid late response is actually queued; a fresh spawned client cannot send another request or consume it |
| Maker crash | Injected exit 73 after gate commit and before result publication; retry yields one effect and one result fact |
| History after reconnect | Fourteen byte-equivalent immutable rows per replica before introducing conflicts |
| Retained conflict | Both competing claims remain; all copies expose the same conflict and block the affected resource |
| Final histories | Nineteen byte-equivalent immutable facts per replica, including seven operation-correlated result facts |
| Result oracle | Exactly one fact per effect; operation ID, epoch, result and authority ID exactly equal the independently read gate receipt and metadata |
| Final SQLite integrity | `ok` for all three resident stores and the external gate |
| Final schema version | Resident and gate stores each report their own schema version 2 |
| Local receipt counts | r0: 31, r1: 26, r2: 18; these local transport receipts are not expected to match |
| Shutdown | Final three resident processes and both clean gate stops report exit code 0; injected r2 kill separately retained |

The observer reads SQLite through fresh read-only connections, compares entire
event rows, recomputes SHA-256 values, inspects actual gate receipts and correlates
them with replicated result facts. It does not infer these outcomes from a health
endpoint or a successful adapter reply. The six TCP proxies close existing streams
when cutting the four links around r2; all residents remain running during the cut.

## Checks run for this increment

| Check | Result |
| --- | --- |
| HA and resident `cargo build --locked` | Passed |
| Integrated `python3 -m unittest -v test_integration` | 3 passed, final run above |
| Python byte compilation | Passed for both new Python files |
| Existing `test_fencing` suite | 22 passed; 600 seeded attempts, 331 accepted, 269 refused, 57 transfers, 158 stale attempts, 46 multi-effect epochs |
| Existing HA Rust suite | 10 reducer + 17 process tests passed; crash helper is intentionally invoked as a child |
| Existing network Rust suite | 9 library + 1 process test passed |
| Existing resident Rust suite | 8 process tests passed, including current control-validation and cleanup regressions |
| `cargo fmt -- --check`, HA/network/resident | Passed |
| `cargo clippy --locked --all-targets -- -D warnings`, HA/network/resident | Passed |
| Working-tree and new-file whitespace checks | Passed |

## Scope closure and review boundary

MI-01 through MI-10 from [README.md](README.md) are implemented and exercised in
the local fixture. They are not deployed or accepted as production HA. No claim
upgrades `MANAGER-HA-ACCEPTANCE.md` G2/G3 or marks complete its real-host scenarios.

The fixture preserves a single unique, intact external gate. It does not make that
authority highly available. Gate loss of reachability is an injected fixture
switch, whereas resident loss of reachability uses real loopback TCP relays.
Gate IPC pins trusted local fixture identities, not remotely authenticated Makers.
The local conflict status check and gate effect are not one transaction. Registry
replication cannot revoke an effect atomically or authorize a transfer. Recovery
after a newer epoch supersedes a committed but unpublished result remains open.

No real-host partition, power loss, DNS/bootstrap, route/IP ownership, Podman
fencing, receipt-backup recovery, key rotation, Logger integration, deployment,
bounded RPO/RTO or zero-loss guarantee is qualified. The existing gate's cloned or
rolled-back authority prohibition still applies.

A Claude Code Opus high counter-review identified missing IPC reply correlation
and evidence predating the current resident/network fixes. This revision corrects
both: unknown actor channels remain quarantined across Maker processes,
late-reply regressions pass, and every dependency was rebuilt or retested against
the sources recorded above. Claude's independent closure review reported no
remaining blocker or important finding for this documented laboratory scope.
