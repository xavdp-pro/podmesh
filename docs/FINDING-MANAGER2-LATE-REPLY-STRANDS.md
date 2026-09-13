# Manager2: why exchanges strand, and why the G2 gate cannot be met reliably

Status: **finding, not a qualification result.** Nothing here changes a gate, a
contract or any captured evidence.

Date: 2026-09-13. Measured and written by Claude Code (Opus 5) under the standing
automation mandate, then verified by four independent lines of inquiry whose every
decision-relevant claim was put to adversarial refuters under distinct lenses.
Thirteen of the twenty-four claims raised in that verification were refuted,
including six of my own; the corrections are listed at the end and the adjudications
are stated inline. One investigation built the real crate at this commit into a
bench and timed it against copies of the laboratory stores.

This supersedes the diagnosis published earlier today in
[REVIEW-MANAGER2-G2-LIVE-ACTIVATION.md](REVIEW-MANAGER2-G2-LIVE-ACTIVATION.md)
(accept-and-drop at `incoming_workers: 1`). That diagnosis is correct for the 123
strands inherited from campaigns 1 and 2 and is **not** the cause of the nine
strands added by the measurement run.

**Evidence conventions used below.** *Measured* = read out of the private read-only copies of the three laboratory stores, or timed on a bench host against copies of those stores. *Source* = read from the repository at `991987b` (the three files relevant here are byte-identical between `ff77b1f946e8`, the deployed build, and HEAD). *Inferred* = follows from source plus measurement but is not itself recorded anywhere. The distinction matters throughout, because **`exchange_audit_events` has no timestamp column of any kind, and `record_json` carries no time field** — verified against the live schema. No latency in this system has ever been recorded. Every statement about "how long" below is either bench-measured off-host or inferred.

---

## 1. What was measured

A reviewed configuration transition raised `incoming_workers` from 1 to 2 on all three hosts (three-host comparison PASS, sealed before activation). A full four-stage campaign was then run and published explicitly as a **measurement run, not a qualification campaign**, to count how many new uncertain attempts remain at limit 2.

| host | at rest before | active-baseline | converged | post-cleanup | new strands |
| --- | --- | --- | --- | --- | --- |
| lab-a | 50 | 51 | 53 | 52 | 2 |
| lab-b | 46 | 47 | 47 | 47 | 1 |
| lab-c | 27 | 29 | 34 | 33 | 6 |

