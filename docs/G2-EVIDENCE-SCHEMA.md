# G2 evidence schema v3 — publishing what the accounting predicate needs

Status: **implemented at `024eb84`, independently reviewed as NO-GO, and not yet run
against hosts.** Lot `codex/g2-accounted-attempts`.
Work item 2 is this document; items 3 to 6 are in the collector, the fold, the comparator
and the suites: typed fresh-store absence, the published `incomplete_attempts` records,
the folded `exchanges` rows, and the eight-condition join that replaced the withdrawn
demand for zero. The fifth independent review found two blocking defects: honest inbound
incomplete attempts abort comparison (N1), and the accepting conditions are not isolated
by the tests (N2). It also found the N3-N7 limits recorded below. Those defects must be
corrected before preserved-store re-evaluation or another campaign. **No campaign has been
run against this version, and nothing here qualifies G2 or manager HA.**

**Two corrections this document made to itself, both from measurement rather than from
reading, are recorded in place below**: an exchange is a fold over one to four audit rows
rather than a single row, and "set" means non-null for identities but non-zero for the
byte counters. A third came from the implementation: the peer field always names the
other party, so the two sides of a join are bound by each naming the other against the
published replica commitments, never by equality.

**The versions are bumped before another capture is allowed.** Evidence `/v2`, comparison
`/v2`, and published inspection `3` name the historical shape only. The widened collector
emits evidence `/v3` with published inspection `4`; the corresponding comparator must emit
comparison `/v3` and refuse the old strings. The frozen candidate still supplies its
private inspection input as version `3`. `capture-host.sh` checks that private contract and
projects it into the distinct, privacy-preserving published inspection version `4`.

Historical `/v2` evidence is not rewritten or silently accepted by the new comparator. It
remains reproducible with the comparator at commit `9d814b2`, the last comparator commit
that consumes that shape. Preserved stores may be captured again under `/v3`, but those new
files are new evidence and must receive new sidecars and a new comparison.

Date: 2026-09-13. Authority: `/tmp/podmesh-claude/DECISIONS-CODEX-2026-09-13.md`,
Decision 1. Predicate: [MANAGER-HA-ACCEPTANCE.md](MANAGER-HA-ACCEPTANCE.md), "The G2
acceptance predicate: accounted, not absent".

## Why a schema change is unavoidable

A field-level inventory of the current pipeline established that **only condition 7 of
the eight is even partially evaluable from today's sealed evidence; conditions 1 to 6
and 8 are not evaluable at all.**

One line caused it. The projection then at `capture-host.sh:137` reduced the canonical
inspection to nine keys. *That citation is now stale by this lot's own hand: commit
`eb0ccd3` moved the projection (line 192 there, `:198` at the current state) and
widened it to sixteen keys.
The line is kept in the past tense because this paragraph describes why the change was
needed, and a reader following `:137` today lands in an unrelated systemd assertion.*

What that projection did: it folded `incomplete_attempts` — a vector of structs each
carrying `direction`, `attempt_id`, `wire_nonce`, `wire_operation_id` and `last_phase`
(`durable.rs:323-331`) — into a single integer, and dropped four fields outright:

| Dropped | Where it lives | What is lost |
| --- | --- | --- |
| `ordered_audit_events` | `durable.rs:316` | every per-attempt fact the join needs: peer, operation, nonce, phase, outcome, request digest, announced size, transferred bytes, reply bytes and digest, local and remote receipts, `replayed` |
| `ordered_receipts`, `receipt_set_sha256` | `durable.rs:313-314` | per-receipt identity and set completeness |
| `audit_set_sha256` | `durable.rs:317` | local audit-set completeness and stage consistency; replica-local audit sets are not expected to equal one another |
| `unaudited_import_receipt_ids` | `durable.rs:319`, computed `:2662-2676` (the set difference itself at `:2673`; `:2667` is only the binding head) | **the** field condition 4 is about |

The amended predicate is therefore not merely unmet — it cannot be computed from what a
campaign seals. This is a collector and schema change first, and a comparator change
second.

## The frozen candidate already emits everything this needs

**No new candidate binary is required, and that was not obvious.** The campaign's whole
evidence chain is pinned to candidate source `ff77b1f`, its binary digest and its package
digest, and `capture-host.sh` refuses to run against anything else. If v4 had needed a
field the candidate does not produce, the lot would have meant a new build, a new
verification and a reinstall on three hosts — a requalification rather than a work item.

