# Exclusive-effect fencing laboratory

Status: isolated executable model, not installed PodMesh functionality or deployed
manager HA. Written in Python with only the standard library to keep this safety
experiment small; this does not change PodMesh's Rust implementation direction.

## Scope and design inputs

One logical **control-services universe** (the former manager) has a distinct
replica on each host. Control observations may remain local and synchronize after
reconnection. A separate question is who may perform an exclusive effect, such as
publishing the active route or changing one shared resource.

This experiment follows `../../INTENT.md`,
`../../docs/CONTROL-SERVICES-UNIVERSE.md` (roles, identities, partition and
reconnection), and the existing `../registry/` and `../manager-ha/` experiments.
It changes none of them. It deliberately adds the missing effect-enforcement
boundary instead of treating the manager model's eligibility report as authority.

The lowest-ID replica may be a coordinator after history reconciliation. Its ID
does not select the newest facts, grant permissions, fence another instance, or
justify takeover. Reconciliation stays in the other experiments. Here, an explicit
trusted operator-fixture action rotates ownership; no timeout, stale observation,
host absence, or manager election calls that action automatically.

## Contract made executable

The laboratory has **one external authoritative effect gate**, with durable state
independent of manager memory, and three makers with separate local SQLite files.
The gate is a deliberately trusted fixture, not an implementation of a distributed
consensus service. Keeping it unique and current is an explicit precondition.

1. The trusted fixture declares a bounded resource. It initially has no owner.
2. The fixture calls `transfer` with the expected current epoch and the authorized
   replica/instance. A SQLite IMMEDIATE transaction compares and advances the epoch.
   Two concurrent transfers with the same precondition cannot both succeed.
3. The resulting permit is serialized and may be delivered to its designated maker.
   It binds `authority_id`, `resource`, `epoch`, `replica_id`, `instance_id`, and an
   opaque `grant_id`. It cannot be reassigned to another instance by copying JSON.
4. The maker rejects a previously superseded epoch from its durable local screen.
   It must still contact the gate for **every** exclusive effect. Cached permission
   alone is never sufficient.
5. The gate checks its current grant and the caller's principal in the **same
   transaction** that performs the complete modeled effect and saves its receipt.
   The effect is an integer counter update, not a command submitted to another
   system. A delayed request from an old epoch is refused even when its maker has
   never learned the new epoch.
6. Reusing an operation ID with the identical current grant and payload returns
   the durable original result. Incompatible reuse fails. A stale grant cannot
   replay a receipt as fresh authority after transfer.
7. Explicit revocation advances the epoch and leaves no owner. No age-based garbage
   collection removes the epochs or operation receipts. Quota exhaustion refuses
   new effects; it does not discard safety state.

**Safety statement:** under the single, non-rolled-back gate precondition, each
resource has at most one current effect owner. At every committed effect, that
request belongs to that current grant. Effects cannot commit at an older epoch
after a newer epoch has committed an effect. A resource may have zero owners after
revocation. Two different resource scopes may have different concurrent owners.

An owner may be changed without proving the old manager process stopped **only
because every modeled effect must pass through this one atomic gate**. The old
process may remain alive and run unrelated, previously authorized local work.
This argument does not apply to an ungated Podman process, a continuing disk write,
or a network address still advertised on an isolated host.

## Epoch scope, time and authenticated identity

Permits are epoch-scoped and have no wall-clock expiry. Delayed delivery cannot
revive a superseded grant. There is no lease, synchronized clock, lease extension,
expiry-based takeover, or claim that an offline holder can safely keep doing
exclusive work. Adding time-limited leases later requires a reviewed clock-drift,
pause, restart, expiry-enforcement and renewal contract at the effect resource.

The `actor` tuple in the gate API is supplied by a trusted test fixture. A real
endpoint must derive that principal from authenticated transport and bind it to
the replica's current incarnation. A caller-provided JSON identity is not enough.
The opaque grant ID is a lookup binding, not a signature, authorization policy,
or replacement for channel authentication. Privileged `transfer`, `revoke`, and
`declare` methods are local fixture controls, never exposed to arbitrary managers.

## Partition, failure and recovery behavior

| Event | Model behavior |
| --- | --- |
| Manager replicas lose contact with each other | No automatic ownership change; control-history behavior belongs to the registry experiment |
| Maker cannot reach the gate | New exclusive effects are refused; no cached-permit fallback |
| Old manager stays alive after an explicit transfer | Its gate requests fail on epoch/grant mismatch |
| Owner process is lost | Loss alone creates no permit; an authorized controller may rotate the gate grant |
| Stale maker database returns | Gate current state still refuses superseded grants |
| Maker dies after the gate committed, before local commit/reply | Identical retry reads the gate receipt and does not repeat the effect |
| Gate process restarts with its current intact database | Authority ID, epochs, effects and receipts remain current |
| Gate database is missing | Opening fails; it is not automatically recreated |
| Gate database is cloned or rolled back | Outside the safety precondition; a negative-control test demonstrates double acceptance by two copied gates |
| Gate unavailable | Exclusive-effect availability is lost, while unrelated local operations need not depend on it |

