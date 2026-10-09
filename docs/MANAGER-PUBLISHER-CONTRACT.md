# The publishing connector follows the active manager

Status: **contract written 2026-09-15 on the operator's decision**
(`CLOUDFLARE-TUNNEL-MANAGER-HA-DECISION-2026-09-15`); what is built is stated in "What is built"
at the end, and nothing above it is a claim about code.

Naming (the operator's decision of 2026-09-17): the **active manager** is the manager replica
whose host holds a live, unsuperseded activation lease on the logical manager resource under the
epoch gate. The identifiers of its mark were renamed that day, compatibly:

- the mark's path inside the carrier universe is `/run/podmesh-manager/active-manager.json`; a
  node also removes `/run/podmesh-manager/governor.json` whenever it writes or removes the mark,
  until no running manager image reads that path;
- its kind in the network effects ledger is `active_manager_mark`; rows of kind `governor_mark`
  written by earlier builds are the same effect, matched wherever the kind is matched and never
  rewritten, until no host's journal holds one;
- `publisher_status` reports it as `active_manager_mark`, and as `governor_mark`, deprecated,
  with the same value for one release.

The logical manager's human-facing interface is published through a Cloudflare Tunnel. That must
not create a second election or a second authority: the connector is transport, it decides
nothing, and a connector being online grants no manager authority.

## Invariant

One public hostname, one logical tunnel (one tunnel UUID), three manager replicas, and — in this
first candidate — **exactly one publishing `cloudflared`**, co-located with the active manager
replica and governed by the same resource (the logical manager UUID), the same epoch and the same
fence as the service address (`UNIVERSE-NETWORK-CONTRACT.md`, `UNIVERSE-HIGH-AVAILABILITY.md`).

Cloudflare accepts several connectors for one tunnel and does not say which receives a request;
three connectors each pointing at their local replica would let a standby receive a write.
Several active connectors are allowed later only when every one of them proxies to the same
stable, epoch-qualified service address and fails closed when that origin is not current.

## Identities and what stays where

| what | where | never |
| --- | --- | --- |
| tunnel UUID, hostname, credential's name, origin port | the host's journal (`publisher_declare`) | — |
| the tunnel credential | Podman's secret store on the host (`secret_declare`), a root-only runtime copy while the connector runs | Git, an image layer, the journal, the replicated store, a recovery point |
| the connector's identity | `cloudflared`'s own journal (`connection=<id>`) | the journal as truth |
| the active manager's mark (resource, epoch, marked at) | `/run/podmesh-manager/active-manager.json`, a root-only file inside the carrier universe, written and removed by PodMesh | anywhere the universe could write it itself |

## The origin, epoch-qualified

The manager universe answers `GET /ready` on its origin port (8080) with its logical manager
and replica identities and, **only while the active manager's mark is present**, the epoch it was
marked with — HTTP 200; without the mark, or on any other path, HTTP 503. The mark is PodMesh's:
written at `publisher_start` under the epoch gate, removed at `publisher_stop` and by the fence.
A connector that reaches a replica that is not the active manager gets nothing. The universe's
`/run` is its overlay, not a tmpfs, so a mark would outlive a stop and a start of the carrier (a
withdrawal while the carrier is stopped finds no running universe to remove it from): the
entrypoint removes both of the mark's paths, and the path `PODMESH_ACTIVE_MANAGER_MARK` replaces
them with when it is set, at every start, before the origin starts, and the
replica claims no role until PodMesh writes the mark again at `publisher_start` (V3-1, web tree).
With `PODMESH_ACTIVE_MANAGER_MARK` set, the origin reads that path only — neither of the two
others — and without a file there it answers 503: it fails closed, and a node writing the usual
path does not mark a replica configured so. The manager's web
interface is not served there yet; this responder is the origin the contract requires today.

## Operations

All journaled, all recorded in the network effects ledger before they are made and reconciled
with it (`UNIVERSE-NETWORK-CONTRACT.md`, "Failure and cleanup states"), all with
`authorization_ref` as provenance.

- `publisher_declare` (`resource`, `hostname`, `tunnel_uuid`, `credential`, optional
  `origin_port`): recorded by reference; refused without an activation policy for the resource
  on this host, or without the credential declared.
- `publisher_start` (`resource`, `takeover_proof`, optional `previous`): refused, in this
  order, when the resource's lease is not live and unsuperseded here (the lease gate's four
  reasons), when the exclusive route and the alias are not effective on this host, when the
  credential is not in Podman's store, and when the **takeover proof** does not bind this
  transition. The proof is the authority's document — the gate that advanced the epoch issues
  it at the rotation (`tools/ha-standby.py rotate`) — bound to the resource, the previous and
  new epochs, the previous and new holders, an issue and expiry time, and a method: `first`
  (no epoch ever existed), `same_holder` (this host held the previous epoch), `fence_receipt`
  (the previous holder's fence, its receipt attested into the proof by `attest-fence`, bound to
  the resource the fence found that host not entitled to, with every withdrawal it made for it
  verified) or `lease_barrier` (an unreachable previous holder: the authority's
  `eligible_after`, the previous lease plus the margin from the rotation, compared with this
  host's clock and refused before it). The host checks every binding — after the proof's
  origin: under a policy that names the authority's Ed25519 key (every policy the tool declares
  does), the proof must be the signed kind and its signature must verify over its canonical
  form before any field is read, so an altered, unsigned or foreign-key document is refused as
  such; under a policy without a key only the unsigned laboratory kind is accepted, on its
  binding alone, and the answer says `signed: false`. Under a policy that names a quorum of replica
  keys (development tree, V3-2, `LOCAL-API.md`, "Quorum certificates"), the proof is a certificate
  that at least the threshold's number of distinct keys of the quorum signed under the policy's
  digest, bound in addition to this boot of the new holder and to the grant the lease was acquired
  under, and the answer names the signers; a single-key policy accepts such a certificate under its
  1-of-1 digest beside its own signed kind. The agent's `previous` narrative is
  recorded beside it and decides nothing.
  **Same-epoch resume (V3-1).** A proof that verifies is recorded with the lease incarnation it was
  verified against (epoch, generation, `acquired_at`), this host's boot, and the policy's authority,
  key and quorum, and its digest (serial included), which a certificate's resume must still match. A later start without a proof, or with one refused (expired, say), resumes under that
  record — method `resume_same_epoch`, journaled as the event `takeover_resumed` with the original
  proof's identity (the verifying operation, its method, issue, expiry and signature) — only while
  every one of these holds, and is refused naming the first that does not: the lease is held
  (`no_lease`), here (`lease_held_elsewhere`), live (`lease_expired`) and unsuperseded
  (`lease_superseded`); a proof was verified for the resource (`no_verified_proof`); for this very
  epoch (`epoch_changed`); under the same generation (`generation_changed`) and the same
  acquisition (`lease_reacquired`: a lapsed lease of this host's retaken keeps its generation and
  epoch, not its `acquired_at`); during this boot (`boot_changed`); under the same authority, key
  and quorum (`authority_changed`). Each is a condition under which a connector that never stopped is already
  allowed to continue — the reconciliation leaves it alone only under a live, unsuperseded lease held
  here at the transition's epoch; it does not outlive its boot — so a restart under them grants
  nothing that continuing did not. One of them holds only when something runs: a connector is
  withdrawn across a lapse only if the follow tick, the fence or a reconciliation (at the daemon's
  start, or before a network or publisher mutation) runs while the lease is lapsed; a lapse that
  nothing observed, then a retake, leaves it running — the resume refuses that retake
  (`lease_reacquired`) where continuing did not. Anything else needs a valid new proof.
  Then: the publisher's transition recorded `starting` before any effect; the active manager's
  mark written inside the carrier universe and verified; the origin asked at the service address
  and required to answer ready with the expected logical manager, the carrier's replica (as its
  resident names it through the control door) and the lease's epoch; the connector started as a
  transient unit from a root-only runtime copy of the credential with an ingress from the
  hostname to the service address, verified active **and registered with Cloudflare** (its
  connection identity from its journal, waited for a bounded time; a unit that exits or never
  registers is not a publication); the transition recorded `effective` last. A failure at any
  step undoes what was made, last first, and reports the compensation; a transition left
  `starting` or `stopping` by a crash, and an `effective` one whose lease this host no longer
  holds, are withdrawn by reconciliation at the next startup, before every network mutation and
  at every fence: a publisher from an operation reported failed never stays active.