It does not. `CanonicalStoreInspection` (`durable.rs:302-321`) already declares
`ordered_receipts`, `receipt_set_sha256`, `ordered_audit_events`, `audit_set_sha256`,
`incomplete_attempts` and `unaudited_import_receipt_ids` as public fields; it derives
plain `Serialize` with no `skip` attribute anywhere; and `--inspect-store` serializes the
whole struct (`main.rs:32`). Every field the amended predicate needs is on the frozen
candidate's stdout today and is thrown away by one projection in the collector
(the projection, now `capture-host.sh:197`). Published inspection v4 is a collector and
comparator contract. It is not a claim that the frozen candidate's private inspection
input changed from v3.

### Correction: `immutable_schema_verified` cannot be published

The schema below listed `"immutable_schema_verified": true` as a new field, annotated
*"`durable.rs:2628` runs it, nothing published it"*. That was the right observation and
the wrong conclusion. The candidate runs `verify_immutable_schema` at that line and
returns an error when it fails — it never exposes the result as a value. A collector
emitting a hard-coded `true` would not be publishing an observation; it would be
publishing an assertion dressed as one, which is the same defect as a signature anchored
in nothing.

What is actually true is weaker and sufficient: **a successful inspection entails that
the check passed**, because a failed check makes the inspection fail. So the field is
dropped. The comparator relies on inspection success, which it already requires, and the
evidence claims nothing it did not observe.

## The one mechanism that already works

The forensic join that diagnosed the nine late-reply strands matched a sender's
stranded attempt to its receiver's records **by wire nonce**, and it closed on all nine
without exception. That is exactly the join conditions 2 to 6 require.

Because every host in a campaign salts with the same private campaign salt, two hosts
independently committing the same nonce produce the **same commitment**. A comparator
can therefore perform that join on commitments and **never see a nonce**. The mechanism
is proven; only its publication is missing. Everything below is an application of it.

## Correction: an exchange is not an audit row, and the join rule as written refuses everything

Measured on the preserved store copy at
`/tmp/podmesh-manager-inspect-54aca614a913501bb5dd4585/store.sqlite` — **one host, 67
nonces, 165 audit rows**. The other preserved copy holds zero audit events, which is
itself the fresh-store case work item 3 exists to capture. So this is a real sample, not
all three hosts, and it is stated that way.

**Every nonce in that store carries one of exactly three phase sets:**

| Phase set | Nonces | What it is |
| --- | --- | --- |
| `outbound_request_prepared` | 17 | a stranded sender attempt — the case this lot is about |
| `outbound_request_prepared`, `outbound_exchange_completed` | 26 | a completed sender attempt |
| `inbound_request_observed`, `inbound_import_committed`, `inbound_reply_prepared`, `inbound_reply_write_observed` | 24 | a served receiver exchange |

So a receiver exchange is **four audit rows sharing one nonce**, not one. The comparator
rule stated below — *refuse on more than one receiver row for a nonce commitment* — would
therefore have refused **every genuine inbound exchange in the measured campaign**. The
join is sound; the row model under it was wrong. `exchanges` publishes **one folded row
per nonce**, and the fold is over the audit rows that share it.

### The fold rule, measured rather than assumed

Within one nonce the rows disagree on some fields, because `inbound_request_observed`
happens before the request is decoded. Counting naively, 24 of the 67 nonces have rows
that "disagree" on peer, operation or request digest. Counting only **non-null** values,
the number of disagreements is **zero**.

That gives the rule, and it is fail-closed rather than lenient:

- each folded field takes the single **set** value found among the nonce's rows;
- a field set nowhere folds to null and is reported as absent;
- **two different set values for one field refuse the campaign.**

**"Set" is not the same as "non-null", and getting that wrong silently loses every byte
count.** A first version of this rule said non-null throughout. Measured, that is right
for the identity and digest fields and wrong for the counters: `request_frame_bytes` and
`reply_frame_bytes` are not nullable, so a phase that does not carry them writes **zero**,
and a non-null fold over four rows would have found `{0, 0, 0, 2735}` — four set values,
three of them false. So:

| Field kind | Fields | "Set" means |
| --- | --- | --- |
| identity and digest | peer, operation, request and reply digests, local and remote receipt | non-null |
| byte counters | request and reply frame bytes, announced body bytes | non-zero |
| flag | `replayed` | carried by the rows after the request phase |