This is deliberately **not quorum-free highly available arbitration**. It shows
where exclusion has to be enforced. Replicating the gate itself safely requires
an external mechanism with a single authoritative ordering, or another verified
exclusion primitive. A local SQLite copy on each host is not that mechanism.

## Real enforcement still required before Podman activation

There are two possible integration contracts; neither is delivered here:

1. **Resource-enforced fencing:** every protected write/route/effect reaches a
   trusted resource that atomically rejects older fencing tokens. Its checks must
   encompass the actual effect, not precede an asynchronous shell command. Work
   already in flight must be finished or excluded before the new epoch is used.
2. **Verified physical/runtime exclusion:** before activating a replacement,
   an independent controller prevents the old instance from producing all relevant
   effects. Examples to evaluate include power fencing plus restart interlock,
   storage reservations and network/route isolation. A hypervisor stop response,
   ping failure, dropped manager connection, or only a local highest-epoch file
   is not comprehensive proof of exclusion.

For either contract, define enrollment, authorized transfer decisions, authenticated
principals, restart/recovery incarnations, independent observation, audit/Logger
correlation, ambiguous outcomes and persistence after power loss. Prevent stale
authority backups or duplicate authority instances from serving simultaneously.
The bootstrap must not depend solely on the DNS service being recovered.

Starting a Podman container after `gate.effect(...)` would introduce a gap: an old
request could have passed the gate, pause, and start after a newer owner. The
laboratory intentionally exposes no `start` adapter until that gap can be enforced.

## Executable inventory and strict limits

| ID | Requirement | Delivered acceptance |
| --- | --- | --- |
| MF-01 | Exact permit/principal/instance/resource binding | Tampering, wrong actor, replacement instance and foreign authority refused |
| MF-02 | Atomic epoch transfer and effect enforcement | Concurrent transfers have one winner; delayed old effects cannot follow new effects |
| MF-03 | Durable maker screen and external gate | Restart and stale maker restore tests |
| MF-04 | Partition refusal and explicit revocation | Unreachable gate refuses effects; no timeout takeover |
| MF-05 | Durable operation deduplication | Retry, changed request, process-kill commit window and quota retry |
| MF-06 | Atomic failure | Injected resource-update failure rolls back preceding receipt insertion |
| MF-07 | Bounded interface/state | Permit framing, identifiers, epochs, resources, receipts and counter limits |
| MF-08 | Three-replica adversarial scheduling | 600 deterministic attempts with transfers, partitions, stale messages and gate restarts |
| MF-09 | Authority recovery limitation remains visible | Cloned-gate negative control reproduces the prohibited double-acceptance topology |
| MF-10 | Explicit storage failure and fixture diagnostics | Typed lock/open/schema failures; workers report setup and unexpected exceptions |

Bounds: 4096 permit wire bytes; exact six fields; identifiers up to 96 ASCII
characters; at most 64 declared resources and 64 cached maker resources; 2048 effect
receipts total per gate; epochs 0 through 2^31-1 without wrap; deltas -1000 through
1000; absolute counter at most 10^9. SQLite waits at most two seconds per lock
attempt; callers do not retry automatically. Process tests use bounded waits and
kill/reap their own children. Disk page/WAL overhead is not included in the quotas;
real storage exhaustion, adversarial deployment and administrator tampering remain
unqualified. SHA checksums/signatures and anti-rollback hardware are not provided.

Gate files use SQLite `application_id=0x504D4647`, `user_version=2`; maker files use
`application_id=0x504D464D`, `user_version=3`. Both values are checked before journal
configuration when reopening. Wrong-role, old-version and malformed stores are
refused without automatic schema creation or migration. These unreleased fixture
versions are independent of the installed PodMesh or manager-ha schemas.

Public methods translate SQLite and filesystem operational failures into `Refused`
with a category: `storage_busy` (BUSY/LOCKED), `storage_schema` (unsupported role or
version, corrupt/not-a-database, missing/broken SQL schema), `storage_io` (open,
read-only, full or I/O failure), or `storage_fault` (other database faults). There
are no implicit retries, and raw SQL/path text is not returned in the public error
message. A storage refusal does not universally promise no effect: the gate may
already have committed before a maker-local failure. Retain the original operation
ID and inspect/reconcile the gate receipt; identical authorized retry is deduplicated.

The old-worker concurrency test has a post-transfer barrier: 32 old-permit requests
are sent only after the new grant and its first effect have committed. All 32 must
be refused. The persisted sequence must contain the one old initial effect followed
by 33 new-epoch effects. The seeded stress requires at least 200 accepted effects,
100 refusals, 50 stale attempts and ten epochs with multiple accepted effects,
so the owner/epoch invariant is exercised beyond single-effect epochs.

Run from this directory:

```sh
python3 -m unittest -v test_fencing
python3 -m py_compile fencing_lab.py test_fencing.py
```

See [EVIDENCE.md](EVIDENCE.md) for what actually ran, and
[STRESS-PLAN.md](STRESS-PLAN.md) for the outstanding system tests. No installed
service, container, network, resource assignment or existing experiment is changed.
