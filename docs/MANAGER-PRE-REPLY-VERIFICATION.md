# The unbounded pre-reply verification, measured

Status: **shape 2 chosen and implemented on 2026-09-17 (lot V2-R, branch
`claude/v2-manager`), its integrity model corrected the same day after an adversarial review
(lot V2-S, branch `claude/v2-store`); not yet qualified on the laboratory.** The integrity model and
its trade-off are in [its section](#the-chosen-shape-2026-09-17-full-verification-leaves-the-reply-path).
How a replica rejoins its peers, and how little an idle manager exchanges, are in
[the last section](#rejoining-and-idling-2026-09-17-the-exchange-lot) (the exchange lot, branch
`claude/v2-exchange`). Everything before those two sections is the original measurement of the
candidate, kept as measured: the defect standing between the manager and any claim of long-running
viability. It is not a G2 evidence problem: the G2 gate passes. It is a property of the candidate,
and it gets worse on its own.

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

## Addendum, 2026-09-14: what a durable digest can and cannot buy

The paragraph above says a durable running digest makes a reply O(new): verify the rows added
since the digest, compare one value for everything older. Before that becomes a lot, the
claim has to survive the test that refused attempt one.

**It does not.** `audit_rows_are_immutable_idempotent_bounded_and_fail_closed` drops the
append-only trigger, edits a column of an *old* row, restores the trigger, and requires the
store to notice. A digest stored beside the rows, however it is computed, was computed before
that edit and says nothing about it. Noticing an edit to an old row means reading that row —
there is no digest, chained, Merkle or otherwise, that lets a verifier detect a change to a
leaf it does not read. Attempt two measured exactly this: every column, every row, still
linear. A durable digest changes only *who* pays the first scan (the writer, once) and not
whether every subsequent verification pays it again, if "verification" keeps meaning "detect
any edit to any row before this reply".

So the choice is not an implementation detail; it is **what the reply path promises**, and
it is the candidate's contract, which makes it Codex's decision. The two honest shapes:

1. **Full verification stays on the reply path.** The curve stays. The sender's timeout, the
   retry storm and the runaway loop are then facts of the design, and the manager cannot run
   for a day at three hosts. This is the current candidate.
2. **Full verification leaves the reply path.** It runs once at startup (O(all), before the
   first reply), and again periodically in a bounded background pass; a reply verifies only
   the rows appended since the last full pass, against a durable digest of that pass — O(new)
   — and the store's claim becomes "an edit to an old row is detected at the next full pass or
   restart, never later than the declared interval", instead of "before every reply". The
   immutability test is then re-expressed against that claim (it edits, then asserts the
   next full pass refuses), not weakened; the G2 predicate, which rests on attempts being
   accounted for, is untouched. This is a new candidate, a new binary digest and a
   requalification, as the paragraph above already said — plus one sentence in the contract
   that the operator has to accept: detection has an interval.

The recommendation is the second, with the interval stated as a number the operator chooses,
because the first is a manager that stops answering. Neither is built here: this addendum
exists so that the lot starts from the true shape of the problem rather than from a digest
that would have been refused by the same test as attempt one.

## The chosen shape, 2026-09-17: full verification leaves the reply path

**Decision.** Shape 2, taken by the operator to make the manager healthy — vital target V2: the
replicas boot, exchange, and keep a majority when one host is lost. Measured on 2026-09-17 on every
laboratory replica: 12,710 to 21,663 audit rows (26 to 44 MB) against 26 facts, replication links
failing for about 23 hours, appends answering `uncertain` or `busy`, and a replica unable to boot
because its boot fact append missed its window. Shape 1 is that manager. Shape 2 is implemented on
branch `claude/v2-manager`; schema version 3, the wire protocol and the G2 attempt accounting are
unchanged. It is a new candidate: a new binary digest and a requalification, as stated above.

**Correction after review (lot V2-S).** The adversarial review of V2-R found the model weaker than
this section said: rows inserted before the last verified row escaped every transaction, stored bytes
that an operation could not read at use closed nothing, a schema found corrupt at a later open closed
nothing, and an operation could commit after the store had closed. Branch `claude/v2-store` fixes the
code wherever the model can hold without a schema change — the rowid rule, reads at use, the
fail-closed state at open and at commit — and states the rest exactly: `REPLACE`, the write lock an
open takes, a closed state that belongs to a database file. The text below describes that branch.

### The integrity model

Each process keeps, for each database file it opens — device, inode and birth time, with replica
and topology — the last verified row of each append-only table (`facts`, `receipts`,
`exchange_audit_events`): its rowid and its stored checksum.

**The rowid rule.** The Store never names a rowid, never deletes a row, and the triggers refuse
`DELETE`, so SQLite numbers the rows of each table 1, 2, 3, … in insertion order: a plain insert takes
the largest rowid plus one, and a failed statement or a rolled-back transaction takes its row back, so
the next insert reuses that rowid. The model relies on it. A complete verification requires the rows
of each table to be exactly the rowids 1 to their count, so the rows at or before the last verified
row are exactly the verified ones, and a row that any writer adds without removing a stored row either
has a rowid at or below 0 or follows the last verified row. Every store written by an earlier build
satisfies the rule ([rowids of existing stores](#rowids-of-existing-stores)).

| When | What is verified |
| --- | --- |
| A process first opens a database file | Everything, in a read transaction: schema shape, the rowid rule, every receipt, every fact through the reducer, every audit row and every attempt sequence — the former `verify_all`. Later opens by the same process (the resident opens a store per connection, per append and per peer attempt) only check the schema and that the last verified row of each table is unchanged. |
| Every transaction: a write, or the read transaction of `export`, `inspect` and `check_service` | The schema shape; the presence and stored checksum of the last verified row of each table, and if either changed, a complete verification inside that transaction; that no table has a row before rowid 1, one seek per table; every row appended since, which must continue its table's rowids without a gap, with the per-row checks of a complete verification, and the complete stored sequence of every attempt those rows belong to. The positions are read before the transaction takes its snapshot, so a verified row missing from the snapshot can only have been changed in place. |
| An operation reads a stored row it relies on | The facts that `observe`, `import`, `export`, `inspect` and `check_service` load are verified through the reducer; an audit record loads no fact. A replayed receipt is verified before its response is returned. A replayed audit event is verified before it is compared. The stored rows of the attempt a new audit row extends are verified and checked as a sequence with it, and the stored receipt it references is read and checked against it. |
| Periodically | `Store::verify_full`: a complete verification in a read transaction, which does not block writers. The resident runs it every `full_verification_interval_ms`, 600,000 ms (10 minutes) by default, between 1 second and 1 day. |
| `--inspect-store` | A complete verification of a private copy, with the rowid rule and SQLite `integrity_check`. `--inspect-store --facts-only` verifies the schema, the identity, the rowid rule of `facts` and every fact of a private copy, reads no receipt or audit row, and prints the facts only. |

**Fail closed.** Any failure to read or verify a stored row once a transaction holds its snapshot —
at a first open, of a transaction's last verified rows, rowids or appended rows, of a row read at
use, or in a periodic pass — closes the database file for the rest of the process, as does a schema
found corrupt when the process opens the file again, by its read-only preflight or by the open's own
transaction. Every later open, append, import, audit record and export of that file in that process
returns the same error: `corrupt`, or `storage` when stored bytes could not be read. No transaction
of the process commits on a closed file: every commit holds the closed state shared while SQLite
commits, and recording a failure takes it exclusively, so a failure is recorded either before a
commit, which then rolls back and returns it — also when the operation began, or waited for the write
lock, before the failure — or once the commits in flight have finished. An error that prevents a
transaction from taking its snapshot or its write lock (a busy or locked store) is not a verification
failure and closes nothing: SQLite reports busy and locked only while a transaction takes those,
never on a read inside a snapshot it holds. Neither is a failed write or commit. **A full disk is not
guaranteed to close nothing**, though: a read an operation makes after its own writes goes through
the same read-at-use rule, and SQLite may have to spill the pages that operation dirtied before it
can serve that read — an authenticated import appends its facts and its receipt before it reads the
rows its audit row extends — so `SQLITE_FULL` or an I/O error there closes the store. It is a storage
failure reported as one (`storage: …`), and the store reopens on a disk with room, since a new
process verifies it completely and finds it sound. The closed state belongs to the database file, not
to its path: another file renamed over the path is another database file, which the process verifies
completely at its first open before it serves it. A new process verifies the store completely at its
first open, so a resident restarted on a corrupt store refuses to start. In the resident, a closed
store answers appends with `append_observation_uncertain`, fails every exchange, and keeps status and
shutdown available. **The closure is written to standard error at the moment it is recorded**, naming
the store path and the failure, whichever operation met it: the operation itself may answer with
nothing else, and the periodic pass that reports it is up to one interval away. A failed periodic
pass is written to standard error too.

**What fails closed with the store.** The resident's `status` carries `store_closed` and
`store_closed_reason`: the closed state of the database file for that process, and the failure that
closed it (`corrupt: …`, `storage: …`). A closed state the resident cannot read is reported closed —
an unknown state is not an open one. The administration app's origin asks the resident for it before
every request under `/admin`, the API included, and answers `503` with the reason `the manager store
is closed` and the resident's own reason as `detail` when the store is closed, when the resident does
not answer, or when it answers without the state. Nothing under `/admin` is served meanwhile: no
administrator is read, named or revoked over a store that no longer verifies. The active manager's
mark is a separate gate and keeps its own reason (`not the active manager`), and `/` and `/ready`
still answer with the mark: a closed store does not withdraw what PodMesh published.

The candidate check of a new audit row reads the rows of its own attempt through the
`UNIQUE(direction, attempt_id, phase)` index, in both directions, instead of the whole table. Every
sequence rule is scoped to one attempt ID — the phase rules of one direction, and the rule that an
attempt ID cannot span both directions — so over a verified store the candidate check gives the same
result, with the same error, as the whole-table check did. The rules themselves are unchanged.

### Rowids of existing stores

Every build that wrote a schema v3 store inserted rows only as the rowid rule assumes. The repository
holds four versions of `durable.rs`, on all its branches: `aef51e3` wrote schema v2, which v3
refuses; `e3251f2` (the G2 candidate, unchanged in the deployed `ff77b1f` and in `e7a536a`),
`47ff761` and `5f492a8` (lot V2-R) write the three tables only with `INSERT INTO facts(event_id,
fact_json, sha256) VALUES (…)`, `INSERT INTO receipts (…) VALUES (…)` and `INSERT INTO
exchange_audit_events (…) VALUES (…)`: one row per statement, no rowid named, no `OR IGNORE` or
`OR REPLACE`, no `DELETE` or `UPDATE`, no `AUTOINCREMENT`. No build could leave a gap:

- a failed statement or a rolled-back transaction removes its row, and the WAL frames of a
  transaction interrupted by a crash have no commit record and are ignored: the next insert reuses
  the rowid;
- the triggers that refuse `DELETE` are created in the same batch as the tables;
- nothing runs `VACUUM` on a store (the inspection fallback runs `VACUUM INTO` on a private copy),
  and either form keeps the rowids of a table with an index, as each of the three tables has for its
  text primary key (checked with the bundled SQLite 3.46.0, gaps included; `VACUUM` renumbers only a
  table without an index, and without a gap); byte copies — the universe entrypoint's copy-up, the
  campaigns' preserved stores — keep them too.

Measured on 2026-09-17: the 492 schema v3 stores on the workstation (laboratory runs rescued on
2026-09-16, the review's scale stores, the V2-R bench stores), holding 367,424 audit rows, 3,001
receipts and 1,633 facts, have no table with a gap. The laboratory replicas' stores were not read;
`--inspect-store` now checks the rule, so running it on a copy of each store before this build is
deployed confirms them.

### The trade-off, stated exactly

An in-place change of a row at or before the last verified row of its table, made by an actor who
bypasses the store, and read by no later operation, is detected **at the next complete
verification** — the periodic pass of each process that has the store open, the first open by any
new process (a restart, the one-request executables), or `--inspect-store` — and not before the
next transaction. A change in place includes a row replaced by `REPLACE` and a removed row. A
replacement that keeps its rowid is found by the content checks of that verification; one that takes
the next rowid is verified as an appended row by the next transaction, and the gap it leaves is
refused by the rowid rule at the next complete verification, or by the next transaction when the
removed row was the first. A removed row leaves the same gap, except at the end of its table: a table
whose last rows were removed is shorter and still verifies when no remaining row depends on them, as
a store truncated in place does. In a running resident that is at most the configured interval, plus the
length of one pass. Until then that process keeps serving: a mutation or a reply made in that window
is not refused because of that change. The same holds for an edit of the last verified row that keeps
its stored checksum column unchanged.

Not weakened:

- a row that any writer adds without removing or replacing a stored row — another process, or a
  direct SQL insert, with or without an explicit rowid — is verified or refused by the next
  transaction of every process that has the store open, before that transaction reads or writes:
  after the last verified row it must continue the rowids and pass the per-row checks, and anywhere
  else it breaks the rowid rule;
- a schema change, including a dropped trigger, is refused by the next transaction, and by the next
  open of that file by the process, which then closes it;
- a store replaced or truncated in place so that the last verified row of a table is missing or
  carries another checksum is verified completely again by the next transaction;
- a replayed receipt or audit event is verified before it is returned, so a checksum refusal still
  precedes that replay (HA-14);
- the per-attempt sequence rules are exactly as strict;
- the G2 accounting: every attempt leaves the same prepared and terminal rows on both hosts.

**Why the operator accepts it.** The SQL triggers refuse `UPDATE` and `DELETE`, but they do not make
an old row unchangeable: `REPLACE INTO` rewrites a row with every trigger in place, because SQLite
fires no trigger for the delete of its conflict resolution while `recursive_triggers` is off — the
default, and a setting of the writer's own connection. Preventing it needs a schema change, for
instance an insert trigger that refuses a row whose rowid or primary key already exists; schema v3
has none, and lot V2-S, bound to open the laboratory's v3 stores unchanged, does not add one. So an
old row can change whenever an actor with write access to the database file writes to it directly.
The store's checksums detect corruption; they never authenticated such an actor, who can rewrite
rows and checksums together (see `experiments/manager-ha/README.md`). The detection interval is a
number the operator chooses. Against that, shape 1 measured, on the test below, a receiver needing
7.46 s before its first reply byte at 30,004 audit rows, against a sender that waits 2 s: a manager
that cannot exchange at all, and whose failures add the rows that slow it down.

### Three more costs removed

- **Export under a read transaction.** `export`, `inspect` and `check_service` do their work in a
  read transaction that does not block writers. Opening a store still takes the write lock:
  `Store::open` checks the identity and the last verified rows in an `IMMEDIATE` transaction, and the
  resident opens a store before each export, each outgoing exchange, each periodic pass and each
  incoming connection, so each of those still waits once, briefly, for a writer that holds the lock.
- **The preflight copy after the process's own checkpoints.** The read-only preflight copies the
  whole database, WAL and SHM whenever the database file's size or times differ from a state the
  process already preflighted, and every checkpoint changes them. A write operation now records the
  state its own commit and checkpoint left, when the state before the operation was already
  preflighted by the process. Without it, 3,000 in-process exchanges made two full copies per
  exchange and grew from 14.6 to 56.7 ms per exchange. *Superseded by the second review's correction
  below*: a preflight now runs only at an open made while this process holds no connection on the
  file, so a process that keeps a store open never copies it again, and the state a write operation
  records serves the next open by a process that holds no connection on it.
- **Unchanged snapshots are not exchanged every interval.** The resident pushes its snapshot to a
  peer when it differs from the digest that peer last acknowledged with an authenticated receipt,
  and otherwise every `unchanged_snapshot_refresh_ms`. This lot introduced it at 60,000 ms; the
  default is 600,000 ms since the exchange lot below, which is what a resident runs. An idle three-replica manager no longer adds six audit
  rows per ordered pair per interval. Every exchange that does happen keeps the same audit
  accounting.

### Measured

`experiments/manager-ha/tests/bounded_cost.rs` times the resident's store work on stores seeded
with realistic attempts (inbound replayed imports, accepted and unavailable outbound attempts); the
first open verifies every seeded row. The candidate column is the unmodified tree at `e7a536a`
(identical in these crates to the deployed `ff77b1f`) run with the same test, 5 repetitions; V2-R
ran 25. Medians, one workstation, stores on tmpfs so the numbers are the work that depends on the
tables rather than the device's fsync latency.

| Resident store work | candidate, 1,004 rows | candidate, 30,004 rows | V2-R, 1,004 rows | V2-R, 30,004 rows |
| --- | ---: | ---: | ---: | ---: |
| append: open and observe | 29.9 ms | 907 ms | 0.82 ms | 0.80 ms |
| one audit insertion | 57.6 ms | 1,858 ms | 0.48 ms | 0.47 ms |
| receiver before its first reply byte: open, observed, import, reply prepared | 235 ms | 7,462 ms | 2.49 ms | 2.53 ms |
| export: open and export | 30.1 ms | 913 ms | 1.04 ms | 1.04 ms |
| first open by a process | 5 ms | 140 ms | 40 ms | 1,064 ms |

With the stores on the ext4 build volume (about 5.7 ms per fsync'd commit), V2-R at 1,004 and
30,004 rows: append 5.09 and 6.22 ms, audit insertion 4.94 and 5.43 ms, receiver before its first
reply byte 18.5 and 17.9 ms (slowest 139 and 125 ms), export 1.07 and 1.08 ms, first open 38 ms and
1.05 s.

`experiments/manager-network/tests/bounded_exchange.rs` runs 3,000 authenticated exchanges between
two stores in one process, each side keeping one store open and opening it for every exchange as
the resident does, a new fact every hundred exchanges. On tmpfs the median time per exchange is
5.06 ms over exchanges 300–599 and 6.00 ms over the last 300, while the receiver's audit table grows
to 12,000 rows; with a single fact it is 4.96 and 5.00 ms, so the remaining drift is the snapshot
growing from 4 to 30 facts, not the audit table. On the ext4 build volume, where each exchange
pays six fsync'd commits, it is 34.8 and 31.2 ms (slowest 257 ms).

**Lot V2-S.** The rowid checks, the reads at use and the commit under the integrity lock keep
the per-transaction work flat. `bounded_cost.rs` on tmpfs, three runs of 50 repetitions each, the
median of the three medians, before (`87690e1`) and after, at 1,004 and 30,004 audit rows:

| Resident store work | before, 1,004 rows | before, 30,004 rows | V2-S, 1,004 rows | V2-S, 30,004 rows |
| --- | ---: | ---: | ---: | ---: |
| append: open and observe | 0.90 ms | 0.92 ms | 0.93 ms | 0.94 ms |
| one audit insertion | 0.47 ms | 0.49 ms | 0.51 ms | 0.52 ms |
| receiver before its first reply byte | 2.52 ms | 2.58 ms | 2.60 ms | 2.64 ms |
| export: open and export | 1.12 ms | 1.15 ms | 1.15 ms | 1.18 ms |
| first open by a process | 36.6 ms | 1,049 ms | 36.9 ms | 1,010 ms |

A first open and the inspections, on the review's seeded stores (21,700 and 100,004 audit rows,
44 and 202 MB, one fact) on the ext4 build volume, private copies on the same volume, release
builds, medians of five runs after a warm-up, peak resident set size the largest of the five:

| | 21,700 rows, before | V2-S | 100,004 rows, before | V2-S |
| --- | ---: | ---: | ---: | ---: |
| first open by a process: time | 730 ms | 708 ms | 3,479 ms | 3,316 ms |
| first open by a process: peak memory | 86.8 MiB | 27.7 MiB | 389 MiB | 102.7 MiB |
| `--inspect-store`: time | 1.15 s | 1.12 s | 5.61 s | 5.44 s |
| `--inspect-store`: peak memory | 87.4 MiB | 48.9 MiB | 389 MiB | 194.9 MiB |
| `--inspect-store`: output | 20.9 MB | 20.9 MB | 96.1 MB | 96.1 MB |
| `--inspect-store --facts-only`: time | — | 0.05 s | — | 0.23 s |
| `--inspect-store --facts-only`: peak memory | — | 5.4 MiB | — | 5.4 MiB |
| `--inspect-store --facts-only`: output | — | 411 bytes | — | 411 bytes |

The preflight of a first open and every inspection capture the store by reading its database, WAL
and SHM twice and keeping them only when both reads are equal. Both reads were held in memory, about
twice the store at peak. A store whose files total more than 16 MiB is now copied into the private
directory and read again against that copy through 1 MiB buffers; a smaller one is still read twice
into memory, which keeps the two reads of a small store closest together. What remains of a first
open's memory is the complete verification, which holds every audit row to check the attempt
sequences; the full inspection also holds its output. The facts-only time grows with the database
file only through that capture, about a millisecond per megabyte here.

A capture of a store that is being written can find the two reads different three times in a row
and fail. Each row below starts from a copy of the store, with one process appending an audit row
at the interval while another inspects it:

| Store and writer interval | inspections that succeeded, before | V2-S |
| --- | ---: | ---: |
| 390 KB store, 5 ms | 300 of 300 | 300 of 300 |
| 390 KB store, 50 ms | 300 of 300 | 300 of 300 |
| 44 MB store, 50 ms | 9 of 20 | 13 of 20 |
| 44 MB store, 500 ms | 20 of 20 | 20 of 20 |

### What this does not settle

- **The first open is still linear in the store.** A new resident verifies everything before it binds
  its control socket: about 1.05 s at 30,004 audit rows on this workstation (about 31 µs per row), to
  be measured on the laboratory hosts. The universe entrypoint gives the socket's bind 25 s (catching
  up is outside that budget since the exchange lot's readiness contract below); a store that keeps
  growing will cross it. The complete verification also holds every audit row in memory, 103 MiB at
  100,004 rows. Change-driven pushes and the ten-minute refresh slow the growth; they do not bound the
  table.
- **The table still only grows.** Retention, sealing or rotation remains a separate design.
- **A preflight copy remains after a change by another process**, and at a process's first open.
- **The facts-only inspection still copies the store.** Its verification and its output do not grow
  with the audit table, but its capture reads the whole database file twice, and the private copy
  needs the file's size in `$TMPDIR`.
- **A busy store can refuse an inspection taken from another process.** With a writer every 50 ms,
  7 of 20 inspections of a 44 MB store failed their three captures (11 of 20 before this lot). The
  administration app reads facts this way — its origin runs the inspection as a separate process, so
  it captures a copy — and a resident that writes that often makes its pages fail about as often. An
  inspection made inside a process that has the store open reads it through SQLite instead and does
  not fail this way.
- **An old row can change without a trigger being dropped**, through `REPLACE`; it is detected at the
  next complete verification. Refusing it at the moment it is written needs a new schema version.
- **Opening a store takes the write lock**, briefly, before every export, exchange and pass of the
  resident. A read transaction would do for an existing store; it is not changed here.
- **The rowid rule on the laboratory stores**: run `--inspect-store` on a copy of each replica's
  store before this build is deployed.
- **Laboratory qualification**: a soak of the three replicas on this candidate, with the incomplete
  attempt counts, exchange latencies and restart times measured there.

### Correction after the second review, 2026-09-17: the capture, the closed state, the administration

A second adversarial review of lot V2-S found three things wrong and two documents overstated.

**The capture released the process's own SQLite locks.** The read-only preflight and every
inspection read the live database, WAL and SHM with plain file reads, and a process that holds SQLite
connections on a file must never do that: POSIX advisory locks are per process and per inode, so
closing *any* descriptor on that inode drops *all* of that process's locks on it ("How To Corrupt An
SQLite Database File", §2.2). SQLite keeps its own descriptors open for exactly this reason; an
`open`/`close` pair around it, from the same process, undoes that. Measured at `b8cb600`: a second
open of a store this process already had open deleted the live WAL and SHM of another process's
connection (a torn "hot journal" recovery), an append this process had acknowledged was lost after a
crash, and a store closed with `disk I/O error` while another process read it and checkpointed it.
It was latent since `e3251f2` and is in the laboratory's deployed build.

The store now reads a file's bytes only while **this process holds no SQLite connection on it**. Each
database file has a live-file entry, keyed by the directory's device and inode and the file name,
counting the connections this process holds on it, with a gate held while a connection is opened or a
capture starts; the connection count drops when the last `Store` on the file is dropped. A first open
(no connection held) preflights as before, on a private copy, so the protection against a swapped or
foreign file at the path is unchanged. A later open, while the process holds a connection, copies
nothing: it compares the file identity (device, inode, birth time) with the file this process opened
and refuses `manager store path no longer names the database file this process has open` — reading
neither the file nor its sidecars, since the sidecars at those names may belong to the open file —
and the open's own `IMMEDIATE` transaction still checks the schema, the identity and the last
verified rows. An inspection of a file this process has open reads it through a read-only SQLite
connection in one read transaction instead of copying it; an inspection of a file no connection holds
copies it as before. A child process for the capture was considered and refused: a library cannot
re-execute an arbitrary host binary (or a test harness) safely, and the design above removes the
plain reads entirely rather than moving them.

**The closed state under a lock shared by commits.** Commit `f2f34d4` held the integrity state, a
plain mutex, across `COMMIT`, and claimed that this "adds no contention between writers" and that "a
reader or a failing verification waits at most for one commit". The first half was right — SQLite's
write lock already serialises the commits — but the second was wrong: every read of the verified
positions took the same mutex, so same-process readers queued behind the whole write pipeline. The
closed state is now a read-write lock of its own, held **shared** by commits and taken exclusively
only to record a failure, with the verified positions under a separate mutex. `commit_lock.rs` (the
review's probe: one writer recording an audit row with a 2 ms pause, one exporter in a loop, one
appender opening a store every 50 ms, on a 21,700-row, 42 MB store, 60 s per run, release build):

| exports in 60 s | `87690e1` (before the lot) | `b8cb600` (reviewed) | this branch |
| --- | ---: | ---: | ---: |
| count | 18,515 / 19,130 / 19,696 | 6,706 / 6,771 | 17,574 / 20,050 / 20,490 |
| p50 | 1.88 / 1.72 / 1.60 ms | 6.59 / 6.63 ms | 2.05 / 1.62 / 1.58 ms |
| p99 | 5.10 / 4.82 / 5.17 ms | 50.9 / 49.9 ms | 5.21 / 4.57 / 4.35 ms |
| max | 7.87 / 5.83 / 13.4 ms | 349 / 297 ms | 7.93 / 9.29 / 8.67 ms |
| write transaction p99 | 49.0 / 60.0 ms | 63.4 ms | 73.7 / 59.2 ms |
| appends refused by a capture race | 6 / 9 / 14 | 47 / 42 | 0 / 0 / 0 |

Readers are back at the cost they had before the lot, writers are unchanged, and the appending opens
still wait for SQLite's write lock (p99 0.74–1.34 s in every column, the cost already listed above as
"opening a store takes the write lock"). The last row is the capture fix: an open by a process that
already holds the store open no longer copies it, so it can no longer lose the race against the
writer.

**The administration app no longer failed closed with the store**, since it reads facts through a
separate inspection process. It does again: see "what fails closed with the store" above.

**Inspecting a store in the laboratory.** Inspect a **stopped** universe's store, or the live one
through the replica's own CLI (`podmesh-managerd --inspect-store [--facts-only]`), which captures the
file by reading it twice and keeping it only when both reads agree, and retries when they do not.
Never inspect a `podman cp` (or `cp`, or `rsync`) of a live store: a plain copy of a database and its
WAL, taken file after file while a writer commits, is torn. The review copied both files in each
order while a writer appended, sixteen times: not one copy matched the store. Two were unreadable
(`database disk image is malformed`), five had a gap in the rowids, most were short of committed rows
(1,412, 1,500 or 2,500 of 4,500), and fourteen failed SQLite's own `integrity_check` — including
copies whose rows looked contiguous. A torn copy is not the replica's state, and the rowid rule
refuses it, so what such an inspection reports says nothing about the store it was taken from.

## Rejoining and idling, 2026-09-17: the exchange lot

Status: **implemented on branch `claude/v2-exchange`, on top of lot V2-R; not yet qualified on the
laboratory.** The schema, the wire protocol and the G2 attempt accounting are unchanged: every
exchange this lot adds is an ordinary exchange with its prepared and terminal audit rows, and an
append refused while catching up touches no table.

### The hazard: a store that lost facts of its own

A fact's identity is `<replica_id>:<20-digit producer sequence>`, and a replica numbers a new fact
after the highest sequence of its own origin in its store. A store that lacks some of its own facts,
deleted or restored from an older copy, numbers its next fact with a sequence its peers already
hold, with other bytes: the boot fact's value carries the host's boot ID. Every import that carries
both versions is then refused as an event identity collision, in both directions, for good.
Importing its own older facts back from a peer first avoids it: an imported fact of its own origin
moves the next sequence past it.

### Catch-up before the first local append

- Until a process has caught up with its peers, `append_observation` answers
  `append_observation_catching_up`, after its authorization checks and without touching the store.
  The state latches: once caught up, a process stays caught up.
- A peer is caught up with when this process has committed or replayed an authenticated import from
  it that carried every fact the peer was known to hold, or when that peer's latest authenticated
  receipt for a push of this process counted exactly the pushed facts. A peer that imported a snapshot
  holds every fact of it, so an equal count means it held nothing this replica lacked; a larger count
  means it held facts this replica lacked, and the peer stays *ahead* until an import from it carries
  at least that many facts (a history only grows, so such an import carried all of them). The receipt
  binds those counts under the pair key, and the transport now returns the count it pushed. A refused
  import or push counts for nothing.
- Every peer caught up with catches the process up. The catch-up window, `catch_up_window_ms`
  (15,000 ms by default, between 1,000 and 15,000), forgives the others only when all of these hold:
  - the latest fact of this replica's origin in the store at start was appended by that store itself,
    as its `observe` receipts record (`Store::highest_local_observation`), not imported back from a
    peer. An emptied store, or one whose latest own facts were only imported back, never qualifies,
    however many later processes start on it;
  - the window has elapsed since the process started exchanging (that clock starts when the
    resident begins exchanging, after its store's first complete verification, not at the process's
    first instruction);
  - at least one peer is caught up with on evidence no older than the window's end;
  - every other peer was attempted after that end and drew no authenticated answer, and none of
    them is known to be ahead. A peer that answered an authenticated refusal was reached, not
    forgiven.

  When the window ends, every peer is made due again at once, with a fresh operation ID, its
  acknowledgement forgotten and its backoff restarted: the evidence the window weighs is then taken
  after its end. Evidence is dated by receipts, not by imports. A receipt counts the peer's history
  at a moment no earlier than the attempt that drew it (for a replayed receipt, no earlier than the
  operation ID this process created for it), while a snapshot this replica imported was exported by
  the peer at a moment this process cannot date. A peer is therefore caught up with *as of* the
  latest receipt from it that counted exactly the pushed facts, or that counted a history an import
  from it has since carried in full.

  The window forgives only peers this process attempted after the window's end and could not reach. A
  peer that answers an authenticated refusal is reached, not forgiven: its exchange service works and
  its history stays unknown, so the process keeps waiting for it. A replica that reaches no peer at
  all never appends, whatever its store: accepted, since it cannot tell a lost network from a lost
  history.

  Why the evidence has to be fresh: only this replica creates facts of its origin, but the peers
  holding ones its store lacks do not keep them to themselves. A peer caught up with early in the
  window can receive them, relayed, from the very peer the window would forgive, and its earlier
  match says nothing about that. With the fresh round that peer answers a receipt counting more facts
  than were pushed, the window does not apply, and the append waits for the import that carries
  them.
- A topology without peers is caught up at once.
- The evaluation follows what an event teaches, never precedes it: a receipt that shows a peer ahead
  counts before the window is weighed again, not after it has latched.
- `status` reports `catch_up`: `caught_up`, `caught_up_by` (`every_peer`, `window` or `no_peers`),
  `caught_up_after_ms`, `peers_imported`, `peers_matched`, `peers_missing`, `peers_ahead`,
  `peers_not_attempted` (those it has neither caught up with nor attempted),
  `own_facts_at_start`, `latest_own_fact_appended_locally`, `window_ms`, `appends_observed` (the
  appends this process answered observed — one of them, for a universe replica, is its boot fact)
  and `blocked_by`, which says why it is not ready when it can tell: `waiting_for_peers` with the
  peers it lacks, `refused_by_peer` when one of those answers authenticated refusals, or
  `identity_collision` with that peer and the colliding event ID. The last one never resolves by
  waiting: every import from that peer is refused and every push to it is refused, so it is reached,
  never caught up with and never forgiven by a window. The universe start ends on it (exit 2, the
  event ID in the log) instead of running for ever without being ready; the qualification helper
  `append-observation.py` stops on it too.

A second adversarial review reproduced two more forks, both now regression tests. The window latched
on what was known before the event being learnt, so a receipt showing a peer ahead, arriving as the
first thing learnt after the window's end, was applied too late to stop it; and a peer caught up with
early in the window received the missing fact, relayed by the peer the window forgave, before the
window elapsed. The first is fixed by evaluating after applying, the second by the fresh round above.

The first rule, counting own facts at start, was refused by an adversarial review of this lot with two
reproduced forks. An emptied store that got some of its own facts back from one peer during a start
that could not catch up held own facts at its next start, whose window then let it reuse a sequence
the missing peer held. And a 1-second window elapsed while the outgoing worker was still inside its
2-second connect attempt to a stopped peer, before it reached the live peer holding the later fact.
Both are regression tests of the resident suite.

Counting imports alone would make every clean restart wait the whole window: its peers hold exactly
its facts, so none has anything to push back, and each still holds its acknowledgement from the
previous process, which it does not push again before the refresh. A replica that starts after its
peers on a fresh manager would likewise wait for their backoff, up to 30 s with the laboratory
settings, past the start budget. The receipt rule catches both up within their own first pushes.

### The readiness contract of a universe start

**Running** means the resident is up and exchanging with its peers. **Ready** means this boot's fact
is observed. A replica that has not caught up is running and not ready: it keeps exchanging, which is
what lets the replicas started after it catch up with it, and the entrypoint keeps asking for the boot
fact with the same operation ID and no deadline, writing one line of the resident's catch-up state to
the log every 30 seconds. Readiness is visible in that log (`boot fact observed`) and in the
resident's status (`catch_up.appends_observed`, `catch_up.caught_up`); whoever waits for a replica
waits for that, not for the container to be up. The laboratory's roll tool already requires the boot
fact in the log.

The 25-second start budget still bounds the control socket's bind and the retries of an `uncertain`
or `busy` answer, so a start that is about to fail still fails within the 30 seconds PodMesh observes
it for; the budget of those retries restarts after each `catching_up` answer, which can come first and
for a long time. Any other answer is still terminal, and the start then exits 2 with the resident
killed. The entrypoint takes the stop signal from the moment the control socket is bound, so a replica
that is still catching up stops as honestly as a ready one.

**One catch-up never ends, and that start is terminal.** A replica whose history already forked from a
peer's is refused every import from that peer, and refused by it: the peer is reached, never caught up
with, and never forgiven by a window, so the gate would hold for ever and a no-deadline wait would
turn that into a container running for ever without being ready. The resident therefore says what
blocks it — `catch_up.blocked_by`, with `identity_collision`, the peer and the colliding event ID —
and the entrypoint reads that state every five seconds while it waits and exits 2 on that reason, with
the event ID in the log. PodMesh then records a universe that is not running when observed. The other
reasons, `waiting_for_peers` and `refused_by_peer`, keep waiting: the first resolves when the peer
answers, the second is a policy question an operator settles and can be transient. A forked replica
restarted against healthy peers ends this way in the process tests, while a replica whose peer is
merely down waits, says `waiting_for_peers`, and becomes ready at its window.

Before this contract, catching up had to fit inside the budget. That was a fork of its own kind: an
adversarial review restarted three replicas 30 seconds apart with the laboratory settings and none of
the three ever became ready — each was killed at its budget while waiting for a peer that was itself
being killed at its own. The same schedule now ends with the three observed (measured below). The old
bound was also tighter than it was stated to be: at the window's end both early replicas answered one
`uncertain` before their append landed, so the socket had to bind within about 7 s, not 8, for a start
that used its 15,000 ms window.

### What a client does with `catching_up`

It is not a refusal, and a client that treats it as one turns a replica that is about to be ready
into a failed operation. The origin's administration API now retries the same operation ID while that
is the answer, for up to ten seconds (`CATCHING_UP_WAIT_MS`), and only then answers that this replica
has not caught up with its peers, is exchanging, and that the same request can be repeated — a 503
the operator can read. The resident's store is untouched by those answers, so nothing is appended
twice.

Two clients of the public tree still treat it as a refusal, and are not changed here (they belong to
the main tree):

- `src/manager.rs` (`manager_observe`) returns an error naming the answer. It should treat
  `append_observation_catching_up` as a typed, retryable outcome: keep the same operation ID, retry
  within the caller's deadline, and report the replica as running and not ready rather than failed.
- `tools/manager-admin.py` (`observe`) waits up to 60 s but builds a **new** UUID for each attempt,
  so each retry is a different operation. It should keep one operation ID for the whole wait, make
  the deadline an argument rather than a constant, and print the resident's `catch_up` status while
  it waits.

### Push-back

When an authenticated import from a peer commits and leaves this replica with more facts than the
peer's snapshot carried, the replica forgets that peer's acknowledgement and makes its next push to it
due at once, at most once per interval per peer. A replay does not: it reports the counts of its
original commit. An attempt that overlapped the request does not record its acknowledgement, so the
requested push follows. The transport reports the import through
`Node::serve_connection_reporting` as soon as it is durable, before the reply, with the source
replica, the snapshot's fact count and the resulting history length. The resident also learns there
that its own snapshot changed, so its status stops calling an acknowledgement current as soon as an
import made a push due, not once the connection has ended.

While a process has not caught up, an authenticated import from a peer also makes its own next
attempt to that peer due at once: the import shows the peer is exchanging again, and the process needs
a receipt of its own, which no import gives. That is what lets a replica waiting for a peer catch up
within one exchange of its return instead of one backoff.

A restarted process holds no acknowledgement and pushes to every peer at once; every live peer that
holds facts the restarted store lacks pushes them back at once. The expected catch-up of a restarted
replica with every peer up is therefore a few exchanges, its first push to each peer and each peer's
push back, each a few fsync'd commits; each unreachable peer adds, to the sequential outgoing worker,
the deadline of its attempt (connect retries up to 2 s on a host that does not answer).

### The refresh, the idle growth and the liveness bound

`unchanged_snapshot_refresh_ms` now defaults to 600,000 ms. An idle replica of a three-replica
manager adds at most twelve audit rows per refresh: two pushes sent at two rows each, two received at
four. At the former 60 s that was about 17,000 rows a day per replica; at 600 s it is at most 1,728,
and an idle store, with the cost of its first open, grows ten times more slowly.

An idle link is confirmed once per refresh. The last authenticated success of a live link is never
older than the refresh plus one interval plus one exchange with each peer, which the outgoing worker
visits in turn (an exchange is bounded by its connect, write and read deadlines of 2 s each), and a
link whose attempt failed is retried within the maximum backoff. **The liveness bound is the refresh
plus the maximum backoff plus one exchange**: 600 s + 30 s + about 6 s with the laboratory settings,
with a margin for the other peers' visits. A peer that stops answering on an idle link is reported by
the first attempt after it, at most one refresh plus that margin after the last success; a resident
that is down is visible at once, since its own status no longer answers. Peer status gains
`last_attempt_age_ms`, `acknowledged_unchanged`, `refresh_ms`, `max_backoff_ms` and `push_backs`.

The console calls a link healthy while its last authenticated success is within the refresh plus the
maximum backoff plus a 15-second margin (one exchange, a visit of the other peer and one interval) and
no attempt failed after it. A link is degraded from the first failed attempt after that success,
while the success is within the bound: the resident has just said that the peer did not answer, and
waiting for the bound would hide a dead peer for up to one more backoff. It is failing past the bound,
or when it never succeeded. A resident that reports neither figure is judged on a 60 s refresh and a
300 s backoff.

### Naming an identity collision

When the hazard above has happened anyway, every import that carries both versions of the event is
refused as a `policy_violation`, the only reason the durable import and the signed refusal carry, and
the same reason as a topology mismatch. The wire is unchanged; the collision is named locally:

- the transport reports every refusal it recorded on an authenticated request through the callback of
  `Node::serve_connection_reporting`, and after a refused import it compares the refused snapshot with
  the local history, reporting how many of its facts this node holds under the same event ID with
  other bytes and the smallest such event ID. That comparison runs once the signed refusal is
  written: the sender's answer never waits for a search whose size is the peer's snapshot;
- the resident counts, for each peer, `refused_imports` and, under `identity_collisions`,
  `imports_refused` (refused imports that carried a collision) and `event_id`, the latest one; it
  writes one line to standard error naming a colliding event ID the first time it sees that ID from
  that peer, and never again for it, however often the refusals repeat and in whatever order the
  colliding IDs arrive. A replica whose own pushes are refused names nothing: the signed refusal
  carries only its reason, so it counts the refusal and waits to refuse an import of its own to know
  which event collided;
- on the sending side the transport now returns the reason of a verified signed refusal
  (`Error::refusal_reason`); the resident counts `authenticated_refusals` and `last_refusal_reason` per
  peer, and counts a `policy_violation` refusal under `identity_collisions.pushes_refused` once it has
  found a collision in that peer's own pushes: facts are immutable, so the peer still holds the fact it
  refused ours for. A sender whose peer never reaches it cannot tell a collision from another policy
  refusal.

### The periodic verification does not silently stop

A periodic complete verification that could not run, because the store did not open or the pass
could not take its snapshot, verified nothing and closed nothing; lot V2-R nevertheless scheduled the
next pass a whole interval later, so one transient failure doubled the detection interval of the
trade-off above. Such a pass is now retried after a backoff that starts at `interval_ms` and doubles
up to `max_backoff_ms`, never later than the verification interval. A verification failure waits
for the next regular pass, since the store is already closed. The resident's main loop supervises
the verification worker like the outgoing one: if it stops, the resident exits with an error instead
of serving without its detection interval.

A verification failure need not be `corrupt`: a stored row that cannot be read once the snapshot is
held fails as `storage` and closes the store too, and every later open returns that same failure. The
worker keeps the last store it opened and asks its integrity state whether the store is closed, so a
closed store is not retried as a pass that could not run: the pass that closed it writes
`resident store verification failed: …; the store is closed for this process`, and each later pass
keeps the verification interval and writes that the store is still closed. A database file replaced
at the path by another that fails at its first open is the one case still classified as a pass that
could not run.

### Measured

Three residents on one workstation, laboratory settings (interval 1 s, maximum backoff 30 s,
default refresh and window), release build, stores on the ext4 build volume, one scenario at a time,
from a scratch copy of the process tests: ten restarts of each of the first two kinds over two series,
two of the third.

| Restart of one replica | Caught up, after it started exchanging | Boot fact observed, after its spawn |
| --- | ---: | ---: |
| clean, store unchanged, both peers live | 40–106 ms | 68–137 ms |
| store deleted, both peers live | 106–912 ms, median 140 ms | 187–973 ms |
| store with facts of its own, one peer down | 15,000 ms (window) | 15,026–15,030 ms |

The third row was measured before the window required evidence taken after its end. It now costs, in
addition, the round of attempts that follows the end: one connect deadline (2 s) for each peer that
does not answer, plus the exchange with the peer that does.

A replica restarted on an older copy of its store, whose peers each held an acknowledgement of their
current snapshot and a refresh of ten minutes, held every fact again 83–425 ms after its spawn. The
slowest samples are consistent with a peer's outgoing worker still inside an attempt towards the
restarted replica, whose connect retries last up to 2 s; this was not isolated.

After the first review's fixes, on the same workstation under a higher load (load average 3 to 6),
the same scenarios gave: clean restarts caught up in 84–226 ms, deleted stores in 144–333 ms,
push-back in 137–241 ms, and the restart with a peer down caught up at its window, 15,000 ms, its
boot fact observed 15,030–15,034 ms after the spawn. That last figure was measured under the rule of
the time, where the window latched at its end; the second review replaced it by the fresh round, and
the same restart now costs the window plus that round (below). Run in alternation with the tip before
the fixes, the clean restart took 84–161 ms against 76–175 ms and the deleted store 169–307 ms
against 175–359 ms: the difference from the table is load, not the fixes.

The resident suite measures the same paths at a 100 ms interval, debug build, `/tmp` on the root
volume, with the whole suite running in parallel beside another lot's benchmarks, over eight runs:
an emptied replica caught up 221–438 ms after it started exchanging, a replica on an older store held
every fact 167–337 ms after its control socket answered, an idle replica added 8.0–10.6 audit rows per
second at a one-second refresh (at most twelve per refresh: each visit waits up to one interval past
the refresh), and an edited row was detected 100–175 ms after a store that had been unreadable during
the periodic pass became readable again.

After the second review's fixes, four full runs of the same suite (debug, four test threads, same
workstation) gave: an emptied replica caught up 229–457 ms after it started exchanging and its first
append was observed 328–581 ms after its spawn; a replica on an older store held every fact 119–173 ms
after its control socket answered; an idle replica added 10.0–10.8 audit rows per second at a
one-second refresh; an edited row was detected 100–150 ms after the store became readable again; and a
store with facts of its own, with one peer stopped and a 2,000 ms window, caught up 4,193–4,441 ms
after it started exchanging — the window, then the fresh round, whose attempt to the stopped peer
spends its 2 s connect deadline — with its boot fact observed 4,255–4,506 ms after the spawn.

The staggered restart the review used, three replicas started 30 seconds apart with the laboratory
settings (interval 1 s, maximum backoff 30 s, 15,000 ms window) and the new readiness contract, ended
with the three boot facts observed, over four runs: the first replica, alone for 30 s, was observed
30,461–30,503 ms after its spawn (caught up by its window 30,188–30,268 ms in, within 300 ms of the
second replica's first exchange), the second 17,352–17,366 ms after its own spawn (its window plus the
fresh round), and the third, with both peers up, 1,722–1,729 ms after its spawn (every peer). Under
the previous contract the same schedule observed none of the three: each was killed at its 25-second
budget.

### What this does not settle

- **A store whose latest own fact it appended itself, restored from an older copy, while the only
  peer holding its later facts is out of reach.** This is the accepted hazard, and these are its
  bounds. It needs, together: a store that is an older copy of itself whose latest own fact it
  appended itself — any file copy inherits the `observe` receipts, so a recovery-point restore, a
  rollback and a lost fsync all qualify, and nothing in the store tells such a copy from a current
  one; later own facts held only by replicas this process cannot reach through the whole window and
  the round that follows it; and those replicas staying out of reach until the append. A holder that
  answers before the fresh round ends prevents it: measured with a 15,000 ms window and one holder
  down, a holder that came back 14.0 s and 17.0 s after the start gave no fork, one that came back at
  19.0 s and 21.0 s did, and the window latched 17,127–17,150 ms in — the end of the window plus the
  2 s connect deadline of the attempt that found the holder absent. Before the fresh round the same
  boundary was the window itself (14.5 s no fork, 15.5 s fork).

  When it does happen, the fork spreads as far as the facts do and no further: the replica that
  lacked the fact imports the forked version and keeps exchanging with the replica that made it,
  while the holder of the first version is refused by both and refuses both. Measured end to end,
  all four links carry the same `identity_collisions.event_id` (`r2:…02`), each of the three
  residents names that event once per peer on standard error, and the three stores hold two facts
  each. The holder is then reachable and refusing, so no window forgives it: it never catches up and
  appends nothing more. It says which it is — `catch_up.blocked_by.reason` is `identity_collision`,
  with the peer and the event ID — and its universe start ends on that rather than running for ever
  without being ready. Nothing repairs the fork; an operator has to.

  **The way out, recorded and not promised.** Nothing inside the store tells a copy of itself from
  the original: a file copy carries the `observe` receipts that make `latest_own_fact_appended_locally`
  true, so a recovery-point restore, a rollback and a lost fsync all look like a store that appended
  its own latest fact. The witness has to come from outside the store, and PodMesh holds it: the node
  knows whether it is starting a universe from its own writable layer or from a recovery point, a
  rollback or a migration restore. Passing that to the replica — one typed fact of the start, an
  environment variable of the universe's start or a root-only file in the runtime directory that the
  entrypoint reads — would let a replica that knows its store is a restored copy require **every**
  peer, with no window, while a replica that knows its store is its own keeps the window; the
  resident would read it beside its configuration, at validation, and hand it to the catch-up gate
  as `latest_own_fact_appended_locally` is handed today. A third adversarial review proposed removing
  the window instead; that is refused, because a replica whose peer is down must still be able to
  boot, which is the availability V2 requires. This paragraph is the shape of the answer, not a
  commitment: what PodMesh knows, and how reliably it knows it, has to be settled before the lot
  that would build it.
- **An emptied store with a peer down**, including one that imported some of its own facts back in an
  earlier process, never becomes ready while that peer stays out of reach: it runs, exchanges, and
  answers `catching_up` to every append, visibly, for as long as it takes. So does a replica that
  reaches no peer. This is chosen: the alternative is a fork.
- **The start budget.** It bounds the control socket's bind and the `uncertain`/`busy` retries, not
  catching up. A replica that is catching up is running and not ready, which whoever starts it has to
  act on: PodMesh sees a universe that is up, and the boot fact appears in the log later, or never.
- **An idle link between two live replicas** is confirmed once per refresh: a partition between them
  shows within one refresh plus the margin above, not sooner.
- **Laboratory qualification**: catch-up, push-back and first-open times on the laboratory hosts, and
  the idle growth of the three replicas over a day.
