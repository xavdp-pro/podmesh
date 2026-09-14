# The unbounded pre-reply verification, measured

Status: **measured, not fixed.** This is the defect standing between the manager and any
claim of long-running viability. It is not a G2 evidence problem: the G2 gate passes. It is
a property of the candidate, and it gets worse on its own.

Nothing here qualifies or disqualifies anything. It is a measurement and a reading of the
candidate's own source.

## What the receiver does before it answers

`verify_all` (`durable.rs:1137-1143`) runs at the head of every public store entry point —
`durable.rs:502`, `:558`, `:611` — and each call performs four passes over durable state:

- `verify_immutable_schema`
- `verify_receipts`, a full scan of the receipts table
- `load`, which materialises the whole canonical history
- `verify_audits`, a full scan of the exchange audit table

The audit table is **append-only by trigger**: an attempt to delete rows from it is refused,
which is how this measurement discovered the property a second time. So the work done before
each reply grows monotonically with the number of exchanges the replica has ever served.

## The read path, measured

The frozen candidate `0.1.0~manager2+gff77b1f946e8` run as `--inspect-store` against four
real stores, five runs each, on one workstation:

| audit rows | ms per read | µs per row |
| ---: | ---: | ---: |
| 165 | 13 | 78 |
| 1802 | 104 | 57 |
| 3341 | 195 | 58 |
| 3672 | 215 | 58 |

**Linear, at 58 µs per audit row**, stable across three well-separated sizes. The 165-row
point is dominated by fixed overhead.

This is the cheaper path. `--inspect-store` does not run `verify_receipts`, `load` or
`verify_audits`; `verify_all` does all three, and the write path adds three fsyncing
IMMEDIATE transactions on top.

## The write path, measured by the campaigns themselves

The sender's socket timeout is two seconds (`manager-network/src/lib.rs:35`, applied at
`:435` and `:453`). Every retry in the table below is one expiry of that timeout on an
identical resubmitted request.

| | audit rows, three hosts | retries for one observation, worst host |
| --- | ---: | ---: |
| campaign 4 | 10541 | 30 |
| campaign 5 | 12930 | **78** |

Same operation, same three hosts, one campaign apart. At roughly four thousand rows per
replica the receiver already exceeds the sender's two-second budget often enough to need
seventy-eight identical resubmissions for a single observation to land.

Extrapolating the *read* cost alone would have put that crossing near thirty-five thousand
rows. The write path reaches it at a tenth of that, which is the distance between the two
paths and the reason the read measurement is quoted here as a lower bound rather than a
prediction.

## Why it compounds

The three facts close a loop:

1. every reply is preceded by work proportional to the audit table;
2. every expired attempt writes another audit row;
3. the table can never shrink.

So a slower receiver produces more timeouts, more timeouts produce more rows, and more rows
produce a slower receiver. Three observations on three hosts added ten and a half thousand
rows in one campaign. The loop does not need load to run away; it runs away on its own
replication chatter.

## What the G2 campaign does and does not say about this

Campaign 5 passed the evidence gate. It also produced, on one host, a single observation
needing seventy-eight attempts. Both are true, and the second is why a passing G2 says
nothing about operational viability — the gate is a correctness gate about accounting for
attempts, and this defect produces attempts faster than it accounts for them.

It also explains the population the gate now separates: 123 of campaign 4's 149 incomplete
attempts had reached no peer at all. Those are connections that failed before the receiver
read anything, which is what a receiver too busy to accept looks like from the other side.

## Two fixes attempted and measured, neither kept

Both were written, built, run against the candidate's own 69-test suite and timed against a
real campaign store of 3672 audit rows, release build, on one workstation. The baseline for
all three numbers is the unmodified candidate.

**Attempt one: remember which rows this process already verified.** Refused immediately by
`audit_rows_are_immutable_idempotent_bounded_and_fail_closed`, which drops the update
trigger, edits a column, restores the trigger and requires the store to notice. It was right
to refuse. The append-only trigger is a guard, not a proof, and a cache that trusts it makes
the store blind to exactly the tampering it exists to detect. **A weakening, not an
optimisation.**

**Attempt two: fingerprint the tables, and skip the per-row proof while the fingerprint is
unchanged.** One scan and one hash against a re-serialisation, a SHA-256 and an SQL query per
row. This keeps every detection: any change to any column moves the fingerprint, whether or
not it was made through this connection and whether or not the triggers were in place. All
69 tests pass.

A first version of it hashed three columns that look like identity — `audit_event_id`,
`record_json`, `sha256` — and the immutability test walked straight through it, because it
edits `request_frame_bytes`, a column that version never read. **A fingerprint that does not
cover a column cannot notice that column changing.** Widened to every column, the test
passes again.

| | ms per operation, 3672 rows |
| --- | ---: |
| unmodified candidate | 140 |
| fingerprint over three columns — **unsafe** | 70 |
| fingerprint over every column | 100 |

Thirty per cent, and still linear: hashing twenty-one columns costs about what the per-row
proof saves. **The change was reverted.** Thirty per cent does not justify a new binary
digest and the requalification of everything pinned to the current one, and it does not
change the shape of the curve — which is the whole problem.

## What a fix has to be, and what it is not

**Not a longer sender timeout.** Raising `IO_TIMEOUT` moves the crossing point and keeps the
curve. The table still grows and the receiver still verifies all of it.

**Not skipping verification.** The immutable-chain and receipt verification are what make the
store's claims worth anything, and the G2 predicate rests on them.

**Not a cheaper full pass either**, which is what the two attempts above measured. Any
scheme that still touches every row on every reply keeps the curve and only changes its
slope.

The work per reply has to stop depending on the table's size at all. That means a **durable**
running digest, maintained as rows are appended and stored beside them, so a replica verifies
the new rows and compares one value for everything older — O(new) instead of O(all). It
cannot be per-process: a fingerprint this process computed proves nothing about a table a
previous process wrote, which is why both attempts above recompute from scratch at startup
and why neither escapes the scan. That changes the candidate, so it is a new
candidate, a new binary digest, and a requalification of everything pinned to the current
one. It is its own lot and it is the one that decides whether this manager can run for a
day.
