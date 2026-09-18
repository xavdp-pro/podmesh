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
entrypoint removes both of the mark's paths at every start, before the origin starts, and the
replica claims no role until PodMesh writes the mark again at `publisher_start` (V3-1, web tree). The manager's web
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
  binding alone, and the answer says `signed: false`. The agent's `previous` narrative is
  recorded beside it and decides nothing.
  **Same-epoch resume (V3-1).** A proof that verifies is recorded with the lease incarnation it was
  verified against (epoch, generation, `acquired_at`), this host's boot, and the policy's authority
  and key. A later start without a proof, or with one refused (expired, say), resumes under that
  record — method `resume_same_epoch`, journaled as the event `takeover_resumed` with the original
  proof's identity (the verifying operation, its method, issue, expiry and signature) — only while
  every one of these holds, and is refused naming the first that does not: the lease is held
  (`no_lease`), here (`lease_held_elsewhere`), live (`lease_expired`) and unsuperseded
  (`lease_superseded`); a proof was verified for the resource (`no_verified_proof`); for this very
  epoch (`epoch_changed`); under the same generation (`generation_changed`) and the same
  acquisition (`lease_reacquired`: a lapsed lease of this host's retaken keeps its generation and
  epoch, not its `acquired_at`); during this boot (`boot_changed`); under the same authority and key
  (`authority_changed`). Each is a condition under which a connector that never stopped is already
  allowed to continue — the reconciliation leaves it alone only under a live, unsuperseded lease held
  here at the transition's epoch; across a lapse it is withdrawn; it does not outlive its boot — so
  a restart under them grants nothing that continuing did not. Anything else needs a valid new proof.
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
  gate by name (`gates`: `lease`, `policy`, `service_address`, `credential`); the epoch the mark
  names (`active_manager_mark_epoch`, read in the origin's order, null when absent); whether the
  origin answers ready at the lease's epoch with this logical manager and the carrier's replica
  (`origin_ready_at_lease_epoch`); the unit's current run (`unit.invocation_id`) and whether the
  connector registered in that run (`connector_registered`; `connector_id` is read from that run's
  journal only, never from an earlier run's lines); the lease's generation and `acquired_at`; and
  what a start without a proof would decide now (`takeover_resume`: `possible`, or the refusal's
  name).

The service address has its own resume, `network_route_resume` (`UNIVERSE-NETWORK-CONTRACT.md`
for the route itself, `LOCAL-API.md` for the operation): the recorded exclusive route and alias of a
role this host still holds, put back after the carrier lost them — the dead row withdrawn, then
published again with the recorded ip, via and resource through every check of a publication — only
while the lease is live, held here, unsuperseded and was acquired or renewed during this boot, a
universe runs at `via`, and the kernel holds no other route for the address.

## Takeover, in order

1. Rotate the epoch through the external gate.
2. Deliver the supersession and fence the previous active manager (or, unreachable, wait its
   lease plus margin: the timer on it withdraws its connector, its mark, its alias and its route
   on its own clock — `packaging/podmesh-fence`).
3. Observe on the previous active manager, when reachable, the connector stopped and the
   address gone.
4. Publish the service address on the new active manager (`network_route_publish`, exclusive).
   The barrier (the previous lease plus the margin, counted from the rotation, since the
   previous holder may have renewed right up to it) can outlast the new holder's own lease:
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

**What V3-1 does not do.** It never changes the holder, never acquires, never mints or extends a
proof, and never renews past the mandate's `not_after`. After a reboot of the holder's host
nothing resumes: the proof was verified during another boot (`boot_changed`) and the ledger's /32
is withdrawn at boot and never re-applied (`network_reapply`); the entitlement has to be decided
again, by the gate today. The takeover at the barrier stays unsound while the old holder's mandate
renews: the barrier (`rotation + lease + margin`) ignores `not_after`, and a holder cut from the
gate but not from its own address keeps renewing its lease in its own journal; that closes with
renewal the old holder cannot grant itself (a later lot). The resume adds no exposure of its own:
it brings back, on the host whose own journal still entitles it, the connector that would have been
left running had its carrier never stopped, and that connector carries the same hazard today.
