# Next milestone: bounded observation exchange

Status: proposed implementation boundary, not deployed replication or HA.

## Smallest useful increment

Run a one-shot exchange between two explicitly configured laboratory peers. Export
original observation bytes, transport bounded batches, and call the existing
`Store::ingest` on the recipient. A third peer then catches up after being offline.
There is no resident daemon, discovery, leader election, IP allocation, DNS update,
standby activation, or garbage collection in this increment.

Use the existing `podmesh-registry-observation-lab/1` envelope unchanged. The
transport envelope is separate and must never reinterpret an observation as a
command or a grant. Preserve original bytes, digest, producer epoch and sequence.
Do not merge files, choose last-write-wins, or discard quarantined history.

## Configuration and authority prerequisites

- A local operator-owned configuration fixes each store path, mesh UUID, peer
  identity and transport endpoint. Incoming payloads cannot select filesystem
  paths, install peers or enroll writers.
- Every recipient has explicit local epoch/replica/resource-prefix enrollment.
  Export does not propagate that enrollment as authority. Missing enrollment is a
  refusal, not automatic trust or an implicit bootstrap action.
- For the first trusted-laboratory exchange, SSH with pinned host keys and a fixed
  remote command is sufficient as a transport proposal. Confirm the target account,
  fixed recipient executable and its writable store before deployment. No arbitrary
  command or path may be supplied by a peer.
- SSH authenticates the transporting peer, not the original event author. Relayed
  events retain only the existing laboratory trust model. Production acceptance
  needs a separately specified origin signature, key binding, rotation/revocation
  and replay policy. WireGuard alone does not supply those event signatures.
- A durable receipt means the recipient committed the bytes. It does not mean the
  reported resource is healthy, that a stream is admitted, or that takeover is safe.

## Protocol choices to freeze before implementation

A narrow v1 can use a sorted digest inventory and exact-byte batches because the
store already caps history at 4096 events. Avoid sequence cursors for now: forks
and missing predecessors mean a maximum sequence is not a sufficient receipt.
Specify independent batch byte/event caps, framing, timeouts and request IDs; use
base64 or length-framed bytes so JSON reserialization cannot change a digest.
Return per-event stored/refused results, with admission/quarantine reported
separately. A duplicate after a lost response must be safe. SQLite busy retries
must be bounded and explicitly owned by the exchange process.

No additional exclusive-authority schema is needed for observation-only exchange.
Grants, IP reservations, manager ownership, fencing and takeover remain separate
contracts and must not be invented in this lot.

## File and operational boundaries

Proposed implementation files are entirely inside `experiments/registry/`:
`src/bin/registry-exchange.rs`, a small transport module, integration tests and this
experiment's README. Do not touch `src/` of the installed PodMesh daemon, web,
Debian packaging, collector implementation or production manager configuration.
Host credentials and peer/store configuration remain outside the public repository.

Qualify with separate persisted stores, independent SQL/byte inspection, duplicate
batches, wrong mesh, unknown epoch, altered body, interrupted transport, lost
receipt, bounded quota refusal, restart, and third-peer catch-up. Preserve pending
and forked events on all peers and verify convergence without promoting any of
those events to runtime authority. Network tests must name their actual endpoints;
three files in one process remain local tests.