- `publisher_stop` (`resource`): the connector stopped and the mark removed, verified.
- At the daemon's start, before the network reconciliation and before anything is served: every
  declared publisher with something present (a transition, a connector's or a mark's ledger row, or
  an active connector unit, recorded or not) whose lease is no longer live, held here and
  unsuperseded — or whose transition is at another epoch than the lease — is withdrawn, connector
  then mark, each verified, in one journaled operation `publisher_startup_withdrawal` (its ID
  `startup-withdrawal-<boot>-<time>`), journaled only when there is something to withdraw. A lease
  that lapsed while the daemon was down left its connector publishing: the unit is systemd's. The
  route and the alias stay; withdrawing them is the fence's.
- `activation_fence`: for every resource this host no longer holds, the publisher is withdrawn
  first — connector stopped, mark removed — **before** the alias and the route go: one
  transition, each step recorded and verified, reported as `publishers_withdrawn`.
  `activation_fence_preview` counts a connector without entitlement as pending.
- `publisher_observed` (`resource`, `observation`): the agent records an external request's
  result — what the public hostname answered — as provenance.
- `publisher_status` (read-only; `resource`): what is declared; the unit's state; the
  connector's identity from its journal; the lease and epoch; the service address and its
  carrier; the carrier's replica identity through the control door; the origin's readiness now;
  the active manager's mark (`active_manager_mark`: true while a file is at its path or at the
  previous one; `governor_mark`, deprecated, carries the same value); `publisher_eligible` with
  the refusal reasons; the last start, stop, fence and observed request. Since V3-1 also: each
  gate by name (`gates`: `lease`, `policy`, `service_address`, `credential`); the mark as read in
  the origin's order (`active_manager_mark_read`: `present` with `active_manager_mark_epoch`,
  `absent` when neither path holds a file, `unknown` with `active_manager_mark_error` when it could
  not be read); whether the origin answers ready at the lease's epoch with this logical manager and
  the carrier's replica (`origin_ready_at_lease_epoch`: true, false when the origin answered
  otherwise, null when it could not be asked or the replica is not known); the unit's current run
  (`unit.invocation_id`) and whether the connector registered in that run (`connector_registered`:
  true, false when that run's journal was read and holds no registration, null with
  `connector_registration_error` when it could not be read; `connector_id` is read from that run's
  journal only, never from an earlier run's lines); the lease's generation and `acquired_at`; and
  what a start without a proof would decide now (`takeover_resume`: `possible`, or the refusal's
  name). A reading that could not be made is never reported as a wrong value.

The service address has its own resume, `network_route_resume` (`UNIVERSE-NETWORK-CONTRACT.md`
for the route itself, `LOCAL-API.md` for the operation): the recorded exclusive route and alias of a
role this host still holds, put back after the carrier lost them — in place: the recorded row kept
and marked `resuming` with the carrier now at `via`, the dead effects removed and verified gone, the
alias and the route made again and verified, the row `effective` again; a failure compensates and
leaves the row recorded for the next resume, and a crash leaves it `resuming`, which the
reconciliation undoes and keeps the same way — only while the lease is live, held here, unsuperseded
and was acquired or renewed during this boot (by the boot's identity, recorded in every lease history
row since 2026-09-18; by the wall clock only for older rows), a universe runs at `via`, and the
kernel holds no other route for the address.

## Takeover, in order

1. Rotate the epoch through the external gate.
2. Deliver the supersession and fence the previous active manager (or, unreachable, wait its
   lease plus margin: the timer on it withdraws its connector, its mark, its alias and its route
   on its own clock — `packaging/podmesh-fence`). `tools/ha-standby.py rotate --previous-host`
   delivers the supersession itself, before the new holder acquires and before any proof is made;
   when the previous holder is not reached, the proof's barrier is no earlier than the `not_after`
   of the follow mandate it may still renew under by itself, plus the lease and the margin; and
   every rotation carries the barrier of the epoch before it (below).
3. Observe on the previous active manager, when reachable, the connector stopped and the
   address gone.
4. Publish the service address on the new active manager (`network_route_publish`, exclusive).
   The barrier (the previous lease plus the margin, counted from the rotation, since the
   previous holder may have renewed right up to it; see "The barrier, as it is" below) can
   outlast the new holder's own lease:
   after it, the new holder acquires again with the rotation's permit — idempotent for the
   holder, the design's answer to a lapsed lease of one's own — before starting.
5. `publisher_start` there: the proof's origin and binding, readiness at the new epoch, then
   the connector.
6. Verify the connector's identity and exercise the public hostname from outside; record it
   (`publisher_observed`).

No DNS mutation is part of a takeover: the hostname is a CNAME to the tunnel, and the tunnel is
the same. A connector whose local origin is absent, stale, wrong-epoch or unauthenticated stops
or refuses rather than proxying elsewhere.

**Laboratory follow (2026-09-16):** a host-side tick under a written mandate
(`docs/PUBLISHER-FOLLOW-LAB.md`, `packaging/podmesh-publisher-follow`) may start the connector
when this host is eligible **and** the gate's current takeover proof is installed on the host.
It does not rotate, publish the exclusive route, or mint a proof. `tools/arm-publisher-follow.py`
arms the laboratory path on `podmesh-dev-ha`. **Since V3-1 (2026-09-18)** the tick starts with the
installed proof only when it is for the lease's epoch and otherwise without one, the node then
resuming at the same epoch or refusing; asks for `network_route_resume` once when the service
address is the only gate missing; and on a running connector compares the mark's epoch, the origin's
readiness at the lease's epoch and the registration of the connector's current run, stopping it on a
mismatch so that the next tick starts it again under the resume rule. The mandate's `not_after`
stays the human bound: after it the tick renews nothing and resumes nothing.

## What this contract does not decide

The manager's web interface behind the origin; Cloudflare's own availability; several active
connectors (later, with their own qualification); out-of-band fencing of a host whose daemon is
wedged (lease-expiry self-withdrawal needs the daemon alive).

## What is built

**2026-09-15, the single publisher:** `src/publisher.rs` on the network effects ledger, the
operations above, the origin responder in the manager universe's entrypoint (web tree), the
fence stopping the connector and removing the mark before the address goes, the preview counting
a connector without entitlement. `tests/check-manager-publisher.py` on lab-b (active manager) and
lab-c (standby) with a real laboratory tunnel and hostname: the standby's start refused by the
lease gate; the active manager's start refused before the service address and without an account
of the previous publisher; started, its unit active, a connection registered, the origin ready at
the epoch with the active manager replica; **an external request through the public hostname
answered with the logical manager, that replica and that epoch**; after the rotation the old
active manager's fence stopped its connector and removed its mark before withdrawing its alias
and route, its origin answered 503 at its own address, it was no longer eligible; the new
active manager published, started, and the same hostname answered with its replica and the new
epoch. Two hosts, not three, on that day (the third was out of reach). The laboratory tunnel and
hostname are the operator's private fixtures, disposable; the credential reaches a host only as a
PodMesh secret. Cloudflare's edge answered 403 (error 1010) to a bare python User-Agent; the
external request carries a browser-like one — the edge's own gate, not the manager's.

**2026-09-15, each gate mutated (item 8):** the permit check, the readiness epoch check
(`tests/check-manager-publisher-readiness.py`, with a lab fault writing the active manager's mark
one epoch behind: the start refused, the mark compensated, the origin back to 503, no unit; the
fault-free start then succeeding), the connector stop in the fence, and the account of the
previous publisher — each removed in turn went red at its own check, the reference build green
on both suites. The main suite ends with the permit gate alone: the connector stopped, the lease
left to lapse, the route and the alias still effective, a start refused by the lease gate and by
nothing else.

**2026-09-15, the hard test (item 6, the operator's decision 4):**
`tests/check-manager-publisher-agent-cut.py` on lab-b (active manager) and lab-c: lab-b cut from
lab-c, its pool and the agent for 120 seconds by an nftables table with the dead man's switch
armed and verified first, **its Internet egress kept** so that Cloudflare could still reach its
connector; its lease (20 s) lapsed on its own clock and its self-withdrawal timer, under a
mandate, stopped the connector and removed the mark 5.8 s after the lapse — the public hostname
answered 530 (no connector) 26 s after the cut, nobody having reached lab-b; the agent, after
waiting lease + margin + 1 on its own clock, rotated the role to lab-c, which published and
started its connector: the public hostname answered with lab-c's replica and the new epoch; the
withdrawal on lab-b came 30.5 s before lab-c's publication (both hosts' clocks recorded); once
the switch reconnected lab-b, it was superseded, followed the role, converged as a standby and
its connector stayed stopped. Not shown: clocks that lie (a one-second allowance), a wedged
daemon on the cut side (self-withdrawal runs through the daemon).

**2026-09-17, the mark's identifiers renamed, not yet run on the hosts:** a node writes the mark
at `/run/podmesh-manager/active-manager.json` and removes the previous path in the same command;
it verifies a write by a file at the mark's path and none at the previous one, and a removal by
no file at either. New ledger rows are of kind `active_manager_mark`; the ledger's apply, verify
and undo, the reconciliation, the withdrawal and the fence match `governor_mark` rows as well, and
a unit test on an in-memory journal holds the withdrawal's selection of them in every state.
`publisher_status` reports both fields. The origin that reads both paths is built in the web
tree; an origin that reads only the previous path answers 503 to a mark at the new one, so a node
running this build is refused at `publisher_start`'s readiness check until the manager image is
rolled.

**2026-09-18, V3-1, the entry point follows its holder — built and tested without the laboratory,
not yet run on the hosts:** the same-epoch resume in `publisher_start`, the record of every verified
proof, `network_route_resume`, the startup withdrawal, the new `publisher_status` fields, the
registration read from the connector's current run, the follow tick's new branches, and the
manager universe's entrypoint clearing the mark at every start (web tree). Held by unit tests on
in-memory journals (`src/publisher.rs`: a verified proof recorded with its lease incarnation and
boot; with every condition true a start resumes, journaled with the original proof's identity, with
no proof or an expired one; each condition false is refused under its name, through the start too;
another acquisition needs a new proof; the startup selection of what is present without
entitlement; the registration parsed from a run's lines. `src/network.rs`: the resume's refusals, in
order, from the journal and from the kernel's routes, through the operation itself for those the
journal alone decides; a gateway compared whole; a verified resume replayed under its ID, repeating
nothing) and by `tests/check-publisher-follow-script.py` against a stubbed CLI. **What only the
laboratory proves** (a leg kept with the laboratory's evidence, not in this tree): the
public page back with no workstation action after the active manager's replica is stopped and
started through PodMesh, after its PodMesh service restarts, and what a reboot of its host does;
the route and alias actually re-made in a restarted carrier's namespace; `_SYSTEMD_INVOCATION_ID`
carried by `cloudflared`'s lines; the startup withdrawal of a connector whose lease lapsed while the
daemon was down.

**2026-09-18, the review's fixes, also without the laboratory:** the rotate tool's supersession and
barrier (below; `tests/test_ha_rotate_barrier.py`); the route resumed in place, a failed or
interrupted resume keeping its row (the review's probe, now a regression test); "could not be read"
reported apart from a wrong value, the tick stopping on a positive mismatch at once and on unknowns
only three ticks in a row; a failed start backed off, 20 s doubling to 300 s; an unreadable proof
file started without, never a crash; renewals of this boot recognised by the boot's identity; every
gateway compared as a whole word; the entrypoint clearing the mark at an overriding path too. The
review's probes of the resume, driven through the real activation operations, are kept as
regression tests (`src/publisher.rs`, `review_regressions`).

**When the page comes back.** Only while the lease lives: the tick renews only an eligible host,
and a host whose replica is down is not eligible, so the lease burns from its last renewal. With
`renew_below` 900 and a lease of 3600 s, the replica must return before the lease ends: within about
890 s to 3600 s of its stop, depending on where the lease stood at the stop. Later, the lease has
lapsed, nothing resumes, and the gate must rotate again.

**The exposure the resume added, and how it is closed (review of 2026-09-18).** Before V3-1, a
holder whose replica was down was not eligible, its tick stopped renewing, and its lease lapsed
before a barrier counted from a rotation elsewhere. With V3-1, if that replica returns while the
lease still lives and nobody told the host of the rotation — `rotate` only printed the permit — the
route resume makes it eligible again, its tick renews under its follow mandate and resumes its
connector, its lease runs past the barrier, and the new holder starts at the barrier: two connectors
on one tunnel. Closed in the rotate tool: to another holder, it delivers the supersession to the
previous holder (`--previous-host`) before any proof is made, when that host is reachable, and says
so in its report (`supersession`); a superseded host renews nothing and resumes nothing. When it
cannot, the barrier covers what that host may still renew by itself: `tools/arm-publisher-follow.py`
records every follow mandate it issues (host, `not_after`, no secret) in the resource's ledger before
installing it, and `rotate` makes `eligible_after` no earlier than that `not_after` plus the lease
plus the margin while such a mandate stands (`barrier_covers_follow_mandate`, and the proof's
`barrier_basis`); a record it cannot read is refused (`follow_mandate_unknown`) before the gate
moves. What the ledger cannot see — a mandate issued before the record existed, or from another
workstation — the operator states (`--follow-mandate-not-after`). The cost: a takeover of an
unreachable holder under a standing mandate waits for that mandate's end. The real closure, a
renewal the old holder cannot grant itself, is a later lot.

**The barrier, as it is (second review of 2026-09-18).** A rotation's `eligible_after`, which the
node enforces for every method (`first`, `same_holder`, `lease_barrier`, `fence_receipt`: a
document is refused until it, on the node's clock, and accepted from it; an expired one stays
refused), is the latest of:

- for a rotation to another holder, now plus the lease plus the margin, where the lease is the
  longer of this call's and the one the previous holder renews under (the ledger's policy before
  this call) — a shorter `--lease` on the call does not shorten what the previous holder holds;
- when that holder was not told of the rotation, its recorded follow mandate's `not_after` plus the
  same lease and margin;
- **the barrier carried from the gate's current epoch**, whatever the method, the same holder
  included: the recorded proof's `eligible_after`, or, when that proof was upgraded to
  `fence_receipt` (its previous holder fenced), only what it carried itself. A holder that was
  never told does not become harmless because the role moved again, nor because the operator
  rotated to the same host again (which `--refresh` does): the proof records it as
  `carried_eligible_after`, and `attest-fence` makes a fence receipt eligible at that barrier, not
  at once — the fence says nothing about the earlier holder.

The proof expires an hour after its barrier, never before (`expires_at` = the later of now and
`eligible_after`, plus 3600): a proof that died before it could be used would leave only a
same-holder rotation. The rotation and a proof as long as it could need to be (as if the
supersession will not be delivered) are recorded in the ledger as soon as the gate moves, before
anything that can fail — a dropped SSH session to the previous holder is reported
(`supersession.delivered` false), never raised; a failure after that leaves the rotation
`gate_moved` with its proof, and a rerun carries it. The ledger is written under its lock
(`ledger_locked` when another rotation, record or replication run holds it for 30 s).

Refusals, all before the gate moves: `no_proof_for_current_epoch` — the ledger holds no proof for
the gate's current epoch (a rotation made from another workstation, or one interrupted before this
record existed), so its barrier cannot be carried; the operator recovers by stating it
(`--barrier-not-before <unix time>`: that proof's `eligible_after`, or the latest second any earlier
holder may still renew by itself plus its lease and margin), a universe activated by `activate`
carrying none; `previous_host_mismatch` — the host named by `--previous-host` is not the gate's
previous holder; `follow_mandate_unknown` — the ledger's record of a follow mandate cannot be read.
When the supersession is delivered, the same visit stops the previous holder's connector
(`publisher_stop`) and reports its unit as observed after (`supersession.previous_connector`).

A follow mandate is recorded before it is installed, keeping the larger of the previous and the new
`not_after` until the host's copy is read back, so that an installation that fails never leaves the
record saying less than what the host may still hold.

Third review (2026-09-18):

- **The nodes first.** Only a node built after 2026-09-18 holds every method until `eligible_after`;
  an older one holds a lease barrier only, and would start a same-holder or fence-receipt document
  carrying a barrier at once. The nodes are deployed before the first use of this tool;
  `tests/podmesh_manager_lab.py` (`prove_takeover`) waits for `eligible_after` whatever the method.
- **`activate` from epoch 0 only.** An activation after a rotation made a fresh epoch with no proof,
  and the rotation that followed took it as barrier-free; `activate` is refused
  (`activate_after_rotation`) unless the gate is at epoch 0, and `rotate` exempts from
  `no_proof_for_current_epoch` only an activation recorded as made from epoch 0 (or, before that
  was recorded, the one that made epoch 1).
- **A fence receipt bound to its transition.** `attest-fence` accepts a fence answer only when it shows
  the previous holder overtaken by the current epoch or later for this resource — in `fenced`, or in
  `unentitled_detail`, the fence's per-resource account of what it found (the epoch that overtook the
  lease, if any) — and refuses any other as `stale_fence_receipt`: an earlier fence, or a lease that
  merely lapsed, says nothing about this rotation.
