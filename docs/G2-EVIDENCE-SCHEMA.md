# G2 evidence schema v3 — publishing what the accounting predicate needs

Status: **design, not implemented.** Nothing here changes a collector, a comparator or
a captured file. Lot `codex/g2-accounted-attempts`, work item 2.

Date: 2026-09-13. Authority: `/tmp/podmesh-claude/DECISIONS-CODEX-2026-09-13.md`,
Decision 1. Predicate: [MANAGER-HA-ACCEPTANCE.md](MANAGER-HA-ACCEPTANCE.md), "The G2
acceptance predicate: accounted, not absent".

## Why a schema change is unavoidable

A field-level inventory of the current pipeline established that **only condition 7 of
the eight is even partially evaluable from today's sealed evidence; conditions 1 to 6
and 8 are not evaluable at all.**

One line causes it. `capture-host.sh:137` projects the canonical inspection to nine
keys. In doing so it folds `incomplete_attempts` — a vector of structs each carrying
`direction`, `attempt_id`, `wire_nonce`, `wire_operation_id` and `last_phase`
(`durable.rs:323-331`) — into a single integer, and drops four fields outright:

| Dropped | Where it lives | What is lost |
| --- | --- | --- |
| `ordered_audit_events` | `durable.rs:316` | every per-attempt fact the join needs: peer, operation, nonce, phase, outcome, request digest, announced size, transferred bytes, reply bytes and digest, local and remote receipts, `replayed` |
| `ordered_receipts`, `receipt_set_sha256` | `durable.rs:313-314` | per-receipt identity and set completeness |
| `audit_set_sha256` | `durable.rs:317` | audit-set completeness across replicas |
| `unaudited_import_receipt_ids` | `durable.rs:319`, computed `:2662-2676` | **the** field condition 4 is about |

The amended predicate is therefore not merely unmet — it cannot be computed from what a
campaign seals. This is a collector and schema change first, and a comparator change
second.

## The one mechanism that already works

The forensic join that diagnosed the nine late-reply strands matched a sender's
stranded attempt to its receiver's records **by wire nonce**, and it closed on all nine
without exception. That is exactly the join conditions 2 to 6 require.

Because every host in a campaign salts with the same private campaign salt, two hosts
independently committing the same nonce produce the **same commitment**. A comparator
can therefore perform that join on commitments and **never see a nonce**. The mechanism
is proven; only its publication is missing. Everything below is an application of it.

## Constraints the schema must satisfy

- **Never publish keys, endpoints, raw UUIDs or request bodies** (Decision 1). Every
  identifier is emitted as a salted commitment, using the same `commit_text` labelling
  discipline already in `capture-host.sh` — one label per *kind* of value, never per
  observation site, or the join silently fails.
- **Identities, never count deltas.** Cleanup legitimately adds terminal rows, so a
  difference of totals describes nothing. The schema carries a **baseline set** and a
  **post-campaign set** of attempt identities, and the comparator works on set
  difference.
- **Fresh-store absence is explicit, not an error.** Pre-activation is captured today
  without `--with-inspection` (`activate-host.sh:89`), and `inspect_read_only` refuses a
  missing store under `set -euo pipefail`. The baseline capture must tolerate "no store
  yet" and say so in a typed field, or it breaks the very fresh-store campaign it exists
  to serve.
- **Bounded.** Per-exchange rows are scoped to the campaign window, so a host publishes
  on the order of one row per inbound exchange it served — about 125 in the measured
  campaign, not the whole audit history.
