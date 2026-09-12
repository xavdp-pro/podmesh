# Resident control-services replication laboratory

Status: local executable laboratory, no deployment or HA claim. One logical
manager retains control facts in distinct replicas, without a shared live
filesystem. This increment runs periodic authenticated snapshot exchange using
the typed `manager-network` and durable SQLite `manager-ha` path dependencies.
It never calls Podman, runs commands, publishes DNS/IP, grants permits, performs
fencing or activation, or changes enrollment.

## Resident behavior

A persistent TCP listener admits a fixed number of workers. Each calls the
transport's `Node::serve_connection` for exactly one authenticated, bounded frame
and atomic import. Excess connections are closed and counted without an application
queue; the kernel has its own finite backlog. One outgoing worker visits the exact
configured peers sequentially, with independent bounded exponential backoff.
Successful exchange resets the peer interval. An unreachable peer does not become
a dead host or an activation decision. A slow peer delays others by its bounded
attempt; hostile admission fairness is not guaranteed.

Operation IDs and nonces use OS randomness. The same observed snapshot reuses an
operation ID, avoiding a new receipt for each unchanged poll. After restart, IDs
are fresh. The transport exports again inside its call: an incoming import between
the scheduler's snapshot digest and that export can cause a transient replay
binding refusal. A changed digest creates a fresh ID on the next attempt. The
transaction refuses mismatches rather than making an unsafe mutation.

SQLite retains the dependency's WAL/FULL, immutable history and receipt checks,
identity binding and atomic import. A held lock file prevents two residents on the
same configured database path. It does not fence copied databases, path aliases
or a privileged actor. No history/receipt compaction or disk quota exists yet.

## Configuration and bounds

The Cargo executable remains `podmesh-manager-resident-lab`; it is intended to be
installed as `podmesh-managerd`. `--version` prints the installed-facing identity
`podmesh-managerd 0.1.0`, independently of the filename. This is the crate version,
not an assertion about a Debian package version or installed deployment.

The package candidate accepts exactly:

```text
podmesh-managerd --config /etc/podmesh-manager/config.json --state-dir /var/lib/podmesh-manager --runtime-dir /run/podmesh-manager [--validate-config]
podmesh-managerd --version
```

All three path flags are mandatory in flag mode and can appear in any order.
Unknown, repeated, mixed positional, missing-value and missing-required flags
are refused. The one positional `CONFIG.json` laboratory form remains available;
its absolute state/runtime boundaries are derived from the configured DB/socket
parents and undergo the same checks. All runtime forms require the explicit
environment value `PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers`.
Missing mode or `disabled` refuses runtime networking before DB, lock or socket
creation. Unknown modes are refused. `--validate-config` works with disabled or
enabled mode but never binds, opens/creates a durable database, takes a lock or
creates a socket. It reads the config and filesystem metadata only; a successful
offline check does not validate an existing SQLite file's identity/integrity.

Paths must be absolute, at most 4096 bytes, without `.`/`..`, repeated/trailing
separators or any symlink in their checked ancestry. The DB and socket must be
direct children of declared state/runtime directories. State/runtime directories
must exist and belong to the effective user: state permits group read/traverse
(package mode `0750`), runtime permits no group/other access (`0700`). Their
ancestors must belong to root or the effective user and disallow other-user
writes, except root-owned sticky temporary directories. Config files must be
regular, singly linked, root- or effective-user-owned and not group/other writable;
root-owned group-readable `0640` config is supported. Existing DB/lock/WAL/SHM
files must be singly linked regular files owned by the effective user and not
group/other writable. Existing sockets must be private and owned by that user.
Validation follows no symlink and performs no automatic directory creation.

These metadata checks assume the effective account and root do not race path
replacement after validation; descriptor-relative race hardening against those
trusted actors is not implemented. Do not run this service under an account
shared with untrusted software. The package networking default stays disabled;
its systemd address-family/firewall policy needs separately reviewed changes
before opting into real networking.

