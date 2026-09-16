# PodMesh — decision record and delivery inventory

Recorded: 2026-09-11. Owner: Xavier de Poorter.
Prepared in collaboration with OpenAI Codex — GPT-6 Astra.

This record consolidates the operator's conversation. It is design input, not
SHAPER canon, an implemented specification, or proof of high availability.
Dialogue is French; repository assets, documentation and comments are English.

## Purpose

Build an open-source Podman management layer for autonomous Linux hosts: physical
servers, VPSs or virtual machines. Priorities: simplicity, freedom to join and
leave, resilience, reproducibility and demonstrable reliability. Product name:
PodMesh. Name/trademark availability has not been assessed.

Public host preparation instructions must be independent of the laboratory's
hypervisor. A separate laboratory example may describe three VMs and the evidence
obtained there. No Proxmox integration or compatibility is a product requirement.

## Source coverage and status

Sources: operator decisions and observed tool results in this conversation;
NETWORK-AND-PLACEMENT.md; targeted existing architecture readings listed below.
The entire SHAPER corpus has NOT been audited in this sequence. No canon changed.

- DECIDED: operator direction, not necessarily implemented.
- PROPOSED: implementation candidate requiring evaluation.
- VERIFIED: bounded executed evidence, not universal capability.
- OPEN: requested work not implemented or not validated.
- DEFERRED: explicitly reserved for later.

## Feature inventory

