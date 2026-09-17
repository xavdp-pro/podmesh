# PodMesh — the ideal scene

Status: direction, 2026-09-17. Owner: Xavier de Poorter. Prepared with Claude (Anthropic).
Companions: [IDEAL-SCENE-PROGRAM.md](IDEAL-SCENE-PROGRAM.md) (the existing scene, the departures and the program
that closes them) and [IDEAL-SCENE-CHECKLISTS.md](IDEAL-SCENE-CHECKLISTS.md) (what proves each part).

## How this document is built

The frame is the administrative scale and the evaluation method of L. Ron Hubbard's management writings, used here
as a planning tool and nothing more. The scale orders the subjects of an activity: *goals, purposes, policy, plans,
programs, projects, orders, ideal scenes, statistics, valuable final products*, and every item must agree with every
other item on the same subject. An **ideal scene** "expresses what a scene or area ought to be"; it is something that
can be achieved, and without one "he will not be able to recognize departures from it". The evaluation compares the
existing scene with the ideal scene, names the major departure (the situation), finds the reason that opens the way
to handling it (the why), and writes the handling as a program of targets. A **statistic** is "a number or amount
compared to an earlier number or amount of the same thing". A **valuable final product** is what the activity
produces that others can exchange for.

Sources: [Administrative Scale](https://www.scientologycourses.org/tools-for-life/targets/steps/administrative-scale.html),
[Plans and Programs](https://www.scientologyhandbook.org/targets/sh17_3.htm), the Data Series titles "How to Find and
Establish an Ideal Scene", "The Situation" and "Handling—Policy, Plans, Programs, Projects and Orders Defined"
([index](https://vinaire.me/2022/07/10/data-series-scientology/)).

This document is the top of the scale for PodMesh. The program and the checklists derive from it; when they
disagree with it, one of them is wrong and is corrected until all agree.

## 1. Goal

**Human-agent tandems operate portable Podman universes that keep serving, on whatever hosts they have, and
never lose what they were told to protect.**

## 2. Purposes

1. Give a tandem (a human who decides, an agent who executes and reports) one contract to create, run, move,
   protect and restore universes, identical from a CLI, an agent tool, a console and the manager.
2. Keep every universe serving through the failures its declared class covers, on one host as on ten.
3. Make the loss of a host a recoverable event: declared, planned, executed and proven, with the data loss bounded
   by what the universe declared.
4. Adapt to the infrastructure found (one host, two, many; shared replicated storage or none; a witness or none)
   without changing the contract, and say plainly what the current infrastructure guarantees.
5. Make growth frictionless: adding a host is one gesture, and the guarantees rise by themselves.

## 3. Policy — the invariants nothing negotiates

1. **One history.** A universe declared single-writer never runs in two places with two histories.
2. **No stop for lack of news.** A running universe is never stopped because a peer, the manager or a workstation
   is silent. It stops only by a recorded decision, by a newer epoch it has verifiably learned, or by the local
   disconnection policy its own class declares.
3. **Decisions carry authority.** Every exclusive decision (where the active instance runs, which replica governs,
   which replica publishes, that a host is lost) carries an epoch issued by a majority or by the operator's recorded
   decision, the provenance (which agent, under which mandate) and a timestamp; every actuator checks the epoch
   before it acts.
4. **Resumable, never repeated.** Every operation has an identity; an interrupted operation is finished or undone
   from what the hosts show, never by repeating an effect.
5. **Nothing indispensable outside the hosts.** No lease, heartbeat, transport or decision lives on a workstation.
   A workstation observes and commands; the hosts and the manager they carry do the work.
6. **Evidence before claims.** No resilience property is claimed that a campaign has not proven on the topology it
   names. No public document promises a restore duration (Rule 10 of the SHAPER canon).
7. **Honest guarantees.** The system always states what the current topology and storage guarantee for each
   universe, and never more.

## 4. Valuable final products

1. **A universe that keeps serving** through the failures its class covers.
2. **A lost host's universes, running again** on the hosts that remain, within the loss each universe declared.
3. **An honest record**: every operation, decision and incident with its evidence, readable by a human and an agent.
4. **A guarantee statement** per universe and per cluster, true for the infrastructure as it is now.
5. **A new host that joins in one gesture** and starts carrying its share.

## 5. The ideal scene

### 5.1 The tandem's experience
- The human states intent ("protect this universe, lose at most a minute", "this host is gone for good", "add these
  ten servers"); the agent turns it into typed operations, shows the expected effects before acting, and reports the
  outcome with evidence and uncertainty.
- The console, the CLI, agent tools and the manager use the same operations, schemas and authority. Nothing is
  possible in one that is impossible or different in another.
- Every refusal says why and what would make the request acceptable.

### 5.2 A node on its own
- A node restarts its service at boot and restarts its universes from its own journal, without the manager, a peer
  or a workstation.
- A node cut from the others keeps serving everything it serves, for as long as the cut lasts.
- A node applies locally the disconnection policy of each universe (below) and nothing else.
- A node that is told, or learns from its peers, that it has been superseded or declared lost stops what it no
  longer holds before it does anything else.

### 5.3 Universe classes and their disconnection policy

Declared per universe, changeable without touching the core:

| Class | Meaning | When its host is cut | When its host is lost for good |
| --- | --- | --- | --- |
| **stateless** | no state worth keeping, duplicates harmless | keeps running; replicas elsewhere keep running | nothing to restore; run elsewhere |
| **stay** | single writer, never moved automatically | keeps running on the cut host; nobody starts it elsewhere | restored elsewhere when the host is declared lost |
| **failover** | single writer, automatic takeover | the cut host freezes or stops it within a bounded local delay; the majority starts it elsewhere only after twice that delay | restored elsewhere, automatically or by declaration |
| **mergeable** | state that merges (facts, CRDTs) | runs on both sides; merges when the cut heals | runs elsewhere; merges what arrives |

Failover-class automation exists only where the topology can decide safely (a majority, or a witness); elsewhere
the class behaves as *stay* and the guarantee statement says so.

### 5.4 The control plane
- The manager is replicated on the hosts. Its facts are append-only, written by their origin, and converge without
  coordination. Its exclusive decisions (epochs) need a majority of its voters, or a witness in a two-host cluster,
  or the operator's recorded decision.
- The manager renews leases, schedules copies, moves bytes from host to host, detects failures peer to peer with a
  stability window, asks peers before concluding that a host is gone, and suspects itself when every host looks
  unhealthy.
- The public entry point (the Cloudflare tunnel) runs only on the replica in charge and stops the moment that
  replica loses its epoch.
- In large clusters only an odd number of replicas vote (three or five, placed in different failure domains); the
  others follow.

### 5.5 Data: copies, backups, images
- **Copies for continuity**: every protected universe has at least the number of copies its policy declares on other
  hosts, newer than its declared loss, verified restorable, placed across failure domains.
- **Backups for disaster**: a Backup Server outside the cluster keeps versioned, immutable, verified recovery points,
  and an off-site copy (Rule 16 level 5); it survives what copies do not (site loss, corruption, deletion, a
  compromised cluster). Restores from it are tested regularly, for real.
- **Images**: never inside a backup (Rule 11); always obtainable where a universe must run: pre-placed on the hosts
  that may run it, or pulled by digest from a source the cluster controls, or rebuilt from a recorded lock.
- **Volumes, databases, secrets and network identity** are covered by the same guarantee as the container: an
  application-consistent recovery point for the data, a dump for a database, secrets restorable from the operator's
  store by name, addresses and routes transferable.
- **Shared replicated storage when present** (CephFS or equivalent): the data layer replicates the data; PodMesh
  moves only the container and its memory, and uses the storage's own fencing (client eviction or blocklisting)
  as the strongest exclusivity mechanism. When absent, copies and the Backup Server do the same job.

### 5.6 Losing a host for good
1. An agent, under a mandate, declares the host lost; the decision is recorded by the majority (or by the operator
   where no majority exists) with its provenance.
2. The host's identity and every epoch it held are superseded at once.
3. The manager computes a restoration plan before acting: for every universe the host carried, the newest verified
   copy or backup, the target host, the image source, the capacity used, and the data that will be lost.
4. The tandem sees the plan; within the mandate the agent executes it, spreading universes across the hosts that
   remain by capacity, priority and anti-affinity; each step is resumable.
5. Each restored universe is verified running, and the guarantee statement is recomputed and shown.
6. If the lost host ever answers again, it starts nothing exclusive, publishes nothing, keeps its stale data aside for
   inspection, and waits to be readmitted as a new host.

### 5.7 Disconnection and reconnection
- Cut: nothing stops for lack of news; each class follows its policy; the manager's majority side decides only what
  the class and the topology allow; the minority side decides nothing exclusive.
- Healing: facts are exchanged until each side knows what it lacks; conflicts on exclusive decisions are recorded, not
  silently resolved; the side that lost an epoch stops what it no longer holds; a single-writer universe that ran on
  both sides keeps the history its policy names and preserves the other for inspection.

### 5.8 Growth, shrinkage and change
- **Adding hosts** is one gesture from the tandem (one host or ten at once): each host is enrolled with a token the
  manager issued, authenticates, receives its identity, its manager role (voter or follower), the images and copies
  its share requires, and starts carrying universes by gradual rebalancing. The guarantee statement rises when the
  copies are in place, and says so.
- **Removing a host** is a drain (planned, zero-loss switchovers) or a declaration of loss; never an accident.
- **Upgrading** is rolling, one host at a time, with the guarantees kept during the upgrade.
- **Policies change** without redeploying: a universe changes class, loss target or copy count, and the system
  converges to the new policy.

### 5.9 Observability and authority
- One view answers, per host: "if this host is lost now, what is restored, where, and what is lost"; per universe:
  its class, copies, their age, image availability, last restore test, guarantee.
- Alerts fire the moment an invariant or a declared policy stops being true (a copy too old, an image missing, no
  capacity left to absorb a host, a manager without majority, an entry point in two places).
- Who may declare a host lost, change a class or add a host is written in a mandate, and every such decision is
  signed and recorded.

## 6. The ideal scene by topology

The contract never changes; the guarantees follow the infrastructure. The system detects the topology and the
storage and states the guarantees; the tandem does not configure them by hand.

| Topology | What holds | What cannot hold, said plainly |
| --- | --- | --- |
| **One host** | lifecycle, restart after a crash or a reboot, local recovery points (including live, with memory), copies to a Backup Server outside the host, restore onto a replacement host from that server after a declared loss | automatic takeover; continuity through the loss of the host (the service is back when a replacement host is restored) |
| **Two hosts, no witness** | everything above; copies each way; planned zero-loss switchovers; a host cut keeps serving; a host lost for good is restored on the other when an agent declares it | automatic failover (neither host can tell "dead" from "cut"): failover-class universes behave as *stay* |
| **Two hosts with a witness** (the Backup Server can carry it) | everything above; automatic failover for failover-class universes, decided by the side that holds the witness; the witness lost alone stops nothing | a cut that also isolates the witness from both hosts: no automatic decision |
| **Three to nine hosts** | a majority manager; automatic failover per class; spread restoration after a declared loss; copies across failure domains | automatic decisions on a side without majority |
| **Ten or more, or many added at once** | the same, with an odd number of voting replicas across failure domains and the others following; bulk enrollment; gradual rebalancing | nothing more than three to nine hosts guarantee: scale adds capacity and placement choices, not new guarantees |
| **Any of the above with shared replicated storage** | data replication by the storage; failover without copying data; exclusivity enforced by the storage's client fencing | memory continuity still needs a checkpoint; storage outages are storage outages |

## 7. Statistics that tell whether the scene is being reached

Counted over time and compared with their earlier values; none is a restore duration promise.

- Share of protected universes whose copies meet their declared loss target, and share whose image is available on
  every host that may run them.
- Restores tested for real in the period, and how many succeeded.
- Hosts that could be lost now without losing a protected universe beyond its target (the coverage of the "lose
  this host" view), and hosts whose loss the remaining capacity can absorb.
- Campaigns passed per topology and per failure (network cut, service down, power off, host destroyed, heal).
- Incidents per period by kind (failover, refused failover, split-brain observed, reintegration), and incidents
  caused by the system itself.
- Hosts added and removed, and how many needed a manual step beyond the one gesture.
