# Fencing laboratory evidence

Date: 2026-09-12. Producer: OpenAI Codex, GPT-6 Astra at high effort, selected by
the coordinating agent for this safety experiment. Owner: Xavier de Poorter. This
is an isolated implementation candidate for independent review.

Environment: Python 3.11.2, SQLite 3.40.1, local Linux process/file fixtures.

## Executed checks

- `python3 -m unittest -v test_fencing`: post-review source rerun, 22 tests passed in
  22.455 seconds (exit 0).
- Seed `20260912`: 600 attempts, 331 accepted effects, 269 refusals, 57 explicit
  fixture transfers, 158 stale-permit attempts and 46 epochs with multiple accepted
  effects. Every accepted effect was independently reread from the gate database;
  epochs did not regress and counters matched unique durable receipts. The test
  enforces minimum acceptance/refusal/stale/multi-effect coverage.
- The concurrent-old-effects test now waits until the new grant and its first
  effect are committed before releasing the old worker. All 32 post-transfer old
  requests were refused. Independently read sequence: one old effect, then exactly
  33 new-epoch effects. This proves the late-request window occurred, without
  claiming a particular SQLite lock-contention schedule.
- Gate and maker writer-lock regressions returned `Refused(code=storage_busy)`;
  no effect was committed until the test explicitly retried after lock release.
- Wrong-role/version, corrupt store, missing-parent open and broken-schema reads
  produced typed storage refusals. Worker setup/unexpected exceptions were returned
  through their queues and child processes exited cleanly, avoiding timeout-only
  diagnostics.
- `python3 -m py_compile fencing_lab.py test_fencing.py`: passed.
- Source whitespace check: passed.

Reviewed-candidate source hashes (SHA-256; identify the candidate, not approval):

```text
0c3756fb192d7db47d81109d2c5ce56e72bc5e7f4cdf8611f67d1f37421f9721  fencing_lab.py
ee8a0fca1e08ea91f18b785f93d2f76cfae00bbadba519720b53a826ca27412f  test_fencing.py
```

The crash test kills an actual child after its gate effect transaction commits
while its separate maker transaction is uncommitted. A separate connection reads
the single effect before retry. The retry keeps exactly one effect. This covers
that explicit process-kill window, not physical power loss or every instruction.

The SQL failure test raises an abort on the resource update after the receipt
insert in the same transaction. The independent reader sees no receipt or effect;
retry succeeds once the injection is removed. It does not fill a physical disk.

The deliberate cloned-gate negative control also passes: it **reproduces unsafe
double acceptance** by two copied authorities. That validates the documented
limitation, not HA. The safety assertion depends on one current gate, and a real
deployment must independently enforce that prerequisite.

## Three-pass boundary

1. Scope/doctrine: control-services replicas, governor policy, makers and the
   external effect gate remain distinct. No current SHAPER law is changed.
2. Evidence/usability: separate files and processes are real; hosts, networks,
   Podman activation, DNS and physical storage fencing are absent. The README
   states the gate availability cost, authenticated-principal fixture and authority
   rollback/clone gap.
3. Independent counter-view: coordinator-provided Claude Opus high review found
   three important issues: the late-request test window was not forced, sparse
   stress acceptance weakened its owner invariant, and SQLite errors/store-role
   binding needed an explicit API boundary. All three were corrected with the
   regressions above. Claude Code Opus at medium effort then reported no remaining
   blocker or important finding for this documented experimental scope. Codex
   independently reran all 22 tests and the syntax and whitespace checks.

No commit, push, package publication, installed service mutation or deployment
was performed by this workstream. See STRESS-PLAN.md for unexecuted adapter and
three-host qualification gates.
