# High availability for a chosen universe

> **Intent Classification**: GENERIC INTENT (Universal / Parameterized Blueprint)
>
> **Perimeter**: P1 — it decides which host may run a workload, so a mistake runs two.

Status: **lots H1 to H9 built and checked; level 1 complete; level 2 runs end to end across two lab hosts, driven by `tools/ha-standby.py` with the fencing laboratory's gate, unsigned; the collector keeps its archives in bounds (M5); level 3 designed, not built.** Written 2026-09-14
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
than the operation silently doing nothing. Since M-U2 it fences roles as well as universes:
an exclusive route (`network_route_publish` with `exclusive_resource`, the service address of
a logical manager whose replicas all keep running) is withdrawn, verified from the kernel, once
this host's lease on that resource has lapsed or been superseded.

**The timer, packaged and disabled (2026-09-15).** `packaging/podmesh-fence` is what a
systemd timer runs so that a host fences *itself* once its leases lapse on its own clock — the
one case no agent covers, a host the agent cannot reach. It refuses to run without the
operator's mandate file (`/etc/podmesh/fence-mandate`: `authorization_ref`, recorded as
provenance in every fence, and `timeout_seconds`), builds one typed `activation_fence` request
with a fresh operation ID, and hands it to the CLI; it decides nothing. `podmesh-fence.timer`
(every 5 seconds, at least twice per shortest lease of 20 seconds or more) is shipped
**disabled**, and the package's postinst never enables it: writing the mandate and enabling the
timer is the operator's decision 4. `tests/check-fence-timer.py` on lab-a: without a mandate
the script refused (exit 3) and withdrew nothing; under a transient timer every 2 seconds the
fence ran while the lease was live and withdrew nothing; once the lease had lapsed on the host's
clock, nobody renewing it and nobody calling anything, the role's route and the address its
universe carried were gone within one interval (5.2 s after the lapse, observation included),
the universe under no policy of its own still running. With the timer enabled, a partition that
cuts the agent from the governor's host ends the way the takeover margin assumes: the cut host
fences itself when its lease lapses, and the standby that waited lease + margin takes over.

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

**The allowance is a judgement, and it is meant to stay one.** An earlier draft of this
section treated "credits" as a missing feature needing a unit, a ledger and an authority. The
operator has corrected that: the word was an image. What it names is the administrator
weighing their own criteria and deciding what a given container gets — **entirely subjective,
and outside the system by design**.

That changes what PodMesh owes it. Not a model: any unit it invented would be a budget nobody
agreed to, and any admission rule built on one would refuse or permit on grounds the
administrator never set. What it owes is **a record of who decided**, kept verbatim beside the
policy that decision produced. `activation_require` already required an `authorization_ref`
like every other operation, validated it, and threw it away — which keeps the obligation and
loses the only part worth keeping. It is now stored and reported as `allocation_decided_by`,
and every status answer says `allocation_is_a_judgement: true` so nothing downstream reads it
as a computed figure.

It is provenance and never a checked credential, exactly as `PREPARE-A-HOST.md:38` says of
every `authorization_ref` in PodMesh.

**Lot H4, the lease follows the migration chain.** Level 1's stated missing work was to wire
the exclusive claim into the handoff so a second destination cannot be authorised while the
first holds it. Doing that exposed a gap in H1 itself: the gate only knew about `start` and
`clone`, and **two migration operations leave a running universe behind without ever being a
`start`** — a destination restore and a local recovery restore. Either could produce a writer
on a host holding no entitlement.

The gate now covers both, and `migration_authorize_transfer` requires a live lease on the
source: only the host entitled to run a universe may originate its handoff, or a fenced host
could begin the very handoff the fence exists to prevent. On `migration_complete_transfer` the
source surrenders its lease under its own history event, `released_by_handoff`, so a reader
can tell a handoff from an operator's release.

**That release is cleanup, not the safety mechanism.** A completed reservation already refuses
`start` on the source in every state but released or collected, so a release lost to a crash
between the two writes blocks and never permits. It is still done because a lease that
outlives the universe it was for is a lie in the journal — and it is the one rule here the
single-host check cannot reach, since it runs only after a completed two-host handoff. That
is written in the code beside it.