- **A gate recovery is a recorded epoch.** When `tools/arm-publisher-follow.py` moves the gate past the
  highest epoch a host has seen (`gate-recovery`), it records that epoch's proof in the ledger under
  its lock: a `gate_recovery` record naming no holder, its barrier now plus the longest lease and
  margin anyone may hold and never earlier than what the latest recorded proof carried. The rotation
  that follows carries it as `lease_barrier` instead of refusing. The arming tool also forwards a
  barrier the operator states (`--barrier-not-before`, or `PODMESH_FOLLOW_BARRIER_NOT_BEFORE`).
- **A stated barrier is a time.** `--barrier-not-before` must be greater than 0 (else
  `barrier_not_before_invalid`); it is recorded in the rotation, in the proof (`stated_barrier`,
  `barrier_basis`) and in the report, carried through a fence receipt like any earlier barrier, and
  the report warns when it is earlier than now plus the lease and the margin.
- **Released only when stopped.** The barrier pushed for an untold holder is dropped only when the
  supersession was delivered AND the previous holder's connector was seen stopped, or no publisher
  is declared there; a stop refused, or a unit not observed, keeps the pushed proof.
- **One writer.** `activate`, `rotate`, `attest-fence`, `cycle`, `takeover`, a mandate record and a
  recovery record each hold the resource's ledger lock for their whole run — `rotate` across its SSH
  calls included — and wait up to 30 s for it (a mandate or recovery record up to ten minutes). The
  follow tick's state file is merged under a lock of its own, each resource through its own
  temporary file.