| ID | Capability / decision | Status and acceptance criterion |
|---|---|---|
| P01 | Join a host already carrying workloads; detach/rejoin or change groups without emptying it | DECIDED / OPEN; preserve workloads and identities across joins — no join/detach operation exists (2026-09-15) |
| P02 | Stable host and universe UUIDs, human-readable names, local runtime IDs mapped separately | Host UUID and universe UUIDs stable, container names `podmesh-<uuid>`: coded, packaged, installed (experimental4→6); clone/name-collision detection OPEN |
| P03 | Inventory, create, start, stop, remove, logs, state and errors | Coded, lab-tested, packaged, installed: create/start/stop/delete/status/logs, clone, deletion ownership (`check-lifecycle`, `check-start-stop`, `check-clone*`, `check-delete-ownership`) |
| P04 | Whole-universe migration including nested Podman processes and memory | VERIFIED for disposable network-free lab workloads only, packaged since experimental5, installed (experimental6); general qualification OPEN |
| P05 | Preflight, checksum, explicit handoff, source stopped before destination active, durable operation report | Serial migration between two hosts coded, lab-tested, packaged (experimental5), installed; release/abandon/local restore (M3) in experimental6; concurrent controllers OPEN |
| P06 | Intent-driven human/agent management through common API/CLI; UI optional later | One local API and CLI for humans and agents: coded, packaged, installed; every later increment goes through it; UI OPEN |
| P07 | Rust core service, Debian packages, reproducible installation/update/removal | Rust core service `podmeshd` packaged and installed (0.1.0~experimental6 on three hosts, signed APT); install/upgrade/rollback rehearsed on one host (experimental3↔4), not on a clean host |
| P08 | Image availability across hosts using content digests; pre-stage before migration | DECIDED direction; OPEN — no digest-based image distribution; the manager image is built by hand on each host |
| P09 | Persistent data replication and coherent recovery points | Recovery points (capture, restore, promote, retention) coded and lab-tested on two hosts, not packaged; general volume/database replication OPEN |
| P10 | Periodic memory checkpoints copied to another host | Bounded checkpoint copy VERIFIED and packaged; continuous replication OPEN; the manager replicates its own store (M-U2) in the development tree |
| P11 | Logical /16, disjoint /24 allocation pool per host, stable workload IP | Coded and lab-tested on three hosts (`check-network-*`: managed bridge, /24 pool per host, stable address, exclusive routes, NAT exemption, effects ledger); not packaged; the prefix is the operator's at declaration |
| P12 | UUID-to-IP table, unique address per universe per network | The allocation table exists in the journal (`network_status`), unique address per universe per network: coded, lab-tested, not packaged |
| P13 | Existing IP connectivity or optional WireGuard, including bulk copies | Existing IP connectivity only (peer routes via the hosts' addresses); WireGuard OPEN |
| P14 | Shared universe naming/DNS | DEFERRED as DNS; the manager's public hostname is one CNAME to one tunnel (publisher, development tree, lab-tested with a real hostname) |
| P15 | SaaS governor and one maker per host, compatible with existing authority contracts | OPEN as integration; the epoch gate (activation leases, permits, fence) is coded and lab-tested in the development tree, not packaged |
| P16 | Same logical manager replicated onto every host; local bootstrap independent of failed manager/DNS | Manager universe replicated on three hosts with one governor, service address, takeover, partition, recovery: coded and lab-tested (M-U2), development tree, not packaged; the manager resident image is built on the hosts, not published |
| P17 | Continue bounded local management during partitions; merge on reconnect | Partition behaviour lab-tested for the manager (real cut, agent cut); generic universes OPEN |
| P18 | Rebuild a host quickly with preserved or deliberately replaced identity | OPEN; no rebuild rehearsal |
| P19 | Public APT repository and branded landing page | Signed APT at deb.xavdp.pro carries 0.1.0~experimental6 (Codex's package of 2026-09-12); the development tree since then is not packaged |
| P20 | Public product presentation, docs, sources and contribution path | OPEN; not provisioned |

## Delivery inventory by state (reconciled 2026-09-15)

The table above keeps the record's wording and its status column reconciled to 2026-09-15. The
table below separates the five states Codex asked for. *Coded* means in the `main` development
tree; *locally tested* means `cargo test` on the workstation; *lab-tested* names the suites under
`tests/` and the number of hosts; *packaged* means carried by a `.deb` on the signed APT
repository; *installed* means running as `podmesh.service` on the three laboratory hosts.
Nothing coded after 2026-09-12 is packaged or installed: the development tree runs on the hosts
only as an isolated transient service (`podmesh-dev-ha`), never as the installed package.
The next packaging target is **`0.1.0~experimental7`** — scope and gates in
[`EXPERIMENTAL7-SCOPE.md`](EXPERIMENTAL7-SCOPE.md) (frozen 2026-09-15).

| Increment | Coded | Locally tested | Lab-tested | Packaged | Installed |
|---|---|---|---|---|---|
| Lifecycle, clone, deletion ownership, journal, CLI | yes | yes | 1 host: `check-lifecycle`, `check-start-stop`, `check-clone`, `check-clone-interrupt`, `check-delete-ownership`, `check-installed` | experimental4→6 | experimental6, 3 hosts |
| Source checkpoint and serial migration (M1, M2), recovery of an interrupted transfer | yes | yes | 2 hosts: `check-migration-source`, `-destination`, `-interrupt`, `-destination-interrupt`, `-recovery`, `check-package-rehearsal` | experimental5, 6 | experimental6 |
| Release, abandonment, local restore, reclaim on proof (M3) | yes | yes | 2 hosts: `check-migration-cleanup` | experimental6 | experimental6 |
| Garbage collector on proof (M4, `main`) | yes | yes | 2 hosts: `check-migration-collector` | **no** — experimental6 carries Codex's own collector completion (`codex/collector-completion`, commits `4d7310b`, `ea2104c`), which `main` does not contain; the two lines diverged on 2026-09-12 and nobody has decided which collector ships | that other line |
| Activation leases, epochs, permits, fence, fence preview, self-fence timer (H1–H8, I1) | yes | yes | 2–3 hosts: `check-activation`, `-epoch`, `-fence`, `check-fence-timer`, `check-ha-three-hosts` | no (`packaging/podmesh-fence*` written, no package built since) | no |
| Recovery points, retention, collector class 5 (M5) | yes | yes | 2 hosts: `check-recovery-point*`, `check-recovery-point-retention` | no | no |
| The agent's side as a tool (`tools/ha-standby.py`, H9) | yes | — | 2–3 hosts: `check-ha-standby-tool`, every manager suite | no (a tool, not a package) | no |
| Manager universe on three hosts: replicas, governor, service address, partition, recovery (M-U2) | yes (`main` + the web tree's resident) | yes | 3 hosts: `check-manager-replicas-managed`, `-governor-managed`, `-service-address`, `-partition`, `-partition-agent-cut`, `-recovery-managed`, `check-manager-universe-ha` | no; the resident's image is built by hand on each host from the web tree | no |
| Universe network: managed bridge, pools, exclusive routes, alias, NAT exemption, effects ledger, crash safety (B1, B3, P2) | yes | yes | 2–3 hosts: `check-network-managed`, `-no-nat`, `-nat-matrix`, `-crash-safety`; each NAT rule mutated | no | no |
| Secrets outside images, state machine, crash safety (B2, P1) | yes | yes | 1–3 hosts: `check-secrets-image-free`, `check-secrets-crash` | no | no |
| The agent's door to the manager, bound to one incarnation (I3) | yes | yes | 1 host: `check-manager-control`, `-control-race` | no | no |
| The publishing connector follows the governor; takeover proof; transitions; registration wait (operator's decision, P0, P1) | yes | yes | 3 hosts with a real tunnel and hostname: `check-manager-publisher`, `-readiness`, `-crash`, `-agent-cut`; `manager.szde.fr` by Codex | no | no |
| The takeover document Ed25519-signed by the gate, verified by the host | yes | yes (`cargo test`, including a document signed by the tool's Python and verified in Rust) | 1 host: `check-takeover-proof-signature` (campaign of 2026-09-15, see CURRENT-STATE.md) | no | no |
| `pause`, `resume`, `resources` (memory and CPU, live) and their console actions | yes | yes | 1 host: `check-pause-resources.py`; the console path measured to lab-c | no | no |
| Moving a universe from the console (`tools/move-universe.py` over the migration chain; network-disabled universes) | yes | yes (gateway, browser) | 2 hosts: lab-c to lab-b, measured | no | no |
| Operation schemas in `capabilities` (58 operations) and `storage_status` with the growth rule | yes | yes | 1 host: `check-capabilities-schema.py`, `check-storage-status.py` | no | no |
| The console's generic operation engine over the schemas (`Run` view, `/api/hosts/:id/operations`) | yes (web tree) | yes (gateway 54, browser) | lab-c through the gateway | no | no |
| Health view: host and universe CPU, memory, disk (`host_status`, `universe_stats`), manager replication links | yes (daemon + web tree) | yes (`check-health.py`, gateway, model) | 3 hosts through the gateway | no | no |
| Replicating a universe to all or N standby hosts: now, on a schedule, stopped (`tools/replicate-universe.py`, console drawer) | yes (tool + web tree) | yes (gateway, fixture browser) | lab-c to lab-a and lab-b, from a real browser | no | no |
| The manager's administration app (Express API, React) | yes (web tree) | yes (`tests/app.test.mjs`, browser) | 3 replicas, driven from the public hostname | no (image built on the hosts) | no |

## Manager, governor and makers

Its complete brief — identities, registry records, storage and event format,
replication, network and DNS, partition behavior, reconnection, priority, bootstrap,
the decisions still required and the implementation and test order — is
[CONTROL-SERVICES-UNIVERSE.md](CONTROL-SERVICES-UNIVERSE.md), the operator's design
direction of 2026-09-12. This section keeps the earlier record.

The manager universe, more precisely the **control-services universe**, is a
Podman-hosted universe containing the registry, existing tools and, in the latest
proposal, DNS. The word manager is a deployment label here, never a governance role:
SHAPER's vocabulary already has the governor for declared desired state and the maker
for local materialization, and this universe is neither. It receives no authority by
being replicated. New text should prefer control-services universe and state that
mapping in the same paragraph; the older phrasing survives in this record.

It has one logical identity replicated across hosts. Logical identity does not make
independent running copies automatically consistent. Each replica requires a distinct
instance identity and hosting record.

The SaaS acts as governor. Existing definitions say it holds the desired-state
ledger, dates observations and does not initiate broad host administration.
One maker per host asks the governor what should exist, executes approved frozen
recipes and reports facts. PodMesh supplies technical placement, transport and
migration mechanisms; it must not introduce a competing desired-state authority.

References checked:
- SHAPER-OS-V1.14/docs/architecture/LEXICON.md (ledger, governor, maker)
- SHAPER-OS-V1.14/docs/architecture/UNIVERSE-PROFILES.md (+parent supervision)
- shaper-three-layers/40-TRANSVERSAL/10_HELM_GOVERNOR_MAKER_MAPPING.md

Fractal example: SaaS -> shop-manager -> shop. These are universe relationships,
not OS/Runtime/Workspace layers. Parent and children may run on different hosts
or sites. Registry placement and reachable endpoints let a parent supervise its
children without co-location, within existing authority limits.

## Latest partition model — preserve this distinction

The operator wants continued local operation without a global quorum dependency:
when communication splits, managers remain operational in their respective
territories; on reunion, histories synchronize and the lowest-priority-number
manager resumes overall coordination. Priority may follow creation order.
A monotonic creation sequence and a random UUID are not the same ordering;
the precise priority field and tie-break rule remain OPEN.

This supersedes the earlier suggestion of universally blocking management writes
without a Raft majority. Raft/rqlite was a candidate, not an adopted dependency.
The earlier single-active-manager description is now qualified: multiple copies
may operate during partitions only within non-overlapping delegated scopes.

Evaluation conditions, not yet proven:
1. Isolation must not expand authority. Makers enforce scoped commands locally.
2. Pre-allocated IP pools and resource ownership must not overlap.
3. Unreachable does not mean stopped. Do not activate a remote workload replica
   solely because its original host cannot be reached.
4. Migration transfers workload responsibility explicitly; origin /24 does not
   imply permanent ownership by its original host.
5. Reconnection exchanges versioned facts before changing coordination. Lowest
   ID/priority does not overwrite newer facts or prove an observation true.
6. Conflicting exclusive actions need rejection/resolution before execution;
   merging records cannot undo already emitted external effects.
7. Restoring an old manager checkpoint must not forget confirmed allocations,
   replay completed commands, or resurrect revoked authority.

Thus local autonomy appears feasible; automatic partition-safe global takeover
is NOT established. Priority alone does not prevent two isolated actors from
claiming the same resource. Quorum, fencing, pre-delegated rights and manual
recovery are design mechanisms to compare, not interchangeable guarantees.

## Network and registry

See NETWORK-AND-PLACEMENT.md for /16-/24 and optional WireGuard decisions.
An IP belongs to a universe within a logical network, not its current host.
Replica copies can carry that identity but must not concurrently announce the
same address onto a connected network. Host addresses remain distinct.
Migrated /32 routes and anti-spoofing/AllowedIPs updates are a proposed mechanism;
bridge behavior and traffic to other addresses in the origin /24 require tests.

Registry candidates: host/instance UUIDs, universe UUID, logical parent,
network UUID, assigned IP, current host, delegated scope, desired version,
observations with origin/time, operation IDs and transfer state. Store public
WireGuard peer keys and endpoint references as needed; private keys stay local
or in the authorized secret store, not a shared Git history.

SQLite per host plus event synchronization was initially proposed. GitDocumentDB,
Dolt and rqlite were researched, not selected. Git could store declarations and
history with a local query index; a clean merge does not guarantee uniqueness.
Small data volume does not settle concurrency and authority semantics.

DNS was initially discussed as distributed resolvers, then as a manager service.
Final serving topology, bootstrap endpoints, duplicate names, TTLs and behavior
while replicas diverge remain OPEN. DNS implementation remains deferred.

## Replication semantics and current evidence

Migration: source cooperates and final state is handed off.
Periodic checkpoint recovery: restore an earlier saved state after failure;
work since that checkpoint may be lost or repeated. Neither is automatic FT.

Public experimental kit:
https://github.com/xavdp-pro/vzcriu/tree/experiment/debian13-nested-podman

- Based on OpenVZ vzcriu; compatibility and restoration patches retained.
- Three automated nested migrations previously passed, with preserved memory
  identity, counter progress and inner Podman exec after reconciliation.
- Commit 9b3d60a adds replication.py and REPLICATION.md: three complete
  --leave-running checkpoints copied, checksum verified; then test container
  SIGKILL and latest checkpoint restored on another host.
- Replica experiment counter38 before failure,26 on initial restored output,
  then27; same UUID. Restore/reconciliation/proof6.575s. Not network outage time.
- Only latest checkpoint restored. No mounted volumes, database, external
  networking, full host power loss or network partition in that test.
- Private evidence: migration-lab-evidence/replication/replication-bfd1d29ac0db.
- Earlier simple-container timings (~0.8s; 256MiB pre-copy ~2.53s) do not establish
  nested manager failover latency and must not be advertised as such.

## Next tests explicitly requested

| Test | Required proof | Status |
|---|---|---|
| T01 Shut down one lab host | Other hosts continue; controlled manager recovery with no duplicate activity | NOT RUN |
| T02 Partition three hosts and keep local operations | Only delegated actions accepted, no overlapping allocations | NOT RUN |
| T03 Reconnect after divergent operations | No lost decisions; conflicts handled; coordinator selected deterministically | NOT RUN |
| T04 Join an occupied host / merge independent groups | Preserve identities and workloads; detect pool collisions | NOT RUN |
| T05 Move a networked universe | Same IP/name, correct routes, no duplicate advertisement; connection behavior measured | NOT RUN |
| T06 Repeat using WireGuard | Control and bulk transfer work; tunnel failure behavior measured | NOT RUN |
| T07 Restore stale manager | No reused IP, replayed external action or renewed stale authority | NOT RUN |
| T08 All managers absent | Bootstrap works without manager DNS; establish authorized state | NOT RUN |
| T09 Package install/update/remove and host recovery | Identity/config preservation and actual restore checked | Only APT validation package tested |

These are PodMesh tests, distinct from the nine earlier SHAPER behavioral tests,
which also remain unexecuted in the recorded conversation.

## Publication and product claims

Public: reusable code, documentation, patches and experiments, credited to Xavier
de Poorter with OpenAI Codex GPT-6 Astra and original upstream contributors.
Private: internal infrastructure configuration. Secrets excluded even from private Git.
The repository landing page matches the main site's cream/green/terracotta design.

Commercial objective: acquire customers and generate revenue through demonstrable
reliability. Describe user benefits with observed evidence and limits. Reliable
components do not prove a reliable assembly. Scalability, seamless HA and zero-loss
recovery are objectives, not established product properties.

## Review and outstanding work

Governance pass: preserve the operator's latest choices; do not turn proposals
into binding canon or let replicated state grant additional authority.
Human pass: public instructions describe generic hosts; lab setup is an example.
Runtime pass: partition conflicts, stale replicas, routing and bootstrap are explicit
open items; small successful experiments are not HA validation.
This consolidation has no independent counter-review yet. Overall implementation
status OPEN. The UUID/IP table, Rust service, manager HA and network layer are not
implemented by writing this record. Review this same inventory against delivery.

## Next experiment: a fictional SaaS fractal across three hosts

Operator request, 2026-09-11: make simple universes easy to install, then exercise
an imaginary SaaS governor creating and supervising real disposable child
universes distributed across the three existing laboratory hosts. No real client
or customer service is involved. "Three hosts" refers to the existing test hosts,
not a request to provision three additional machines.

The governor holds the desired-state ledger; each host's maker obtains its
approved work and invokes local PodMesh operations. Use minimal child workloads
with observable identity and activity. Do not substitute manual container creation
for proof that the governor-to-maker-to-PodMesh chain works.

The initial scenario should prove child creation, placement on different hosts,
parent-child discovery and observed state. Follow with controlled movement,
shutdown, partition and reconnection scenarios once their prerequisites exist.
Cross-host reachability, stale observations and failed operations must appear in
the results, not only successful health responses.

Produce an operational assessment after execution: desired versus observed
placement, operation completion and evidence, resource consumption, response and
recovery times, manual interventions, state/data loss or rollback, duplicate
activation/address checks, failure handling and unresolved limits. Distinguish
implemented, tested and operator-accepted capabilities. Use the assessment to
choose the next corrections rather than claim the architecture proven in advance.

Inventory additions:
- P21: fictional SaaS governor creates/supervises simple child universes across
  three hosts through makers and PodMesh. Requested; NOT IMPLEMENTED/NOT TESTED.
- P22: evidence-based operational assessment of the assembled fractal experiment.
  Requested; pending P21 execution and the applicable failure scenarios.

## Human-agent tandem product direction

Final operator clarification: PodMesh is a system for human-agent tandems.
The human provides intent, judgment and decisions; the agent performs authorized
operations and reports evidence. This supersedes the immediately preceding
"agent-first" wording. Human interfaces and agent tools share operation contracts.
See INTENT.md. P23: discoverable typed API/CLI, durable operation IDs, explicit
retry/recovery semantics, structured evidence and understandable human feedback.
Requested direction; implementation and end-to-end tandem proof remain OPEN.
Existing SHAPER intents must be reviewed for integration before edits; this
session creates the PodMesh design intent without silently rewriting the canon.

Delivery order clarification: implement for Xavier's human-agent tandem first;
then add the human interface for observation, control and actions using the same
API/CLI contracts. The GUI is not a prerequisite for the first operational proof.

Rust implementation tasks and acceptance criteria: see RUST-IMPLEMENTATION-PLAN.md.

P24 — dual deployment requirement: PodMesh runs standalone on Linux hosts for
users without SHAPER OS, and inside a SHAPER OS universe for our integration,
reusing existing logs and supervision. Both modes must be tested; neither is
implemented by this decision record. See INTENT.md and the Rust plan for access
boundaries and the required deployment matrix.

## Operator clarifications, 2026-09-11 evening

Recorded when Xavier handed implementation over to Claude Code to conserve GPT use.

- P25 — placement agent: an agent observes measured CPU, memory, disk I/O and network
  use per host and proposes rebalancing. Proposals become desired state only through
  the governor's ledger; makers execute them through PodMesh operations. Requested
  direction / OPEN; no metric collection or placement logic exists. See "Placement and
  rebalancing" below for the three automation levels and their prerequisites.
- P26 — no shared cluster filesystem and no global quorum dependency: persistence moves
  by explicit checkpoint, replication and backup (P09, P10, Backup Server). Losing a
  host loses that host, not the ability to rebuild its universes elsewhere. DECIDED
  direction / OPEN; host-loss recovery is untested (T01).
- Migration transfer authority and source exclusion: see
  [MIGRATION-PROTOCOL.md](MIGRATION-PROTOCOL.md), a provisional design taken under the
  standing mandate.

### Placement and rebalancing

The operator's comparison is automatic resource balancing in commercial hypervisors.
That product balances live-migrated virtual machines under a central cluster authority
with shared storage. PodMesh moves a workload by stopping, copying and restoring it: the
pause is real and its duration is not yet measured, mobility is proven for one workload
shape without networking or volumes, and there is deliberately no central authority and
no shared filesystem. Automatic rebalancing of running universes is therefore not the
first target; it is the last.

Who organizes placement does not change: an agent measures and proposes, the governor
writes the desired placement as ledger rows, each host's maker invokes PodMesh, and
PodMesh reports facts. The agent never moves a universe itself, and being able to read
every host's metrics grants it no authority over any of them.

Three levels, in the order they become defensible:

1. **Placement at birth.** Choose the host for a universe that does not exist yet, from
   measured free capacity and declared constraints. No move, no pause, no data risk; it
   captures most of the value and needs only the metric collection and a capacity record.
2. **Advisory rebalancing.** The agent proposes a move with its reason, its measured cost
   (archive size, expected pause, required free space on both hosts) and its rollback
   path. A human or an authorized maker accepts. Nothing moves without that acceptance.
3. **Bounded automatic rebalancing.** Only under a standing mandate that names what may
   move (never an exclusive production universe), between which hosts, how often, within
   which hours, and what stops it. It requires anti-flapping bounds in the sense of the
   convergence guard: hysteresis, a cooldown per universe, and a fleet-wide stop when too
   many moves fail.

Prerequisites shared by levels 2 and 3, none of which exist today: per-universe measured
resource use, a cost model of moving, network identity that survives a move, volume
handling, and a measured downtime figure. Until they do, an automatic mover would be
optimizing a placement while risking the workload it moves.

Shared supervision integration: [Shaper supervision](SHAPER-SUPERVISION.md) links
to the common architectural definition instead of maintaining a second loop here.