The three new refusals are each verified by removing the rule and watching the check go red
at the case named for it. Every migration-path refusal is asserted to be the **gate's own**,
named as such — a request well-formed enough to pass parsing and no further, so that a later
check's refusal cannot be mistaken for this one.

Level 1 is now complete as designed: a planned handoff that cannot be raced.

**Lot H5, the capture side of level 2.** `recovery_point_prepare` turns a stopped universe
into an immutable, digested recovery point in this host's outbox, with a manifest binding
every field the Backup Server design requires of one. It is B1's step 1 and the first half
of step 2, and it is what level 2 restores from.

**It is `prepared`, not `sealed`, and the manifest says so.** This code base carries no
signing crate — serde_json and rusqlite are its entire dependency list, and the crate
registry answers 403 from this build — so the Ed25519 signature the design's root of trust
rests on cannot be produced here. Rather than pretend, the point sits in the `prepared` state
of the design's own typed list with `signed: false`, `signature: null`, and a format string
ending in `unsigned-unencrypted`. A verifier that treats it as sealed is wrong; one that
refuses it is doing its job. Adding a signing dependency is a supply-chain decision, and it is
the operator's.

Three things in it were got right only because the design had already been wrong about them:

- **The stop is observed, not looked up.** The observations journal keys by operation name
  and cannot attribute a past `stop` to a universe. So the manifest records the container's
  own status, exit code and finish time, and treats exit code 137 as the escalation signature
  the API document names.
- **An escalated stop has no class.** The capture is refused outright — never downgraded to
  `crash-consistent`, which the adapter section forbids, and never `incoherent`, which is
  defined for a running universe. This is the rule revision 6 corrected in prose; H5 is the
  first code that enforces it, and the check reaches it with a bare `sleep` as PID 1.
- **The export is renamed only once it has a size and a digest**, Rule 12's own discipline,
  applied to the one archive B1 does produce.

**The check verifies the marker inside the exported tar.** An export of a never-started
container is byte-identical to its image, so a check that compared digests alone would pass
on a capture that captured nothing. That is the false-pass trap the fifth review named, and
the check writes the marker as a literal in the container's command — in both the file name
and the contents — exactly as step 0 specifies.

**Lot H6, the restore side of level 2.** `recovery_point_restore` takes a point from this
host's inbox and creates a quarantined, new-identity universe from it: no network, not
started, under a UUID that must differ from the source's — and that rule is enforced even
when the source is unknown here, which is exactly the standby host's situation. The archive is
checked against the manifest's size and digest, the manifest against its canonical form and
pinned format, and a manifest that claims a signature is refused: this build cannot verify
one, and a signature nobody can check is not a signature. The container is created through
the ordinary `create` under a derived operation ID, so ownership needs no new rule, and the
restored universe starts and stops like any other once the operator lifts the quarantine.
The check proves the round trip by the marker the source wrote while running, found again in
a fresh export of the restored container, and proves each refusal by weakening the daemon.

Two things it does not claim. It does not verify the manifest's **origin**: the archive is
bound to the manifest, nothing binds the manifest to a producer, and the response says so.
And it does not move bytes: the check copies the outbox into the inbox by hand, because the
transport controller is the design's, not PodMesh's, and it does not exist.

**Lot H7, the takeover: promotion under the lease.** A quarantined restore has a new
identity by design, and the lease is per universe — so a standby holding a lease for the
universe and a quarantined copy of it had no typed way to make the one run as the other.
`recovery_point_promote` is that step. It names the quarantined copy and the identity it is
promoted into, and creates that universe from exactly the image and command the copy was
created from — read from the copy's own verified create in the journal, not from the manifest
again, so what runs is what the operator inspected. No network, not started; the start is
the caller's and goes through the same gate.

Its contract is refused in a stated order, and each refusal was verified by removing the rule
and watching the check go red **with "accepted"** at its own case — a first draft of the check
had three of them going red for a neighbouring rule's reason, which proves nothing about the
rule removed:

- no quarantined copy under that identifier here; a copy of a different universe; a copy
  promoted into itself;
- the universe is under no activation policy on this host — a promotion is a takeover, and a
  takeover with no lease semantics would be a start on nobody's authority;