All three replicas converged to one `logical_history_sha256`, history 3 → 9, no conflict, clean typed shutdowns. *(Measured: the derived incomplete sets reproduce 50/46/27 → 52/47/33 exactly, using the source's own `phase_rank`/terminal rule at `manager-ha/src/durable.rs:2557-2600`. Zero pre-existing strands resolved; the delta is exactly nine.)*

The typed status of all three live residents reported `incoming_limit: 2`, `peak_incoming: 2`, **`rejected_connections: 0`**.

**What `rejected_connections: 0` does and does not prove.** `shared.rejected` is incremented at exactly one site — `manager-resident/src/lib.rs:406-407`, the `workers.len() >= config.incoming_workers` branch that drops the stream. So the counter is an exact witness that *that* branch never fired, and the previously blamed accept-and-drop therefore did not occur once. It is not a general proof of "no connection was dropped": this review already notes a second, silent drop at `manager-resident/src/lib.rs:415` (a worker that cannot open the store drops the connection with no reply, no audit row and no counter), and the counter is a per-process `AtomicUsize` read at one instant. **Adjudication:** the load-bearing evidence is not the counter but the per-attempt join in §2, which excludes *every* drop-before-serve path at once, because any of them leaves the receiver with no audit row at all. The counter corroborates it from an independent source.

The transition also had the right sign, which the earlier framing did not state: campaign 2 at limit 1 added +49/+45/+24 = 118 strands; the measurement at limit 2, on strictly larger stores, added 9. Roughly a thirteen-fold reduction. It did not reach zero, which is what the gate requires.

**The outbound ledger closes exactly** *(measured, and verified as a bijection by `wire_nonce`, not merely as matching totals — 383/383 outbound nonces distinct, 376/376 inbound, no orphan on either side, no `preauth:` nonce anywhere)*:

- 383 attempts prepared (135 / 122 / 126 by sender)
- 7 never reached a peer (2 / 2 / 3) — terminal `outbound_exchange_completed / unavailable / transport_unavailable` with `request_frame_bytes = 0`, i.e. connect-level failures, and no inbound row for their nonce on any host
- 376 requests reached a receiver (122 / 129 / 125 served)
- 376 complete reply frames written by receivers — 100%, no exceptions
- 367 replies read and terminally recorded by senders (byte count and `reply_sha256` matching the receiver's row on every one)
- **9 unread**

367 + 7 + 9 = 383, and 367 + 9 = 376.

---

## 2. What the nine stranded exchanges actually are

All nine are outbound, frozen at `outbound_request_prepared`, `outcome=incomplete`, with `request_announced_body_bytes` of 4025–5531.

**Correction to the earlier forensic note:** `reply_frame_bytes = 0` and `request_frame_bytes = 0` on a prepared row are *structural* — every `outbound_request_prepared` row in all six stores has that shape, successful attempts included, because the row is written at `manager-network/src/lib.rs:415` before `connect()` and `validate_outbound_audit_bytes` (`durable.rs:1490-1513`) *requires* it. Those fields are not a fingerprint of reply loss. The nine are distinguishable from a healthy attempt only by the absence of a successor row.

**Joining each of the nine to its receiver by `wire_nonce`** *(measured; agrees on all nine with the sender's own `peer_claim` / `authenticated_peer_id`)*:

```
inbound_request_observed  (accepted)
  -> inbound_import_committed (accepted)
     -> inbound_reply_prepared
        -> inbound_reply_write_observed (accepted, 693 or 694 bytes)
```

For all nine the receiver's `request_sha256` equals the sender's prepared digest bit-for-bit and `request_frame_bytes` equals `announced + 4` exactly (5535 ×7, 5033, 4029), so the request crossed intact and passed the HMAC check at `manager-network/src/lib.rs:948`.

Two precisions on that chain. `inbound_reply_prepared` carries `outcome=incomplete` on all 376 inbound exchanges by construction — three of the four phases are `accepted`, not all four. And `inbound_reply_write_observed` records that `metered_write_frame` returned `Ok` for the whole frame, i.e. that every byte was accepted by a local `write()`. **It is not an acknowledgement and does not establish delivery.**

**Direction: eight of the nine were directed at lab-b, not seven.** lab-a → lab-b ×2, lab-c → lab-b ×6, lab-b → lab-c ×1; lab-a received none. *(Measured twice, independently: by `authenticated_peer_id` on the sender's row and by the nonce join into the receiver's store. All four investigations that checked this agree; the "seven" in the earlier text is wrong.)*

Traffic was balanced across all six ordered pairs (a→b 68, a→c 67, b→a 62, b→c 60, c→a 63, c→b 63), as was payload (86% of all 383 attempts announce the identical 5531-byte body), so neither exposure nor payload explains the concentration.

**No fact was lost.** Every one of the nine was followed by an accepted exchange to the same peer 12–39 audit rows later; for eight of them the retry carried the same wire operation id and came back `replayed=1`. *(The ninth was never retried — its operation was superseded by the next snapshot — so its no-loss finding rests instead on the receiver's own `inbound_import_committed` and on the identical nine-fact digest in all three stores.)* Note `replayed=1` is the ~95% base rate on accepted completions, so it is confirmation, not discrimination.

---

## 3. The mechanism

**Stated exactly:** the receiver's durable pre-reply work outlasted the sender's fixed reply-read deadline. The reply was written to a socket the sender had already stopped reading; the sender took the deliberate early return and stranded the attempt permanently; the receiver recorded a complete, successful exchange. The reply was not lost — it was late, or at any rate too late to be read.

### Source anchors

- `IO_TIMEOUT = 2 s` — `manager-network/src/lib.rs:35`. It is used as **three independent budgets** inside `sync_to`, not one: connect (`:416`), the request write (`:435`), and the reply read (`:453`). The reply-read clock is constructed at `:453`, i.e. the instant `metered_write_frame` returned having copied the last request byte into the kernel send buffer. It covers the whole reply frame — `metered_read_frame` shares one absolute deadline across the length prefix and the body.
- A read that returns nothing yields `ErrorCategory::Unavailable` with `evidence.frame_bytes` still 0 (`:1582` is the only increment; deadline exhaustion at `:1570-1572`, `SO_RCVTIMEO` expiry → `WouldBlock` → `:1586`).
- The guard at **`manager-network/src/lib.rs:456-462`** tests exactly `frame_bytes == 0 && category() == Unavailable` and returns at `:461` without calling `finish_outbound`, which sits immediately below at `:463`. Its comment is at `:459-460`. *(The earlier text cited `:453-460`; `:453` is the read, the guard is `:456-458`, the return `:461`.)*
- Receiver side, in order: the accept loop is a single-threaded poller that takes at most one connection per iteration and services the control socket inline, on a 10 ms tick (`manager-resident/src/lib.rs:406-422`, sleep at `:511`); each worker calls `conf.network.open()` (`:415`), which is a fresh `Store::open` with its own `IMMEDIATE` transaction (`durable.rs:419`); then `record_audit(observed)` (`manager-network/src/lib.rs:822`), `execute_authenticated_import` (`:855`), `record_audit(prepared)` (`:1099`), and only then `metered_write_frame` (`:1100`).
- Each of those three is an `IMMEDIATE` transaction that runs `verify_all` first (`durable.rs:611`, `:558`, and `:502` for the outgoing thread's `Export`). `verify_all` (`durable.rs:1137`) re-verifies the whole schema, every receipt, every fact and **every audit row** — `load_audits` (`:2484`) re-decodes, re-serialises and re-digests each row and issues a receipts lookup for each row carrying a receipt link.
- WAL with `synchronous = FULL` (`durable.rs:463`, `:466`), so three fsyncing commits precede the first reply byte.
- `busy_timeout` is **5 s** (`durable.rs:416`) against the sender's 2 s. A blocked writer is permitted to wait 2.5× longer than the sender will wait.
- The audit table can only grow: `exchange_audit_events_no_update` / `_no_delete` (`durable.rs:2953-2954`).

### Magnitude (bench-measured, off-host)

**Adjudication — the investigations disagreed and the disagreement is resolvable.** One counted three `verify_all` calls and derived ~341 ms at 3,341 rows; another counted the additional `load_audits` passes inside `insert_audit → validate_candidate_audit_sequence` and `prevalidate_authenticated_import_audit` and derived seven or eight full traversals. Both are right about different things, and the numbers reconcile: one full audit-table traversal costs **~35 µs per row**; `record_exchange_audit` performs two, hence the independently measured **~70 µs per row per call** (131 ms at 1802 rows, 186 at 2582, 239 at 3341, linear, no quadratic term); the pre-reply window contains **seven traversals on the replay path and eight on a fresh import**. Driving the real three-transaction sequence against a copy of the lab-b store measured **~0.93 s at 3,341 rows**. On that bench host, the uncontended pre-reply path reaches 2 s at roughly **7–8k audit rows** (2.05 s directly observed at 8,401 rows).

Two things follow, and neither is a prediction about the lab. First, at present store sizes the *uncontended* receiver path consumes on the order of 40–50% of the sender's budget on the bench host — enough that ordinary jitter, queueing behind the other writers on the single write lock, or slower storage crosses 2 s in the tail, but not enough to make it certain. Second, this is a bench workstation with a warm cache; the lab hosts are LXD containers and their constant is unmeasured. A throughput-derived estimate from the campaign log (lab-a made 135 outbound attempts in ~259 s against a nominal 1 s interval, ~1.9 s per attempt across ~7 serialised store transactions) suggests the lab hosts are roughly 1.5–2× slower per transaction, but that is *inferred*, not measured.

### What was ruled out

By trace shape, and confirmed against the rows:

- **Accept-and-drop** (the previously published cause). A dropped connection writes no receiver row; all nine have the complete four-phase chain. Conversely, **all 123 inherited strands have no audit row of any phase on any peer** — that is the accept-and-drop signature, and it is 100% clean: the two populations are disjoint, split exactly on the limit-1/limit-2 boundary.
- **Malformed or truncated reply** — would give `frame_bytes == 4` or "truncated frame" or a `Malformed` outcome, all of which fall through to `finish_outbound` and leave a terminal row. None of the nine has a second row.
- **Partial reply** — a single arriving byte routes to `:463` and writes a terminal row. No row anywhere in the corpus records `0 < reply_frame_bytes < 693`; the `:463` branch has never fired in this system's history.
- **Receiver closed without replying** (`Ok(0)`, "peer closed before sending a frame") — contradicted by the receiver's own write record.
- **Authentication failure** — would produce `inbound_diagnostic_reply_written` or a refusal row. There are zero such rows anywhere in either snapshot.
- **Process death / shutdown truncation** — the nine sit 292–740 rows before the tail with normal traffic continuing immediately after each; every table ends on a terminal row; six on one host would need six restarts.
- **Stale or wrong socket** — the stream is moved into one worker thread, pairing is by a 32-byte OS-random nonce (1,675 distinct), and `authenticate_request` rejects anything not addressed to the local replica.

### What was **not** ruled out

**Adjudication: three investigations independently refuted the claim that the early return is the unique producer of this store residue, and I accept that refutation.** Three code paths leave a byte-identical trace, and the stores cannot separate them:

1. The read-deadline early return at `:456-462` (the proposed mechanism).
2. **A sender-side terminal-audit failure.** `finish_outbound`'s last act is `self.record_audit(&event).map_err(... "outbound result is uncertain because terminal audit failed" ...)?` at `manager-network/src/lib.rs:649-653`. If that insert fails — SQLITE_BUSY past the sender's own 5 s `busy_timeout`, a `verify_all` refusal — it returns `Err` having written nothing, from any of `finish_outbound`'s seven call sites. The repository tests exactly this (`outbound_terminal_audit_failure_returns_local_uncertainty_after_signed_success`), and the test asserts the same residue: one prepared row on a sender whose peer imported and replied.
3. **A sender-side stall.** If the sender's thread is descheduled past its own deadline, `read_exact_metered` finds the deadline already gone before its first syscall and gives up on a reply that arrived on time.

(2) and (3) are strongly disfavoured but not excluded. Against (2): across the whole corpus not one durable write is known to have failed — all 1,496 inbound attempts carry all four phases (~6,000 inserts), there is not a single inbound strand, and the inbound path would strand visibly if a write failed; and the nine sort by **receiver**, which a sender-store defect cannot produce. Against (3): nothing, directly.

**Genuine wire loss, a mid-flow reset, or a post-write send-queue discard are also not excluded.** The earlier draft of this analysis argued they were, on three counts; I have adjudicated against that argument:

- *Integrity* (376 requests crossed intact) proves the forward path delivered; the replies are the reverse path, and 9/376 = 2.4% is a perfectly ordinary rare-event rate.
- *Retransmission* excludes only i.i.d. per-segment loss. A flow-level black-hole (conntrack eviction, NAT rebind, policy drop) kills every retransmission with probability 1, and an RST needs no loss at all.
- *Topology* misstates its own data: `b→c` is the reverse of `c→b`, the **same host pair**, so only two host pairs carry the nine. Receiver-level lab-b vs lab-c is Fisher p = 0.036; pair-level {b,c} 7/123 vs {a,b} 2/130 is p = 0.095. Neither is excluded at nine events.

What genuinely favours a receiver-side cause is a different observation the earlier text did not make: **two different senders strand on the same receiver, interleaved in that receiver's own timeline** (lab-b's eight sit at inbound positions 16, 19, 60, 65, 68, 76, 78, 80 of 129, with lab-a's two inside lab-c's cluster). Dependence spanning two senders that share no state is by definition a property of the receiver. Model comparison over the six ordered pairs agrees: a receiver-only model (AIC 77.9) beats sender-only (86.7), pooled (87.3) and even the saturated pair model (79.8), and no sender contrast is significant.

**So: "late reply, not lost reply" is the best-supported mechanism and the only one that explains the receiver concentration. It is consistent with everything recorded and no rival survives as well. It is not proven, because the stores contain no clock.**

---

## 4. Does it worsen as the store grows?

**Two separate claims, with different standing.**

**The latency-growth mechanism is established** — from source (`verify_all` is unbounded whole-table re-verification, run inside every audit insert, on a table protected by no-delete/no-update triggers) and by direct measurement (linear at ~35 µs per audit row per traversal, ~70 µs per row per `record_exchange_audit`, R² ≈ 0.999 from 1,802 to 13,002 rows, no quadratic term). The receiver's pre-reply latency grows with the store, without bound, and the store cannot be pruned in place.

**The claim that the strand *rate* grows with the store is under-determined by this data, and locally contradicted.** Stating this plainly:

- lab-b entered the run with 2,582 audit rows and lab-c with 2,557 — **1.0% apart**, about 2 ms of insert latency by the measured slope — and they took **8 and 1** strands as receivers (lab-b 8/129, lab-c 1/125, lab-a 0/122; Fisher lab-b vs lab-c p = 0.036, lab-b vs lab-a p = 0.007). An eight-fold rate difference across a one-percent size difference is not a size effect.
- Within the run, tables grew 29–42% with no rising hazard. lab-b's eight strands fall 2/1/5/0 across quartiles of its own growth — **none in the final quartile**, at the largest table the system has ever held; its last 48 inbound exchanges, all at record row counts, all succeeded. lab-c's single strand is its second inbound exchange of the run, at its smallest table.
- **Campaign 1's 0/0/2 is not evidence for the corollary and that sentence must be withdrawn.** Both of those strands have no receiver record for their nonce: they are accept-and-drop, the other mechanism. The late-reply class has *zero* occurrences at `incoming_workers: 1` at any table size, and all nine appeared in the first run at limit 2. What discriminates the two classes is the worker limit, not the row count.
- The one place store size *does* track outcome is the accept-and-drop population, where attempt-level strand probability rises monotonically with the receiver's table size across 1,292 limit-1 attempts (Spearman ρ = +0.274, z = +9.8). That is a real growth effect on a mechanism the transition has removed. Isolated to the nine late replies, ρ = +0.061 (z = +1.19) — no signal.

**Honest statement:** the receiver's pre-reply work is O(table) and unbounded, so the margin against the sender's 2 s deadline erodes monotonically and a fresh store cannot be treated as a mitigation with a computable margin. Whether the *observed rate* tracks table size at present scales is not established, and something host-local about lab-b — disk latency, CPU, scheduling, LXD/ZFS I/O contention — is doing work the stores cannot see.

**What would settle it,** in order of cost: (a) a monotonic timestamp or elapsed-microseconds field on each audit row, which turns the bench projection into a lab observation and makes `inbound_request_observed → inbound_reply_write_observed` directly measurable; (b) making the sender's early return record evidence (phase, elapsed, bytes read) so a strand carries its own cause and separates the three residue-identical paths in §3; (c) the controlled experiment — the same configuration and the same four-stage campaign run against stores pre-seeded to 0 / 3k / 10k / 30k audit rows, counting new strands per attempt. Only (c) separates row count from configuration, stage and host identity.

---

## 5. What this means for the gate

`compare-evidence.py` requires `incomplete_attempt_count == 0` at the converged stage (`:176`) and again at post-cleanup (`:154`, `:177`).

**On the present three stores the gate cannot be passed by any configuration or procedure.** The count stands at 52 / 47 / 33 and no strand has ever healed (0 of 123 across the two snapshots). *Correction to an earlier internal claim:* this is **not** because the count is physically irreversible. `incomplete_attempts` is a max-`phase_rank` fold over an **append-only** table, and `UNIQUE(direction, attempt_id, phase)` admits a further phase row; two investigations demonstrated by execution that appending one terminal row per strand takes all three stores to 0/0/0 with every trigger intact, `verify_all` passing, `logical_history_sha256` unchanged and the store reopening read-write. The measurement's own converged → post-cleanup drops (lab-a 53→52, lab-c 34→33) are that same mechanism observed in the field. **The bar is therefore evidential, not technical:** writing a terminal row retrospectively for an attempt no live process resolved is exactly what this document already names as evidence forgery. That decision belongs to whoever owns the Stage D audit contract, and no script in the repository performs it. The practical options are a fresh store, or an operator-authorised retirement that must be argued on its own terms.

**On a fresh store, with no code change, the gate is passable only by luck — and the reliability is not known.** Three qualifications:

1. The residual late-reply rate at limit 2 on a fresh store **has never been measured**. No such run exists. Campaign 1 was limit 1 and a different mechanism. Transporting the measured 2.35% (9/383, Wilson 1.24–4.41%) onto a fresh-store run is exactly the store-size extrapolation §4 says is unsupported.
2. Even at the measured rate, the odds of a strand-free campaign are not a point estimate. The strands are clustered, not Poisson (Monte Carlo p = 0.041 against uniform placement; six lab-b strands inside 21 of 129 inbound exchanges, scan p ≈ 0.006). Counting attempts gives ~1 in 8,000; counting episodes gives ~1 in 20 to 1 in 400; the rate's own confidence interval spans several further orders of magnitude. Any single figure here would be false precision.
3. **The converged predicate is additionally a race that a correct sender also loses.** The prepared row is committed before `connect()`, so any exchange merely in flight at the capture instant counts as incomplete. The measurement proves it happened: an append-only table can only produce a *decrease* by a terminal row arriving after the earlier capture, so lab-a (53→52) and lab-c (34→33) each had at least one attempt open at the gated instant. The measurement log's repeated live inspections move both up and down, with excursions of up to +3 above the at-rest floor — consistent with one outgoing thread plus two inbound workers. Zero at post-cleanup is a well-defined at-rest invariant; zero at converged is a snapshot-timing property.

**Re-capturing the converged stage until it reads zero is forbidden.** It is selection on the gated quantity, it is undetected by the comparator (a capture starts no service, so it does not disturb the `main_pid` / `n_restarts` guard), and it cannot touch the post-cleanup reading. Anyone who does it must say so in the campaign report.

**Adjudication on the transition knob:** the reviewed path can reach only `incoming_workers` 1 or 2 — `compare-three-hosts.py:14` hard-codes `{"from": 1, "to": 2}`, `:64-66` refuses any evidence whose `changed_keys` is not exactly `["incoming_workers"]`, and `config-tool.py:18-19, :105-107, :118-120` refuses any source not exactly 1 and any applied value not exactly 2. `3..=8`, which the resident itself would accept (`manager-resident/src/lib.rs:55-57`), and any change to `interval_ms` or `max_backoff_ms` are unreachable through a reviewed transition. Both reachable values leave a strand mechanism. I reject the stronger claim that raising the limit "has the wrong sign": it reduced strands thirteen-fold. It simply did not reach zero.

---

## 6. The repair, and its cost

There are two distinct defects and they need distinct answers. Nothing below is a recommendation to rewrite captured evidence.

### 6a. The latency defect (the cause)

The receiver performs seven-to-eight whole-audit-table traversals and three fsyncing `IMMEDIATE` commits before the first reply byte, on a table that only grows, behind a lock budget 2.5× the peer's patience. Three changes attack this and are within the crate's own contract:

- **Make the per-insert verification incremental** rather than whole-table. This is the dominant term and the only unbounded one. It must be made incremental, **not removed**: `verify_all` inside the write transaction is what makes the store fail-closed under HA-I11 and HA-14 ("checksum or schema refusal precedes replay/mutation").
- **Move `record_audit(inbound_reply_prepared)` off the reply-critical path**, or defer it past the write. *Do not* fold `record_audit(observed)` into the import's transaction: `observed` must be durable before `refuse_authenticated` and `close_authenticated_preserving` can reference it, and a rollback would erase the evidence that the request was ever seen.
- **Take `Request::Export` off the write lock.** It is read-only — the stores contain only `authenticated_import` and `observe` receipts across ~1,172 Export calls — yet `execute_with_receipt` takes `IMMEDIATE` plus a full `verify_all` (`durable.rs:497-502`), and the resident pays it twice per outgoing attempt.

**Explicitly refused, with reasons:**
- *Raising `IO_TIMEOUT`.* There is no number to justify while the receiver's work is unbounded in the store's size; and any raise also stretches the connect and write budgets, which are not the problem. (I reject the stronger published form "no finite `IO_TIMEOUT` is defensible": once the receiver's path is bounded, a deadline that dominates it is exactly the number one can justify.)
- *Lowering `busy_timeout` below `IO_TIMEOUT`.* Traced through the code, this is counterproductive: SQLITE_BUSY surfaces as `DurableError::Storage`, which is not a signable reason, so the receiver writes no reply byte, the sender reads zero bytes with an `Unavailable` error and strands on the *same* early return — and the import is lost as well. `incomplete_attempt_count` would rise, not fall. Its only real benefit is making the two stores agree about a failed exchange.
- *Replying before the durable commit.* The reply body carries `receipt_operation_id` and `receipt_sha256` and the sender durably banks them into its own terminal audit; a pre-commit reply would let the sender hold a remote receipt for facts that may never become durable. Refused on HA-I09.
- *Retiring attempt N when its retry N+1 succeeds.* The successor carries a different `attempt_id` and a different nonce and says nothing about N's reply. That is HA-06's forbidden inference — "acknowledgements cannot be inferred from outbound success" — and it would be so even though, measured, all nine retries did succeed.

### 6b. The ledger defect (the symptom the gate counts)

Independently of latency, the sender's early return **records nothing**, and that is a real loss of information: it discards `request_transfer`, the one fact the sender knows for certain — that all `N+4` request bytes left the host. Because `validate_outbound_audit_bytes` forces the prepared row to `request_frame_bytes = 0` and the row is unamendable by trigger, that fact can only ever live in a second row. Today a strand is indistinguishable from a process that died before it ever connected, and it conflates at least three causes (§3).

Two candidate repairs, both of which are **contract decisions, not code decisions**:

- **(i) Delete the guard at `manager-network/src/lib.rs:456-462`** so the `finish_outbound` call at `:463` runs. Mechanically this was verified by execution: it emits `outbound_exchange_completed / unavailable / transport_unavailable` with `request_frame_bytes = N+4` and `reply_frame_bytes = 0`, the store accepts it, the attempt leaves `incomplete_attempts`, and no new `AuditPhase` or `AuditOutcome`, no schema change, no `phase_rank` change and no validator change is needed. Its costs, stated fully because the earlier internal draft understated them: the record shape it writes (unavailable *with a fully transferred request*) exists **nowhere** in the three stores — all 56 existing unavailable completions are `request_frame_bytes = 0` connect failures, so this is new behaviour, not an existing one; it breaks **two** test targets (`manager-network/src/lib.rs:2781-2786` and the cross-process proof `manager-network/tests/three_processes.rs:254-275`); it amends the frozen Stage D specification at `docs/MANAGER-G2-DURABLE-EXCHANGE.md:470` and `:548-549`, plus `manager-network/README.md:79-89` and `manager-network/EVIDENCE.md:91, :140`, where the present behaviour is the recorded disposition of a prior independent-review finding; under the repository's own stage rule (`MANAGER-G2-DURABLE-EXCHANGE.md:637-642`) amending a Stage N contract restarts stages R, P and Q; **and it asserts `transport_unavailable` for nine exchanges the peer demonstrably committed, receipted and replied to**, putting the two stores into flat contradiction on one nonce. It also blinds `incomplete_attempt_count`, which this review elsewhere calls the only sound live indicator of the silent drop path at `manager-resident/src/lib.rs:415`.
- **(ii) Add a terminal phase for "request complete, reply unread, remote outcome unknown."** This preserves the epistemic state the Stage D comment is protecting while making the attempt resolvable, at the cost of a new `AuditPhase`/`AuditOutcome`, a `phase_rank` entry, and a decision about whether the gate counts it.

**I do not choose between them here.** (i) is cheaper in code and more honest about what the sender observed; (ii) is more honest about what the exchange actually was. Both change what the audit table is permitted to assert, and that is the call of whoever owns the Stage D contract.

### 6c. Cost of re-qualification

*(Read from source; no removal path exists in the repository.)* The retained activation marker binds `package_version` and `binary_sha256` (`activate-host.sh:55-58`; `transition-host.sh:78, :85, :152`). **Any new binary is hard-refused on all three hosts.** Shipping any of §6a or §6b therefore requires redoing: the candidate verification report; the sealed `incoming_workers` 1→2 transition and its rollback-window closure; the single-host activation phase; the full four-stage three-host campaign; and the marker itself. That needs either an operator-authorised marker retirement — for which no script exists — or three clean disposable hosts. This is the cost of shipping *any* new binary, and it is the same for the four-line deletion as for the incremental-verification work; it should not be used to argue for the smaller change.

### 6d. A capture change that needs no binary

`capture-host.sh:137` reduces the incomplete set to `(.incomplete_attempts|length)`, discarding direction, `last_phase` and attempt/nonce ids, and never publishes `unaudited_import_receipt_ids`; `activate-host.sh` captures pre-activation without `--with-inspection` (only `:107` and `:212` pass it), so no at-rest baseline is expressible. Publishing those fields would have made this campaign diagnosable from the sealed files rather than only judged. **Two costs must be named before anyone does it:** `compare-evidence.py`'s `obj()`/`validate_inspection` pin the inspection object to exactly nine keys against a sealed schema version, so the comparator, its fixtures and the schema string change in lockstep and must be re-sealed; and adding `--with-inspection` to the pre-activation capture would abort activation on a host with no store yet (`inspect_read_only` refuses a missing store, and `capture-host.sh` runs under `set -euo pipefail`), i.e. it would break the fresh-store campaign it is meant to serve unless made explicitly tolerant of an absent store. Note also that a baseline-anchored delta is already expressible today and is **unsound**: at-rest 50/46/27 versus active-baseline 51/47/29 means converged − baseline reports 2/0/5 against a true 2/1/6.

---

## 7. What remains unproven

1. **No latency was ever recorded.** `exchange_audit_events`, `facts` and `receipts` carry no timestamp column and `record_json` no time field. "The receiver exceeded 2 s" is an inference from source structure plus off-host bench timings, never an observation. This is the single largest gap and it makes several of the questions above formally undecidable from these stores.
2. **Three residue-identical paths.** The read-deadline early return, a sender-side terminal-audit failure (`manager-network/src/lib.rs:649-653`), and a sender-side stall all leave one prepared row and nothing else. The corpus contains zero known durable-write failures and the nine sort by receiver, which strongly disfavours the second; nothing directly addresses the third.
3. **Wire loss, a mid-flow reset, and a post-write send-queue discard are not excluded.** `inbound_reply_write_observed` records a returning `write()`, not delivery. At n = 9 the receiver-level and pair-level splits are p = 0.036 and p = 0.095 respectively; neither reading is eliminated. The strongest datum for a receiver-side cause is the cross-sender clustering in lab-b's own timeline, which is suggestive, not exclusionary. A single packet capture on lab-b during one run would decide it outright; per-host TCP counter deltas (retransmits, `TCPAbortOnData`, `ListenDrops`) would narrow it cheaply.
4. **Why lab-b.** Store size does not explain it (1% larger than lab-c, eight times the rate). Nothing else in the stores separates them — page count, `record_json` bytes, receipts, facts and request payload distribution are all within ~1%, and audit-row interleaving shows no contention signature (375 of 376 inbound exchanges wrote four contiguous rows, the nine included — which does *not* exclude lock contention, since a blocked writer commits nothing while blocked and `Store::open` and `Export` take the lock without writing a row, but leaves no positive evidence for it either). Something host-local is doing the work.
5. **The fresh-store rate at limit 2 is unmeasured**, and with it the actual reliability of a pass.
6. **`verify_all`'s real cost on the lab hosts is unmeasured**, as is the split between scan time, lock waiting and fsync. That split determines whether the incremental-verification change alone would have prevented these nine.
7. **The receiver-side dangling case was not traced to a conclusion.** The test at `manager-network/src/lib.rs:2763-2817` shows a receiver left at `inbound_import_committed` when the reply is dropped after the durable decision. It did not occur in the field — every one of 1,496 inbound attempts across both snapshots reached a terminal phase — but `send_signed_reply`'s partial-write branch was not traced to confirm it always terminates the attempt. A repaired sender could still strand on the inbound side.
8. **Whether activation policy permits discarding the qualified durable state** after the sealed transition. `compare-evidence.py` checks only the state directory's ownership and mode, never its contents, and `state_directory_listing_unchanged` is a precondition of the transition rather than a prohibition on a later wipe — so the gate does not refuse a fresh store. Whether the policy does is not a question the repository answers.

---

## 8. The gate against the frozen contract

Everything above treats the nine strands as a defect to be repaired. That framing is
only half right, and the half that is wrong matters more.

`docs/MANAGER-G2-DURABLE-EXCHANGE.md` freezes the Stage D durable contract. Its
**Failure semantics** table is normative — its columns are *Failure* and *Required
result* — and it contains this row verbatim at `:470`:

> | Reply is lost after destination commit | Destination retains import receipt and audit; **source retains a prepared/incomplete attempt.** Identical operation retry with a fresh nonce receives a replayed signed receipt. |

and, at `:472`:

> | Process dies after a prepared phase | Read-only inspection reports the attempt incomplete; **recovery never invents a terminal outcome.** |

and the invariant `G2-I07` at `:80`:

> | G2-I07 | Every completed exchange has bounded durable evidence; **a prepared attempt with no terminal event remains explicitly incomplete.** |

The nine strands are that row, exactly: the destination retained its import receipt
and audit, the source retained a prepared attempt, and eight of the nine retries
came back `replayed=1`. **The candidate did what its contract requires of it.** The
comment at `manager-network/src/lib.rs:459-460` is not a shortcut; it is an
implementation of a specified requirement.

Three consequences follow, and they change what the decision is about.

**First, the repair labelled (i) in §6b is a contract amendment, not a bug fix.**
Deleting the guard so that `finish_outbound` writes
`outbound_exchange_completed / unavailable / transport_unavailable` would make the
source stop retaining a prepared attempt in exactly the case the table requires it
to, and would assert `transport_unavailable` for an exchange the destination
committed and replied to. It contradicts `:470` and `G2-I07` directly. Under the
repository's own stage rule (`:637-642`) that restarts stages R, P and Q. It should
be evaluated as an amendment to a frozen contract, by whoever owns that contract,
and not as a four-line deletion.

**Second, the gate does not test correctness here — it tests a failure rate.**
`compare-evidence.py` requires `incomplete_attempt_count == 0` at two instants. A
campaign passes that check only if a specified, anticipated failure mode does not
occur during it. That is a reliability bar, and no reliability bar can be met by a
system whose pre-reply path is unbounded in the size of a table that only grows.
It is also a bar the contract itself never states: the word the contract uses about
incomplete attempts is *retained*, never *absent*.

**Third, this splits the problem cleanly, and the two halves have different owners.**

| | What it is | Who decides |
| --- | --- | --- |
| The gate requires zero occurrences of a contractually specified failure | a specification question | the author of `compare-evidence.py`, against `MANAGER-G2-DURABLE-EXCHANGE.md` |
| The receiver's pre-reply path is O(audit table) and unbounded, which turns an anticipated rare failure into a frequent one | a genuine defect, independent of G2 | the candidate's owner |

The second is worth fixing whatever happens to G2: seven to eight whole-table
re-verifications and three fsyncing write transactions before a single reply byte,
on a table with no pruning path, is a scaling defect that would surface far worse on
a manager that has run for weeks than on one that has run for seven minutes. Fixing
it does not make the gate passable — it only makes the specified failure rare again.

**What I am not saying.** I am not saying the gate is wrong to care. An exchange
whose outcome the sender cannot confirm is a real gap in knowledge, and a system
that accumulates them is worth worrying about. I am saying that "zero at two
instants" measures the weather, and the contract measures the climate; if G2 is
meant to establish that the durable exchange behaves as specified, then the
predicate should be one the specification can guarantee — for instance that every
incomplete attempt is *accounted for*: its request fully transferred, its retry
successful, its facts present in the converged history, and no partial import
anywhere (`unaudited_import_receipt_ids`, which the collector does not currently
publish at all). Every one of the nine would pass such a test. None of them
represents a lost fact.

That is a proposal, not a change, and it is not mine to make.

---

## Corrections to the previously published text

| Published | Correct |
| --- | --- |
| Accept-and-drop at `incoming_workers: 1` is the cause | True for the 123 inherited strands; **not** the cause of the nine — `rejected_connections: 0`, and all nine have a complete receiver chain |
| "Seven of the nine were directed at lab-b" | **Eight** (lab-a ×2, lab-c ×6; the ninth is lab-b → lab-c, and lab-a received none) |
| Non-zero `request_announced_body_bytes` with `reply_frame_bytes 0` as a forensic finding | The constant shape of **every** prepared row, successful ones included; not a fingerprint |
| "Consistent with campaign 1 (empty stores) ending at 0/0/2" | **Withdrawn.** Both of those strands have no receiver record — accept-and-drop, the other mechanism. The late-reply class has zero occurrences at limit 1 at any table size |
| "The defect gets worse the longer the system runs" | The *latency* does, provably and without bound. The *rate* is not established at these scales and is locally contradicted (lab-b vs lab-c: 1% size difference, 8× rate difference) |
| Early return at `:453-460` | The read is at `:453`; the guard is `:456-458`, its comment `:459-460`, the return `:461`; `finish_outbound` follows at `:463` |

*Adjudications made in writing this section, all against earlier internal drafts of the analysis: the early return is not the unique producer of the residue; genuine wire loss is not excluded; the strand-rate growth corollary is unsupported; `incomplete_attempt_count` is not physically irreversible (the barrier is evidential); the 1→2 transition had the right sign; and the pre-reply traversal count is seven or eight, not three, which reconciles the two conflicting cost models.*
