# Network transport laboratory evidence

## Resident integration seam increment

The resident milestone extracts `Node::serve_connection(TcpStream)` from the
one-shot listener without exposing raw handlers, keys or unbounded frames.
Focused tests prove authenticated receipt through an already accepted stream and
an absolute two-second frame deadline despite trickling bytes. The reader/writer
now share one deadline across the frame prefix and payload instead of resetting
the limit for every syscall. This increment was independently reviewed with the
resident service after correction.

The resident counter-review additionally restored interrupted-system-call retry
in the absolute read/write loops. A retry uses the original deadline. Existing
frame/seam tests were rerun; no deterministic signal injection is claimed.

Date: 2026-09-12. Scope: `experiments/manager-network/` only.
Source dependency: the unmodified local `experiments/manager-ha/` durable manager
laboratory, imported as a typed path dependency. No host service, Debian package,
PodMesh daemon, manager-ha source file, database outside disposable test paths,
or network configuration was changed.

Implementation model: OpenAI Codex GPT-5.6 Terra at high effort, selected by the
coordinating agent for this bounded implementation. This is not an independent
counter-review. The resident seam and EINTR corrections were implemented by
OpenAI Codex GPT-6 Astra at high effort.

## Executed qualification

| Command | Result |
| --- | --- |
| `cargo test --locked --manifest-path experiments/manager-network/Cargo.toml` | 10 tests passed: nine library transport cases and one three-process integration case |
| `cargo clippy --locked --all-targets --manifest-path experiments/manager-network/Cargo.toml -- -D warnings` | Passed |
| `cargo fmt --manifest-path experiments/manager-network/Cargo.toml -- --check` | Passed |

The process integration test launches two independent `serve-once` destination
replica executables and a third `sync` source executable. Before listeners start,
the source durable SQLite store records its observation while both other stores
remain offline and empty. Each destination then receives the source snapshot over
loopback TCP and is independently inspected through a fresh durable SQLite store.
Both retain exactly one imported fact.

The library tests use separate durable SQLite files and direct local TCP sockets
to inspect the following outcomes:

| Case | Observed result |
| --- | --- |
| Configured peer and replay | HMAC-authenticated exchange imports one fact; identical source operation ID/nonce replays the durable receipt without another fact |
| Partition/reconnect catch-up | A previously empty third replica receives the complete retained source snapshot |
| Unavailable endpoint | Connection failure is returned as `unavailable`, distinct from rejection |
| Wrong peer key | Recipient sends an unsigned `refused` diagnostic; `sync_to` exposes it only as `UnauthenticatedRemoteDiagnostic` and durable history remains zero |
| Duplicate configured pair key | Local configuration is refused before its durable SQLite store opens |
| Oversize local configuration | More than 1 MiB is refused before JSON parsing or durable SQLite store opening |
| Truncated or oversize frame | Recipient returns `malformed` before JSON parsing; durable history remains zero |
| Invalid signed snapshot | Snapshot with one valid fact followed by a structurally invalid fact returns `refused`; durable history remains zero |

The no-partial-import test is evidence for the existing durable manager transaction
as reached through the new typed mapping. It does not cover physical power loss,
all SQLite failure points or a real host filesystem.

## Review boundary

The test and evidence above establish only this isolated local TCP increment. They
do not prove authenticated deployment on a LAN or WireGuard, confidentiality,
availability, HA, replication convergence under unbounded history, or safe
exclusive activation. A separate Claude Code counter-review and Codex verification
found and corrected an inverted diagnostic address, untyped remote status, an
unbounded configuration read, an unignored build tree and one inaccurate bound.
Claude Code Opus at medium effort then reran the eight tests, strict Clippy and
formatting and reported no remaining blocker or important finding for this
documented experimental scope. Codex independently reran the same gates.

For the resident increment, Claude Code Opus at high effort found and prompted
corrections for strict control request fields, error-path socket cleanup, stale
count-delta reporting and interrupted-syscall retry. Its closure review reported
no remaining blocker or important finding. Codex reran all ten transport tests,
strict Clippy and formatting after those corrections.
