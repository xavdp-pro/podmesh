# Decision follow: the replicas' certificates delivered to this node

Status: **development tree, V3-5 (2026-09-19)**. Shipped disabled, runs nothing without the operator's
mandate. Tested on one machine with real residents and real nodes; not yet run on a laboratory host.

The PodMesh manager's replicas decide an epoch rotation or a same-holder re-issue by majority
(`LOCAL-API.md`, "Quorum certificates"; the manager's side is in the web tree,
`experiments/manager-resident`, "The manager decides"). A decision is a quorum certificate: k signatures
of distinct replica keys over one document. It grants nothing until a PodMesh node verifies it with the
keys of its own policy. This page is the host's side: how a certificate reaches the node, with no
workstation in the path and no new network door on the host.

## What runs

`podmesh-decision-follow.timer` starts `podmesh-decision-follow.service` every 10 seconds. The service
runs `/usr/bin/podmesh-decision-follow` once. For each resource the mandate names, the tick:

1. reads the node's `activation_status`. A resource whose policy names no `authority_quorum` is not
   followed. A certificate would be checked against nothing the replicas signed;
2. reads the resource's current decision from **this host's** manager replica: through the node's door,
   `manager_decision` (read-only, relayed into the manager universe like `manager_status`), or through the
   resident's own control socket, when the resident runs as a host service. The answer is the certificate
   the replica assembles from the votes its store holds, or nothing yet;
3. brings the node to it, with the one request that fits:
   - the certificate names this host, and this host does not hold the lease at its epoch:
     `activation_acquire` with the certificate. The node refuses it before its barrier, and the next tick
     asks again;
   - the certificate names another host, and its epoch is above this node's screen: `activation_supersede`.
     A supersession is delivered to any host, because learning that one is overtaken can only stop things;
   - otherwise nothing;
4. when the mandate says `publisher=1`, a publisher is declared here, the node says it is eligible, and no
   connector runs or is recorded: `publisher_start` with the certificate as its `takeover_proof`.

One JSON line per resource goes to standard output (the journal), naming what was done.

## What it does not do

- **It verifies no signature.** The node does, with its own policy's keys. The node refuses a minority's
  certificate (`below_threshold`), a key counted twice (`duplicate_key`), another policy's certificate, and
  a replayed or late one: its epoch screen moves forward on certificates only, never backwards.
- **It never moves the screen by itself.** It never resynchronises the screen from anything a replica
  reports, and it never renews, proposes or votes.
- **It opens no listener.** It connects to two local Unix sockets: the node's API, and the resident's
  control socket in the host-service form. The unit restricts it to `AF_UNIX` with `IPAddressDeny=any`.
- **It never proposes a takeover.** Who proposes, and which proposals the replicas vote for, is the
  manager's. Declaring a host lost stays the operator's recorded decision.

## Idempotent and fail-closed

Every action is recomputed from the node's own state at every tick:

- a certificate this host already holds the lease for is left alone;
- a supersession at or below the screen is not sent;
- a request that is retried in the same situation uses the same operation ID, so the node's journal
  replays or re-evaluates it rather than repeating it. The situation is the certificate plus this host's
  lease row. A lapse of this host's own lease is a new situation: the tick re-acquires under the live
  certificate, as the node allows the holder;
- a publisher that runs or is recorded is not started again, and a start is keyed on the certificate and
  the lease it was acquired under (generation and `acquired_at`), never on the clock: two ticks before the
  connector is visible send one operation, which the node's journal replays. A start that failed and a
  later one under the same lease are the same operation too, and the node re-evaluates a failed one.

The tick exits 3 and delivers nothing for a resource when it cannot read something:

- the mandate;
- the node's status;
- a policy that names a quorum;
- the replica's decision: an error, a decision for another resource, a certificate without a holder, or
  two decisions certified for one epoch, which is a broken promise the operator settles.

A refusal of the node, such as a barrier not reached yet, is reported, and the next tick retries. One
resource that cannot be read does not stop another's delivery.

## The mandate

`/etc/podmesh/decision-follow-mandate`, root-only, one `key=value` per line:

| Key | Meaning |
| --- | --- |
| `authorization_ref` | the mandate's name, recorded as provenance in every request |
| `manager_universe` | the UUID of this host's manager replica universe: the decision is read through `manager_decision` |
| `resident_socket` | the absolute path of the resident's control socket, for a resident run as a host service |
| `resource` | a resource to follow, one line each, at least one |
| `publisher` | `1` to start the declared publisher with the certificate; `0` by default |

Exactly one of `manager_universe` and `resident_socket` is given. Enabling the timer is the operator's
decision, taken by writing the mandate and enabling `podmesh-decision-follow.timer`.

## Tests

- `tests/check-decision-follow-script.py` runs the tick against a stubbed node and a stubbed resident.
  It holds each branch above, the fail-closed cases, the idempotence, the door, and the absence of any
  listener. No daemon, no host.
- The web tree's `experiments/manager-resident/tests/e2e/decisions-e2e.py` runs the same tick against
  three compiled residents and three real, unprivileged `podmeshd`. It covers readmission through the
  evidence collector, then:
  - an epoch decided by two replicas of three, delivered and accepted: the holder acquires, the others
    are superseded, every screen moves;
  - a same-holder re-issue;
  - a rotation held by the node until its barrier;
  - a barrier-breaking proposal refused by the voters;
  - replayed and late certificates refused by every node;
  - a minority's vote that decides nothing, and a hand-made 1-of-3 certificate refused.

What only the laboratory shows: the door's relay into a running manager universe, `publisher_start` with
a certificate on a real connector, the units under systemd, and all of it with the workstation off.