- the lease gate itself, with its three reasons: none held, held by another host, this host's
  own lapsed. The decisive case is the previous holder's lease **inside the takeover margin**:
  the acquisition is refused, and so is the promotion; once the margin has passed the
  acquisition advances the generation and the promotion records it.

**The proof is taken before the first start, and that is not a detail.** The check exports
the promoted container while it is still `created` and finds the marker the source wrote
while running. Taken after a start, the same assertion is vacuous — the universe's own
command writes the marker again — and a promotion from the pristine image passed it until
the check was moved. The restore check already took it before the start; this one now does.

**What the lease proves, in every answer.** `scope` says: the lease lives in this host's
journal; it proves this host's own restraint, not mutual exclusion; nothing here proves the
previous holder is stopped. That is the honest state of H1 carried forward, not a new
weakness. The quarantined copy is left in place: removing it is the collector's or the
operator's, never a side effect of a takeover.

**Level 2 on one host is now a complete sequence of typed operations**: stop, prepare,
carry outbox to inbox, restore into quarantine, require and acquire after the margin,
promote, start. The two-host suite's controller already carries an outbox to an inbox over
SSH for migrations, and the recovery point's two files ride it unchanged. What is still
missing is exactly what no single host can supply: a signed manifest (the operator's
dependency decision), a datastore that verifies and catalogues, a failure detector that
decides *when* the standby begins its wait, and the lease replicated as a fact so that the
margin is measured against the previous holder's clock rather than a fixture. Until then the
standby's wait is measured against its own journal, and the design says so.

**Lot H8, the epoch half: PodMesh as the fencing laboratory's maker.** The lab in
`experiments/manager-fencing` (Codex, 2026-09-12) models exclusion the other way round from
the leases above: not a lease that expires, but an **epoch** issued by one external gate,
rotated only by an explicit trusted action — never by a timeout, a stale observation or a
host's absence — with each maker keeping a durable screen that refuses any epoch it has
already seen superseded. It deliberately exposes no `start` adapter, because a Podman start
after a gate check leaves a gap the gate cannot close. The two designs compose, and the
composition is the operator's sentence "I choose the host with my agent": **the agent is the
lab's rotation controller, and PodMesh is its maker.**

A policy may name an `authority_id`. Acquisition then requires a permit in the lab's exact
six-field form, bound to the universe, to this host and to this **boot** — a rebooted host
must be authorised again, since whatever it was doing before, nobody has re-decided it. The
screen refuses a permit at an epoch already seen superseded; a takeover from another holder
needs an epoch newer than that holder's, on top of the margin; `activation_supersede`
delivers a newer grant bound to someone else, after which the gate, the renewal and the fence
all treat this host's lease as void. Nine rules, each removed in turn, each turning the check
red with "accepted" at its own case.

**What this changes about safety, and what it does not.** With epochs, two standbys cannot
both activate for one rotation: the gate's compare-and-swap issues one permit per epoch, and
that is the lab's proof, not PodMesh's. What remains PodMesh's is the maker's discipline —
refuse what the screen says is stale — and the honest gap: PodMesh cannot verify a permit's
origin (no signature, no gate call), so a permit is provenance from a root-only channel, and
a forged **higher** epoch can stop a universe here but never start a second one. The margin
stays, because the ungated start is exactly what the lab refuses to gate.

## Level 2 across two hosts, measured on 2026-09-14

`tests/check-recovery-point-two-hosts.py` ran the whole sequence between two lab hosts, on a
transient development service carrying the same release binary on both, with every product
mutation through the API and the suite in the three roles the design leaves outside PodMesh:
transport controller, agent, and the fencing laboratory's epoch gate. First on the H7 binary
(sha256 `aee5d980…8af9a0f`, twenty-eight checks), then on the H8 binary (sha256
`37d86fef…68ae72`, thirty-seven checks), all passed, in this order:

1. The active host creates the universe, declares a policy of one standby among the two
   hosts under the gate's authority, is refused a start before any lease, is refused an
   acquisition without a permit and one with the standby's permit, acquires under **epoch 1**,
   starts. The container writes the marker while running.
