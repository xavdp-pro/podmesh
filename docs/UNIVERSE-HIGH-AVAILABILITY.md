# High availability for a chosen universe

> **Intent Classification**: GENERIC INTENT (Universal / Parameterized Blueprint)
>
> **Perimeter**: P1 — it decides which host may run a workload, so a mistake runs two.

Status: **design, nothing implemented.** Written 2026-09-14 against what PodMesh actually
has, not against what an HA product usually has. Every capability it names as missing was
verified in the source, and the citations are below.

The operator's requirement, in their words: high availability for containers of their
choice, with **the AI agent choosing the host or hosts**. That second half is not a detail —
it removes one hard problem and leaves the harder one untouched.

## What exists today, and what does not

**The manager replicates facts, and nothing else.** Its exchange payload is literally
`Snapshot { configuration, replica_id, facts }` (`manager-ha/src/durable.rs:130-134`). No
memory, no container state, no file deltas. The G2 campaign that passed on three hosts
proved those facts converge; its own output carries `ha_claim: "absent"`.

**There is a real exclusion mechanism, and it is careful.** A fact may carry an
`exclusive_resource` and an `active_claim`, and `authorize_exclusive_service`
(`manager-ha/src/lib.rs:523-551`) issues a permit only when the requester is the reconciled
coordinator, the resource is not blocked by conflicting facts, and a reconciled active claim
supports it. The permit names the supporting event and the exact reconciled history, and
`advertises()` voids it the moment that history changes. This is the right shape for the
safety half of HA.

**But it is a laboratory model, and the module says so in its own header**
(`manager-ha/src/lib.rs:4`):

> *"It has no network authentication, failure detector, fencing, DNS or Podman adapter."*

Network authentication has since been built and qualified — that was G2. **The other four are
still absent**, and `CheckService`, the operation that issues a permit, is not reachable from
the packaged resident at all. Nothing in the manager has ever touched a container.

**Migration is a planned handoff, not replication.** The chain preflight → checkpoint →
transfer authorization → restore → completion moves one universe once, with memory
continuity through CRIU. `CONTROL-SERVICES-UNIVERSE.md:282-284` refuses the stronger reading
itself: *"a foundation for explicit handoff, not proof of generic live migration, replicated
memory or automatic HA."*

So the gap between here and HA is five things: failure detection, fencing, address takeover,
a Podman adapter in the control plane, and **any replication of a container's state at all**.

## What the agent choosing the host does and does not remove

It removes **election**. No quorum, no leader, no split vote: the agent names the host. That
is a genuine simplification and it matches the canon's agent-first model.

It removes **nothing** about safety. If host A is partitioned but alive, and the agent starts
the universe on host B, two instances are running and both are writing. The agent cannot see
that A is alive; that is what a partition means. **Fencing is not an automation convenience;
it is the only thing standing between this design and two writers.**

A design where the agent decides placement and the machine proves exclusion is a good design.
A design where the agent decides placement *and* is trusted to know A is dead is not a design,
it is a hope.

## The three honest levels

Each is a lot. Each is useful on its own. None of them should be called the next one.

### Level 1 — proven handoff, planned only

What the migration chain already does, plus an exclusive claim so it cannot be raced. The
agent asks for a move; the source is checkpointed, the destination restores, the claim moves.

- **Recovery point**: none lost — the handoff is planned and the source is live until it is not.
- **Covers**: maintenance, rebalancing, draining a host.
- **Does not cover**: a host that fails. There is nothing to hand off from.
- **Missing work**: wire the exclusive claim into the migration chain so a second destination
  cannot be authorised while the first holds the claim.

### Level 2 — warm standby from a proven recovery point

The universe runs on A. The Backup Server captures recovery points of it. A fails. The agent
names B. PodMesh refuses to start on B until A is **proven stopped or fenced**, then B
restores the most recent verified recovery point and starts.

- **Recovery point**: everything since the last capture is lost. That is the honest RPO and it
  is a number the operator chooses by choosing the capture interval.
