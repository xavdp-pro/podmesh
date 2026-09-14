# Campaign 4: what the first evidence-v3 run measured

Status: **the campaign did not pass, and it was not expected to.** It was the first run of
the v3 evidence contract against the three laboratory hosts, and its purpose was to find
what only a live run can find. It found four defects. Three were mine, in code every suite
passed. One is a property of the candidate, and it is the reason this document exists.

Nothing here qualifies G2, manager high availability, fencing, DNS takeover, automatic
failover or production use. Host aliases and salted commitments only.

## What ran

Three replicas of the frozen candidate `0.1.0~manager2+gff77b1f946e8`, activated in the
frozen host order, one owned non-exclusive observation submitted on each, then a typed
graceful shutdown and a post-cleanup capture on each. All three shutdowns were unforced and
the canonical state was retained. Every replica converged on one logical history.

## The three defects in the harness

Each was correct against the 165-row preserved store used in development, and wrong against
a live replica. That distance is the finding.

**The store path was a guess.** The typed-absence branch proved a fresh host by looking for
`store.sqlite`; every host names its database in its configuration, and on all three that is
`manager.sqlite`. So the presence test looked for a file that exists nowhere and **every
capture of a populated store reported `store_present: false`** — a false absence, at every
stage, which is precisely the fail-open the branch was written to prevent. Absence must be
observed, and observing the wrong path observes nothing.

**The commitment map was passed on the command line.** A live replica's audit table carried
thousands of rows, the map grew to match, and `jq` refused with *Argument list too long*. The
capture then produced no evidence at all, which is worse than any wrong field.

**So did the evidence assembly.** Same limit, next site along: the inspection object and the
folded exchange list. A live replica publishes over a thousand folded rows.

None of the three is reachable by a fixture. The first needs a host with a configuration;
the other two need a store large enough to overflow a process limit. The suites were green
throughout.

## What the candidate does under load, which is the result that matters

| host | audit rows | folded exchanges | terminal | incomplete | reached no peer |
| --- | ---: | ---: | ---: | ---: | ---: |
| lab-a | 2981 | 1032 | 975 | 57 | 50 |
| lab-b | 3799 | 1295 | 1240 | 55 | 46 |
| lab-c | 3761 | 1303 | 1266 | 37 | 27 |
| **total** | **10541** | | | **149** | **123** |

Three observations produced ten and a half thousand audit rows, and **149 incomplete
attempts of which 123 — 82 % — have a wire nonce that appears on no other host at all.**

A sender row for one of them, in full: direction outbound, the single phase
`outbound_request_prepared`, outcome `incomplete`, **4025 bytes announced and zero
transferred.** The request was prepared and reached nobody.

That is not the case the frozen Stage D contract anticipates. The contract's case is a reply
lost *after* the destination commits, which leaves a strand on the sender **and a record on
the receiver** — 26 of the 149 are exactly that, and they are joinable. The other 123 are a
connection that failed before the peer read anything, and the candidate keeps a permanent,
never-terminated audit row for each.

**These compound.** The audit table is append-only. The receiver verifies it before replying,
so the more rows it holds the slower it answers; the slower it answers the more senders time
out; every timeout writes another row. One observation on lab-c needed thirty identical
retries before it was accepted. The unbounded pre-reply verification already has its own lot,
and this measurement gives it a number: one short campaign, three observations, ten thousand
rows.

## What the gate said, and what it should say

The comparator refused the campaign and named 149 unaccounted attempts. Under the amended
predicate that is the correct verdict on this evidence: an attempt with no receiver-side
authenticated join **is** unaccounted, and the predicate says so in terms.

But the verdict is not useful as it stands, because it does not distinguish the two
populations. Twenty-six attempts are the contract's own anticipated representation and could
not be joined only because this campaign's converged captures were contaminated by the store-path
defect. One hundred and twenty-three are a different fact about the candidate: requests that
never left. Reporting them under one number tells an operator that the gate failed without
telling them what to fix.

That distinction belongs in the accounting output before another campaign runs, and it is
the open work this campaign creates.

## One honest record the gate nearly refused

One sender row on lab-c read the four-byte length prefix of a reply announcing 690 bytes and
then lost the connection: outcome `unavailable`, reply frame 4 against announced 690, no
receipt, no reply digest. The candidate recorded the truncation faithfully.

The campaign-wide framing check treated every row with a non-zero reply frame as a completed
reply, so that faithful record failed the whole campaign. Applied to every row, the check
would have refused any real campaign containing a single mid-reply disconnection — the exact
failure this lot exists to account for. It now considers only rows whose outcome set is
exactly `accepted`.

## What this campaign does not show

It does not show that the join works end to end, because the converged captures were taken
with the defective collector and carry a false absence. Twenty-six joinable attempts exist in
the evidence but were never decided by a clean run. A campaign with the corrected harness is
required, and this one is kept as the record of why.
