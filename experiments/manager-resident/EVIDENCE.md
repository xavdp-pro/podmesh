# Resident replication evidence

## Package-candidate CLI increment

At commit `13e7051f687775125fcef10290de6a148c2643f5`, thirteen process
integration tests pass, including strict CLI errors,
installed-facing `--version`, explicit network opt-in, exact declared directories,
symlink/hardlink/traversal refusals, group-readable state layout and offline
validation. An already bound configured TCP address does not prevent offline
validation; no database, resident lock or socket appears. Existing non-SQLite
bytes remain unchanged because validation does not open the durable store.
Legacy runtime tests explicitly set `authenticated-static-peers`.

A separate disposable local proof ran the binary as effective UID 1000 against
a root-owned `0640` config with group 1000, state `0750`, runtime `0700`, mode
disabled and the configured TCP address already held. Exit was 0, response was
`configuration_valid=true`, `durable_store_checked=false`, `network_started=false`;
both state/runtime directories remained empty. Only that temporary config's
ownership was changed with sudo; no service or package configuration was changed.

Transport static validation was extracted as pure `ConfigurationFile::validate`;
its focused regression confirms no store/listener opening. Claude Code Opus high
found that the integrated laboratory still invoked the legacy positional form
without the now-required explicit network mode. That caller was corrected and the
integrated suite rerun. Independent closure of the remaining CLI boundary is
complete: the source review found no remaining blocker or important issue in the
CLI, path boundary, offline validation, network refusal or legacy compatibility.
It identified deployment-procedure defects outside this source boundary; the
installation qualification review tracks their correction separately.

Date: 2026-09-12. Scope: disposable loopback processes and SQLite stores.
Implementation: Codex GPT-6 Astra; inherited effort not independently exposed.
Claude Code Opus high counter-review found four important issues. All are corrected:
strict empty-struct control variants reject unknown fields; owned socket cleanup
precedes fallible joins; failed exchanges clear historical count differences; TCP
and Unix I/O retry EINTR without renewing the deadline. Claude's independent
closure review reported no remaining blocker or important finding for this
documented experimental scope.

| ID | Boundary | Test |
| --- | --- | --- |
| MR-01 | Periodic authenticated replication | Three simultaneous compiled children converge three independently created facts |
| MR-02 | Live partition/reconnect | Six TCP proxies isolate a still-running replica; two others append/exchange; reconnection catches up |
| MR-03 | Durable restart | SIGKILL, collect child exit, explicitly remove stale private socket, restart same store, verify history |
| MR-04 | Conflict visibility | Two owners claim one exclusive resource; all replicas retain both facts and report blocked resource; activation authority false |
| MR-05 | Authentication/atomicity | Wrong key yields no authenticated successes/imports; correctly signed valid-then-invalid batch reaches durable validator and commits zero facts |
| MR-06 | Admission/frame/shutdown | Thirty connections remain at two workers; excess counted; shutdown drains; oversize/trickle cannot mutate history |
| MR-07 | Config and duplicate instance | Invalid bounds/unknown fields refused; second resident with distinct free bind/socket but same DB lock cannot start |
| MR-08 | Earlier error/control regressions | At the recorded package-candidate increment, extra fields on status/shutdown were refused, a status Store failure terminated and cleaned the socket, and partition reset the count delta. The Stage R increment below supersedes the status/store behavior. |

Eight integration tests passed at that increment. Tests inspect durable state through independently
opened Store instances and the typed status interface, beyond process existence.
Transport tests additionally exercise the accepted-connection seam and absolute
frame deadline.

The interrupted-syscall retry branches were inspected and surrounding I/O gates
rerun; no deterministic signal/EINTR injection is claimed. After replacing four
release-and-rebind ephemeral-port test patterns with retained listeners, Codex
reran the complete resident suite, strict Clippy and formatting at the commit
above. The recorded run completed with 13 passed and no failure.

