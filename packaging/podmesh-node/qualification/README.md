# Private node product proof scenario

Status: source driver prepared, execution pending. It does not qualify the node
from provider parser/effect fixtures. Runtime owns preparation, process control,
VM campaigns, provider policy, capture/restore and cleanup. This driver performs
only the requested application API operations and read-only private SQL oracles.
It never runs Podman, restarts services, signals processes, reboots, edits policy,
imports a database or removes infrastructure. Run it only on a declared disposable
candidate. No execution belongs on NOW7.

## Inputs and immutable manifest

Require exact CT-built `podmeshd`, provider and node OCI bytes from the selected
source pin, extracted-binary/archive hashes, base manifest/platform, root policy
hash and image lock. Prepare actual application UID/GID1102, its own fresh private
MariaDB pod/server/database/user/config and app/runtime volumes. DB admin access
stays with runtime. The driver uses the app-scoped password profile only; it reuses
`podmesh-manager/qualification/activation/dump-store.py`'s reviewed client parser.
Supply a unique, declared MariaDB server hostname and database name. SQL checks
`@@hostname,DATABASE()` against these inputs, including the restored endpoint;
ambient DSN and client defaults cannot select another server.

Run Python plus `mariadb` in the declared private pod network namespace as actual
UID1102, with the API socket, app state and profile accessible and a private0700
evidence directory. Runtime supplies its known tooling environment; do not install
Python into or modify the immutable application image. Capture the actual namespace,
UID, executable/image digests and effective profile hash around this driver.

Generate the plan before policy preparation, so nested grants can name both exact
UUIDs and the base image. Plan generation uses no host capability:

```sh
python3 -B packaging/podmesh-node/qualification/check-private-node.py plan \
  --image "$LOCAL_IMAGE_SHA256" --universe-profile "$UNIVERSE_PROFILE" \
  --mandate "$ENGINEERING_MANDATE" --output "$PLAN"
```

Each of flat and nested uses a separate fresh candidate/store/plan/evidence set.
Nested provider grants require source and clone UUID/base-image grants, exact empty
host mounts, private namespaces/network-none and explicitly declared Rule11 device
access. This test's `sleep` workload proves outer lifecycle only; inner Podman and
the intended nested service/disk/log recovery require a separate real workload.

Common invocation (runtime supplies the concrete paths; no implied defaults):

```sh
python3 -B packaging/podmesh-node/qualification/check-private-node.py run \
  --plan "$PLAN" --phase "$PHASE" --socket "$APP_API_SOCKET" \
  --profile "$APP_EFFECTIVE_PROFILE" --state-dir "$APP_STATE_DIR" \
  --evidence "$PRIVATE_EVIDENCE_DIR" --source-revision "$SOURCE_REVISION" \
  --expected-server-hostname "$PRIVATE_DB_HOSTNAME" \
  --expected-database "$PRIVATE_DB_NAME"
```

JSONL evidence is private0600, append/fsync within one phase, never overwritten.
A failure preserves it; no automatic teardown hides a failed leg. The same phase
requires a new declared evidence directory on retry. Output never exposes DSN,
password/profile contents or client error bodies. Each phase proves only itself.

## Lifecycle and before-effect ordering

`lifecycle` requires an empty node operation journal. It calls create, start,
resources, pause, resume, stop, clone and delete (clone target only), verifies fresh
API effects, then repeats identical IDs and compares persisted result/attempt rows.
Resources requires actual running cgroup verification; start/resume must observe
running, pause paused, stop not running, create/clone a container ID and delete
absence. A changed create command under the same ID must refuse without changing
its SQL evidence. Parent remains stopped for process/recovery phases. Unrelated
host state remains runtime's independently captured inventory oracle.

For causal ordering, runtime first prepares a clean running app/provider and records
that the plan UUIDs are absent. Runtime externally holds only its own provider
process (exact PID/artifact recorded), then launches `ordering-create` with the common
invocation. The application commits its pending operation/attempt, then blocks
on provider inspection. In another tooling process run `pending-probe`; this phase
uses SQL only because the application's single-thread API is busy. It requires
one committed `pending`/NULL-result row and one NULL-outcome/NULL-finished attempt.
Runtime independently inspects its host to prove the universe remains absent,
records the hold/release boundaries, releases its provider, and waits for successful
`ordering-create` terminal SQL and replay. Then `lifecycle --resume-ordering` permits
exactly that one original verified CREATE and exercises the remaining seven actions.
No driver flag performs the hold/release; PID control and cleanup are runtime-only.
This leg proves intent-before-effect at the common lifecycle entry. Other operations'
source path shares that journal ordering; do not report every transient pending row
as independently observed unless separate held-provider evidence was captured.