Which phase carries which is itself measured rather than assumed, and it differs by
direction: inbound, the request bytes and digest are on `inbound_request_observed`, the
reply bytes on `inbound_reply_write_observed`, the local receipt from
`inbound_import_committed` onward; outbound, the announced body size is on
`outbound_request_prepared` — **including on all 17 stranded attempts**, which is why
condition 2 has a binding field to join on at all — and the frame bytes, reply digest and
remote receipt on `outbound_exchange_completed`.

Across all seven folded fields, over 67 nonces, **the number of nonces carrying two
different set values is zero**. That is why the refusal must be a negative fixture: a rule
whose violation has never been observed is a rule nobody has tested.

The folded row also carries the **set of phases reached**, not a single `terminal_phase`
as drafted below. The terminal phase is derived from the set, and a set missing a phase is
a visible fact rather than a silently absent one.

### Pre-authentication nonces cannot be joined, and must be published as such

`wire_nonce` is not always a peer protocol nonce. Before decoding reaches one, the
receiver labels the attempt `preauth:<sha256>`, built locally from the process id, an
atomic counter and a nanosecond timestamp (`manager-network/src/lib.rs:1666-1675`). The
store's own validator confines such a nonce to **inbound direction and three phases only**
— `inbound_request_observed`, `inbound_diagnostic_reply_written`, `inbound_connection_closed`
— and forbids it from carrying an authenticated peer (`durable.rs:1292-1315`).

Two consequences the draft below missed:

- **A preauth nonce can never join.** It is minted locally, so two hosts never commit the
  same value for one exchange. Under a fail-closed comparator, "no receiver row for this
  nonce commitment" is a refusal — so a campaign containing any preauth observation would
  be refused for a reason that is not a defect.
- **The sender side is safe.** Preauth is inbound-only and enforced, so an outbound
  stranded attempt — the only kind condition 2 joins from — always carries a real peer
  nonce. The measured store contains **zero** preauth rows, so this was not observed; it
  is permitted by the contract, which is enough.

Each folded row therefore carries `nonce_authority`: `peer-validated` or
`pre-authentication`. The join is defined **only** over peer-validated nonces.
Pre-authentication rows are published as observations that are explicitly not joinable,
are never counted as a duplicate, and never cause a refusal by their absence from the
other side.

## Run end to end against a preserved store

Work item 7, for one host. Not a campaign, and it qualifies nothing — but it is the first
time the whole chain has been exercised on real data rather than on fixtures.

The frozen candidate binary was located, its digest checked against the pinned
`cbd5020a…3660128`, and run as `--inspect-store` against a **copy** of a preserved
campaign store. Its configuration was rebuilt from the store's own recorded topology
rather than supplied, which is why the first three attempts were refused with
`identity_mismatch`: the grants that produce the scope owners have to be reconstructed
from them.

What the candidate reports for that store:

| | |
| --- | --- |
| history / receipts / audit events | 7 / 13 / 165 |
| incomplete attempts | 17, **every one** outbound at `outbound_request_prepared` |
| unaudited import receipts | 0 |
| SQLite integrity | ok |

Every incomplete attempt in a real store is the exact shape the frozen Stage D contract
describes, and the count the withdrawn gate demanded to be zero is 17. Condition 4 holds
on this store with nothing to explain.

The collector's own helpers were then replayed verbatim over that output, and **re-run at
each later state of the branch** rather than left describing a chain from several commits
ago. At the current state: 321 commitments in one batch, 67 folded exchange rows whose
largest collapses four audit rows and which together collapse **165 rows against a
reported 165**, 17 published attempt records, and 10 imported operations against 13
receipts. Both the projected inspection and the folded rows are **accepted by the
comparator's validators**, and no raw identifier reaches the published rows.

What this does not do: it covers one host, so no cross-host join was exercised against
real data — the join is exercised only by the constructed strand in the suite. The other
preserved copy holds zero audit events, which is the fresh-store case rather than a
second sample.

## Constraints the schema must satisfy

- **Never publish keys, endpoints, raw UUIDs or request bodies** (Decision 1). Every
  identifier is emitted as a salted commitment, using the same `commit_text` labelling
  discipline already in `capture-host.sh` — one label per *kind* of value, never per
  observation site, or the join silently fails.
- **Identities, never count deltas.** Cleanup legitimately adds terminal rows, so a
  difference of totals describes nothing. The schema carries the **pre-activation set**
  and the **post-cleanup set** of attempt identities, and the comparator works on their
  set difference. The stopped pre-activation capture is the sole debt anchor.
