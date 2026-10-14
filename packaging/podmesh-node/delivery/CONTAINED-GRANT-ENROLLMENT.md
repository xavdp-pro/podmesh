# Appending a contained receiving grant

A receiving universe consumes a unique identity even when its attempted birth or
recovery fails. Keep its grant and journal history. To authorize another fresh
receiver, this separately pinned root administrator appends exactly one grant to
a stopped private NODE. It does not rebuild, replace or adopt the installed
bundle, expand provider verbs, edit private SQL, reset journal rows, create a
child or start services. Enrollment is root configuration administration;
functional recovery qualification remains separate.

Run `enroll-contained-grant.py --help` from the separately frozen tool source.
Supply the original installed `--bundle`, `--scope`, expected bundle-manifest,
policy and stopped-instance receipt checksums, the reviewed `--new-policy` and
its checksum, an existing `--template-uuid`, a dedicated protected
`--enrollment-root`, a new `--operation-id`, and the tool's own immutable source
pin/checksum. The expected bundle manifest authenticates `instance.py` before
import. That original administration module verifies all installed payloads and
executes its original `Instance.read` checks. Its native `operator.lock` excludes
concurrent supported administration.

The reviewed policy preserves every byte outside `allowed_contained_nested`
and every old descriptor's bytes. It appends one template-shaped `userns-v1`
grant: the same image, network, range length, mount destinations/access and
unchanged global limits. Only its new UUID, unused UID/GID ranges, dedicated
volume root and corresponding mount source paths differ. All old/new mount
inodes are observed and retained; the new configuration and saved-volume paths
must contain directories only, with no prior SQL/files or symlinks. Allocate
ranges across other independently managed host stores too; this tool observes
the selected instance's actual store and retains all existing grants.

Before native stop, collect a real root-observed private SQL/inventory absence
receipt for the new receiving UUID. Pass it as `--freshness-receipt` and its SHA.
It supplies `scope`, `host_machine_id`, `universe_uuid`, `policy_sha256`, integer
`actual_sql_entries: 0`, `inventory_absent: true`, `provider_record_absent: true`,
`db_container_id`, `db_started_at` and epoch-seconds `observed_at`. An `evidence`
object has exactly `sql`, `inventory` and `process`, each a `{path, sha256}`
reference to a preserved root0600 real observation. Absence booleans alone do not
satisfy enrollment. Every snapshot has `scope`, `host_machine_id`, `observed_at`:

- SQL: `universe_uuid`, `database: "podmesh-node"`, `user: "podmesh-node"`,
  `db_container_id`, `db_started_at`, `app_container_id`, integer `count: 0`, and
  the actual query `SELECT COUNT(*) FROM operations WHERE request LIKE '%UUID%';`.
- Inventory: `app_container_id`, `operation: "inventory"`, `ok: true` and the
  actual `containers` array. The tool checks that the UUID is absent from it.
- Process: `app_container_id`, `app_started_at`, positive `host_pid`, decimal
  string `starttime` and `executable_sha256` equal to the original bundle's
  `podmeshd` hash, observed through the actual running process.

All times must fall inside the intersection of the last app/DB process runs,
using their actual now-stopped `StartedAt`/`FinishedAt`; a different generation
or out-of-run timestamp refuses. There is no arbitrary wall-clock age threshold.
These root-attested snapshot hashes and explicit identities retain real query,
API and executable evidence; the tool does not query a stopped MariaDB or infer
SQL state from provider intent filenames. This is prerequisite evidence,
never a synthetic zero written by this tool. Keep the controller quiescent between
that observation and stop. Use the original native stop first: the receipt must
say `stopped`, every recorded service/target must be inactive or failed. Services
must explicitly report zero main/control PIDs; only the exact immutable native
target may omit these systemd service properties (present nonzero PIDs refuse),
and both owned app/DB containers must be stopped. The owned pod's networkless
infra process is not a serving app or database.

Each operation keeps root-protected versioned policy/instance preimages,
postimages, directory identities, provider-state hashes and a durable intent in
`INSTANCE_ROOT/enrollments/OPERATION_ID`. The actual commit changes only
`policy.json` and the truthful
`instance.json.configuration_sha256["policy.json"]`. It then proves the original
`Instance.read`, native stopped state and unchanged provider history. Only then
it records `ROOT_CONFIGURATION_ENROLLED`, with `runtime_qualified: false`.

A crash between the two file replacements can leave a configuration checksum
mismatch. Partial or previous incomplete enrollments are refused and kept for
explicit root inspection; no automatic rollback, adoption or replay runs.
Preserve the preimages and intent, keep native services stopped, and resolve the
observed file state explicitly before attempting another operation. The old
bundle's ordinary start command does not know this new transaction protocol:
the operator must check the completed enrollment record before invoking start.
After a completed enrollment, use the original native start and qualify actual
new-grant creation and full recovery on the target host. Enrollment alone is
neither a successful birth nor a restoration proof.

The standard-library guard tests are in `tests/test_enroll_contained_grant.py`.
Run them on the explicit build host, then separately verify native
stop/enroll/start and actual grant/recovery behavior on the authorized runtime.
