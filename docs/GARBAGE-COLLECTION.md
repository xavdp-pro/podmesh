# Garbage collection contract: proof before age

Date: 2026-09-12. Audience: PodMesh implementation and review agents.
Status: operator design direction for a later bounded implementation lot.
This document does not authorize a scheduler, destructive action, or production
collection by itself.

## Core rule

**Age never justifies collection. Proof justifies collection.**

Time is a retention condition only after every safety condition is already true.
A stale directory, a failed request, an unreachable host, or a stopped service is
not proof that no valid restore, transfer, process, evidence need, or ownership
claim remains.

The collector must preserve the distinction between:

1. **State transition:** closing a reservation or a failed restore claim through an
   explicit PodMesh operation with its own authority and verification contract.
2. **Runtime recovery:** terminating a proven orphan from PodMesh's own failed
   restore attempt. This is not ordinary file deletion.
3. **Artifact collection:** removing only unreferenced operation-owned files after
   terminal state, declared retention, and evidence-hold checks.
4. **History retention:** keeping enough immutable journal/tombstone information to
   prevent an old runtime container, operation or universe UUID from regaining an
   unsafe meaning.

No collector may replace a missing migration, restore, abort, retirement, or
ownership decision with a filesystem deletion.

## Authority model

The first version has two explicit modes:

- **Plan / dry run:** calculate candidates, blockers and proposed effects. Bounded
  read-only Podman inspection and its own operation/attempt/plan audit writes are
  permitted. It makes no Podman mutation, signal, domain-state transition, file
  deletion, or retention change. Audit writes do not authorize a later apply.
- **Apply:** repeat every proof immediately before each effect, perform the bounded
  action, independently verify the result, and write an immutable operation record.

There is no autonomous timer in the first version. A monitoring agent may prepare a
plan and alert the tandem. Apply requires a named operator decision or a named
mandate with explicit collection scope.

A future permanent production mandate must state, separately for each class, what it
may collect, the minimum retention, the evidence-hold policy, the maximum effect per
run, escalation rules, and whether it may request runtime reclaim. No generic
“cleanup” permission is sufficient.

## Immutable safety exclusions

The collector must refuse a candidate when any of the following is true:

- an authorization is open, issued, or otherwise not terminal;
- a destination may still hold a valid restore claim under the recorded handoff;
- a restore claim is verified as `restored`;
- the universe container is running, paused, stopping, or cannot be observed
  reliably;
- the container cannot be proven to have been created by the exact failed restore
  claim being considered;
- a required source or destination journal/outcome is missing, malformed, or bound
  to a different handoff;
- the artifact belongs to an active operation, active claim, active reservation or
  active transfer outbox/inbox;
- an evidence hold, investigation hold, or minimum retention interval applies;
- a process cannot be proven by cgroup membership and start time to belong to the
  failed attempt;
- a required graph-root or state-directory free-space observation fails, returns a
  non-success status, or cannot be parsed; measured zero is distinct from unknown;
- the operation attempts to delete a logical-history row, ownership tombstone,
  transfer outcome, evidence manifest, or audit record needed for future safety.

An unknown fact is a blocker. The collector reports it; it does not guess.

## Terminal collection classes

### 1. Checkpointed reservation after every authorization ended `not_restored`

This is eligible for a **release/archival decision**, not an unconditional deletion,
only when all conditions hold:

1. The source reservation is still the recorded container and fresh observation
   proves it is stopped and checkpointed.
2. Every authorization emitted by that reservation is terminal and recorded as
   `ended_not_restored`, with an outcome whose handoff hash, universe UUID, source
   host and destination host all bind to the authorization.
3. There is no issued authorization, unresolved source completion, active destination
   claim known from the recorded protocol, or pending outcome.
4. The collector preserves every authorization and outcome record as history before
   it releases or archives the reservation.

Within the PodMesh protocol, a verified `not_restored` outcome closes the destination
claim for that authorization and prevents the destination from later using that
authorization through the API. It does **not** prove that an out-of-band root actor
cannot bypass PodMesh. The collector must state this boundary, never promise that a
host is physically unable to make another copy.

The safe effect is a terminal archived/tombstoned reservation state that lifts only
the specified generic gate. It must not silently start the source. A later local
memory restore or ordinary start remains a separate explicit operation with its own
result and evidence.

### 2. Reservation whose container disappeared before any authorization

This is the existing abandonment shape. It is eligible only when:

1. Fresh inspection proves the reserved container is absent by both expected name
   and recorded container ID.
2. No authorization was ever issued for the reservation.
3. There is no active restore claim for its universe on this host.
4. The original reservation, failed checkpoint records and artifacts remain
   referenceable from history.

The collector may transition it to `abandoned` or an archived equivalent. It must not
allow a blind `create` of the same universe UUID: the tombstone/history protects
against accidental identity reuse. Reusing that logical UUID requires a separately
defined verified handoff or explicit replacement procedure.

### 3. Failed destination restore claim and orphan runtime processes

This is primarily a **recovery operation**, not garbage collection. The collector may
propose it, but the actual action must use the explicit failed-restore abort/reclaim
contract and an opt-in field such as `reclaim_processes: true`.

Before signalling any process, prove all of the following immediately before the
signal:

1. The claim is not `restored` and the restore scope has ended.
2. The target container is absent or non-running, carries the expected universe
   identity, was created after the claim, and is owned by no verified operation.
3. Every target PID belongs to the exact container cgroup
   (`libpod-<container-id>.scope`) or conmon cgroup
   (`libpod-conmon-<container-id>.scope`).
