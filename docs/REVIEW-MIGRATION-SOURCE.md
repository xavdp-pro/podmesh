# Source-side checkpoint milestone review

Status: development tree only, not packaged, not committed. Reviewer: Claude Code (Opus 5),
2026-09-11, taking over the review step that CURRENT-STATE.md assigned before destination work.
This is not an independent counter-review: the same model family wrote the milestone. A separate
reviewer (another model or Codex) should still read the diff before acceptance.

## Scope read

All Rust sources, both new test scripts, LOCAL-API.md, MIGRATION-INTEGRATION.md, the
checkpoint mission and implementation report, raw evidence of development runs 1 to 4, the
packaged runtime shim and wrapper sources, and the installed package versions on the three lab
hosts. The local tree was confirmed byte-identical to the lab build tree before any change.

## Confirmed sound

- Every identity is bound and validated before the journal records anything; refusals have no
  Podman effect and create neither reservation nor artifact directory.
- The reservation is persisted before any suspension and gates create, start, delete and clone
  (as target and source). A second migration operation on a reserved universe is refused.
- The runtime chain is fully pinned: the shim executes the wrapper by absolute path and the
  wrapper executes the binary by absolute path, and all three are hashed. Finalization also
  requires the dump log to name the qualified git ID.
- Running the checkpoint in its own transient scope is the right answer to the observed fact that
  killing the checkpoint process group destroys the application. The mid-dump service kill test
  proves the scope survives and the retry finalizes without capture.
- A failed checkpoint never restarts the source; a verified one is never recaptured; replay and
  status re-hash the preserved artifacts and detect tampering.
- No caller-controlled paths or shell: artifact paths derive from a validated operation ID, and
  the copied dump log must equal the container's own static directory under the default store.

## Defects corrected in this review

1. **The checkpoint targeted the universe name, not the bound container ID.** A container
   replaced out of band under the same name between the checks and the command would have been
   captured under a reservation naming another ID (finalization would then fail, but only after
   suspending the wrong process). The command now names the reserved container ID.
2. **The 300 s bound could destroy large sources.** GNU timeout signals the process group, which
   reproduces the destructive kill observed in the probe if a dump is still running. Memory was
   unbounded in preflight. Sources above 1 GiB `memory.current`, or with an unreadable value, are
   now refused before suspension (about 513 MiB was qualified).
3. **Space was checked only in the state directory.** `--keep` leaves uncompressed image files in
   the container storage, which may be another filesystem. Both locations are now checked.
4. **Scope completion used `is-active`.** A scope still activating or deactivating reads as not
   active, so a retry could race the finishing command; a failed query also counted as finished.
   Only `inactive` or `failed` (including an absent unit) now count as finished; a failed query
   refuses the retry.
5. **A crash between creating the artifact directory and inserting the reservation made the
   operation ID permanently unusable.** An empty directory, which is all such a crash leaves, is
   now reused and made private; a non-empty one, or a non-directory, is still refused.
6. **Documentation said stop "cannot destroy the source".** Stopping a reserved source whose
   checkpoint failed does end its process and memory state. LOCAL-API.md and the code comment
   now say so.

## Open findings, not corrected

- **Stuck reservations are permanent.** A reservation whose container is gone (for example the
  aborted development run left one in `checkpointing` with an archive but no manifest) blocks the
  universe UUID forever, with no API path out. Fail-closed is correct for now; the release and
  abandonment semantics are the design decision below.
- **Undeclared dependencies.** The code needs `podmesh-vzcriu`, `podmesh-vzcriu-helpers-node`,
  GNU `tar` with `zstd`, and `systemd-run`. The `podmesh` package declares only podman, systemd
  and coreutils. All are present on the three lab hosts; a clean host would fail preflight.
- **Pins live in source.** Any runtime or helper package update needs a code change and
  requalification. Acceptable while experimental; a later release could generate the pins from
  the qualified package set at build time.
- **Unpinned neighbours.** Podman, crun and the distribution libcriu used by crun are recorded but
  not pinned; an upgrade of any of them may change archive compatibility.
- **Retry paths still unexercised:** refusal while the scope is running, recapture of the same
  still-running process (including the deliberate `allow_frozen` re-assessment that lets a retry
  proceed over a cgroup left frozen by an interrupted dump), a real CRIU failure with its failure
  record, and resume from `reserved` or `checkpoint_failed`. The empty-directory path is exercised.