## Provider/app restart, real boot and nominal recovery

1. Runtime captures host/container/provider-record/image-lock inventory and the
   `lifecycle.jsonl` baseline, then restarts only its own provider and application.
   Keep app/DB volumes and provider state. Verify/remove only an owned stale socket
   endpoint when needed; neither daemon automatically unlinks an existing endpoint.
   Run `after-restart --baseline "$LIFECYCLE_JSONL"`. Same IDs must replay, original
   SQL results/attempts must equal saved baseline, and provider inventory must still
   carry the originally observed container ID.
2. Run `arm-boot` while parent remains the real fixture. Capture its host `boot_id`,
   `booted_at`, clock observation and the private SQL/records. Runtime enables the
   separately authorized private-app boot pass under the exact mandate and reboots
   its own declared guest. A provider/app restart alone does not satisfy this leg.
   Run `after-boot --boot-baseline "$ARM_BOOT_JSONL"` (an optional copied
   `--previous-boot-id` must match this saved observation). Host boot ID must change
   from the baseline tied to this exact plan,
   and the stable child start row must already be verified before this probe. Two
   explicit/replayed boot passes must add no child start attempt. Runtime also
   captures actual fresh running container/kernel state and startup logs. The
   driver deliberately does not start a missing boot child to rescue this oracle.
3. Capture the private node DB using the parameterized dump utility and hash the
   completed nonempty private artifact. Capture app/runtime volume inputs, provider
   policy/state, immutable OCI/base/workload images, graph-root/container identity
   evidence and the SQL baseline. A SQL dump alone does not preserve the host
   effects; list each required recovery component and its hash.
4. Runtime restores in its declared isolated recovery target, with a fresh explicitly
   named private DB endpoint/profile (sanitize inherited DSNs before product startup),
   recovered app/provider state and declared host identity/graph-root recovery.
   Never import onto the source database or overwrite source infrastructure. The
   product's host binding and provider's observed container IDs must remain valid;
   don't fabricate new ownership records or bypass these refusals. Run
   `after-restore --baseline "$LIFECYCLE_JSONL"` using the new endpoint's unique
   hostname input. Equal SQL rows/attempt IDs/results and original provider-bound
   container ID are required, followed by real application operation/boot evidence
   captured by runtime. If recovery cannot satisfy an identity boundary, record a
   failure rather than downgrading the oracle to read-only SQL similarity.
5. After preserving evidence, `cleanup` stops/deletes only the original parent via
   product API (clone was deleted in lifecycle). Runtime tears down only its declared
   app/private DB/pod/provider/volumes/fixture resources, proves source/unrelated
   inventory unchanged, and retains dumps/manifests/evidence as declared. No prune.

## SQL criteria and pending product gaps

For each requested ID: exactly one `operations` row bound to the same parsed JSON,
status `verified` with a persisted result; a terminal `operation_attempts` row with
matching outcome and non-NULL finished_at. Replay adds no attempt and changes no
persisted result. Causal probe has pending operation/attempt and no host container.
No `state.sqlite`, WAL or SHM may appear in accessible UID1102 app state. API host
UUID must equal SQL metadata host UUID; query server/database must equal declared
endpoint identity. Hash SQL dump before/after transfer and compare restored rows,
not just row counts. Runtime proves MariaDB engine/constraints/triggers and actual
scoped credential role/connection, plus application/provider/namespace identities.

No combined PASS until lifecycle, ordering, process restart, real boot, nominal
capture/restore, exact-byte provenance and owned cleanup all have real evidence.
Nested inner service, managed network, secrets, manager host-state mounts, scoped
statistics, migration/replication effects and imported lease/epoch boot gates remain
separate required gaps. No RAM continuity claim arises from this nominal workflow.
