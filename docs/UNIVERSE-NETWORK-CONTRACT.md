# The universe network contract

Status: **contract written 2026-09-14 on the operator's decision** (`OPERATOR-DECISION-UNIVERSE-NETWORK`)
and Codex's direction for M-U2; **the smallest implementation follows it step by step** — what is
built is stated in "What is built" at the end, and nothing above it is a claim about code.

Networking is part of the normal universe contract. `--network=none` was the contract of the first
lots and of every proof so far; it is now the **isolated** profile, kept as it is, explicit and
testable. The **managed** profile is the normal one. Nothing in PodMesh runs on its own: every
network effect below is a journaled operation the agent asks for, verified from outside.

## Identities that stay distinct

| identity | what it names | never derived from |
| --- | --- | --- |
| host UUID | the machine PodMesh runs on | an address |
| universe UUID | the workload; its logical address is bound to it | its placement |
| network UUID | one logical `/16` and its allocation pools | a host |
| logical manager UUID / replica UUID / boot UUID / epoch and grant | the manager's identities (`UNIVERSE-HIGH-AVAILABILITY.md`) | any of the above |

A universe's IP is **allocated to its UUID**, from the pool of the host that allocated it, and it
keeps that IP when it moves. Allocation ownership and current placement are two facts.

## The logical network

- One `/16` per network UUID, declared by the operator; nothing here chooses a prefix.
- One `/24` **allocation pool per host**, declared on that host (`network_declare`) with the
  pools of the other hosts and the address each is reached through. The host's own pool is the
  subnet of one Podman bridge network created for it; the other pools are routes to their hosts.
- A universe under the managed profile receives **one** address from the local pool, recorded in
  `network_allocations` and carried by the container as labels; the effective address is read
  back from Podman and reported beside the requested one.
- **Exactly one active announcement per universe IP.** On a host where a universe is not placed,
  its address is reachable only through a `/32` route published by `network_route_publish`,
  which refuses while any route for that address is already effective, refuses while the
  universe is allocated on this host, and is undone by `network_route_withdraw`. Withdrawal is
  verified from outside before a new publication is accepted anywhere: that verification is the
  agent's, across hosts, and the refusal is each host's.
- Inter-host transport is the hosts' existing routable network. WireGuard is an optional
  transport when hosts cannot or should not talk directly; it changes no identity and no
  allocation, and it is not built here.
- Bootstrap never depends on a DNS the manager would serve: peers are explicit authenticated
  endpoints, and the managed bridge is created with DNS disabled.

## Operations

All are journaled with the same contract as every other operation (one request per operation
ID, verified replays as history, interrupted attempts re-evaluated) and all carry
`authorization_ref` as provenance.

- `network_declare` (host-wide; `network_uuid`, `prefix`, `pool`, optional `peer_pools`
  `[{pool, via}]`, optional `nat_exemption`) records the declaration and makes it effective: the
  bridge network with the local pool as its subnet and DNS disabled, one route per peer pool, and
  the nftables table that keeps Podman's source NAT off traffic inside the prefix. It verifies
  from outside (`podman network inspect`, `ip route`, `nft list tables`) and refuses on any
  overlap with an existing route, over an existing bridge or table. A host carries at most one
  declaration. `nat_exemption` chooses how the source NAT is kept off: **`null-snat`** (the
  default) — a null source NAT of the local pool's traffic to the prefix, each address to itself,
  in a nat chain evaluated before netavark's, so the kernel holds a binding and netavark's
  masquerade does nothing; connection tracking is kept; **`notrack`** — prefix-to-prefix traffic
  left untracked in both directions, which keeps the NAT off but removes connection tracking
  from that traffic (a stateful firewall dropping untracked traffic then blocks it: measured);
  **`none`** — Podman's NAT left in place, a universe seen elsewhere as its host. Traffic between
  a universe and a host address, or leaving the prefix, keeps Podman's NAT under every backend.
