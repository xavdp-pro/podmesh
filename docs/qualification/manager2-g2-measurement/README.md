# Manager2 G2 measurement run — not a qualification campaign

This directory holds a four-stage three-host run of candidate
`podmesh-manager 0.1.0~manager2+gff77b1f946e8` that was executed to **measure**
something, not to qualify anything. It fails the gate by construction, and the
failure is not the point.

## Why it was run

The three laboratory stores already carried 50 / 46 / 27 incomplete attempts from
campaigns 1 and 2. `incomplete_attempt_count` is folded from every row ever written
to an append-only audit table, so any run against these stores inherits that floor
and fails `compare-evidence.py` before it starts. Raising `incoming_workers` from 1
to 2 could only affect *new* attempts. Whether it drove them to zero had never been
measured — it was a code-shape argument. So the argument was measured on stores that
were already spent, before deciding anything about the ones that are not.

## What it measured

| host | at rest before | active-baseline | converged | post-cleanup | new strands |
| --- | --- | --- | --- | --- | --- |
| lab-a | 50 | 51 | 53 | 52 | 2 |
| lab-b | 46 | 47 | 47 | 47 | 1 |
| lab-c | 27 | 29 | 34 | 33 | 6 |

Nine new strands, against 118 added by campaign 2 at `incoming_workers: 1` on
smaller stores — a thirteen-fold reduction that does not reach the zero the gate
requires. All three replicas converged to one `logical_history_sha256`, history
3 → 9, no conflict, clean typed shutdowns, unrelated services, containers, routes
and firewall commitments unchanged.

The typed status of the three live residents reported `incoming_limit: 2`,
`peak_incoming: 2` and `rejected_connections: 0`: the previously blamed
accept-and-drop did not occur once.

Joining each of the nine to its receiver by `wire_nonce` shows that every one of
them was received, imported durably, replied to, and the reply written — the reply
was not read in time. See
[FINDING-MANAGER2-LATE-REPLY-STRANDS.md](../../FINDING-MANAGER2-LATE-REPLY-STRANDS.md)
for the full diagnosis, what it rules out, and what it leaves unproven.

## What is here

- `lab-*/` — the twelve stage files and their sidecars, exactly as the collector
  wrote them.
- `comparison.json` — the strict three-host comparison. **FAIL**, five lines, all
  driven by `incomplete_attempt_count`.
- `transition/` — the `incoming_workers` 1 → 2 configuration transition that
  preceded the run: the three secret-free results, their closure documents, and the
  three-host comparison (**PASS**).
- `SHA256SUMS` — covers everything else in this directory.

Both comparisons reproduce byte for byte from this directory with the checked-in
comparators:

```sh
packaging/podmesh-manager/qualification/activation/compare-evidence.py --phase three-host \
  --pre lab-*/pre-activation.json --active-baseline lab-*/active-baseline.json \
  --converged lab-*/converged.json --cleanup lab-*/post-cleanup.json

packaging/podmesh-manager/qualification/activation/config-transition/compare-three-hosts.py \
  --transition incoming-workers transition/lab-*/config-transition-result.json
```

## What is not claimed

Nothing. This run qualifies no gate and establishes no capability. It is a
measurement whose result was that the hypothesis under test — that raising the
incoming worker limit removes the strand mechanism — is false. The converged
captures were taken exactly once per host with no retry, because retrying until the
gated number reads zero would be selection on the gated quantity and would have
destroyed the measurement.