```sh
cargo test --locked --manifest-path experiments/manager-resident/Cargo.toml
cargo clippy --locked --all-targets --manifest-path experiments/manager-resident/Cargo.toml -- -D warnings
cargo fmt --manifest-path experiments/manager-resident/Cargo.toml -- --check
cargo test --locked --manifest-path experiments/manager-network/Cargo.toml
cargo clippy --locked --all-targets --manifest-path experiments/manager-network/Cargo.toml -- -D warnings
cargo fmt --manifest-path experiments/manager-network/Cargo.toml -- --check
```

No installed hosts, customer services, power failures, encrypted tunnels, DNS,
Podman activation or fencing were tested. Flood shutdown passes a 15-second test
deadline; that is not a guarantee under arbitrary storage/OS stalls. Exact causal
lag, retention, lost receipts, admission fairness and HA remain unproven.

## Stage R observation and read-only inspection increment

Date: 2026-09-13. Scope: resident source, disposable loopback processes and
SQLite stores. No package, service, host, network-policy or deployed-system
change was made.

The resident requires `observation_writer_uid`. A Unix `append_observation`
request is credential-checked before a worker can call Store and has no authority
fields: it creates only a nonexclusive, inactive `Observe` request. The regression
covers successful UID-bound append, replay and conflicting operation-ID reuse;
unauthorized UID with no observation committed; rejected authority fields,
reserved `network:` operations and invalid token/scope/value input; disconnect
after durable commit followed by restart/replay; and changed operation content
after restart refusal.

One append worker is admitted at a time. Read, worker wait and response share an
absolute 250 ms connection deadline. The SQLite `BEGIN IMMEDIATE` proof holds an
external write transaction, receives `append_observation_uncertain` before that
budget, then an immediate identical retry receives `append_observation_busy`
without admitting another worker. It receives shutdown promptly, releases the
lock, drains the child, and
retries the same request to recover its committed receipt. This proves bounded
control response and idempotent recovery, not cancellation of SQLite work.

The raw control frame limit is 32,768 bytes, while decoded nonempty UTF-8 values
remain limited to 4,096 bytes. Process tests exercise backslash-heavy and maximally JSON-escaped control-character
4,096-byte values
and exact 32,768/32,769-byte raw frames. Scope tests cover hierarchical owned
scopes and reject empty, traversal, leading/trailing or malformed segments.

Live status is intentionally a compact diagnostic, with no Store call and no
canonical durable output. A regression inflates the durable audit table beyond
600 rows after initial attempts, then proves `resident_observation` remains within the 32,768-byte response bound and points canonical verification to `--inspect-store`.
A one-shot sub-300 ms regression proves status remains available while one
outgoing peer has accepted a request but withholds its reply and an incoming peer
simultaneously stalls a partial frame. Canonical inspection
remains external/offline through `--inspect-store`.

`--inspect-store` requires only config and state directory. Its process proof uses
unusable bind, peer key, interval, backoff and worker settings and still obtains a
canonical read-only inspection; it compares the main DB source bytes before and
after and verifies a missing store is refused without creation. This run does not
claim preservation of live WAL/SHM sidecars, independent privileged-writer
concurrency, or a root inspection path for service-account-owned state. The
durable inspector copies its source below `$TMPDIR`.

The status/shutdown controls rely on the private `0600` socket; they have no
separate peer-UID authorization. Stage P must define their policy before any
socket ACL/group broadening.

The 2026-09-13 recorded checks passed: resident process suite 22/22, strict
resident Clippy and formatting, and `git diff --check`; manager-network
regressions 28 plus one three-process test; manager-ha regressions 10 plus 69,
with one documented helper test ignored. The three-process test routes normal
observations through the Unix append API; direct Store observations remain only
for the explicit lower-layer exclusive-conflict setup.

No UID/group ACL deployment design, package account, external writer admission,
real host permission proof, system service, HA/failover, activation or fencing
claim follows from these tests. The package-facing private `0600` socket currently
makes UID 0 or the service account the practical writer boundary; Stage P owns any
group/ACL design.

## Signed votes and the signing ledger (V3-4)

Date: 2026-09-18. Scope: the library's signer, ledger, readmission and assembly, with real child
processes for crashes and lock races, and one compiled resident process driven over its control
socket. Temporary directories and test-only keys (a one-byte seed); no laboratory host, no real key.
Code at `12a5adf`, the lock test's stagger at `19c287d`.