- **Recovery time**: the restore, which is bytes over the wire plus a start.
- **Depends on**: the Backup Server, which has a GO on its design and no implementation, and
  whose first lot B1 proves one round trip of one stopped container.
- **This is the first level that is actually high availability**, and it is reachable without
  inventing anything new.

### Level 3 — continuous replication, short recovery point

The universe's writable layer is replicated to the standby continuously, so the recovery point
is seconds rather than an interval. This is the level the operator described earlier: a live
machine and one or more warm replicas receiving deltas.

- **Needs a storage layer PodMesh does not have.** ZFS, Btrfs or LVM2 thin with send/receive
  are optional capabilities today (`PREPARE-A-HOST.md:42`), and lot B0, which qualifies them,
  has not run.
- **Memory continuity is a separate and harder question.** CRIU moves memory at a planned
  checkpoint. Continuously replicating a running process's memory to a standby is not
  something the migration chain does or was designed to do, and nothing in PodMesh does it.
  Level 3 should be scoped to **disk** replication first, with memory left to the planned
  handoff of level 1.

## Fencing, which is the whole problem

Three mechanisms, in descending order of how much they actually prove.

**Storage lease.** The universe's data lives where writes require a lease, and the lease
expires. A partitioned host stops being able to write, whether or not it knows it. This is the
strongest and it requires the storage layer level 3 needs anyway.

**Host fencing, out of band.** Power off or reset host A through IPMI, a PDU, or the
hypervisor. Proves A is gone rather than inferring it. Needs out-of-band access PodMesh does
not have today, and an operator decision about who may cut power.

**Self-fencing by watchdog.** The host running the universe must renew its claim within a
bounded period; if it cannot reach its peers to renew, it **stops the universe itself**. This
needs no hardware and fits the agent-first model, and it is the weakest: it trusts a host that
may already be sick to act correctly. It is defensible only with a margin — the standby waits
strictly longer than the self-fence deadline before starting, so the two windows cannot
overlap.

**The recommendation is self-fencing for the first implementation, with the margin stated as a
contract**, and storage leases when the storage layer exists. Out-of-band fencing is the right
answer for production and it is an operator decision, not an engineering one.

## What PodMesh has to gain, concretely

1. **An exclusive claim per universe**, carried as a fact with `exclusive_resource` set to the
   universe UUID, replicated by the manager that G2 just qualified.
2. **A gate in the local API.** `start` must refuse when the caller cannot present a valid
   permit for that universe. The API already refuses `create`, `start`, `delete` and `clone`
   for a universe under a migration reservation (`LOCAL-API.md:91`), so the shape exists; this
   adds a second reason to refuse.
3. **`CheckService` reachable from the resident**, since the permit it issues is the gate.
4. **A failure detector** — which the manager does not have, and which is the input the agent
   needs in order to decide anything.
5. **A watchdog that stops a universe** when its claim cannot be renewed. This is the Podman
   adapter the manager has never had.

Items 1 to 3 are the safety half and can be built and proven without any storage work. Items 4
and 5 are what turn a safety property into availability.

## What is the operator's to decide

- **The recovery point.** Level 2's capture interval is the data you agree to lose. There is no
  correct answer, only a chosen one.
- **The fencing mechanism**, and if it is out-of-band, who may cut power to a host.
- **Which universes.** HA per universe is a choice with a cost, not a default. A universe that
  is cheap to rebuild does not want it.
- **What happens when the agent is wrong.** If the agent names a host that cannot serve, does
  the system refuse and wait, or fall back? Refusing is safer and is the recommendation.

## What this document does not claim

Nothing here is implemented. No lot has been opened. The G2 campaign proves fact replication
and its accounting; it proves nothing about containers, failover, fencing or addresses, and
its own output says so. The Backup Server has a design GO and no code. Level 2, the first
level that deserves the name, is gated on the Backup Server existing.