2. It captures: stop (not forced), `recovery_point_prepare` (8.6 MB, `signed: false`), renews
   the lease, starts again. Capture costs the universe a stop; that is level 2's price and it
   is stated.
3. The suite carries the manifest and the archive from the outbox to the standby's inbox over
   SSH and compares digests on both sides.
4. The standby is refused a restore into the source identity, restores into quarantine, and
   **the marker is found in the quarantined copy before anything has started on the standby**.
   A promotion before any policy exists there is refused without effect.
5. The active host "fails": it stops renewing. The suite waits for the lapse **on the active
   host's own clock**, a renewal is refused, the self-fence stops the universe without
   escalation, and a start there is refused.
6. The suite waits the takeover margin, again on the active host's clock. The standby
   declares the policy, is refused a promotion before the lease and an acquisition under the
   active host's grant, and the gate rotates: the standby acquires under **epoch 2**, promotes,
   and **the marker is found in the promoted universe before its first start**; then it
   starts and runs.
7. The active host returns. The agent delivers the standby's grant to it: its screen advances
   to 2, and it is refused under its old grant, under a fresh permit at epoch 1, and under a
   second grant at epoch 2 — only a new rotation could bring it back.

**What the run found.** A standby that has never run an activation operation had no
activation tables, and the promotion answered "no such table" where it should have said "no
activation policy". The single-host check cannot reach that state, because it starts the
source before anything else; the two-host run reached it on its first attempt. The promotion
now prepares that schema itself, and the run was repeated on the fixed binary.

**What the run does not prove, recorded in its own report under `not_proven`:** exclusion
beyond the epoch (the gate was the suite; PodMesh verified permit binding and its screen,
never a permit's origin), failure detection (the suite decided when the wait began and when to
rotate; no host did), the manifest's origin (unsigned), and transport (bytes were carried by
the suite, not by PodMesh). The lease generations were 1 on both hosts and the epochs 1 and 2:
two journals each counting alone, and one rotation that both of them recorded.

## Lot H9 — the agent's side, as a tool and not a timer

Everything level 2 needs outside the two hosts is the agent's by the canon's rule, and until now
it lived inside a test. `tools/ha-standby.py` is that side made runnable by a human or an agent
from a workstation: invoked, one bounded thing, one JSON report, exit. Whether it may ever run
on a schedule is a production mandate, exactly as for the collector.

Four subcommands. `gate` creates and drives the epoch gate — the fencing laboratory's own
`Authority`, imported from its tree (reviewed candidate `0c3756fb…`) and never copied, one
SQLite compare-and-swap file on the host the tool runs on, with the laboratory's precondition
(one current copy, never cloned or rolled back) stated as the operator's obligation.
`activate` declares the policy under that authority, rotates the epoch to a host and acquires;
starting stays the operator's. `cycle` is one capture: declare the collector's retention on the active host, stop,
prepare, renew, start again, carry the two files to **one or more standbys** (`--also`),
restore into quarantine on each, prune the older quarantined copies of each through the API
and keep a ledger per standby — with three nodes, one or two replicas per universe, as the
operator asked; it refuses from a host that does not
hold the lease, and reports how long the universe was stopped. `takeover` refuses while the
active host is reachable and entitled — that is a planned handoff, not a takeover, and the
tool will not start a second writer; otherwise it fences the active host if it can be reached
and waits the margin on that host's clock, or, if it cannot, waits **lease plus margin on the
standby's clock, as recorded in the ledger when the universe was activated** — never this
invocation's defaults, which is what the independent review caught in the first version — — any lease the active host holds expires at most a lease after its last
renewal, which is not later than now, and the margin is the clock-skew budget; then it
rotates the epoch, acquires, promotes the newest quarantined copy, starts, and supersedes the
active host if it can be reached. Every report carries `data_lost_since_seconds` and a
`not_proven` list.

Measured on the two lab hosts (`tests/check-ha-standby-tool.py`, the tool driven as a
subprocess): a cycle from the wrong host refused; two cycles with the marker in the quarantined
copy and the older copy pruned; a takeover refused as a handoff while the active host was
entitled; a takeover after the lapse with the fence, the margin on the active clock, epoch 2,
the marker present before the first start, the active host stopped, refused and superseded;
and **a takeover with the active host unreachable**: 26 seconds waited on the standby's clock,
promoted and started there, and the active host — still running its copy exactly as a
partitioned host would — stopped by its own fence when that was run, without escalation, its
lease having lapsed before the standby started. The two windows did not overlap; that is
level 2's safety claim, and it held.