What each test proves (`src/vote_tests.rs`, `src/quorum.rs`, `tests/votes.rs`):

| Test | Proves |
| --- | --- |
| `a_vote_is_signed_once_per_resource_and_epoch_across_restarts` | one decision per (resource, epoch): the same decision re-signed, with a fresh life too; another holder, another grant or another barrier refused (`epoch_already_promised`); a lower epoch refused (`epoch_superseded`); the same after the signer is reopened from its files; another resource's epoch is its own |
| `a_vote_is_signed_only_for_a_live_payload_under_the_replicas_policy` | no vote for an expired, too long, too early, foreign-policy, float-carrying, epoch-skipping or already-signed payload, and nothing promised for any of them |
| `a_crash_between_the_ledger_write_and_the_signature_releases_nothing` | a child process aborted after the write before its fsync, after the rename before the directory's fsync, and after the directory's fsync before the signature, releases nothing; the ledger reads back; where the write was not durable, both disk states a power loss can leave are tried; every signature released for the epoch names one holder; after the directory's fsync the promise holds |
| `a_ledger_restored_from_an_older_copy_trips_the_wire` | a ledger put back from an older copy signs what it forgot while it is shown none of its later votes (the hole the restore procedure's mark covers), trips `ledger_behind_own_votes` once shown one, and `own_vote_unknown_to_ledger` when it has signed past the old numbers; the mark is durable; a forged vote in its name fires nothing |
| `a_ledger_naming_another_host_or_key_signs_nothing` | a copied vote directory on another host signs nothing (`ledger_foreign_host`), nor marks nor readmits; a ledger in another key's place (`ledger_foreign_key`), a missing, a new, and a hand-altered ledger (`ledger_missing`, `ledger_unadmitted`, `ledger_unreadable`) sign nothing; a key the policy does not name, or names with another public half, or a group-readable key file opens no signer |
| `two_signers_on_one_ledger_are_serialized_by_its_lock` | two processes asked for epoch 9 for X and for Y, pausing 300 ms and 900 ms between reading the ledger and checking the promise: exactly one signs, three rounds |
| `a_forged_origin_is_refused` | a vote made by replica-b's key in replica-a's name (`bad_envelope_signature`), a real vote renumbered by a relay (`bad_envelope_signature`), a replaced payload signature (`bad_signature`), a key outside the policy (`unknown_voter`), a vote in another replica's scope (`origin_mismatch`); with them, the assembly still counts one key |
| `k_minus_one_votes_make_no_certificate` | one vote, one voter twice, and votes on two payloads are `below_threshold`; two voters on one payload make a certificate the node's rules accept |
| `k_votes_assemble_into_the_certificate_the_node_accepts` | the assembled certificate is byte for byte the pinned vector, which the node's own verifier at `5022a7b` accepted (below); a vote is no certificate (`certificate_kind`, `mixed_forms`); a policy-change certificate assembles and verifies |
| `a_policy_change_cannot_reopen_an_epoch` | the same key after a change to serial 1 still refuses epoch 5 to another holder; one change per (resource, from_serial) (`serial_already_promised`), none from below a promised serial (`serial_superseded`) or from a policy it does not vote under; a re-keyed replica's new ledger, readmitted with the retired key's vote in its store, has floor 5 and refuses epoch 5 under the new policy; with the retired key not configured, readmission refuses rather than skip the vote |
| `readmission_with_an_unreadable_input_refuses` | the counter-review's D8b without an omniscient operator: a peer store missing or altered after inspection, a peer ledger altered, a screen of another host, the own store unreadable, and evidence collected before the mark each refuse by name, the ledger staying unadmitted; with every input readable it waits out the certificate life (`readmission_too_early`), then sets the floor at the highest epoch seen (the screen's 3), raises the sequence above the key's own vote, and refuses epoch 1 for another holder |
| `the_tally_finds_no_conflict_when_promises_are_kept` | the tally makes one certificate per decision and names a conflict only when two certified decisions share an epoch, which needs signatures made outside a ledger |
| `quorum::node_tests` (9) | the node's verifier tests ported: its pinned policy digests (2-of-3 at serials 0 and 1, the single key's 1-of-1), its Python-signed certificate, k-of-n and k-1, duplicates, foreign keys, relabelled payloads, byte flips, malformed certificates, declaration refusals |
| `tests/votes.rs` (one process test) | through the control socket: the ledger's operations refuse another UID; a missing and a new ledger sign nothing; readmission names the three inputs it lacks, then waits, then admits above the screen; a vote is recorded in `votes/r0` before it is answered; the promise holds across a restart of the process; the ledger put back from an older copy trips the wire on the resident's own recorded vote, which the status and standard error report; the directory seen from another host signs nothing; the resident's vote and replica-b's make a certificate |

