# PodMesh — checklists toward the ideal scene

Status: checklists, 2026-09-17. Derive from [IDEAL-SCENE.md](IDEAL-SCENE.md) and
[IDEAL-SCENE-PROGRAM.md](IDEAL-SCENE-PROGRAM.md). These are certification criteria: an item is met only with evidence (a suite, a campaign record, or an observation a
second person can repeat), and an item that stops being true is no longer met. The maintainers keep the current status of
each item with the laboratory's evidence.

An item marked `[–]` has been **withdrawn by a recorded operator decision**, with its date and reason. It is neither met
nor owed. It is struck through rather than deleted, so that what left the scope, and when, stays readable.

## A. The core invariants (checked in every campaign)

- [ ] A1 A single-writer universe never ran in two places with two histories.
- [ ] A2 No running universe was stopped for lack of news.
- [ ] A3 Every exclusive decision carried a majority epoch or a recorded operator decision, with provenance.
- [ ] A4 Every interrupted operation was finished or undone without repeating an effect.
- [ ] A5 Nothing indispensable ran on a workstation during the campaign (the workstation was switched off).
- [ ] A6 The guarantee statement printed before the campaign matched what the campaign observed.

## B. A node on its own

- [ ] B1 The service starts at boot (packaged unit).
- [ ] B2 After a reboot, every universe the journal says should run is running again, by class, without a peer.
- [ ] B3 After a service restart, universes that ran are untouched and the journal reconciles.
- [ ] B4 A cut node keeps its universes running (no fence armed).
- [ ] B5 A node that learns it was superseded or declared lost stops what it no longer holds before anything else.
- [ ] B6 Disconnection policy applied per class, verified for `stateless`, `stay`, `failover`, `mergeable`.

## C. Declaring a universe

- [ ] C1 The class is declared and shown (`stateless`, `stay`, `failover`, `mergeable`).
- [ ] C2 The loss target (copy interval), copy count, placement and priority are declared.
- [ ] C3 The image source is declared and available on every host that may run the universe (Rule 11 respected).
- [ ] C4 Volumes, databases, secrets and network identity are listed with their capture form.
- [ ] C5 The coverage view shows the universe covered within its target.
- [ ] C6 A real restore of the universe has succeeded in the current period.

## D. Disconnection and reconnection

- [ ] D1 Network cut of the active host, VM alive: no second instance while the cut host could still run.
- [ ] D2 Service down on the active host, universe running: no failover, incident recorded once.
- [ ] D3 Power off of the active host: failover within the declared rules, memory of the copy kept.
- [ ] D4 The same three cuts pass with the workstation switched off (authority in the manager).
- [ ] D5 A cut isolating one host from a majority: the majority decides only what the class allows; the minority
  decides nothing exclusive.
- [ ] D6 A total cut (every host alone): no automatic exclusive decision anywhere; every universe keeps running.
- [ ] D7 Flapping link and VM pauses: no decision inside the stability window; measured false decisions: zero.
- [ ] D8 Healing: facts exchanged; exclusive conflicts recorded; the side that lost the epoch stops what it no longer
  holds; a single-writer history kept by policy, the other preserved.
- [ ] D9 The public entry point was never live in two places (C1 answered and enforced).

## E. Losing a host for good

- [ ] E1 An agent declares the host lost under a mandate; the decision is recorded with provenance.
- [ ] E2 The host's identity and every epoch it held are superseded.
- [ ] E3 A restoration plan is shown before acting: per universe, source copy or backup, target host, image source,
  capacity, data lost.
- [ ] E4 The plan executes spread across the remaining hosts by capacity, priority and anti-affinity; each step
  resumable.
- [ ] E5 Every restored universe is verified running; the guarantee statement is recomputed.
- [ ] E6 The lost host returning starts nothing exclusive, publishes nothing, keeps its stale data aside.
- [ ] E7 The host rebuilt (disk wiped) is readmitted as a new host and starts carrying its share.
- [ ] E8 One universe restored from a lost host onto a standby with memory.

## F. Data protection

- [ ] F1 Live copies with memory, staged with a restorability preflight, promoted running.
- [ ] F2 Planned switchover with no data lost.
- [ ] F3 Copies of every protected universe by policy, without per-universe manual setup.
- [ ] F4 Image availability policy chosen (C3 of the program) and enforced.
- [ ] F5 Volumes and databases captured application-consistently.
- [ ] F6 Backup Server outside the cluster: versioned, immutable, restore tested.
- [ ] F7 Off-site copy (Rule 16 level 5) with deletion lock.
- [–] F8 ~~Shared replicated storage (CephFS test bed): data not copied, storage-level fence verified.~~
  **Withdrawn 23 September 2026 by operator decision (no CephFS).** Not met, not pending: out of scope. `[–]` marks
  an item withdrawn with provenance, which is not the same as one still owed.

## G. Topology certification (one campaign each, A1 to A6 included)

- [ ] G1 **One host**: reboot, application crash, service crash; host destroyed then restored on a replacement host
  from the Backup Server; the guarantee statement says "no automatic takeover".
- [ ] G2 **Two hosts, no witness**: planned switchover both ways; a cut keeps both serving without a second instance;
  a host declared lost is restored on the other; failover-class universes behave as `stay`.
- [ ] G3 **Two hosts with a witness**: automatic failover decided by the witness side; the witness lost alone stops
  nothing; the witness and one host lost together: no automatic decision.
- [ ] G4 **Three hosts**: D1 to D9 and E1 to E7.
- [ ] G5 **Ten hosts enrolled at once**: enrollment, odd voter set across failure domains, rebalancing, one host lost,
  two hosts lost in different failure domains.
- [–] G6 ~~**Shared storage**: failover without data copy; storage-level fence; storage outage behaviour stated.~~
  **Withdrawn 23 September 2026 by operator decision (no CephFS).** No topology campaign certifies shared
  replicated storage; the guarantee statement says so rather than staying silent.

## H. Adding and removing hosts (the tandem's experience)

- [ ] H1 One gesture (agent or human) enrolls one host: identity, trust, manager role, images, copies.
- [ ] H2 One gesture enrolls ten hosts; no manual step per host.
- [ ] H3 The console and the agent show the enrollment's progress and the guarantee statement rising.
- [ ] H4 Rebalancing is gradual and never breaks a declared loss target.
- [ ] H5 Draining a host moves its universes by planned zero-loss switchovers.
- [ ] H6 Rolling upgrade of every host keeps every guarantee during the upgrade.

## I. Observability and authority

- [ ] I1 Health view: hosts, universes, manager links, continuity panel.
- [ ] I2 Coverage view: "if this host is lost now" per host.
- [ ] I3 Alerts on every broken invariant or policy (old copy, missing image, no capacity, no majority, entry point
  twice).
- [ ] I4 Mandates written for: declaring a host lost, changing a class, enrolling hosts, arming any automatic action.
- [ ] I5 Decisions signed and recorded.

## J. Before any campaign

- [ ] J1 Stale policies, units, copies and containers from earlier suites removed; the reset recorded.
- [ ] J2 What is armed on the hosts listed, with its mandate; nothing armed without one.
- [ ] J3 The dead man's switch armed and verified before any network cut.
- [ ] J4 The public entry point's state recorded before and after.
- [ ] J5 Evidence directory named and durable (never a temporary directory).
