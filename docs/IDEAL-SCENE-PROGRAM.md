# PodMesh — the program toward the ideal scene

Status: program, 2026-09-17. Derives from [IDEAL-SCENE.md](IDEAL-SCENE.md); each target is proven by an item of
[IDEAL-SCENE-CHECKLISTS.md](IDEAL-SCENE-CHECKLISTS.md).

The handling is written as a program of targets in the order the frame requires: the vital targets are kept in before
any production target, and conditional targets find out what must be known before committing. The comparison of the
ideal scene with the existing scene, and the maintainers' organizational targets, are kept with the laboratory's
evidence outside this repository.

## The targets

### Major target
Every protected universe on any PodMesh topology keeps serving through the failures its class covers, and comes back
within its declared loss when a host is lost for good, with the guarantees stated and proven.

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
- ~~**C4** Shared storage: on a CephFS test bed, whether client eviction/blocklisting gives a storage-level fence
  PodMesh can call, and how a universe's data directory is bound to it.~~ **Withdrawn by the operator on 23
  September 2026: no CephFS.** Nothing found out under it is assumed; reopening it means finding it out again for
  whatever mechanism replaces it.
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
   once; campaign per topology; the guarantee statement verified against each. Shared storage left the target with
   C4 on 23 September 2026; O6 closes without it.
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

## How to know the handling works
The statistics of [IDEAL-SCENE.md](IDEAL-SCENE.md) section 7 are recorded after each operating target. If the share of
covered universes and the share of losable hosts do not rise as O1 to O5 close, the evaluation behind this program was
wrong and is redone before continuing.