4. Each PID start time is at or after the durable claim timestamp; reread it just
   before signalling to protect against PID reuse.
5. The action records cgroup paths, PID, start time, signal, result and the free
   space of the actual graph filesystem before and after the attempt.

After the signal, wait for cgroup disappearance, verify the container is absent,
reinspect remaining PIDs, and measure disk space externally. A surviving PID, a
surviving cgroup, or unrecovered space produces `incomplete_reclaim`, not success.
An argv/cmdline search is diagnostic fallback only; it never authorizes a kill.

With `reclaim_processes: false`, the collector may report the same facts but must
send no signal and delete no process-owned state.

### 4. Failed local restore

Treat this exactly as a failed destination restore, but bind all proof to the local
restore operation, its container ID and its cgroups. Never infer that a local restore
is harmless because it used a local archive: it may still have created a container,
processes, logs or kept checkpoint files.

The collector may remove only a non-running container proven to be created by that
failed local restore. It must not delete a verified local restoration, a running
universe, or checkpoint files still referenced by an open recovery decision.

### 5. Operation artifacts after declared retention

Artifacts may be collected only after all conditions hold:

1. The associated operation is terminal and its effect/result is preserved in the
   journal.
2. No active reservation, claim, authorization, replay, retry, recovery or transfer
   references the artifact.
3. The declared retention period has elapsed.
4. There is no evidence or investigation hold.
5. A small retained manifest preserves original path class, content hashes, sizes,
   operation ID, terminal state, collection time and collecting operation ID.
6. The candidate path is proved to be under the operation-owned state directory;
   the collector never accepts a caller-provided arbitrary path.

Evidence of incidents needs a stronger default. “Nobody has read it” is not a
machine-verifiable lifecycle. Instead, an incident record is held by default until
an explicit review acknowledgment or retention release is recorded. The collector
must not attempt to infer human reading from access timestamps.

## Required collector record

Every dry run and apply run creates a durable, queryable record with:

- collection operation ID, requester and authorization reference;
- collector version and policy/retention version;
- mode (`plan` or `apply`);
- candidate identity, class and every proof fact observed;
- explicit blockers and exclusions;
- proposed effect during planning;
- action attempted during apply, per-object result and fresh verification;
- artifact hashes/sizes before and retained manifest after collection;
- whether any runtime signal was attempted;
- start/end timestamps and evidence references.

An apply request must use a stable operation ID. Replaying a verified collection
returns its historical result plus fresh observations; it must never repeat deletion
or signalling merely because a client lost its response.

## Bounded execution rules

To make an error recoverable:

- impose a maximum number of candidates, files, bytes and runtime reclaims per apply
  run;
- stop at the first unexpected identity, authorization, cgroup or retention mismatch;
- do not recursively traverse outside known operation directories;
- do not delete database history, audit facts, outcome documents or ownership
  tombstones;
- preserve a report and a retained manifest before removing artifacts;
- perform a fresh proof immediately before each individual destructive effect;
- make no cross-host inference solely from absence of a response.

## Recommended implementation order

1. Add a read-only `garbage_collect_plan` operation. It enumerates no more than a
   configured bounded set and produces evidence/blockers only.
2. Add terminal reservation archival/tombstones for classes 1 and 2, with tests for
   open authorization, mismatched outcome, running/replaced source, and UUID reuse.
3. Integrate failed-restore runtime handling by calling the already explicit abort
   and reclaim mechanism, not by duplicating process-management code.
4. Add operation artifact collection with retention policy, evidence holds and
   retained manifests.
5. Add a separately authorized apply operation with idempotency and bounded limits.
6. Only after all above are proven, consider a monitoring agent that proposes plans.
   Do not add a periodic autonomous collector until a production mandate defines its
   exact scope and escalation behavior.

## Acceptance tests

The implementation is not accepted until it demonstrates all of the following:

- dry run has no Podman events, signals, deletion or domain-state mutation; only
  its operation, attempt and immutable plan audit records may be written;
- apply refuses every open authorization, verified restore, running universe,
  uncertain identity and active evidence hold;
- each terminal collection class succeeds only with the stated fresh proofs;
- a stale/replayed apply operation ID does not repeat its destructive effect;
- abort with `reclaim_processes: false` reports but does not kill;
- verified reclaim proves cgroup membership/start time, no remaining cgroup/PID,
  container absence and measured graph-root recovery;
- injected cgroup, `/proc`, and `df` producer failures reach the collector verdict
  as explicit unknown facts, leave the effect/run unverified, and stop later
  candidates;
- a restore watchdog freezes no cgroup unless an exact immutable container ID is
  attested by the current durable operation-attempt, authority, universe, name and
  image markers plus exact conmon ID/name arguments; unrelated or ambiguous global
  cgroups cause only the attempt's own transient scope to be stopped;
- failed/incomplete reclaim remains visible and does not falsely claim recovered
  space;
- retention collection retains a manifest and never removes an active or referenced
  artifact;
- source/destination migration integrity and ownership tests continue to pass;
- raw evidence records commands, versions, hashes, disk measurements and independent
  observations.

## Production decision left to the operator

The remaining human decision is not whether proof is required; it always is. The
decision is what an explicit permanent production mandate may collect automatically.

That mandate should name allowed classes, environments, retention durations,
incident-hold behavior, byte/count limits, whether verified runtime reclaim is ever
permitted, notification/escalation, and the required independent audit trail. Until
then, the collector plans and the root tandem or a named mandate applies.