- `network_undeclare` (host-wide; `network_uuid`) refuses while any allocation or published route
  remains, removes the peer routes and the bridge, and verifies their absence.
- `create` (`network_profile`: **required**, `isolated` or `managed`). Managed allocates the next
  free address of the local pool to the universe UUID, creates the container on the bridge with
  that address, and reports `network: {profile, requested: {network, ip}, effective: {…}}`.
  Isolated is today's `--network=none`, reported as such. Absent is refused: a universe's
  network is a decision, not a default that depends on host state.
- `delete` releases the allocation of a managed universe when the container is gone.
- `network_route_publish` / `network_route_withdraw` (`universe_uuid`, `ip`, `via`): the `/32`
  that follows a universe placed elsewhere, with the refusals above; both verified from `ip route`.
  With `exclusive_resource`, the route is the exclusive effect of a role — the service address
  of a logical manager, whose three replicas all run — and is published only by the host holding
  a live, unsuperseded activation lease on that resource under the epoch gate
  (`UNIVERSE-HIGH-AVAILABILITY.md`); the self-fence withdraws it once the lease is gone, so the
  old governor's withdrawal precedes any new publication the agent asks for. An exclusive route
  must point at a running universe of this host — the governor's replica — which then **carries
  the address** as an alias inside its own network namespace, added before the route and
  verified from inside, withdrawn with the route (by the fence or by `network_route_withdraw`);
  the replica listens on every address of its universe (bind `0.0.0.0`) so that it answers there.
  On the other hosts the agent publishes the plain route that follows the role (the address via
  the governor's host), and withdraws it before the role moves.
- `network_status` (read-only): declaration, allocations, published routes, and the effective
  state read from Podman and the kernel; an observation that cannot be made is `unknown`.
- `inspect`/`observe` report the requested profile from the labels and the effective network from
  Podman; a container without the labels is reported `unknown`, never `isolated`.

`recovery_point_restore` always creates its quarantined copy isolated — a quarantine is not on
the network — and reports the source's network from the labels the manifest recorded
(`source_network`: profile, address, network UUID). `recovery_point_promote` takes
`network_profile`, required as for `create`, and under the managed profile the `network_address`
to put the universe back at: the address allocated to its UUID, which the restore reported. A
released allocation is history, so a universe deleted and put back is allocated again.
Not carried yet by the managed profile, and refused rather than assumed: `clone` and
`migration_restore` — they still create isolated containers and say so in their answers.

## Failure and cleanup states

- **No effect outlives its record.** Every kernel or Podman mutation this contract makes —
  bridge, peer route, nftables table, /32 route, address alias — is recorded in the effects
  ledger, with what verifies and undoes it, and committed before `ip`, `podman` or `nft` run;
  its state follows the mutation: `applying` until verified from outside, `effective`, then
  `removing` until the removal is verified. A route's own record is written first, `applying`,
  so the fence has a durable target from the first moment.
- **Compensation.** A declaration or publication that fails after an effect undoes everything
  made so far, last first, verifies each removal, and reports the original error beside the
  compensation. A route fully compensated loses its record; a declaration that failed keeps its
  row `failed` with the evidence until nothing of it remains.
- **Reconciliation** runs at daemon startup (its report in the journal), before every network
  mutation (returned as `reconciliation_before`) and at the start of every fence: it undoes,
  whole and never finishes, every route or declaration whose making or unmaking was
  interrupted and every lone effect that is not effective; releases allocations whose container
  is gone; reports drift — an effective effect the kernel no longer shows — and never touches
  it. While anything remains after reconciliation, every network mutation refuses and says
  what remains; `network_status` shows the ledger and `incomplete_effects`.
- A managed `create` whose container could not be observed on the bridge with the allocated
  address releases the allocation and refuses; a partial container is removed.
- Unknown is unknown: an `ip route`, `podman network inspect` or `nft` that cannot be run makes
  the effective state `unknown`, and any operation that needs it refuses; a removal that cannot
  be verified leaves its record `removing` with what was observed.
- The laboratory injects faults at these points (`PODMESH_FAULT`, never set by the packaged
  units): a storage failure at the point, or a crash there; `tests/check-network-crash-safety.py`
  is the contract's proof.

## What this contract does not decide

WireGuard configuration, DNS publication, nested Podman and internal containers, and
production fencing of a route. Each is its own contract. The agent's path to a manager's
control socket is not a network path: it is the typed door of `LOCAL-API.md` (`manager_status`,
`manager_observe`), through the universe's own namespaces on its host.

## What is built

**Step 2 (2026-09-14):** `src/network.rs` and the profile in `create`. `network_profile` is
required on `create` (`isolated` or `managed`); a managed universe receives the next free
address of the host's pool, allocated to its UUID and carried by the container as labels;
`delete` releases it; observations report the requested profile and the effective network.
`network_declare` creates the bridge (`podmesh-managed`, DNS disabled, subnet = the local
pool) and one route per peer pool, verifies both from outside, refuses any overlap with an
existing route and a second declaration; `network_undeclare` refuses while a live allocation or
a published route remains, removes the routes and the bridge and verifies their absence;
`network_route_publish`/`network_route_withdraw` keep exactly one announcement per address,
verified from `ip route`; `network_status` reads the effective state and reports `null` for
what could not be observed. Clone, restore, promote and migration restore still create
isolated containers and say so. A first version made every address unique across released
allocations, so a released address could never be reused; the table is rebuilt once.

**Step 3 (2026-09-14):** `tests/check-network-managed.py` on lab-a, as root, verified from the
host — 17 checks passed: three refusals of a create without or with an unknown profile and
without a declaration; three refusals of bad pools (outside the prefix, a host address, an
overlap with the host's own LAN); a declaration effective with the bridge and two peer routes;
a second declaration refused; a managed universe created, started and reached from the host at
its allocated address, with Podman and the labels agreeing; a second universe with a distinct
address; a `/32` published and withdrawn, a second announcement and a route outside the prefix
refused, undeclare refused while anything remains; and cleanup that left the host's routes and
networks byte-identical to its initial state.

**Step 4 (2026-09-14):** `create` may name the address it wants inside the local pool
(`network_address`), so that universes whose configurations name each other can exist; and
`tests/check-manager-replicas-managed.py` ran **three manager replicas of one logical manager
concurrently on the managed network across the three lab hosts** — one bridge per pool, two peer
routes per host, each replica at its declared address, explicit authenticated endpoints, no DNS —
and proved from outside, with all three running, that their facts converged: three boot facts,
one per owned scope, byte-identical sets on every replica, authenticated import receipts from
both peers on each, exchange audit rows present, integrity ok; cleanup returned every host's
routes and networks to their initial state. This is campaign 6's data path inside universes.

**Step 5 (2026-09-14), the governor role with all three running:** `network_route_publish`
takes an optional `exclusive_resource`: the route is then the exclusive effect of a role, and is
accepted only from the host holding a live, unsuperseded activation lease on that resource
under the epoch gate — refused under no policy, then for the lease gate's four reasons — and
recorded with the resource; `activation_fence` withdraws every exclusive route whose resource
this host no longer holds, verified from the kernel, and reports `routes_withdrawn`. The tool
gained `rotate`, which rotates the epoch of a resource to a host and acquires it there without
promoting or starting anything, and prints the permit for the other hosts' supersession. The
resource is the logical manager's UUID; the exclusive effect is the `/32` of its service
address (`10.86.0.100` in the lab, inside the prefix and outside every pool). Answering traffic
at that address inside the replica is not built: what is proven is the announcement.
`tests/check-manager-governor-managed.py` on the three hosts: step 4 reproduced; no host may
publish before a policy exists; after `rotate` to lab-a (epoch 1) only lab-a publishes, lab-b
is refused; `rotate` to lab-b (epoch 2), lab-a and lab-c superseded, lab-a's fence **withdrew
the service route before lab-b published it**, the kernels showed no announcement in between
and exactly one at every observed moment; lab-a refused to publish again under its superseded
lease; a forged epoch-1 permit bound to lab-c was refused, and lab-c refused to publish without
the role; **all three replicas ran throughout**, and their facts were still converged after the
takeover. Each of the three rules was removed in turn and the suite rerun on the hosts: without
the policy precondition, the first refusal was accepted; without the lease gate, lab-b's
publication was accepted; without the fence's withdrawal, the fence reported no route withdrawn
and the check on it failed — each red at its own case, then green again on the reference build.
Two things the suite learned: the gate must be durable across runs (the resource is a fixed
UUID and every host keeps the highest epoch it has seen, so a fresh gate per run is refused as
superseded by the second run — the laboratory's precondition, met the hard way; a gate found
behind the hosts is brought forward by explicit transfers and the report says so), and a store
copied out of a running replica is a moving target (SQLite's WAL files come, go and grow under
the copier), so a copy the copier could not complete is retried rather than trusted.

**Step 6 (2026-09-14), the refusals and the reconnection:** in the same suite, a fourth
universe asking for a replica's address was refused (no address is allocated twice); lab-c's
replica was stopped, the two others stayed converged, and back it appended a fourth boot fact
that reached all three, verified from `podman cp` copies by the attested inspector. Cleanup
returned every host's routes and networks to their initial state. Not shown: a real partition
(the loss here is a stop, not a cut), a host loss, an agent path to the control API, and the
replica actually serving at the service address.

**Step 7 (2026-09-15), a recovery point combined with the running replica set:** `promote`
carries the managed profile (above), the restore reports the source's network, and a released
allocation no longer blocks the same universe from being allocated again (a second one-time
rebuild of the allocations table: only live allocations are unique, per address and per
universe). `tests/check-manager-recovery-managed.py` on the three hosts: three replicas
converged with the governor announced on lab-a; lab-c's replica stopped through the typed stop,
a recovery point prepared from it — its manifest recording the managed address — and the
replica **deleted**, its address released; the two others moved on to a fourth fact it never
saw, the announcement unmoved; the point restored on lab-c into quarantine (isolated, the
source's managed address reported), a promotion at an address outside the host's pool refused,
then promoted into the replica's own identity at its address under a lease, started, back at
that address from Podman; it imported the fact it had missed from both peers, appended its own
boot fact, and the three converged on five facts, the governor unchanged throughout; cleanup
returned the hosts to their initial state. Not shown: a host loss (the rescue is on the same
host, a replica's address living in its host's pool), a real partition, a signed manifest.
The requirement of a profile on `promote` was removed and the single-host promote suite went
red at its refusal; the reference build passed it again, with the network, two-host recovery,
HA-tool, three-host, epoch, fence, restore, retention, governor and recovery suites. One
defect of the two-host test helper surfaced on the way: each refusal snapshot hashed every
archive both delivery directories held, so a suite under a 20-second lease lapsed on its own
bookkeeping once other suites' leftovers reached gigabytes; snapshots now compare a stat
fingerprint and transfers hash only their own documents.

**The replica answers at the service address (2026-09-15):** the exclusive route gives the
governor's replica the service address as an alias inside its network namespace (`nsenter -n`
with the host's `ip`, nothing required inside the universe), verified from inside, withdrawn
with the route by the fence and by `network_route_withdraw`; refused when nothing runs at `via`
on this host. The replicas' configurations now listen on every address (the generator's
default; a replica bound to its own address alone did not answer at the alias, which the first
attempt found). `tests/check-manager-service-address.py` on the three hosts: with nothing
announced, no replica carries the address and a connection to it fails at once; the governor
on lab-a carries it — read inside each universe's namespace, on lab-a's only — and a TCP
connection to the service address and port from lab-b and from lab-c is accepted, the
resident's `peak_incoming` counting it; the role moves to lab-b with all three running: lab-a's
fence withdraws the route and the alias, the follow routes are withdrawn, a connection fails
everywhere, lab-b publishes and its replica carries the address, the connection from lab-a and
lab-c is accepted there, lab-a's replica carries nothing; a plain withdrawal takes the alias
with the route. Mutations: no alias on publication, and no alias removal by the fence, each red
at its own check. Not shown: an authenticated exchange at the service address (the listener
accepts; the protocol then needs a peer key), a real partition, a host loss.

**A real partition (2026-09-15):** `tests/check-manager-partition.py` cuts the governor's host
from the two others with an nftables table of its own (every packet dropped at prerouting and
output, both directions; a dead man's switch on the host removes it after ten minutes whatever
happens to the suite) while the agent still reaches every host. Measured on the three hosts:
the two connected replicas converged on a fourth fact the cut replica never saw (it kept
running with three facts and kept carrying the service address); the service address was
unreachable from the connected side; the agent moved the role to lab-b — rotation, supersession
delivered to the cut host, its fence withdrawing route and alias on request — and lab-c reached
the service address on lab-b; on reconnection the cut replica converged as a simple replica and
reached the service address through the follow route. What it does not show, and says: a
partition that also cuts the agent from the governor's host — that host keeps its alias until an
agent reaches it, since the self-fence is an operation and PodMesh runs no timer; whether a
timer may run it is the operator's decision (`UNIVERSE-HIGH-AVAILABILITY.md`). The first
attempt hooked `input` only and the forwarded connections crossed the "cut"; the suite records
that.

**Crash and storage-failure safety (2026-09-15, Codex's finding B1):** the effects ledger,
compensation and reconciliation above, built after the review found that a route, an address,
a bridge or a table could survive a crash or a storage failure without a record for the fence
to find. `tests/check-network-crash-safety.py` on lab-a, restarting the transient daemon with
each injected fault: a publication failing after the alias, after the route and at the final
record — refused, compensated, nothing left; a publication crashing after the route — the
route and the address survived the crash unowned, the restart's reconciliation undid both and
the fence then had nothing to find; a withdrawal crashing after the route; a declaration
crashing after the bridge; a declaration failing after a peer route (recorded `failed` with
its evidence, cleared once nothing remained, a new declaration effective); an undeclaration
crashing after the table — each finished or undone by the restart, the host as before.

**The source NAT, removed inside the prefix (2026-09-15):** Podman's network firewall
source-NATs traffic leaving the bridge's subnet, so a universe reaching another host's universe
was seen there with the host's address (measured on 2026-09-14). The declaration creates an
nftables table of PodMesh's own (`inet podmesh-managed`), verified from `nft list tables`,
reported by `network_status` as `nat_exemption` (with its backend), refused over an existing
table, removed and verified gone by the undeclaration. The first build used `notrack`; Codex's
review (finding B3) named its consequence — no connection tracking on that traffic — and the
default is now the narrowest rule netavark leaves room for, the null source NAT above, with
`notrack` selectable and `none` explicit. `tests/check-network-no-nat.py` (two hosts): the
source universe's own address seen at the destination, both directions.
`tests/check-network-nat-matrix.py` (two hosts, both backends): TCP and UDP exchanges inside
the prefix, both directions, each universe seen with its own address and answered; a stateful
firewall on the destination host dropping untracked and invalid forwarded traffic **lets the
exchange through under `null-snat` and blocks it under `notrack`**; a universe reaching the
other host's address is seen as its host (Podman's NAT outside the prefix kept); a host reaching
the other host's universe is seen as the host; after a fence's reconciliation the table and its
rules are intact with nothing drifted; the undeclaration removes the table for both backends.
Identity between manager replicas stays the HMAC pair key.
