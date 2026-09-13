# Manager2 G2 live activation review

Status: **not passed; coordination decision pending.** Two live three-host
campaigns of the frozen manager2 candidate converged and cleaned up through the
reviewed harness. The checked-in four-stage comparator does not return PASS,
and neither the evidence nor the gate was altered to make it.

Date: 2026-09-13. Implementer and author of this review: Claude Code (Fable 5.1),
under the standing automation mandate. Independent review of the harness fix
before it was shipped to the hosts: a fresh, read-only Claude Opus context (GO,
with two should-fix findings applied and one pre-flight check performed). Three
additions post-date that GO — the per-function static label check, the
elsewhere-bound fixture and the sidecar file-name binding (HD-3) — and were
first reviewed in the pre-commit review recorded at the end.

## Result in one paragraph

Three authenticated manager2 replicas were started on the three laboratory
hosts, one owned non-exclusive observation was accepted on each, all three
canonical stores converged to one logical history digest, and every replica was
shut down through the typed control operation with its durable state retained
and no unrelated service, container, route or firewall commitment changed. That
happened twice today. The comparator refuses both campaigns. The first cannot be
compared at all because of two harness defects, now fixed and proven on the
second. The second fails exactly one condition: every store retains outbound
exchange attempts that the candidate deliberately records as uncertain, and the
gate requires zero. Why that happens, what it does and does not mean, and the
two ways to resolve it are below.

## Candidate binding

- Source commit: `ff77b1f946e82af421de2ccdbe06d8cd45b70c33`
- Package: `podmesh-manager 0.1.0~manager2+gff77b1f946e8`
- Binary SHA-256: `cbd5020a37d2b6b2ab94ee41a7ea9d2f50d0da36c6fb972f795c8a1fe3660128`
- Debian SHA-256: `4e475a2421302b5a3c1c6583853f9e04515076cbd00f430e805ecb9a3ecefc8d`

Every capture of both campaigns re-verified the installed version, the exact
binary hash and a clean `dpkg --verify` against the candidate-verification
document. The protected three-grant configuration and the persistent
first-activation marker from the earlier configuration transition were in place
on all three hosts before either campaign and were not modified.

## The two campaigns