**The node accepts the certificate.** The certificate replica-a's and replica-c's votes assemble for
the fixed payload of `k_votes_assemble_into_the_certificate_the_node_accepts` was verified by the
PodMesh node's own `signing::verify_takeover` and `Quorum::verify` at `5022a7b`, in a scratch copy of
that tree with one added test (not part of the node): accepted, signers `replica-a` and `replica-c`,
policy digest `965bd61a...`; refused `below_threshold` with one signature removed, and
`bad_signature` with `new_holder` changed. The test pins the same bytes, so a drift of the
assembly fails here.

**Negative controls.** Each protection was removed in turn at `19c287d`, the test that claims it run,
and the source restored byte for byte; all thirteen fired, and every test passed again after:

| Removed | Test failed with |
| --- | --- |
| the refusal of a second decision for a promised epoch | another barrier signed for epoch 5 |
| the order write, fsync, then sign (the vote sealed and released before the write) | a crash after the write released a signature |
| the tripwire | the restored ledger signed epoch 7 for Y while shown its own later votes |
| the machine-id comparison | the copied directory signed on the other host |
| the `flock` | both children signed epoch 9 |
| the envelope signature's check | the forgery in replica-a's name fell through to the payload check (`bad_signature` instead of `bad_envelope_signature`); a renumbered vote, whose payload signature is genuine, would then count |
| both signatures' checks | the vote in replica-a's name, made by replica-b's key, counted |
| the assembly's count (k-1 enough), the node-rule self-check kept | refused as `certificate_self_check`, not `below_threshold`: the self-check caught it |
| the count and the self-check | a one-signature certificate was returned |
| the sorted canonical form | the policy digest was no longer the node's |
| promises keyed on the resource alone (the digest added) | epoch 5 for Y signed after the change to serial 1 |
| readmission's reading of the stores' votes | the re-keyed ledger was admitted with no floor, and the retired key's vote went unread |
| readmission's refusal of unreadable inputs | the ledger was admitted with replica-b's store missing |

The recorded checks: resident library 23/23, resident process suite 42/42 plus the votes process
test and the two inspection tests; manager-ha and manager-network suites unchanged and passing;
strict resident Clippy and formatting; the tree's lexicon test.

