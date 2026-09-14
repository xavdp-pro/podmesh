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
  `[{pool, via}]`) records the declaration and makes it effective: the bridge network with the
  local pool as its subnet and DNS disabled, and one route per peer pool. It verifies from outside
  (`podman network inspect`, `ip route`) and refuses on any overlap with an existing route. A host
  carries at most one declaration.
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
- `network_status` (read-only): declaration, allocations, published routes, and the effective
  state read from Podman and the kernel; an observation that cannot be made is `unknown`.
- `inspect`/`observe` report the requested profile from the labels and the effective network from
  Podman; a container without the labels is reported `unknown`, never `isolated`.

Not carried yet by the managed profile, and refused rather than assumed: `clone`,
`recovery_point_restore`, `recovery_point_promote`, `migration_restore` — they still create
isolated containers and say so in their answers. Carrying the profile through them is the next
step of this contract, after the replicated manager has run on the managed network.

## Failure and cleanup states

- A declaration whose bridge or routes cannot be verified is recorded `failed` with what was
  observed; nothing is retried on its own, and `network_undeclare` cleans what exists.
- A managed `create` whose container could not be observed on the bridge with the allocated
  address releases the allocation and refuses; a partial container is removed.
- A published route that is not effective after `ip route` is reported `unverified`, and a
  withdrawal that leaves a route effective is a failure, never a success.
- Unknown is unknown: an `ip route` or `podman network inspect` that cannot be run makes the
  effective state `unknown`, and any operation that needs it refuses.

## What this contract does not decide

WireGuard configuration, DNS publication, the agent's authenticated path to a manager's control
API over the managed network (the resident exposes none today; until it does, the only writer
inside a manager universe is its entrypoint), nested Podman and internal containers, and
production fencing of a route. Each is its own contract.

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

**Known deviation, stated:** Podman's network firewall source-NATs traffic leaving the bridge's
subnet, so a universe reaching another host's universe is seen there with the host's address.
Identity between manager replicas is the HMAC pair key, never the address; removing the NAT
for the logical prefix is a later step of this contract.