- **Versioned in lockstep.** `compare-evidence.py` pins the inspection object to exactly
  nine keys against a sealed schema string, so the collector, the comparator, its
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
  "immutable_schema_verified": true,        // new — durable.rs:2628 runs it, nothing published it
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
      "peer_commitment": "sha256:…",        // commit_text replica-id
      "direction": "outbound",
      "last_phase": "outbound_request_prepared"
    }
  ]
}
```

`direction` and `last_phase` are enumerations of the candidate's own vocabulary
(`durable.rs:220-232`), not free text, and they identify nothing.

### `exchanges` — the receiver side of the join

A new sibling of `inspection`, present only on a capture taken `--with-inspection`, and
scoped to the campaign window by the baseline watermark:

```jsonc
{
  "window": { "from_audit_row": 2557, "to_audit_row": 3303 },
  "rows": [
    {
      "nonce_commitment": "sha256:…",       // the join key
      "direction": "inbound",
      "terminal_phase": "inbound_reply_write_observed",
      "outcome": "accepted",
      "peer_commitment": "sha256:…",
      "operation_commitment": "sha256:…",
      "request_sha256_commitment": "sha256:…",
      "request_announced_body_bytes": 5531,
      "request_frame_bytes": 5535,
      "reply_frame_bytes": 693,
      "reply_sha256_commitment": "sha256:…",
      "local_receipt_commitment": "sha256:…",
      "remote_receipt_commitment": "sha256:…",
      "replayed": true
    }
  ]
}
```

Byte counts are published raw: they identify nothing and conditions 2 and 5 are about
them. Digests are published as commitments rather than raw, because a raw request digest
plus a guessed body is a confirmation oracle.

## How each condition becomes decidable

| # | Decided by |
| --- | --- |
| 1 | the attempt commitment is in the post-campaign set and not in the baseline set, and `package` binds the candidate — already compared across stages |
| 2 | one `exchanges` row on exactly one other host with the same `nonce_commitment`, whose `peer_commitment`, `operation_commitment`, `request_sha256_commitment`, `request_announced_body_bytes` and `request_frame_bytes` all bind |
| 3 | that row's `terminal_phase` is `inbound_import_committed` or `inbound_refusal_recorded` with a matching `outcome`, and it carries a `local_receipt_commitment` |
| 4 | `unaudited_import_receipt_count == 0` on every host, with the commitments published so a non-zero case names which |
| 5 | the receiver's row reaches `inbound_reply_write_observed` with `reply_frame_bytes > 0`, **and** the sender's attempt is still `outbound_request_prepared` — the honest retention the contract requires |
| 6 | a later `exchanges` row with the same `operation_commitment` and `replayed: true`, **or** the imported fact present in the converged history on all three replicas |
| 7 | `sqlite_integrity_result`, `immutable_schema_verified`, and the two set digests agreeing across replicas |
| 8 | the attempt is still in `incomplete_attempts` at post-cleanup — visible, not silently retired |

## What the comparator must gain

A fail-closed join, refusing on: no receiver row for a nonce commitment; **more than
one** receiver row for it; any binding field that disagrees; a receiver row that never
reached a terminal phase; a non-zero unaudited-receipt count; an attempt that appears in
the post set, is claimed accounted, and is absent from `incomplete_attempts` at
post-cleanup; and any attempt outside the declared window.

Negative fixtures, one per refusal, plus: wrong peer, wrong nonce, wrong digest, short
request, partial import, unsigned or wrong reply, missing retry with absent converged
facts, a corrupt store, and an attempt outside the campaign window.

Positive fixtures: a terminal attempt, a replay-accounted attempt, and a
convergence-accounted superseded snapshot — the ninth strand of the measurement is
exactly that third case, and it is the one no synthetic fixture would have thought to
write.

## Size, and what it costs

Per host in the measured campaign: about 33 incomplete-attempt records and about 125
exchange rows. On the order of tens of kilobytes per stage file, against 13 KB today.
The audit history itself is never published — only a window of summaries and two set
digests over it.

## What this design does not do

It does not qualify G2, does not change a threshold, does not touch a captured file, and
does not decide whether the nine measured attempts are accounted for. It makes the
question answerable from sealed evidence; the answer is a campaign's business.

It also does nothing about the receiver's unbounded pre-reply verification. That defect
has its own lot, and a passing G2 under this schema would still say nothing about
long-running operational viability.
