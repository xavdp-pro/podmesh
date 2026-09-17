# PodMesh — from the existing scene to the ideal scene

Status: program, 2026-09-17. Derives from [IDEAL-SCENE.md](IDEAL-SCENE.md); each target is proven by an item of
[IDEAL-SCENE-CHECKLISTS.md](IDEAL-SCENE-CHECKLISTS.md). Owner: Xavier de Poorter. Prepared with Claude (Anthropic).

The evaluation follows the order the frame prescribes: the existing scene, compared with the ideal scene; the
departures; the major departure (the situation); the reason that opens the way to handling it (the why); the
handling as a program of targets, in the order the frame requires: primary and vital targets kept in before any
production target.

## 1. The existing scene (facts as of 2026-09-17, with their evidence)

| Area | What exists | Evidence |
| --- | --- | --- |
| Contract | Typed operations with identities, machine-readable schemas published in `capabilities`, replay and interruption rules, CLI, console, agent tools | `LOCAL-API.md`, `check-capabilities-schema.py` |
| Node alone | Lifecycle, pause, resources, clone, ownership; the packaged service restarts at boot | `check-lifecycle`, `check-start-stop`, `packaging/podmesh.service` |
| Node alone | **Universes are not restarted after a host reboot**: `create` sets no restart policy; the laboratory's transient service does not come back | lab-c power-on, 2026-09-16 and 2026-09-17 |
| Copies | Stopped and live recovery points, staging with a restorability preflight, promotion running with memory, planned zero-loss switchover, crash paths proven | `check-live-replication-*`, `check-live-switchover`, `CURRENT-STATE.md` |
| Copies | Configured universe by universe from a workstation tool; nothing guarantees that every universe is covered | `tools/replicate-universe.py`, the demo universe's stale copies |
| Continuity | Automatic failover proven for one universe on three topology failures (network cut, service down, power off), then the fence disarmed because the guardian ran on the workstation | `CURRENT-STATE.md`, "Continuity of service, automatic" |
| Control plane | A manager replicated on three hosts, facts append-only, governor role, publisher following the governor | M-U2 suites |
| Control plane | **Replication links failing for hours; the lab-c replica does not boot** (`BOOT FACT NOT OBSERVED`); the epoch gate is a SQLite file on the workstation | Health view, `podman logs`, `tools/ha-standby.py` |
| Data classes | Live capture and migration refuse universes with volumes or mounts; stopped points cover only the writable layer; images must already be present (PodMesh never pulls); no image policy under Rule 11 | `migration.rs` blockers, `BACKUP-SERVER.md` |
| Backups | Backup Server designed (immutable chunks, off-site level 5), not built | `BACKUP-SERVER.md` |
| Capacity | Facts only (`host_status`, `universe_stats`); no reserve, no admission, no multi-host placement | `health.rs` |
| Host loss | Takeover of one universe to one standby; no declaration of a lost host, no plan for a whole host, no readmission | `tools/replicate-universe.py takeover` |
| Topologies | Laboratory of three VMs only; one-host, two-host, ten-host and shared-storage scenes never exercised | `CURRENT-STATE.md` |
| Hygiene | Laboratory journals hold stale policies from past suites (87, 50, 25), which once made lab-a's fence preview take 7.5 to 12 s | measured 2026-09-17 |

## 2. Departures from the ideal scene

1. Universes do not come back by themselves after a reboot (ideal 5.2).
2. Authority lives partly outside the hosts: the epoch gate and the guardian on a workstation (policy 5).
3. The manager that should carry authority is not healthy (ideal 5.4).
4. Protection is a per-universe configuration, not a property of the system; coverage is not measured (5.5, 5.9).
5. Images, volumes, databases and network identity are outside what a copy guarantees (5.5).
6. No off-cluster backup exists (5.5).
7. A lost host cannot be declared, planned and restored as a whole (5.6).
8. Topologies other than three hosts, and shared storage, are unproven; the guarantee statement does not exist (6).
9. Adding a host is a manual laboratory procedure, not one gesture (5.8).
10. Universe classes and their disconnection policies are not declared anywhere (5.3).

## 3. The situation

**PodMesh can prove continuity for one prepared universe in a laboratory driven from a workstation, but it cannot yet
promise, for all the universes of an arbitrary set of hosts, that a lost host's work comes back elsewhere within a
declared loss, nor that a cut host keeps serving without risking two histories.**

## 4. The why

The system was built operation by operation for a three-host laboratory with an operator at a workstation, before an
ideal scene fixed two things: **where authority lives** (on the hosts and in the majority of their manager, never on a
workstation) and **that protection is a property of the system** (declared per class, measured, and stated per
topology), not the result of configuring each universe by hand. Each lot was correct in itself; nothing forced them to
compose into guarantees for arbitrary topologies.

