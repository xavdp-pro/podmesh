# Campaign 5: the join decides real strands

Status: **the campaign did not pass, and for the first time the reason is a property of the
candidate rather than of the harness.** Three replicas of the frozen candidate
`0.1.0~manager2+gff77b1f946e8`, one owned observation each, typed graceful shutdown on all
three, canonical state retained.

Nothing here qualifies G2, manager high availability, fencing, DNS takeover, automatic
failover or production use.

## What the gate returned

| | |
| --- | --- |
| canonical convergence evidenced | **true** |
| pre-existing incomplete attempts (debt from campaign 4) | 149, retained and reported |
| new incomplete attempts | 62 |
| **accounted** | **59** |
| unaccounted | 3, all `join-failed`, none `peer-has-no-record` |
| terminal attempts | 300 |

Fifty-nine strands were decided one at a time by a cross-host join, on evidence nobody
wrote by hand. That is the first time the amended predicate has done the thing it exists to
do.

The one hundred and forty-nine attempts carried over from campaign 4 were classified as
pre-existing debt by identity, retained and reported, and never folded into a success claim
— which is what the predicate demands of them.

## The three that remain, measured

All three are on one host, all three were already present at the converged capture so none
is a shutdown artefact, and each has exactly one receiver row for its own wire nonce: **the
request was served.** What separates them is what the sender did next.

- **Two were never retried.** No other row anywhere in the campaign carries their operation.
  The receiver committed the import and wrote a reply, the sender never saw it, and the
  sender never asked again. Under the predicate these are unaccounted, and correctly so:
  the only evidence that the effect survived would be eventual convergence, which the
  predicate refuses on its own in so many words.
- **One was retried** — two further rows carry its operation — but the retry does not
  satisfy the full binding the replay branch requires. That one may yet be a defect in the
  comparison rather than in the candidate, and it is the next thing to measure.

## What the harness got wrong, found only here

A folded row's `outcomes` is the **union** over the audit rows it collapses, and every row
carries `incomplete` from its first phase, where nothing is decided yet. Two checks compared
that union to a single value and were therefore unsatisfiable against any real row:

    the replay branch required   outcomes == ["accepted"]
    the framing filter required  set(outcomes) ⊆ {"accepted"}

Measured over 4320 folded rows, the vocabulary is exactly three shapes:

| outcomes | rows | what it is |
| --- | ---: | --- |
| `accepted`, `incomplete` | 3997 | a completed exchange |
| `incomplete` | 211 | a strand: nothing decided |
| `incomplete`, `unavailable` | 112 | a transfer that failed part-way |

So "this exchange completed" is `accepted` present and `unavailable` absent, and
`outcomes == ["accepted"]` can never be true of anything. Before the correction the gate
accounted for **none** of the 62; after it, 59. The evidence was there throughout; the
comparison could not see it.

## The latency defect, now with two data points

The same operation, on the same three hosts, one campaign apart:

| | campaign 4 | campaign 5 |
| --- | ---: | ---: |
| audit rows at capture, three hosts | 10541 | 12930 |
| retries needed for one observation, worst host | 30 | **78** |

The audit table is append-only, the receiver verifies it before replying, so more rows means
slower replies means more timeouts means more rows. The unbounded pre-reply verification has
its own lot; this is its second measurement and the curve is the wrong way up.

## What campaign 5 does not show

It does not qualify anything. It shows that the evidence contract, the collector, the fold
and the eight-condition join work end to end on three real replicas, and that the gate's
remaining refusals point at named, measurable facts. Three strands stand between this and a
passing campaign, and at least two of them are the candidate's own retry behaviour rather
than the gate's.