Unknown JSON fields are refused.
The configuration has `network` (the complete transport `ConfigurationFile`),
`control_socket`, `interval_ms`, `max_backoff_ms`, and `incoming_workers`.
`network` fixes the replica identity/topology, local DB, TCP bind and all exact
peer endpoints with distinct pair keys. Requests cannot select paths, keys,
endpoints, commands or topology. Protect local config/DB; never commit secrets.

| Boundary | Limit |
| --- | --- |
| Local config | 1 MiB before parsing |
| Peers | At most 15, all other topology members exactly once |
| Incoming workers | Configured 1–8; tests use 2 |
| Outgoing workers | 1 |
| Control handlers | 1, serialized in main loop |
| Network request/reply | 512 KiB frame |
| Network read/write | Absolute 2-second deadline per frame, including trickled bytes |
| Connection retry | 2 seconds |
| Periodic interval | 100–60,000 ms |
| Maximum backoff | At least interval, at most 300,000 ms |
| Control request | 256 bytes, absolute 250 ms read deadline |
| Control response | 512 KiB, absolute 250 ms write deadline |
| Socket path | Absolute, at most 100 bytes, private parent directory |

SQLite uses a five-second busy timeout. These are not hard real-time guarantees:
serialization/verification scale with local history and storage/OS can stall.
Oversize snapshots fail to exchange; history is never truncated into a success.

## Private control and observation

The Unix socket is mode `0600` in a directory without group/other permissions.
Send one JSON object and close the write half. Only two requests exist:

```json
{"operation":"status"}
```

```json
{"operation":"shutdown"}
```

Status includes durable inspection/conflicts, peer counters, last authenticated
success age, next retry delay, acknowledged history count, local history count
observed before attempt, and `history_count_delta`. This is not exact causal lag
or convergence proof: equal counts can differ, and replayed receipts describe a
historical committed result. Unknown values remain null. Unsigned diagnostics
and failed exchanges clear `history_count_delta` to null; the previous acknowledged
count is retained only as historical data with its last-success age. A new local
count is never compared with an unreachable peer's old count as a current lag.
Unsigned diagnostics remain explicitly `unauthenticated_remote_diagnostic`; no authenticated authority
is inferred. `activation_authority` is always false. Peer status restarts unknown.

Shutdown stops scheduling/admission and removes the owned private socket before
fallible worker joins, then drains and joins workers. Store errors, status errors
and outgoing worker failure terminate the resident through this same cleanup path.
Every started worker is joined even when another worker fails. Abrupt death leaves a stale
socket: restart refuses it until the operator verifies process death and removes
that exact private path. Tests do so only after collecting the killed child's exit.
There is no automatic stale-file cleanup or lost-identity recovery procedure.

TCP and Unix reads/writes retry interrupted system calls against the original
absolute deadline; an interrupt never renews the deadline. No signal injection
test is claimed for this retry branch; frame deadline and process tests cover the
surrounding I/O paths.

## Validation and gaps

[EVIDENCE.md](EVIDENCE.md) covers three simultaneous compiled processes, live TCP
partition/reconnect, SIGKILL/restart, conflicts, wrong keys, authenticated invalid
batches, framing/admission limits and shutdown. The partition test uses six TCP
proxies to cut both directions around one still-running resident.

Remaining: key rotation/revocation, signed original provenance, encryption,
WireGuard/real-host qualification, bounded incremental history, retention/quota,
receipt/backup recovery, identity fencing across paths/hosts, Logger, integrated
control-services deployment and actual exclusive-effect fencing. There is no
automatic HA, quorum-free takeover, zero-loss or production availability claim.

```sh
cargo test --locked --manifest-path experiments/manager-resident/Cargo.toml
cargo clippy --locked --all-targets --manifest-path experiments/manager-resident/Cargo.toml -- -D warnings
cargo fmt --manifest-path experiments/manager-resident/Cargo.toml -- --check
```