- **Not fencing.** A direct administrator `podman start` or an earlier package version bypasses
  the reservation. Unchanged, documented.

## Tests after correction

Rebuilt on the same development host (`cargo test` 2 unit tests, release build without warnings,
`cargo clippy --release` clean) and run against the same isolated development service, never the
installed one. All seven suites passed on the corrected binary: source checkpoint 35 checks (two
new: non-empty artifact directory refused, empty one reused and made private), interrupted
checkpoint 8, lifecycle 6, deletion ownership 13, start/stop 40, clone 25, interrupted clone 9.
Afterwards: no test containers, no checkpoint scopes, no Podman temporary leftovers; the
development service was stopped and the installed service left untouched. The memory bound, the
container-storage space check and the scope-state query are exercised only on their passing side;
their refusal sides are not tested. Still one host and not packaged.

## Independent counter-review

A read-only review by Claude Sonnet 5, a different model from the implementer, read the
sources, tests, documentation, raw evidence and the pinned runtime scripts, and rebuilt, tested
and linted the tree on a newer toolchain. Verdict: commit as is, no blocking defect. It confirmed
the six corrections above, the absence of any reservation bypass across retries and operation-ID
reuse, the recapture and finalization conditions, the path and shell safety, the runtime pinning
chain, and the evidence counts by parsing the JSON.

Its findings, verified against the code and addressed before commit:

1. **Should-fix, corrected.** Every service start empties the shared Podman scratch directory,
   which the checkpoint command received as `TMPDIR`, while the scope design exists precisely to
   let a restarted service coexist with a dump still running. The checkpoint now uses a
   temporary directory of its own operation under the migration state directory, never emptied.
2. **Should-fix, corrected.** The graph-root space check assumed the default path. The graph root
   is now read from Podman; any other graph root is a blocker, since the qualified scope is the
   default store, and the space check measures the real graph root.
3. **Note, corrected.** The archive's `0600` mode depended on inherited umask; it is now set
   explicitly before hashing.
4. **Note, documented.** Repeating a `migration_preflight` operation ID returns the historical
   report, like every operation; LOCAL-API.md now says a fresh assessment needs a new ID.
5. **Note, corrected.** An interruption-test diagnostic still matched the universe name after the
   checkpoint command switched to the container ID; it matches the ID again.
6. **Note, corrected.** The writable-layer size lookup used the name; it uses the observed ID.
7. **Note, recorded.** Recapture over a frozen cgroup is untested (see open findings).

The full report is kept with the local evidence. After these corrections the tree was rebuilt on
the same development host and all seven suites rerun against the same isolated service, with the
same counts: source checkpoint 35, interrupted checkpoint 8, lifecycle 6, deletion ownership 13,
start/stop 40, clone 25, interrupted clone 9. The interruption diagnostic now reports the running
checkpoint command, the preflight facts report the graph root read from Podman, each checkpoint
received its own private temporary directory, and no test container, scope or Podman temporary
file remained.

## Decision taken for destination work

Release and recovery semantics decide whether a checkpointed source may ever run again locally.
The sketch below was adopted and refined in [MIGRATION-PROTOCOL.md](MIGRATION-PROTOCOL.md), which
supersedes it wherever the two differ. Original sketch:

1. PodMesh records a transfer authorization as a journal row on the source (operation ID,
   destination host UUID, archive hash, issuer reference) before any artifact leaves the host.
   Restore on the destination requires presenting that record's identity and hash.
2. `migration_release` is allowed only when fresh observation shows the same reserved container,
   not running, and the journal holds no transfer authorization for the reservation. It releases
   the reservation only; it never starts the application.
3. Local recovery after release is an explicit, separate `migration_restore_local` from the
   preserved archive, keeping memory state; a fresh start remains an ordinary `start` with its
   loss of memory made explicit in the result.
4. Once an authorization exists, only the destination's verified outcome (restored, or
   refused with proof of non-restoration) can end the source reservation. An unreachable
   destination leaves it held.
5. `migration_abandon` for a reservation whose container is gone marks it abandoned with
   preserved artifacts, only when no authorization exists, and still refuses UUID reuse.