This why is a hypothesis to keep or replace: the handling below is built so that its first production targets would
show whether it opened the way (the coverage statistics rise without per-universe work).

## 5. The program

### Major target
Every protected universe on any PodMesh topology keeps serving through the failures its class covers, and comes back
within its declared loss when a host is lost for good, with the guarantees stated and proven.

### Primary targets (organization, roles, communication — kept in throughout)
- **P1** The ideal scene, the program and the checklists are the reference; each lot names the checklist items it
  closes; a lot that contradicts the ideal scene is stopped until the scale agrees again.
- **P2** Roles per lot: Xavier decides direction and mandates; one engineer implements; an independent adversarial
  review before building and a campaign after; Codex counter-reviews what touches the manager and packaging.
- **P3** One source of truth for status: `CURRENT-STATE.md` updated per lot with evidence; nothing claimed elsewhere.
- **P4** The laboratory is reset to a known state before each campaign (stale policies, units, copies) and the reset
  is recorded; nothing is armed on it without a written mandate.
- **P5** Communication with the tandem in the operator's language; repository documents in English.

### Vital targets (must be done to operate at all)
- **V1** A node restarts its universes from its own journal after a reboot or a service restart, without any peer.
- **V2** The manager is healthy: its replicas boot, exchange, and keep a majority when one host is lost (resident audit
  table, boot fact).
- **V3** Nothing indispensable on a workstation: the guardian's duties (renewal, schedules, transport, decisions) move
  into the manager; the workstation guardians and the laboratory gate are retired.
- **V4** Universe classes (`stateless`, `stay`, `failover`, `mergeable`) are declared per universe and enforced by the
  node and the manager.
- **V5** A coverage view: per host "if lost now: restored where, lost what"; per universe its copies, ages, image
  availability and guarantee.

### Conditional targets (find out before committing)
- **C1** Cloudflare: what happens to traffic with two live connectors of one tunnel, and how fast a stopped connector
  stops receiving it.
- **C2** Failure detection at three hosts: whether a stability window alone keeps false decisions away under the
  laboratory's real link flaps and VM pauses; measure them.
- **C3** Images under Rule 11: pre-placement on eligible hosts, a cluster-controlled registry pulled by digest, or
  rebuild from a lock — which one PodMesh adopts, and the decision recorded.
- **C4** Shared storage: on a CephFS test bed, whether client eviction/blocklisting gives a storage-level fence
  PodMesh can call, and how a universe's data directory is bound to it.
- **C5** Two hosts: the witness's form (a small service outside the cluster, possibly beside the Backup Server) and
  its failure rules.
- **C6** Volumes and databases: which capture forms are application-consistent for the first real workloads.

### Operating targets (direction and sequence; dates are the operator's)
1. **O1 — Autonomous node.** V1; restart policy per class; boot-time reconciliation from the journal; campaign:
   reboot a host with every class of universe on it.
2. **O2 — Healthy manager.** V2 with Codex; campaign: lose any one host, the manager keeps a majority and its facts.
3. **O3 — Coverage.** V5, C3; copies of every protected universe by policy, images available by the chosen method,
   restore tests scheduled; campaign: the coverage view says every host is losable.
4. **O4 — Authority in the manager.** V3, V4, C1, C2; epochs by majority, the guardian's duties in the manager,
   the entry point following the epoch; campaigns: network cut, service down, power off, heal — with the
   workstation switched off.
5. **O5 — Losing a host for good.** Declaration, plan, spread execution, protection against return, readmission;
   campaign: a host destroyed (disk wiped), everything restored elsewhere, the host rebuilt and readmitted.
6. **O6 — Topologies.** One host with a Backup Server, two hosts without and with a witness (C5), ten hosts enrolled at
   once, shared storage (C4); campaign per topology; the guarantee statement verified against each.
7. **O7 — Real data.** Volumes, databases, secrets, network identity (C6), the Backup Server and its off-site copy;
   campaign: restore a real application from the Backup Server after the loss of every host.
8. **O8 — Growth without friction.** One-gesture enrollment of one or many hosts, gradual rebalancing, rolling upgrades;
   campaign: add ten hosts at once, measure the manual steps (target: none).

### Production targets (quantities to reach and keep)
- 100 % of protected universes covered by their declared copies and image availability, measured continuously.
- Every protected universe restored for real at least once per period the operator sets; zero failed restores left
  unexplained.
- Every topology in section 6 of the ideal scene passes its campaign; every campaign re-run after any change to the
  core.
- Zero invariant violations in campaigns (two histories, a stop for lack of news, an unauthorized exclusive decision).
- Zero manual steps when adding a host.

## 6. How to know the handling works
The statistics of [IDEAL-SCENE.md](IDEAL-SCENE.md) section 7 are recorded after each operating target. If the share of
covered universes and the share of losable hosts do not rise as O1 to O5 close, the why above was wrong and the
evaluation is redone before continuing.