Not shown here: a power loss (A2 is the disk's; the laboratory disks' `cache=none` was read on
2026-09-18), a whole-VM clone (indistinguishable, A3: the operator's rule), a peer that forges votes
over a live authenticated link between two residents (the counting refuses them on any path, shown
at the library), and anything of V3-5: proposing, collecting, deciding, delivering.

### After the counter-review of V3-4 (`3531ca4`)

The counter-review (`podmesh-lab` `records/reviews/REVIEW-V3-4-SIGNED-VOTES-2026-09-18.md`, "holds
with changes") found five points; the operator told us to follow its recommendations. Date:
2026-09-19. The read-only mounts of these tests are real: a helper process holds a read-only bind
mount in its own user and mount namespace (`unshare -rm`), and the test reaches it through
`/proc/<helper>/root`. A test that cannot make one says `SKIPPED` on standard error; none skipped in
the recorded runs, and the break script refuses a run that does.

| Test | Proves |
| --- | --- |
| `evidence_substituted_after_its_digest_was_stated_is_refused` | a peer's store replaced after its digest was stated, by a shorter history consistent in itself that drops the peer's vote for epoch 4, is `readmission_evidence_mismatch`, naming it; so is a digest stated for a file readmission does not read; a required file with no digest stated is unreadable; the ledger stays unadmitted; vouched for as it is, the same substitute would have been admitted with a floor below 4 |
| `evidence_placed_in_the_vote_directory_is_ignored` | complete, correct evidence in `<vote_dir>/readmission/` with its digests stated, and an empty evidence directory: only the own store is read and the five inputs are named unreadable; an evidence directory that is the vote directory or inside it is `evidence_dir_inside_vote_dir` |
| `the_evidence_directory_must_be_one_the_replica_cannot_write` | a directory of the replica's own user is `evidence_dir_writable`, at mode 0755 and at 0555 with 0444 files (it could `chmod` them back); the same directory mounted read-only is accepted; a symlinked evidence file in it is refused |
| `the_host_identity_is_read_only_from_a_read_only_mount` | the machine-id on a writable mount is `host_identity_not_read_only`; a symlink to it and a relative path are `host_identity_unreadable`; mounted read-only it reads |
| `a_replaced_lock_file_lets_no_second_signer_in` | signer X paused after reading the ledger and holding the lock; the lock file of the previous design removed and replaced; signer Y started: exactly one signs epoch 9, three rounds |
| `readmission_with_an_unreadable_input_refuses` (changed) | `retry_at` is pinned at the mark plus the certificate life plus 60 s |
| `tests/votes.rs` (extended) | the resident given its machine-id on a writable mount says `host_identity_not_read_only` on standard error at the start, in its status, and to `vote_sign` and `vote_ledger_init`; given it and the evidence directory through read-only mounts, it runs the whole chain, readmission stating digests; the screen replaced after its digest was stated is `readmission_evidence_mismatch`, then the original is admitted |

**Negative controls.** The thirteen earlier breaks were run again at `3531ca4` and all fired; one
break per new protection was added, and all six fired; every test passed again after the source was
restored byte for byte:

| Removed | Test failed with |
| --- | --- |
| the comparison of each evidence file with its stated SHA-256 | the substituted store was admitted |
| the separate evidence directory (read from `<vote_dir>/readmission/` again) | the five inputs were read from the vote directory |
| the check that the replica cannot write the evidence directory | the replica's own directory was accepted |
| the read-only check of the machine-id's mount | the machine-id was read from a writable mount |
| the lock on the vote directory (a lock file again, with an inode check after locking) | both signers signed epoch 9: an inode check does not stop a signer that locks the replacement |
| the 60-second bound (30 s again) | readmission admitted 30 s before `retry_at` |

**What was not closed, and why.**

- **Whole-VM clones.** The read-only check keeps a replica from rewriting its own identity. It does
  not tell a whole-VM clone from its original, which carries the same machine-id, key and ledger. The
  rule of no VM clone of a laboratory host on the managed network (A3) stays what closes that.
- **A silently restored ledger.** No local marker narrows this case soundly. Everything kept on the
  host (a marker file, the ledger's mtime or nonce, the store) is reverted with the host's snapshot
  and stays consistent with the ledger it was reverted with. The boot ID changes at every legitimate
  reboot. What survives a revert is off the host: the other hosts, which the tripwire reads, and the
  operator, who marks the ledger. The README's restore checklist makes the mark the precondition of
  every restore.
- **Ownership by the operator.** The ownership of the evidence directory by the operator, rather than
  by the service's user, is packaging (V3-5). A root resident passes the check only through a
  read-only mount.

The recorded checks at `3531ca4`: resident library 28/28, resident process suite 42/42, the votes
process test and the two inspection tests; manager-ha and manager-network suites unchanged and
passing; strict resident Clippy and formatting; the tree's lexicon test.

## The manager decides (V3-5)

Date: 2026-09-19. Scope: the library's rules, compiled residents on loopback, the operator's collector,
and one end-to-end test on a single workstation. That test runs three residents and three real PodMesh
nodes (`podmeshd` run unprivileged on its own state directory and socket: activation needs no Podman),
with the node tree's host agent (`claude/v3-5-host-agent`) delivering certificates. The keys are
test-only, and the temporary directories live under the build scratch. No laboratory host, no real key.

| Test | Proves |
| --- | --- |
| `decision_tests::the_view_is_the_highest_certificate_or_the_baseline` | the view is the highest-epoch certificate the store's verified votes assemble into, with its holder, barrier and expiry, and the certificate verifies under the node's rules; one vote moves nothing; the operator's baseline stands until a certificate passes it; two decisions certified for one epoch are a conflict, and nothing is decided on a view that holds one |
| `decision_tests::a_proposal_is_checked_against_the_view_rule_by_rule` | each rule by its refusal: the kind, no unbound field, the policy, the resource, the life (expired, too long, issued too far ahead), the holder among the nodes, the identifiers, the barrier before the expiry, the next epoch only, the view's previous holder, `first` only before any epoch, `same_holder` only to the current holder, `fence_receipt` never, an unknown method never; the barrier each method requires, at its exact bound and one second before it: carried for the same holder; for another holder, also the current certificate's expiry, the renewal bound and the proposal's issue, each plus the lease and the margin |
| `tests/decisions.rs` `two_replicas_of_three_decide_and_one_does_not` | three residents: `first`, a same-holder re-issue for a new boot, then a rotation, each proposed on one replica and read decided on another, with a certificate the node's rules accept; a rotation whose barrier ignores the current certificate's expiry is refused by every voter (`barrier_too_early`) and through `vote_sign`, and never decided; a signature retried on a decided payload is answered with the recorded vote (`replayed`), twice, and one vote fact is stored; a replica whose ledger is marked unadmitted does not vote and the two others decide; with one of those stopped, one vote decides nothing and the read says one is missing; every vote operation answered within the vote deadline |
| `tests/decisions.rs` `a_peer_forging_votes_over_a_live_link_has_none_counted` | a peer holding its own key and its link's pair key pushes, as an authenticated exchange, a proposal the honest replicas refuse, its own genuine vote, a vote in replica-a's name made with its key in its own scope, and a fact it attributes to replica r0 carrying another such vote. Both honest replicas import all of it, and each counts one vote, the peer's own: nothing is decided |
| `tools/test_collect_readmission_evidence.py` | the collector writes nothing, and exits 3, when the plan misses an input or names an extra one, or when a command fails, prints no JSON, or prints another input than it claims (a store without its digest, another key's ledger, another node's answers). It also writes nothing for a missing evidence directory, or for a replica whose ledger is not marked unadmitted. Otherwise it writes one read-only envelope per input, collected after the mark, and prints the digests of exactly those bytes |
| `packaging/podmesh-manager/universe/replicated/test_votes_tools.py` | `vote-key.py` derives the test vectors' public halves in pure Python, writes a new seed 0600, prints only its public half, and never overwrites a key; `add-votes.py` gives a generated replica set its vote and proposal scopes and a votes section under the node's pinned policy digest `965bd61a...`, and each configuration passes the resident's own offline validation |
| `tests/e2e/decisions-e2e.py` (14 checks, below) | the V3-5 predicate on one machine, end to end |

**The end-to-end test.** Run: `PODMESH_MANAGERD=<release resident> PODMESHD=<release podmeshd>
PODMESH_DECISION_FOLLOW=<node tree>/packaging/podmesh-decision-follow python3 -B tests/e2e/decisions-e2e.py`.
It shows, in order:

1. New ledgers sign nothing: a proposal made while every ledger is unadmitted gets no vote.
2. The operator's readmission of two ledgers, through the collector: two stores, two ledgers and
   three nodes' `activation_status`, their digests stated, after the 80 s wait (life 20 s plus 60 s).
   The third ledger stays unadmitted and never votes.
3. Epoch 1 is decided by those two replicas. The agents deliver it: the named host acquires, the two
   others are superseded, every screen moves to 1.
4. A same-holder re-issue (epoch 2) is decided and delivered.
5. A rotation to another host whose barrier the view does not allow is refused by the voters, and
   nothing is delivered. The rotation with the barrier the view requires is decided; the new holder's
   node refuses it until its barrier ("27 seconds from now on this clock") and takes it after, the
   previous holder is superseded, and every screen moves to 3.
6. Epochs 1 and 2, delivered by hand as supersessions and as acquisitions, are refused by all three
   nodes: by the screen, by their expiry, or as naming another host. No screen moves.
7. With one replica stopped and one unadmitted, the vote left decides nothing, and the agents deliver
   nothing. A 1-of-3 certificate hand-made from that vote is refused by the node (`below_threshold`),
   and so is the same signature listed twice (`duplicate_key`).
8. The stopped replica is back. The same decision, re-issued with a fresh life, is decided (its first
   voter signs it again under one promise) and delivered: every screen moves to 4. A further run
   delivers nothing twice, and every vote operation answered within 2 s.

**Timings.** The process tests run with a debug build whose curve arithmetic is optimized
(`Cargo.toml` profile). The longest `decision_read` measured was 342 ms and the longest voter pass
308 ms, with three residents on one disk; typical values were 17 to 60 ms. That can exceed the 250 ms
control deadline. Vote operations therefore answer within a vote deadline of 2 s, and the control loop
keeps serving meanwhile (README).

**Negative controls.** `podmesh-lab/claude/v3-5-manager-decides/breaks.py` removes each protection in
turn, runs the test that claims it, and restores the source byte for byte. The heads were web
`cd754f5` and node `3eadcfc`. The 22 breaks below all fired, and every test passed again after.

| Removed | Test failed with |
| --- | --- |
| the same holder carries the barrier | a same-holder barrier earlier than the current one passed |
| a rotation covers the current certificate's expiry | the library's bound passed one second early; the process test's early rotation was not refused |
| a rotation covers the renewal bound | the required barrier fell from the renewal bound to the expiry's |
| the next epoch only | a skipped epoch passed |
| the previous holder compared with the view's | a wrong previous holder passed |
| `fence_receipt` refused | a fence receipt was voted for |
| no unbound field | a payload with an extra field passed |
| the holder among the nodes | an unknown holder passed; the forger's proposal was not refused |
| the view needs k votes | one vote moved the view |
| a conflict decides nothing | a view with two certified decisions for one epoch passed a proposal |
| the voter checks its view | the early rotation and the forger's proposal got votes |
| `vote_sign` checks the rules | the early rotation was signed directly |
| a retried signature answers the recorded vote | the retry was refused `epoch_not_next` |
| an unadmitted ledger signs nothing | the unadmitted replica voted |
| a vote's two signatures verified | the forged votes counted |
| the agent supersedes only above the screen | a supersession sent at the screen |
| the agent acquires only what names this host | another host's certificate acquired |
| the agent leaves a held lease alone | the held lease acquired again |
| the agent delivers nothing on a conflict | a conflicting decision delivered |
| the agent starts only an idle publisher | a running publisher started again |
| the node's door relays only `decision_read` | the agent's whole request relayed |
| the evidence mounted read-only | the mount arguments lost `ro=true` |

Two more breaks ran the whole end-to-end test, with the binary they touch rebuilt. The heads were web
`61d9456` and node `99e2cd1`, and the baseline and the run after restoring passed, 118 s each. Both
fired at the check that claims them:

| Removed | End-to-end failed at |
| --- | --- |
| the voter's check of its view (every live proposal above the view voted for) | "r0 refuses the early rotation": the rotation that ignored the barrier got votes |
| the node's count (k-1 signatures accepted, in the node's `signing.rs`) | "the node refuses a hand-made 1-of-3 certificate": it was accepted |

The recorded checks at the final heads: resident library 30/30, the process tests `decisions` 2/2,
`votes` 1/1, `processes` 42/42 and `inspect_facts` 2/2; manager-ha and manager-network suites
unchanged and passing; strict resident Clippy and formatting; the collector's and the vote tools' tests;
the end-to-end test, 14 checks; the lexicon test.

**Not shown here.** These need a laboratory host or the operator:

- the relay of `manager_decision` into a running manager universe;
- the three host-state mounts made by a real `podman create`;
- the units under systemd;
- `publisher_start` with a certificate on a real connector;
- a power loss right after a signature;
- all of it with the workstation switched off.

The majority does not extend leases (V3-6). A rotation away from a holder that may still renew by
itself therefore waits for the renewal bound.