**What V3-1 does not do.** It never changes the holder, never acquires, never mints or extends a
proof, and never renews past the mandate's `not_after`. After a reboot of the holder's host
nothing resumes: the proof was verified during another boot (`boot_changed`) and the ledger's /32
is withdrawn at boot and never re-applied (`network_reapply`); the entitlement has to be decided
again, by the gate today.

**Known limit: a clock stepped backward.** Expiry is judged on the wall clock. A host whose clock
steps back past a lapse sees its lease live again; nothing was retaken, so the resume's conditions
all hold, and the tick resumes as if the lease had never lapsed. The takeover margin is the stated
clock-skew budget; a step larger than it is not covered here (a later lot: a monotonic record of
observed lapses, or renewal decided by the majority).

**2026-09-18, V3-2, quorum certificates on the node — built and tested without the laboratory.** A
policy may name its authority as a quorum of replica keys (`authority_quorum`: n public keys under
stable key ids, a threshold k with 2k > n); the node then accepts an exclusive decision — the
acquisition of an epoch, a same-holder re-issue, a supersession, the takeover proof at
`publisher_start` — only as a certificate that k distinct keys of that quorum signed under the
policy's digest, and verifies it itself, offline. The full rule, the payload and the named refusals
are in `LOCAL-API.md`, "Quorum certificates". What stays as it was: every single-key policy, its
permits and its signed documents (the tool's document is a verbatim test vector), so no deployed
node or tool changes behaviour. What the unit tests prove (`src/signing.rs`, `src/activation.rs`,
`src/publisher.rs`): k-of-n accepted at k and above and refused at k-1; a key counted once; a key
of another policy, a certificate of another policy, resource, holder, boot or epoch refused; every
single flipped byte of a payload or a signature refused; the screen never moving backwards, and
moving only on a verified certificate, over a seeded sequence of 400 mixed attempts; the authority
set changing only under a certificate of the policy in place or the operator's re-declaration naming
its digest. What it does not do yet: produce certificates (the manager's promise rule, signed votes
and resident operations are the next lots), extend a lease by majority, or run on a laboratory host.

**2026-09-18, V3-2's decisions, taken for the operator:**
- **Changes between single keys keep today's behaviour.** Moving a policy from one single
  `authority_key` to another, or from no key to one, needs no certificate and no named digest, so
  that the tools deployed today keep working. It is recorded (`activation_policy_changes`,
  `redeclared_single_key`) and moves the serial. **V3-10, retiring the gate, closes it:** once no
  resource is under a single key, every change of an authority set is a change to or from a quorum.
- **The holder's boot in every certificate is intended.** A certificate entitles one boot of its
  holder; after a reboot the majority decides again (the V3-1 rule, `boot_changed`).
- **A strict majority at declaration.** A threshold with 2k <= n is refused.
- **A monotonic policy serial.** The authority set carries a serial its digest covers; a
  policy-change certificate binds `from_serial` (the current one) and `new_serial` (one more), and
  the operator's re-declaration moves it too. A live change can no longer be replayed on a host that
  has moved on since, whether or not it ever applied it. Existing policies start at 0. A host joining
  late is declared at its peers' serial (`authority_serial`) so that it shares their digest.
- **The barrier on the certificate path is kept:** a certificate's `eligible_after` holds the
  acquisition itself, whatever the method.

**2026-09-19, V3-5, the manager decides: built and tested without the laboratory.** The replicas propose,
vote and assemble certificates themselves (web tree, `experiments/manager-resident`, "The manager
decides"). Each voter checks a proposal against its own view before it votes. The view is the current
epoch and holder its store's votes prove, and the barrier rules above:

- `same_holder` carries the barrier;
- `lease_barrier` to another holder also covers the current certificate's expiry, the renewal bound of
  the follow mandates, and the proposal's issue, each plus the lease and the margin;
- `fence_receipt` is refused, because a receipt is the previous holder node's unsigned answer, which no
  replica can verify.

A host-side tick delivers the certificates through the node's local socket
(`packaging/podmesh-decision-follow`, `DECISION-FOLLOW.md`), reading them through a new read-only door,
`manager_decision`. The certificate names the holder's host: `activation_acquire`, then `publisher_start`
with it as the takeover proof when a publisher is declared, eligible and idle. On every other host:
`activation_supersede` above the screen. No listener is added anywhere.

What the local end-to-end test shows, with three residents and three real nodes: an epoch and a
same-holder re-issue decided by two replicas of three and applied by the nodes; a rotation held until its
barrier; a minority's vote deciding nothing and its hand-made certificate refused; replayed and late
certificates refused by the screen.

**Before any V3-5 rotation to another holder (review of V3-5, findings 1 and 2).** The replicas cannot
see a follow mandate, and the node does not re-check one against a quorum certificate. So, until the
majority extends leases (V3-6):

- the V3-1 follow mandates (`packaging/podmesh-publisher-follow`, `tools/arm-publisher-follow.py`) are
  frozen (no new `--refresh`) or removed on every host;
- every replica's `renewal_not_after` for the resource is set to the latest `not_after` of those
  mandates;
- a voter refuses a `lease_barrier` that changes holder while `renewal_not_after` is 0
  (`renewal_unbounded`), or earlier than the current proof's expiry or the proposal's issue
  (`renewal_bound_too_early`): a mandate may still stand past it;
- a resource that leaves the gate carries in its baseline the gate's last proof's `expires_at`. Until a
  certificate the replicas assembled passes the baseline, that is the current expiry, and a change of
  holder is refused while it is absent (`baseline_expiry_unknown`).

A same-holder decision needs none of this: it does not change who may run.

What only the laboratory shows: the relay of `manager_decision` into a running manager universe,
`publisher_start` under a certificate on a real connector, and the campaign with the workstation off. The
majority does not extend leases yet (V3-6). Until it does, a rotation away from a holder that may still
renew by itself waits for the follow mandates' `not_after`, and declaring a host lost stays the
operator's.
