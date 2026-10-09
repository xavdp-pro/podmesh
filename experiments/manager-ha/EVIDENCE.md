# Durable exchange core qualification

Date: 2026-09-13. Scope: Stage D in `experiments/manager-ha/`, with
compatibility checks against the unchanged manager network and resident
laboratories. Source baseline: `2be0437` on `codex/web-console`, plus the
uncommitted source fingerprints below. Runtime: local Linux x86_64 development
workspace `/tmp/podmesh-web`. Compiler: `rustc 1.98.0 (88d9e12ae 2026-08-18)`;
Cargo 1.98.0.

No dependency, Cargo manifest, lockfile, package, host, service, network, commit
or push change was made by this Stage D work.

## Qualified Stage D behavior

The local durable laboratory uses an exact schema-v3 SQLite shape with
append-only facts, typed receipts and append-only exchange-audit events. It
provides these bounded properties:

- wire operation, locally generated `attempt:<sha256>`, validated decoded wire
  nonce, destination-local receipt and remote receipt are distinct identities;
- before a peer nonce is decoded, a locally generated
  `preauth:<64 lowercase hexadecimal SHA-256>` nonce may identify only one local
  inbound observation and its diagnostic or close terminal; it stays stable and
  carries no peer, operation, receipt, replay or effect authority;
- the `network:` receipt namespace is reserved for authenticated imports;
- authenticated imports derive a bounded source-namespaced local receipt ID and
  atomically commit accepted facts, that receipt and the accepted import audit;
- an authenticated accepted request observation has exactly one durable
  decision: accepted import or authenticated refusal with a typed reason;
- accepted and refused decisions must precede reply preparation, and a completed
  reply write must retain the peer, receipt, outcome and refusal reason;
- unauthenticated, malformed and unavailable observations cannot be promoted to
  an accepted result or authenticated refusal;
- complete and partial unsigned diagnostic replies and zero-byte no-reply closes
  are distinct terminal cases; partial diagnostics carry only unavailable
  semantics, and none can authenticate a decision;
- one sequence validator applies before insert, after load, and while deriving
  ordered typed incomplete attempts;
- byte counts are per-phase actual transfers: decision and prepared phases
  transfer zero, while prepared intent retains bounded digests and announced
  request or reply sizes;
- complete request and reply transfers require exact framed-byte accounting;
- signed write failure uses a zero-byte close or a nonzero partial unavailable
  write; a full signed frame can only retain its accepted/refused outcome;
- unavailable transport, reply-write and close evidence uses
  `transport_unavailable`, while `unsafe_store` remains limited to actual unsafe
  canonical-store paths and neither is a signable peer-request refusal;
- malformed and unauthenticated diagnostic evidence accepts only the malformed
  category with `invalid_request`; every other closed reason is rejected on
  insertion and a validly checksummed stored violation is corruption;
- durable failures distinguish closed typed refusals, corruption, storage
  failure and invalid local audit evidence;
- legacy v1/v2, mismatched, symlinked, non-regular, corrupt and
  unexpected-schema stores fail closed on the tested paths;
- canonical inspection opens only stable private snapshots, including its
  private materialization fallback, and returns receipts, audits, incomplete
  attempts and unaudited laboratory import receipts; and
- every append checks that exactly one row was inserted, while the complete
  non-internal `sqlite_master` shape is verified using an exact `sqlite_`
  prefix exclusion.

Checksums provide corruption detection. They do not authenticate an
administrator who can replace both rows and checksums. The process-local
preflight cache includes file device, inode, size, modification time, change
time, replica and topology. The later read-write open uses SQLite `NOFOLLOW`
and rechecks path metadata. These checks reduce path-replacement exposure but do
not claim protection from a privileged process racing arbitrary filesystem
operations. The Linux `O_NOFOLLOW|O_NONBLOCK` ABI use is compile-time restricted
to the currently qualified Linux x86_64 target.

## Executed verification

| Command | Result |
| --- | --- |
| `cargo test --locked --manifest-path experiments/manager-ha/Cargo.toml` | Exit 0; 10 model tests and 69 process tests passed; one crash child helper ignored by the harness and invoked by passing parent tests |
| `cargo test --locked --manifest-path experiments/manager-network/Cargo.toml` | Exit 0; 10 library tests and one three-process test passed |
| `cargo test --locked --manifest-path experiments/manager-resident/Cargo.toml` | Exit 0; 13 process tests passed, including three-resident partition/reconnect/restart compatibility |
| `cargo clippy --locked --all-targets --manifest-path experiments/manager-ha/Cargo.toml -- -D warnings` | Exit 0; no warning |
| `cargo clippy --locked --all-targets --manifest-path experiments/manager-network/Cargo.toml -- -D warnings` | Exit 0; no warning |
| `cargo clippy --locked --all-targets --manifest-path experiments/manager-resident/Cargo.toml -- -D warnings` | Exit 0; no warning |
| `cargo fmt --all --manifest-path experiments/manager-ha/Cargo.toml -- --check` | Exit 0 |
| `cargo fmt --all --manifest-path experiments/manager-network/Cargo.toml -- --check` | Exit 0 |
| `cargo fmt --all --manifest-path experiments/manager-resident/Cargo.toml -- --check` | Exit 0 |
| `git diff --check -- experiments/manager-ha/src/durable.rs experiments/manager-ha/src/main.rs experiments/manager-ha/tests/process.rs experiments/manager-ha/README.md experiments/manager-ha/EVIDENCE.md` | Exit 0 |
| `git diff --no-index --check /dev/null docs/MANAGER-G2-DURABLE-EXCHANGE.md` | No whitespace diagnostics; exit 1 denotes the expected new-file diff |

