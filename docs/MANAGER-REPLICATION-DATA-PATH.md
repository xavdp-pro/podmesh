# The manager's replication data path, qualified separately from activation

Status: **measured on the three preserved stores of campaign 6 (2026-09-14); qualifies the
replication of non-exclusive observations in owned scopes, and nothing about exclusion.**
Codex's second engineering item of the day: *three replicas are replicas of one logical
manager, not three independent managers; state precisely what converges, what is immutable
history, and what is still an exclusive decision.*

The derivation is `packaging/podmesh-manager/qualification/activation/campaign/replication-path.py`,
run on the inspections the candidate itself produced from the preserved copies; its public
output is `docs/qualification/manager2-g2-live-6/replication-path.json`. No identifier
leaves it.

## One logical manager, three replicas

The store's `identity` row carries one `logical_manager_id`, three declared replicas and a
`scope_owners` map: exactly one replica owns each disconnected-write scope. Every fact names
the logical manager, its origin replica and that replica's host, and `validate_fact`
(`manager-ha/src/lib.rs`) refuses at import any fact whose origin is not a declared replica,
whose logical manager differs, whose scope is not owned by its origin, or whose event ID is
not `<origin>:<producer_sequence>`. Measured: one logical manager ID on all three stores,
three distinct replica IDs, three scopes each with exactly one origin replica — the owner.

## What converges: the fact set, and therefore the view

A **fact** is an immutable event: content-addressed by `event_id` and a SHA-256, appended by
its owner (`Replica::observe`) or imported from a peer (`Replica::ingest`), which refuses an
event identity that arrives with different bytes. Per subject, facts chain by
`subject_revision` and `predecessor`; the materialised **view** keeps the last revision of an
unbroken chain and blocks a subject whose chain is missing a predecessor or forks.

Measured across the three preserved stores: **18 facts, byte-identical sets on every host**;
`logical_history_sha256` identical; the materialised view identical (three subjects, one per
scope); per scope, six revisions, contiguous 1 to 6, each chained to its predecessor; zero
conflicts, zero blocked subjects. This is what "converged" means and it held on real stores
across a campaign whose transport needed up to 114 resubmissions to land one observation:
convergence is of the set, indifferent to how many attempts it took.

## What is immutable history, and what is per host

Three tables are append-only by trigger — `facts`, `receipts`, `exchange_audit_events` refuse
UPDATE and DELETE — but only the first is **replicated**. The other two are each host's own:

| | replicated | measured on campaign 6 |
| --- | --- | --- |
| facts | yes — the set converged | 18 / 18 / 18, identical |
| receipts (what this replica accepted: its own observations, its authenticated imports) | **no** | 40 / 44 / 44, different sets; 34–38 imports per host |
| exchange audit events (every phase of every attempt this host sent or served) | **no** | 4340 / 5165 / 4819, three distinct digests |
| incomplete attempts (a strand this host opened and never saw closed) | **no** | 130 / 103 / 81, all `outbound_request_prepared` |

So a G2 comparison joins **per-host** records across hosts to account for attempts — that
is why the accounting is a cross-host join and never a count — while convergence is read
from the one thing that is meant to be equal everywhere. Receipts differing between hosts is
not a defect: a receipt is *this* replica's acceptance, and two replicas that imported the
same operation hold two receipts for it.

## What is still an exclusive decision, and is not evidenced here

A fact may carry `exclusive_resource` and `active_claim`. Materialisation blocks a resource
claimed by more than one current fact and reports the conflict; a `Reconciliation` over an
agreed history names a coordinator, and `authorize_exclusive_service` issues a permit only to
that coordinator, only for an unblocked resource it holds a reconciled active claim on, and
the permit dies with the history it names. That is the model. **Campaign 6 carries zero facts
with an exclusive resource, zero active claims, zero conflicts and zero blocked resources**, so
nothing in this evidence says anything about exclusion — and `CheckService`, the operation
that would issue a permit, is not reachable from the packaged resident. The fencing
laboratory's epoch gate and PodMesh's activation leases (main tree, lots H1–H8) are where
exclusion is exercised today, outside the manager.

## What this qualifies, in one sentence

Replication of non-exclusive observations in owned scopes across three replicas of one
logical manager: the fact set, its digest and the materialised view converge and are
immutable; receipts, audit and open attempts are per host and are what a G2 accounting
joins. It does not qualify exclusion, takeover, partition behaviour, the pre-reply latency
curve, or anything about a host that is lost.