- **Fresh-store absence is explicit, not required.** Pre-activation is captured with
  `--with-inspection` before the first start and with manager process count zero. A typed
  `store_present: false` is a valid fresh state. A present store is also valid and may
  contain retained pre-existing debt; it is not described as empty or first-use.
- **Current capture extent is explicit.** `exchanges` currently folds every audit row
  retained by the inspected store, one output row per nonce. It is not scoped by a
  published wall-clock window or row watermark. A future bounded projection requires a
  new evidence version and must not be implied by this schema.
- **Versioned in lockstep.** `compare-evidence.py` pins the inspection object to an exact
  key set against a sealed schema string — nine keys when this was written, sixteen now, so the collector, the comparator, its
  fixtures and the schema version change together and are re-sealed together.

## The schema

Inspection `schema_version` moves 3 → **4**; evidence
`podmesh-manager-live-activation-evidence/v2` → **`/v3`**; comparison output
`…-comparison/v2` → **`/v3`**.

### `inspection` — replacing the folded count

```jsonc
{
  "schema_version": 4,
  "store_present": true,                    // false on a fresh host; every field below then null
  "logical_manager_commitment": "sha256:…",
  "replica_commitment": "sha256:…",
  "logical_history_sha256": "…",
  "receipt_set_sha256": "…",                // new — durable.rs:314, already a digest
  "audit_set_sha256": "…",                  // new — durable.rs:317, already a digest
  "sqlite_integrity_result": "ok",
  // immutable_schema_verified was here and is dropped; see the correction above.
  "history_count": 9,
  "receipt_count": 15,
  "audit_event_count": 3303,
  "incomplete_attempt_count": 33,           // kept, no longer the only thing
  "unaudited_import_receipt_count": 0,      // new — condition 4
  "unaudited_import_receipt_commitments": [],   // new — commitments of durable.rs:319
  "incomplete_attempts": [                  // new — the list, not the length
    {
      "attempt_commitment": "sha256:…",     // commit_text attempt-id
      "nonce_commitment": "sha256:…",       // commit_text wire-nonce   ← the join key
      "operation_commitment": "sha256:…",   // commit_text wire-operation-id, null when absent
      "nonce_authority": "peer-validated",  // or "pre-authentication"
      "direction": "outbound",
      "last_phase": "outbound_request_prepared"
    }
  ]
}
```

`direction` and `last_phase` are enumerations of the candidate's own vocabulary
(`AuditDirection` at `durable.rs:213-216`, `AuditPhase` at `:220-230`; an earlier `:220-232`
named neither cleanly and reached into `AuditOutcome` at `:234`), not free text, and they
identify nothing.

### `exchanges` — all locally retained folded exchange strands

A sibling of `inspection`, present only on a capture taken `--with-inspection`. It is an
array, and each element folds every audit row retained locally for one nonce. The current
collector publishes the whole retained audit table; no `window` or `rows` wrapper exists.

```jsonc
[
  {
      "nonce_commitment": "sha256:…",       // the join key
      "nonce_authority": "peer-validated",  // or "pre-authentication": published, never joined
      "joinable": true,
      "direction": "inbound",
      "phases_reached": ["inbound_request_observed","inbound_import_committed",
                         "inbound_reply_prepared","inbound_reply_write_observed"],
      "row_count": 4,
      "outcomes": ["accepted"],
      "peer_commitment": "sha256:…",
      "operation_commitment": "sha256:…",
      "request_sha256_commitment": "sha256:…",
      "request_announced_body_bytes": 5531,
      "request_frame_bytes": 5535,
      "reply_frame_bytes": 693,
      "reply_sha256_commitment": "sha256:…",
      "local_receipt_commitment": "sha256:…",
      "remote_receipt_commitment": "sha256:…",
      "reply_announced_body_bytes": 689,
      "replayed": true
  }
]
```

Byte counts are published raw: they identify nothing and conditions 2 and 5 are about
them. Digests are published as commitments rather than raw, because a raw request digest
plus a guessed body is a confirmation oracle.

## How each condition is evaluated or declared undecidable