The 103 executed tests across the three crates are compatibility evidence for the
local Stage D substrate. They are not one end-to-end G2 qualification. The
manager-network and manager-resident crates were not modified by this work.
One cross-crate parallel orchestration exposed two transient manager-network
listener-start failures; the unchanged network suite then passed both with one
test thread and in a standalone default run. This evidence does not claim that
the laboratory's dynamic-port allocation is collision-free under concurrent
test-suite orchestration.

Process tests use the compiled JSON executable, separate processes, temporary
mode-0700 directories and independent SQLite connections. They verify exit
status and persisted evidence rather than inferring completion from a health
endpoint. Corruption fixtures restore the exact expected immutability trigger
before reopening the store, so exact typed errors reach checksum, metadata,
peer, sequence and receipt-link validation paths.

Specific regressions cover:

- complete accepted and authenticated-refusal inbound chains;
- refusal outcome and reason mismatch before reply write;
- request and reply transfer evidence that does not match the corresponding
  prepared digest or announced size;
- unauthenticated promotion refused on insert and classified as `Corrupt` when
  injected into stored rows with a valid checksum;
- refusal recorded before restart reported as a typed incomplete attempt;
- a matching unauthenticated no-reply close that is terminal without claiming
  an authenticated decision or transferred reply bytes;
- complete and partial unsigned diagnostic replies after unauthenticated and
  authenticated unavailable observations, with exact framed bytes and terminal
  inspection;
- partial diagnostics rejected unless their outcome and category are unavailable,
  full unavailable diagnostics rejected when they do not match their observation,
  and stored partial semantic violations classified as corruption;
- authenticated diagnostics retaining their observed authenticated peer for
  partial and complete writes, with a validly checksummed stored authenticated
  peer change classified as corruption;
- unauthenticated partial diagnostics retaining their observed peer claim on
  insertion without authenticating it;
- successful partial diagnostic boundaries at one and three prefix bytes, a
  complete four-byte prefix, and one byte short of the full frame;
- zero-byte diagnostics and diagnostic phases that repeat request-transfer
  evidence rejected with their exact error classes, including missing reply
  announced size or digest and request announced-size-only or digest-only reuse;
- valid pre-authentication observations ending in complete diagnostics or
  zero-byte closes without authority, plus exact refusal of malformed pre-auth
  nonce shape, authority fields, invalid phases and nonce replacement within one
  local attempt;
- `transport_unavailable` display and serialization, its use on unavailable
  audit evidence, and exact refusal when it or `unsafe_store` is presented as an
  authenticated peer-request refusal;
- exact insertion rejection for malformed and unauthenticated diagnostic rows
  carrying any reason other than `invalid_request`, including policy, operation,
  schema, identity, missing, unsafe-store and transport reasons;
- exact insertion rejection for both diagnostic outcomes when a valid
  `invalid_request` reason is paired with unavailable or refused categories;
- a validly checksummed stored malformed diagnostic changed to
  `identity_mismatch`, classified as `Corrupt` by the same load validator;
- a validly checksummed stored unauthenticated diagnostic changed to the refused
  category, also classified as `Corrupt` by the same load validator;
- diagnostic replies rejected after signed preparation or alongside a no-reply
  close, including validly checksummed stored-corruption variants;
- every meaningful inbound decision, preparation, write and close sequence
  rejection branch, including pair, receipt, metadata and terminal conflicts;
- prepared-zero, partial-outbound, exact inbound and exact reply-write byte
  accounting, including announced reply body size;
- accepted inbound observations without a complete request frame and
  non-accepted outbound completions that assert remote receipts;
- malformed import audit input taking precedence over independently refused
  snapshot content, with no fact, receipt or decision mutation;
- local configuration failures classified outside signable request refusals;
- request-time policy failure as `Refused`, stored fact/model/receipt/audit
  corruption as `Corrupt`, and unreadable SQLite content as `Storage`;
- modified receipt responses and requests with exact corruption errors;
- modified audit typed fields, recomputed audit hashes, invalid authenticated
  peers and validly checksummed links to missing receipts;
