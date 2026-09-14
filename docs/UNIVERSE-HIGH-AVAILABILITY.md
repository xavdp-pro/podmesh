# High availability for a chosen universe

> **Intent Classification**: GENERIC INTENT (Universal / Parameterized Blueprint)
>
> **Perimeter**: P1 — it decides which host may run a workload, so a mistake runs two.

Status: **lots H1 to H3 built and checked; levels 1 to 3 designed, not built.** Written 2026-09-14
against what PodMesh actually has, not against what an HA product usually has. Every
capability named as missing was verified in the source, and the citations are below.

H1 is the local half of exclusive activation: this host refuses to start a universe it holds
no live lease for. That is a self-restraint, not mutual exclusion across hosts, and the
distinction is kept in the code, in every status answer, and in the section that describes it.
Nothing here is high availability yet: level 2 is the first that deserves the name.

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

## What is built

**Lot H1, the local half of exclusive activation.** Implemented in `src/activation.rs`,
gated in `src/lifecycle.rs`, checked by `tests/check-activation.py`.

A universe may be put under an activation policy naming a lease duration and a takeover
margin. `start` and `clone` then refuse unless this host holds a lease that has not expired,
and the refusal says which of the three reasons applies: no lease at all, a lease held by
another host, or one of this host's own that has lapsed. `stop` stays available, for the same
reason it survives a migration reservation — stopping can never produce a second writer.

Five typed operations, each idempotent by operation ID like every other PodMesh operation:
`activation_require`, `activation_acquire`, `activation_renew`, `activation_release`,
`activation_status`.

Three rules are worth naming because each is a place this could have been got wrong:

- **A lapsed lease is retaken by acquisition, never by renewal.** Renewing would silently
  extend an entitlement that had already ended, while another host may already have begun its
  takeover wait.
- **Re-acquiring one's own live lease keeps the generation.** A repeat is idempotent, not a
  takeover, and the history says so.
- **A different holder may acquire only after the previous expiry plus the margin.** The
  margin is therefore also the clock-skew budget between two hosts, since wall clock is the
  only clock two hosts share. It is stated rather than assumed.

**What H1 proves: a self-restraint.** This host will not start a universe it has no lease
for, which is what stops an agent that names the wrong host. **What it does not prove: mutual
exclusion.** The lease lives in this host's journal; a host that never asks is not restrained
by it, and a partitioned host is restrained only by its own copy. Every status answer carries
that sentence in a `scope` field so a caller reading only the object cannot mistake one for
the other. Mutual exclusion needs the lease replicated as a fact and a permit issued against a
reconciled history — the manager's job, and the next lot.

Each of the four rules was verified by weakening it in the source and confirming the check
goes red at the case named for it: the gate itself, the expiry, the takeover margin, and the
refusal to renew a lapsed lease.

**Lot H2, self-fencing.** `activation_fence` stops every universe under a policy that this
host holds no live lease for — whether the lease lapsed, was never taken, or belongs to
another host. A universe whose lease is live is left alone and the report says so, rather
than the operation silently doing nothing.

**It is an operation and not a timer, deliberately.** PodMesh does not act on its own; the
garbage collector carries the same constraint for the same reason. So the *timeliness* is the
caller's obligation: whoever drives it must call it at least as often as the shortest lease,
or a lapsed lease leaves a universe running. That dependence is precisely why self-fencing is
the weakest of the three mechanisms — a host too sick to renew may be too sick to fence, and
then nothing here stops it. What makes it defensible is the takeover margin: the standby waits
strictly longer than the lease plus the margin, so an honest but slow host has already been
asked to stop before anyone else may start. A storage lease or out-of-band fencing proves what
this only asks for.

Checked against a real container: a live lease is left alone, an unentitled universe is
stopped without escalating to SIGKILL, the fence is idempotent, and the gate still refuses the
restart afterwards. Two of its three rules were verified by weakening them and watching the
check go red. The third — refusing to report a stop that did not happen — is defence in depth
and unreachable by the check, since it guards `podman stop` succeeding while the container
still runs. That is written in the code beside it rather than left looking like a tested
safeguard.

**Lot H3, the replication intent and the facts to decide it on.** How many standbys a
universe gets is a **per-universe choice**, not a cluster setting: a standby costs storage and
reserved headroom, so a universe cheap to rebuild wants none and one that must not stop wants
two. With three nodes that is one or two.

The policy now carries `desired_standbys` and `eligible_hosts`. Absent means **none**, never
"as many as possible" — a policy that says nothing about standbys is declaring none. A target
no placement can satisfy is refused at declaration rather than discovered later: two standbys
named among two eligible hosts, one of which runs it, cannot be honoured.

**PodMesh reports the intent and refuses to claim the placement.** It sees one host — this one
— so `standbys_placed` is null and `placement_verified` is false, always. A number there would
be a claim about hosts it has never contacted. Verifying a placement needs something that can
see the other hosts, which is the manager's job and the lot after next.

**And it now measures what it never measured.** PodMesh knew disk and nothing else, so "does
this host have room for a standby" could not be answered honestly at all. `activation_status`
reports memory available, CPU count, one-minute load and state-directory space. `MemAvailable`
is read rather than `MemFree`, because free memory on a busy host is small and says nothing —
page cache is reclaimable, and the kernel's own estimate of what a new workload can claim is
the question being asked.

These are **facts and not a decision**. Nothing in PodMesh decides whether a standby fits:
that depends on what the universe needs and on whatever allowance the operator has granted it,
neither of which PodMesh knows. An admission rule that guessed would be worse than none.

**The allowance itself does not exist yet.** Credits per universe need a unit, a ledger and an
authority that grants them, and PodMesh has none of the three. It is named here as the
operator's decision rather than modelled, because a budget invented by the implementer is a
budget nobody agreed to.

## What PodMesh still has to gain

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