| # | Decided by |
| --- | --- |
| 1 | the attempt commitment is in the post-cleanup set and not in the stopped pre-activation set, and `package` binds the candidate — already compared across stages; no narrower wall-clock window is declared |
| 2 | one `exchanges` row on exactly one other host with the same `nonce_commitment`, whose `peer_commitment`, `operation_commitment`, `request_sha256_commitment`, `request_announced_body_bytes` and `request_frame_bytes` all bind. The sender's transferred request byte count is not published, so that sub-condition remains explicitly undecidable |
| 3 | the receiver's `phases_reached` includes `inbound_import_committed` or `inbound_refusal_recorded`, its `outcomes` vocabulary is compatible with those phases, and the accepted-import branch carries a `local_receipt_commitment` |
| 4 | `unaudited_import_receipt_count == 0` on every host, with the commitments published so a non-zero case names which |
| 5 | the receiver's row reaches `inbound_reply_write_observed` with `reply_frame_bytes > 0`, **and** the sender's attempt is still `outbound_request_prepared` — the honest retention the contract requires |
| 6 | the reachable branch is a complete replay on the original receiver: a different peer-validated nonce, the same operation and peer commitments, `replayed: true`, accepted outcome, committed import and reply-write phases, the original receipt commitment, and a complete reply. The former all-replica convergence branch is unreachable for this candidate because operation IDs are per peer and import receipts are receiver-local |
| 7 | `sqlite_integrity_result`, successful inspection — which entails the immutable-schema check — and each replica's receipt/audit set digests remaining consistent with its own later capture. The set digests are replica-local and must not be required to agree across replicas. |
| 8 | the attempt is still in `incomplete_attempts` at post-cleanup — visible, not silently retired |

## What the comparator must gain

A fail-closed join over **folded** rows, one per nonce, and over peer-validated nonces
only. It refuses on: no receiver row for a peer-validated nonce commitment; **more than
one folded** row for it — the pre-fold rule would have refused every real inbound
exchange, see the correction above; two different non-null values for one field inside a
fold; any binding field that disagrees across hosts; a receiver row that never
reached the required phase set; a non-zero unaudited-receipt count; an attempt that appears in
the post-cleanup set, is claimed accounted, and is absent from `incomplete_attempts` at
post-cleanup; and any attempt introduced after the stopped pre-activation debt anchor
that cannot be classified individually. The capture publishes no narrower declared
wall-clock window.

Negative fixtures, one per refusal, plus: wrong peer, wrong nonce, wrong digest, short
request, partial import, unsigned or wrong reply, a missing or malformed replay, a corrupt
store, and a new attempt that remains unaccounted at post-cleanup.

Positive fixtures: a terminal attempt and a replay-accounted attempt. The former
all-replica accounting topology is not a valid candidate fixture and must not be used.

## Size, and what it costs

Per host in the measured store: about 33 incomplete-attempt records, and one folded
exchange row per nonce — 67 in the preserved store measured, folded from 165 audit rows.
That was on the order of tens of kilobytes per stage file, against 13 KB before this
projection. The raw audit history is never published, but the current folded projection
covers the whole locally retained audit table and can therefore grow without bound. A
bounded window remains a separate, versioned change.

## Known implementation limits at `024eb84`

The schema describes the intended decision contract. The fifth independent review found
that the implementation at `024eb84` does not yet meet it:

- **N1:** corroboration accepts only outbound strands. An honest inbound incomplete
  attempt, including pre-existing inbound debt, aborts comparison instead of receiving a
  direction-aware classification.
- **N2:** the accepting conditions are not independently pinned by the test suites; a
  number of single-condition mutants can still turn a refusal into a pass.
- **N3:** pre-activation debt is not yet required to remain byte-identical at every later
  stage and can disappear from the result.
- **N4:** the replay branch currently rests on a receiver row without requiring the
  matching sender retry row, and unmatched inbound rows are not reported. Until this is
  corrected, replay evidence is receiver-asserted under the published trust model.
- **N5:** the all-replica convergence branch is unreachable and must be removed from the
  implementation and its fixtures; it is not part of the contract above.
- **N6:** the pre-activation capture does not re-read unit and process state after store
  inspection, leaving a small capture race.
- **N7:** peer, operation, request digest and announced size are not frozen per nonce
  across stages, and outbound folded row shape is not capped.

After N1, direction-aware corroboration must bind the same direction, nonce, authority
and operation, require the listed last phase to be the highest observed non-terminal
phase, and classify a new inbound strand still open at post-cleanup as unaccounted rather
than aborting the comparator.

## What this design does not do

It does not qualify G2, does not change a threshold, does not touch a captured file, and
does not decide whether the nine measured attempts are accounted for. Even after the
limits above are corrected, a G2 result proves bounded authenticated exchange accounting
only. It does not prove takeover, fencing, exclusive activation, DNS recovery, host-loss
recovery, long-running viability, or manager HA.

It also does nothing about the receiver's unbounded pre-reply verification. That defect
has its own lot and must not be hidden by a G2 result.