**Three hosts, and the old active rejoining** (`tests/check-ha-three-hosts.py`, on lab-a,
lab-b and lab-c): one cycle restored the point into quarantine on both standbys with the
marker; the active host lapsed; one standby took over under epoch 2 and the other was
informed — its screen refused a stale epoch-1 permit bound to it; the old active, superseded,
was refused under its old grant, under a fresh permit at the old epoch and under a second
grant at epoch 2, and then **rejoined as a standby**: a cycle from the new active restored a
quarantined copy on it, marker present, with no journal reset. That is the shape of the
acceptance document's HA-10 without a real partition — the active host fails by not
renewing, and no network was cut.

What the tool does not do: verify a permit's origin (nobody can, yet), prove the unreachable
host stopped (the wait is the design's margin, not a proof), or delete the active host's
archives (those are the collector's, after a declared retention).

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

Restated on 2026-09-14, after lots H1 to H8 and M5: these are the decisions that now stand
between the measured level 2 and a production mandate. Each has a recommendation. **On the
operator's "go" of the same evening, the recommendations below are taken as the tool's
defaults, by hypothesis and revocable**: the laboratory's gate; capture cycles the agent
runs (the operator's figure: every fifteen minutes), `keep_latest` 3 on the active host with
a minimum age of one hour, three quarantined copies on the standby; a 20-second lease with
a 5-second margin; the self-fence; no timer. Signing stays open — there is no crate to take.

- **The gate.** Epochs need one external authority that rotates them. Three candidates: the
  fencing laboratory's `Authority` (a single SQLite compare-and-swap gate, qualified by its own
  22 tests, run on the agent's host), the manager's `authorize_exclusive_service` permit (a
  new candidate and a requalification campaign), or a storage lease once level 3's storage
  exists. **Recommendation: the laboratory's gate first**, on the host the agent runs on, with
  its documented precondition — one current copy, never cloned or rolled back — stated as the
  operator's obligation. It is the only one that exists and is qualified today.
- **Signing.** The recovery point manifest is unsigned because this build has no signing
  crate and the registry is unreachable from it. Adding one is a supply-chain decision (which
  crate, how it is vendored, who reviews it). Until then a standby verifies bytes against a
  manifest whose origin it cannot verify. **Recommendation: decide the crate before any host
  outside the lab receives a point.**
- **The recovery point interval and retention.** Level 2's capture interval is the data you
  agree to lose; `keep_latest` and `minimum_age_seconds` are what the disk keeps. There is no
  correct answer, only a chosen one per universe. **Recommendation: declare both at the same
  time as the activation policy, never leave a universe under HA without a retention.**
- **The fencing mechanism**, and if it is out-of-band, who may cut power to a host. The
  self-fence is built and its margin is the contract; it trusts a sick host to act.
  **Recommendation: keep the self-fence and the margin, and add out-of-band fencing for
  production once someone is named who may cut power.**
- **Which universes.** HA per universe is a choice with a cost — a standby's storage and
  headroom, a capture's stop — not a default. A universe cheap to rebuild does not want it.
- **What happens when the agent is wrong.** If the agent names a host that cannot serve, does
  the system refuse and wait, or fall back? **Refusing is safer and is the recommendation**,
  and it is what every operation here does.
- **Who runs the agent's side.** Failure detection, the wait, the rotation and the transport
  are the agent's; nothing in PodMesh runs on its own, by the canon's rule. The next lot
  makes that side runnable as a tool rather than a test; whether it may ever be a timer is a
  production mandate, exactly as for the collector.

## What this document does not claim

Nothing here is implemented. No lot has been opened. The G2 campaign proves fact replication
and its accounting; it proves nothing about containers, failover, fencing or addresses, and
its own output says so. The Backup Server has a design GO and no code. Level 2, the first
level that deserves the name, is gated on the Backup Server existing.