- duplicate phases, invalid attempt IDs and invalid prepared outcomes;
- two local attempts under one reused peer nonce;
- authenticated-import retry with one audit ID and with a fresh attempt, while
  ordinary audit replay cannot normalize a changed replay flag;
- exact separation of wire operation, local receipt and remote receipt IDs;
- refusal of authenticated-import namespace squatting by observations and
  laboratory imports;
- unaudited laboratory imports reported by canonical inspection;
- an audit collision rolling back facts, receipt and audit together;
- deterministic remote receipt identity validation;
- a killed-child live WAL-mode v2 store whose database, WAL and SHM bytes remain
  identical after refusal;
- a killed-child live WAL-mode v3 store whose database, WAL and SHM bytes remain
  identical after mismatched read-only inspection and read-write-open refusal;
- main-path and sidecar symlink refusal, early `lstat` FIFO sidecar refusal without blocking,
  and non-UTF-8 database paths with WAL state;
- an in-place same-inode identity replacement carrying killed-child, nonempty live
  WAL state whose changed metadata cannot reuse a cached preflight or survive an
  unnoticed read-write open;
- external inspection that does not change canonical store bytes;
- a `sqlitex_hidden` trigger proving exact internal-prefix exclusion, plus
  zero-row insert prevention; and
- a manager- and topology-bound logical history digest, including empty stores.

## Review status

Claude Code's independent revision-10 review dated 2026-09-13 found no blocker or
important defect. It confirmed the transport-unavailability separation,
pre-authentication nonce closure, fixed diagnostic reason sets, stored-load
validation, fingerprints and bounded claims. Its only new low finding was missing
direct coverage for a valid `invalid_request` reason paired with an unavailable
or refused category. This increment closes that coverage gap on insertion and
with a validly checksummed stored-corruption regression.

Revision-9's dynamic-port race remains openly tracked as an accepted Stage N/Q
test-harness limitation; Stage D does not claim collision-free concurrent suite
orchestration. Its diagnostic-specific missing-size condition is accepted as
cosmetic redundancy: shared transfer validation rejects that malformed shape
first, while the local guard keeps the diagnostic invariant readable without
changing accepted or rejected behavior. Claude Code's narrow revision-11
confirmation found no blocker or important defect and verified the new category
tests, stored-corruption regression, fingerprints, counts and bounded claims.
Its plan-mode session independently ran format and diff checks; the locked tests
and strict Clippy results recorded above were produced by the implementation and
Codex verification runs. Codex self-review and the commands recorded above passed.

## Strongest supported claim

On local Linux x86_64, Stage D provides a tested schema-v3 durable exchange
substrate: receipt-bearing idempotent local mutations, atomic authenticated
import with append-only audit evidence, a durable authenticated-refusal decision,
checked accepted/refused/signed-reply/unsigned-diagnostic/no-reply inbound chains, deterministic
source-namespaced receipt identity, per-phase framed-byte evidence, typed failure
classes, typed incomplete-attempt inspection and a supported external read-only
inspector. Existing manager network and resident laboratory suites remain
compatible.

This evidence does not claim that Stage N uses the new authenticated-import and
audit APIs, that Stage R exposes installed observation append/inspection, or
that G2 is complete. It does not prove package qualification, three installed
hosts, transport confidentiality, dynamic membership, takeover, fencing,
split-brain prevention, DNS, Podman effects, physical power-loss survival or
manager high availability.

## Source fingerprints

| Path relative to repository root | SHA-256 |
| --- | --- |
| `experiments/manager-ha/Cargo.toml` | `6dd587e350de4ba196983c053502e5436e0160460b3d7e6011de7d8480bf05d3` |
| `experiments/manager-ha/Cargo.lock` | `68b37364176d8c038ce48913980e1d53c5d8377c43b8ed97381334ba55d6ee4e` |
| `experiments/manager-ha/src/lib.rs` | `45be5c39be8c95494b15cff946ced84a4c1dd3d20e4fc713262f9aa4c0802103` |
| `experiments/manager-ha/src/durable.rs` | `2e802cd705df20536766591046f1531b57524fba6ad60ed0ad64f820f824b373` |
| `experiments/manager-ha/src/main.rs` | `2e802c70ae41dd7d714784c0d37d8d49fd33b7a33e528872c67f3db06075f9b0` |
| `experiments/manager-ha/tests/laboratory.rs` | `11f974698e7ca18ba12531896fd36ccf7e521cc60380f9f6f3ef2082a97ed198` |
| `experiments/manager-ha/tests/process.rs` | `a043efec76fb0a4f039caf069acf564fcc6c54597a1981b6591be288ae228969` |
| `experiments/manager-ha/README.md` | `8ff9bf5af92ed66cb78c4ed9d7042a7980bf1d0ce38e8bb44d5ecba230c34a29` |
| `docs/MANAGER-G2-DURABLE-EXCHANGE.md` | `16887141688f2e987698fb3714e495ad9f27193e6c668210650148c2f90a6a90` |