| | Campaign 1 | Campaign 2 |
| --- | --- | --- |
| Orchestrated by | OpenAI Codex (operator's external campaign) | Claude Code, through `run-campaign.sh` (private, retained) |
| Harness | as of commit `cfffd7a` | `capture-host.sh` `248b4a43…`, the other five collector scripts unchanged from `cfffd7a` (hashes in `campaign-summary.json`) |
| Activation, in frozen order lab-a, lab-b, lab-c | passed | passed; 05:59–06:01 UTC |
| Observations accepted | 3 | 3, each on its first attempt |
| Converged digest, all three replicas | `6244a8daad86df04b15200d01513b3f7d4ff0af63721f92cf0309e37f6178f72` | `652145a63a80b109000f848aee90f9c22a8d31c3ed2906c6066a279588230e3e` |
| History count at converged / after cleanup | 3 / 3 | 6 / 6 (the three earlier facts plus three new) |
| Digest retained after cleanup on all three | yes | yes |
| Typed graceful shutdowns | 3, plus one cleanup-only restart on lab-a | 3, no restart |
| `incomplete_attempt_count` converged (a/b/c) | 1 / 0 / 1 | 51 / 40 / 23 |
| `incomplete_attempt_count` post-cleanup (a/b/c) | 0 / 0 / 2 | 50 / 46 / 27 |
| Comparator as of `cfffd7a` | FAIL: `active-baseline:lab-a.dropin.semantic_limits: unsafe shape` | — |
| Comparator of this commit | FAIL, 17 failures, all cross-host joins plus convergence | FAIL, 5 failures, all `incomplete_attempt_count` |

The second campaign's twelve stage files and their sidecars are published
byte-for-byte under `docs/qualification/manager2-g2-live/`, with the strict
comparison and a public summary; the checked-in comparator reproduces
`comparison.json` from that directory. The sidecars carry the evidence path the
producing hosts wrote them under, which names aliases only. The summary separates
what the twelve files carry from what was read live during the campaign and is
therefore reported, not reproducible from the directory. The first campaign's files are retained
privately with these digests:

```text
0af6a451b089cf249ee5f23f12792848107ddf0eae4391d904cc88825c93263a  lab-a/pre-activation.json
999820df7db486175035f92b239de22de9ef57040c08b11b768f42786ab4c57a  lab-a/active-baseline.json
b408cc047b7399438ba8f0772d22f750fcd842443d8ac1838e26c0088974698c  lab-a/converged.json
c7c1d356f57dfac99ba8553723d934954adf8a24e594d485ba8f6eb04ca04c4e  lab-a/post-cleanup.json
438cd7eed5b8f50ceae3c73af2ee55dd155063cd74e0ef12eeb548926cd5f81c  lab-b/pre-activation.json
65366bde0a3adce6ada90514b3be3a67ddbba6e737df0ec5c192c935f71a523f  lab-b/active-baseline.json
ae3c1e23b88da7e8b7d94fc7f524bb5eab5c2c3ad25113f20aa273cb30c8b7ee  lab-b/converged.json
634de6312e95bf41cbd57d22b052aa668897768beeb420c2ea3b0bd351640716  lab-b/post-cleanup.json
06fb78cbad2da7f9b495cc9d6871ded0fb36b169d480a5cae9a0a67752f042ca  lab-c/pre-activation.json
a79fe25e4fb5244b5e925ac90d1afaa7e55fe8927c94062db62646c0bd0e9cd5  lab-c/active-baseline.json
c1dd6a932630140356340d5dd9677cb0dcf5b72575fb76035169f00d76fb2f78  lab-c/converged.json
89e61d334a2f9b719b0743335ab854ca1fe45bd1153c2a13bef695f20cb8b686  lab-c/post-cleanup.json
```

## Harness defects found, fixed and proven

Both are contract drift between the collector that runs on the hosts and the
comparator that runs offline, and both were invisible to the synthetic
fixtures, which build their evidence in the comparator's own image.

**HD-1 — the comparator refused the collector's `semantic_limits`.**
`validate-dropin.py` returns the SHA-256 of the drop-in bytes it parsed inside
its result object; `capture-host.sh` embeds that object verbatim as
`dropin.semantic_limits`, beside an outer `dropin.sha256` taken separately from
the installed file. The comparator required exactly four fields. It now
requires, for a present drop-in, exactly five fields, a valid SHA-256 and
equality with the outer hash — the validated grammar and the installed policy
must be one file — and keeps the four-field shape for an absent drop-in. Tests:
a missing, a malformed and a mismatched inner hash and a stray hash on an
absent drop-in are each refused for their own stated reason; the passing
fixtures are asserted to carry the bound hash rather than bypass it.

**HD-2 — the collector committed one identity under a different label at each
observation site.** A commitment is `SHA-256(salt ‖ label ‖ value)`. The
replica ID was committed as `local-replica` in the configuration, `peer-replica`
in peer entries and `inspection-replica` in the inspection; the logical manager
ID as `logical-manager` and `inspection-logical`; endpoints as `peer-endpoint`
and `bind-endpoint`. The comparator joins exactly those values across sites and
across hosts, and two labels for one value make its commitments unrelated, so
no real capture could ever satisfy the joins. This was confirmed with the
campaign salt before anything was changed: recomputing each recorded field with
its site label reproduces the recorded commitment on all three hosts, and
recomputing under one label makes every join hold. No private value left its
environment. The collector now uses one label per kind of value — `replica-id`,
`logical-manager-id`, `endpoint` — at every site; labels that are only compared
same-site or for distinctness are unchanged. The static regression check
verifies the exact call at each observing function of `capture-host.sh`, not a
file-wide count, and a new three-host fixture proves that a replica bound
elsewhere than its advertised endpoint fails the join. Campaign 2 is the proof
on real evidence: none of campaign 1's seventeen join failures remains.

**HD-3 — sidecars are bound to a file name, not to a directory.** A sidecar is
written by the producing host with that host's absolute path. The comparator
accepted only that exact path or the bare file name, which forced the evidence
tree to be mirrored path-for-path and made the published copy unverifiable in
place. It now accepts a sidecar whose file name matches and still refuses one
naming another file; the digest still has to match the bytes. Tests cover the
relocated and the misnamed case. The published evidence is therefore verifiable
with the checked-in comparator exactly as produced, sidecars included.

## The remaining failure, and why campaign 1 nearly passed

Every incomplete attempt in every store of campaign 2 is an outbound exchange
whose last audited phase is `outbound_request_prepared`, and the same attempt
identifiers persist from one inspection to the next. The candidate does this on
purpose. In `experiments/manager-network/src/lib.rs`, after the request has
been fully written, a read that returns no reply byte with an unavailable error
returns without the terminal `outbound_exchange_completed` audit:

> A complete request followed by total reply loss is uncertain. Stage D
> intentionally preserves only the prepared attempt.

The reply is lost because the resident accepts at most `incoming_workers`
concurrent inbound connections and, in `experiments/manager-resident/src/lib.rs`,
accepts and then drops any further connection without replying. The laboratory
configuration, like `packaging/podmesh-manager/config.example.json`, sets
`incoming_workers` to 1 and `interval_ms` to 1000: three replicas each
synchronize with two peers every second, so two peers reach one replica at the
same moment continually. The typed status of the three live residents at
06:05 UTC reported `incoming_limit: 1` and `rejected_connections` of 18, 29 and
35; lab-a's audit trail held 176 `outbound_request_prepared` events against 131
`outbound_exchange_completed` accepted and 14 unavailable. Audit events are
immutable and a retry gets a fresh attempt identifier, so the derived
`incomplete_attempt_count` never returns to zero. The inbound side is perfectly
balanced on all three hosts: no partial import exists anywhere.

Campaign 1 started from empty stores and ended with 1 / 0 / 1 uncertain attempts
at converged and 0 / 0 / 2 after cleanup. It was one or two collisions away from
a pass. A pass of the gate as written is therefore a matter of timing, not of
correctness, for this candidate and this configuration.

The gate is not wrong to ask. `docs/MANAGER-HA-ACCEPTANCE.md` HA-01 requires
byte-equivalent logical histories, distinct identities, no conflict, no
undeclared peer and no effect; the activation README adds zero incomplete
attempts so that a history called complete carries no unresolved exchange. An
uncertain outbound attempt is not a partial import, but it is an unresolved
question the store keeps honestly, and a gate that ignored it would be weaker
than the candidate's own bookkeeping.

## What the evidence proves, and what it does not

Proven from outside the residents, in both campaigns: three distinct replica
identities under one logical manager and one topology; reciprocal declared
peers with symmetric pair-key commitments and each peer endpoint bound to the
remote listener (campaign 2, through the comparator); one manager invocation per
host for the whole active campaign, with the expected UID, arguments, listener
set, state/runtime/socket ownership and modes; one owned observation admitted per
replica; one canonical history digest on all three replicas at the converged
stage and again after every resident had stopped; typed graceful shutdown with
`Result=success` and `ExecMainStatus=0`; unchanged existing PodMesh services,
rootful containers, routes and nftables commitments across all four stages.

Not proven, and not claimed: G2 as gated by the checked-in comparator; HA-01;
any activation authority; takeover, fencing, DNS, exclusive activation,
host-loss recovery or high availability. Nothing here says the manager is HA.

## The cleanup-verifier incident of campaign 1, and campaign 2

In campaign 1, lab-a's first cleanup verifier rejected Debian's numeric
`ExecMainCode=0` although the journal and systemd showed that the typed
shutdown had succeeded; the harness's `resume-cleanup` mode started the same
candidate once more, proved readiness, requested typed shutdown immediately and
resumed the hash-bound removal. That recovery was hardened and reviewed before
use, is disclosed here, and adds nothing to the activation or convergence
claim. `graceful-shutdown.py` and the comparator accept `exited`, `0` and `1`
for `ExecMainCode`. In campaign 2 all three hosts reported `exec_main_code: "0"`
and no host needed a cleanup-only restart.

## Open decision

Two paths keep every principle above; one is recommended.

1. **Raise `incoming_workers` through a reviewed configuration transition, then
   run a third campaign.** Each replica has one outgoing worker and two peers,
   so a limit of 2 removes collision drops entirely; the candidate's honest
   uncertainty bookkeeping and the gate stay exactly as they are. The
   `config-transition/` harness exists for this kind of change, the
   `configuration.document_commitment` must stay constant across a campaign, and
   the transition is an operator-reviewed protected-configuration change, so it
   is a separate lot with its own evidence. **Recommended**: it fixes the cause
   and changes no proof.
2. **Redefine the gate.** Extend the evidence schema so the inspection reports
   incomplete attempts by direction and last phase, require zero *inbound*
   incomplete attempts (no partial import, HA-I09) and disclose the outbound
   uncertain count as a number the report must state. This keeps candidate and
   configuration but weakens the contract the harness author wrote, so it is
   theirs to accept, and it would still leave a store full of unresolved
   questions.

Not options: rewriting captured evidence, or changing the candidate binary.

## Verification of this lot

- `packaging/podmesh-manager/qualification/activation/tests/run-tests.sh`: pass,
  with the new negative and positive cases.
- `packaging/podmesh-manager/qualification/activation/config-transition/tests/run-tests.sh`,
  `packaging/podmesh-manager/qualification/tests/run-tests.sh` (which chains the
  refusal and upgrade suites) and `packaging/podmesh-manager/test-packaging.sh`:
  pass.
- Repository: `cargo test --locked` 42 passed; `web` tests 47 passed;
  `git diff --check` clean; every relative link in `docs/*.md` resolves.
- The published evidence directory: 27 files — twelve stage files, their twelve
  sidecars, the strict comparison, the public summary and `SHA256SUMS`, which
  lists the other 26 and verifies; the checked-in comparator reproduces
  `comparison.json` from it; scanned for raw addresses, UUIDs, the campaign
  salt, the alias map and the replica identifiers, none present.
- Final host state, read after the campaign: manager inactive and disabled on
  all three hosts, no drop-in, no runtime directory, no control socket, durable
  state retained; the lifecycle daemon's PIDs identical to before the campaign.

## Three-pass closeout

Governance: authority stayed where it is — no activation authority was claimed,
no exclusive effect happened, the gate was not weakened to fit the evidence, and
the decision that remains is named and handed to the coordinator. Operator path:
both campaigns ended with every host in the required inactive state through the
typed path, and the second needed no recovery. Runtime: two harness defects and
one sidecar-portability limit found and fixed with regression tests; one
candidate-and-configuration behaviour found, explained from source and counters,
and left for decision. No independent counter-view of the campaign execution
itself was available; the harness fix was reviewed independently before it ran.

## Pre-commit review

A second fresh, read-only Claude Opus context reviewed the whole lot before this
commit: the three additions that post-date the first review, the published
directory, every number in this document and in `campaign-summary.json` against
the stage files, the claim boundaries of the three edited documents, and privacy.
It reproduced `comparison.json` byte for byte from the published directory, and
showed that `incomplete_attempt_count` is the sole blocker by setting it to zero
in a scratch copy, changing nothing else, and obtaining `status: PASS` with an
empty failure list. Verdict GO, conditional on one blocking finding — this
document promised a review record that did not yet exist — and three should-fix
findings, all applied before the commit: the static label check's slice for the
last shell function ran on to the end of the file, so a literal in the main body
could have satisfied it (each slice is now bounded at its function's closing
brace); five facts in `campaign-summary.json` were not carried by the published
stage files (moved under `reported_not_in_this_evidence`); the file count above
was wrong (corrected). It did not run the Rust, web and packaging suites because
the workstation's root filesystem was nearly full at the time; those three
results above are the implementer's own runs.
