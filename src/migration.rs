//! Experimental source-side migration preparation: preflight, reservation and checkpoint.
//!
//! Scope: the default rootful Podman store; network-disabled, mount-free containers owned by this
//! host's journal whose processes are musl-based; checkpoint with the separately packaged
//! podmesh-vzcriu runtime through its private-path shim, never the distribution CRIU.
//! Transfer authorization and completion live in `transfer.rs`, destination restore in `restore.rs`,
//! and the recovery paths of a reservation that never left this host — release, abandonment and local
//! restore — in `recovery.rs` (docs/MIGRATION-PROTOCOL.md).
//! A checkpoint result never authorizes a restore: only a transfer authorization issues a handoff.
use crate::cleanup::{Bound, Watch};
use crate::lifecycle::{self as lc, failure, Error};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

pub(crate) const RUNTIME_REAL: &str = "/usr/lib/podmesh-vzcriu/criu";
const RUNTIME_WRAPPER: &str = "/usr/bin/podmesh-vzcriu";
const RUNTIME_SHIM: &str = "/opt/podmesh-vzcriu-kit/bin/criu";
// Qualified bytes of the runtime and of the packaged scripts that select it (podmesh-vzcriu
// 3.15.5.3+podmesh1~experimental1, podmesh-vzcriu-helpers-node 1.0.0+podmesh1~experimental1).
const RUNTIME_REAL_SHA256: &str = "4ecb663e7e3b019cdfa534c0d4cce4134938f87ae3b45cac35a7481c0a947cd4";
const RUNTIME_WRAPPER_SHA256: &str = "97d0f0728c54dd340b6ec6d002e1946b3a41fdd8ceb3ded5d4d5cccfda44b28e";
const RUNTIME_SHIM_SHA256: &str = "fcbec55d0401080d020b1a56299d007792ca8215f68c533756080ff5e86948eb";
pub(crate) const RUNTIME_GIT_ID: &str = "v3.15.5.3";
// Podman and crun look up a binary named `criu` on PATH; the private shim directory comes first.
const RUNTIME_PATH: &str = "/opt/podmesh-vzcriu-kit/bin:/usr/sbin:/usr/bin:/sbin:/bin";
const CHECKPOINT_SECONDS: u64 = 300;
pub(crate) const ARCHIVE: &str = "checkpoint.tar.zst";
pub(crate) const MANIFEST: &str = "manifest.json";
const MAX_PROCESSES: usize = 64;
// About 513 MiB was dumped in under a second on the lab. A dump still running when the 300 s bound
// expires is killed, which was observed to destroy the application: refuse larger sources before suspension.
const MAX_MEMORY_BYTES: u64 = 1024 * 1024 * 1024;
pub(crate) const SPACE_MARGIN_BYTES: u64 = 64 * 1024 * 1024;
// Podman keeps the uncompressed checkpoint image files (--keep) under the container storage.
pub(crate) const CONTAINER_STORAGE: &str = "/var/lib/containers/storage";
const SCOPE: &str = "experimental source-side checkpoint: default rootful Podman store, network-disabled, mount-free, journal-owned container with musl processes, packaged podmesh-vzcriu 3.15.5.3 through its private-path shim";
const SCOPE_NESTED_PREFLIGHT: &str = "experimental nested-lab preflight only: Rule 11 outer universe — privileged rootful Podman container, network-disabled, mount-free, journal-owned; inner Podman reconciliation and the checkpoint/restore chain are not implemented in this backend";
const SCOPE_NESTED_LAB_CHECKPOINT: &str = "experimental nested-lab checkpoint: same outer-shape assess as migration_profile nested preflight; durable reservation, preflight.json, inner Podman metadata reconciliation for the kit counter fixture when present, nested VFS store binding assessment and binding artifact, Rule 11 destination restore chain assessment and artifact, outer checkpoint capture with the packaged podmesh-vzcriu runtime in its own scope when preflight passes, archive/manifest/hashes under the state directory with nested sidecars recorded; the two-host carry/restore/complete chain is not implemented (docs/MIGRATION-INTEGRATION.md)";
const SCOPE_NESTED_OUTER_CHECKPOINT: &str = "experimental nested-lab outer checkpoint capture: privileged Rule 11 outer universe suspended with the packaged podmesh-vzcriu runtime; inner Podman metadata, VFS binding and destination-restore-chain artifacts are carried in the manifest; destination nested restore execution and the two-host move chain are not implemented";
const NESTED_LAB_CHECKPOINT_GAPS: [&str; 4] = [
    "inner_podman_metadata_reconciliation",
    "nested_vfs_store_binding",
    "rule11_destination_restore_chain",
    "nested_outer_checkpoint_capture",
];
const NESTED_LAB_GAPS_AFTER_INNER_RECONCILIATION: [&str; 3] = [
    "nested_vfs_store_binding",
    "rule11_destination_restore_chain",
    "nested_outer_checkpoint_capture",
];
const NESTED_LAB_GAPS_AFTER_VFS_STORE_BINDING: [&str; 2] = [
    "rule11_destination_restore_chain",
    "nested_outer_checkpoint_capture",
];
const NESTED_LAB_GAPS_AFTER_DESTINATION_RESTORE_CHAIN: [&str; 0] = [];
/// Gaps remaining immediately after outer capture, before destination restore hooks existed (tests only).
#[allow(dead_code)]
const NESTED_LAB_GAPS_AFTER_OUTER_CHECKPOINT_CAPTURE: [&str; 2] = [
    "nested_destination_restore_hooks",
    "two_host_checkpoint_carry_restore_complete",
];
pub(crate) const NESTED_LAB_GAPS_AFTER_DESTINATION_RESTORE_HOOKS: [&str; 0] = [];
/// Remaining product gap after outer capture and destination restore hooks until a verified two-host move closes G3.
pub(crate) const NESTED_LAB_GAPS_UNTIL_TWO_HOST_MOVE_COMPLETE: [&str; 1] = [
    "two_host_checkpoint_carry_restore_complete",
];
/// Reservation state after the kit-style inner Podman proof and pre-checkpoint metadata reconcile succeed.
pub(crate) const NESTED_INNER_RECONCILED: &str = "nested_inner_reconciled";
/// Reservation state after host outer / inner VFS store binding is assessed and recorded.
pub(crate) const NESTED_VFS_STORE_BOUND: &str = "nested_vfs_store_bound";
/// Reservation state after the Rule 11 destination restore chain is assessed on the source host.
pub(crate) const NESTED_DESTINATION_RESTORE_CHAIN_ASSESSED: &str = "nested_destination_restore_chain_assessed";
const INNER_PODMAN_METADATA: &str = "inner_podman_metadata.json";
const NESTED_VFS_STORE_BINDING: &str = "nested_vfs_store_binding.json";
const RULE11_DESTINATION_RESTORE_CHAIN: &str = "rule11_destination_restore_chain.json";
const NESTED_OUTER_CHECKPOINT_PLAN: &str = "nested_outer_checkpoint_plan.json";
pub(crate) const NESTED_MANIFEST_FORMAT: &str = "podmesh-nested-source-checkpoint/1";
pub(crate) const NESTED_CHECKPOINT_SIDECAR_FILES: [&str; 4] = [
    INNER_PODMAN_METADATA,
    NESTED_VFS_STORE_BINDING,
    RULE11_DESTINATION_RESTORE_CHAIN,
    NESTED_OUTER_CHECKPOINT_PLAN,
];
/// Inner Podman graph root inside the outer universe (default rootful store layout).
const INNER_PODMAN_GRAPH_ROOT: &str = "/var/lib/containers/storage";
/// Inner Podman inside the outer universe uses the kit's isolated VFS store (contrib/nested-podman).
const INNER_PODMAN_CLI: &str =
    "podman --storage-driver=vfs --cgroup-manager=cgroupfs --events-backend=file";
const NESTED_COUNTER_INNER_NAME: &str = "counter";
/// Kit reconcile script (node.py), run inside the outer container before outer checkpoint when the counter fixture is present.
const NESTED_INNER_RECONCILE_PY: &str = r"import json, os, shutil, sys, tempfile
from pathlib import Path
expected=json.loads(sys.argv[1]); ident=expected['inner_id']; pid=expected['pid']
p=Path('/run/crun')/ident/'status'; d=json.loads(p.read_text())
assert d['pid']==pid, 'PID changed unexpectedly'
cmd=Path('/proc/%d/cmdline'%pid).read_bytes()
assert b'token=$(cat /proc/sys/kernel/random/uuid)' in cmd, 'Unexpected process'
log=Path('/var/lib/containers/storage/vfs-containers')/ident/'userdata/ctr.log'
workload_uuid=expected.get('uuid', expected.get('workload_uuid'))
assert workload_uuid and workload_uuid in log.read_text(), 'Workload identity missing'
start=int(Path('/proc/%d/stat'%pid).read_text().rsplit(')',1)[1].split()[19])
backup=Path(tempfile.mkdtemp(prefix='podmesh-nested-reconcile-', dir='/tmp'))
shutil.copy2(p,backup/'crun-status.json')
alive=Path('/run/libpod/alive'); shutil.copy2(alive,backup/'alive')
d['process-start-time']=start
q=p.with_suffix('.migration-tmp'); q.write_text(json.dumps(d)); os.replace(q,p)
alive.write_bytes(Path('/proc/sys/kernel/random/boot_id').read_bytes())
print(json.dumps({'pid':pid,'start_time':start,'backup':str(backup)}))
";
/// Refreshes inner Podman boot-ID cache after outer CRIU restore so `podman start` accepts the namespace boot ID.
const NESTED_INNER_RUN_STATE_RECONCILE_PY: &str = r"import json
from pathlib import Path
boot=Path('/proc/sys/kernel/random/boot_id').read_bytes()
alive=Path('/run/libpod/alive')
alive.parent.mkdir(parents=True, exist_ok=True)
prior=alive.read_bytes() if alive.exists() else b''
alive.write_bytes(boot)
print(json.dumps({'boot_id_bytes': len(boot), 'alive_refreshed': prior != boot}))
";
const AUTHORITY: &str = "This checkpoint does not authorize restore on any host and does not release the reservation. Only migration_authorize_transfer issues a handoff, and only a verified destination outcome bound to it ends the reservation.";
/// Which migration assess rules apply. Default flat scope refuses privileged containers; nested is preflight-only until a separate backend exists (docs/MIGRATION-INTEGRATION.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MigrationProfile {
    Flat,
    Nested,
}
impl MigrationProfile {
    pub(crate) fn parse(raw: &str) -> Result<Self, Error> {
        match raw {
            "flat" => Ok(Self::Flat),
            "nested" => Ok(Self::Nested),
            other => Err(format!(
                "migration_profile must be flat or nested, not {other}"
            )
            .into()),
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Nested => "nested",
        }
    }
}
/// Reservation states this file, `recovery.rs` and `collector.rs` share.
pub(crate) const RELEASED: &str = "released";
pub(crate) const ABANDONED: &str = "abandoned";
pub(crate) const RESTORED_LOCALLY: &str = "restored_locally";
/// Terminal state of a reservation the garbage collector swept on proof (docs/GARBAGE-COLLECTION.md). Like
/// `released` it blocks no generic operation; unlike it, a tombstone keeps refusing a blind `create` of the
/// same universe UUID for good.
pub(crate) const COLLECTED: &str = "collected";

/// Identity binding carried by every migration request.
pub(crate) struct Binding<'a> {
    pub container_id: &'a str,
    pub image: &'a str,
    pub source_host: &'a str,
    pub destination: &'a str,
    pub profile: MigrationProfile,
}

static MIGRATIONS: OnceLock<PathBuf> = OnceLock::new();
pub(crate) fn prepare(dir: &Path) -> Result<(), Error> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    MIGRATIONS.get_or_init(|| dir.to_path_buf());
    Ok(())
}
pub(crate) fn base() -> Result<&'static PathBuf, Error> {
    MIGRATIONS
        .get()
        .ok_or_else(|| Error::from("Migration state directory not prepared"))
}
pub(crate) fn ensure_schema(db: &Connection) -> Result<(), Error> {
    // Separate tables: earlier package versions keep working on this journal after a rollback,
    // but they do not enforce reservations, authorizations or restore claims.
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS migration_reservations(universe_uuid TEXT PRIMARY KEY, operation_id TEXT NOT NULL,
         container_id TEXT NOT NULL, image_id TEXT NOT NULL, source_host_uuid TEXT NOT NULL, destination_host_uuid TEXT NOT NULL,
         container_started_at TEXT NOT NULL, state TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, detail TEXT);
         CREATE TABLE IF NOT EXISTS migration_authorizations(authorization_id TEXT PRIMARY KEY, operation_id TEXT NOT NULL UNIQUE,
         universe_uuid TEXT NOT NULL, checkpoint_operation_id TEXT NOT NULL, destination_host_uuid TEXT NOT NULL, handoff TEXT NOT NULL,
         handoff_sha256 TEXT NOT NULL, state TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, outcome TEXT,
         outcome_sha256 TEXT, completed_by_operation TEXT);
         CREATE TABLE IF NOT EXISTS migration_restore_claims(authorization_id TEXT PRIMARY KEY, operation_id TEXT NOT NULL,
         universe_uuid TEXT NOT NULL, handoff TEXT NOT NULL, handoff_sha256 TEXT NOT NULL, source_host_uuid TEXT NOT NULL,
         source_container_id TEXT NOT NULL, image_id TEXT NOT NULL, state TEXT NOT NULL, container_id TEXT, created_at INTEGER NOT NULL,
         updated_at INTEGER NOT NULL, outcome TEXT, outcome_sha256 TEXT, detail TEXT);
         CREATE TABLE IF NOT EXISTS migration_reservation_history(id INTEGER PRIMARY KEY, universe_uuid TEXT NOT NULL, operation_id TEXT NOT NULL,
         container_id TEXT NOT NULL, image_id TEXT NOT NULL, source_host_uuid TEXT NOT NULL, destination_host_uuid TEXT NOT NULL,
         container_started_at TEXT NOT NULL, state TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, detail TEXT,
         archived_at INTEGER NOT NULL, archived_by_operation TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS migration_universe_tombstones(universe_uuid TEXT PRIMARY KEY, class TEXT NOT NULL,
         class_number INTEGER NOT NULL, container_id TEXT NOT NULL, container_absent_at_collection INTEGER NOT NULL,
         checkpoint_operation_id TEXT NOT NULL, collected_by_operation TEXT NOT NULL, collected_at INTEGER NOT NULL, proof TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS migration_collection_history(id INTEGER PRIMARY KEY, universe_uuid TEXT NOT NULL,
         class TEXT NOT NULL, class_number INTEGER NOT NULL, container_id TEXT NOT NULL,
         container_absent_at_collection INTEGER NOT NULL, checkpoint_operation_id TEXT NOT NULL,
         collected_by_operation TEXT NOT NULL, collected_at INTEGER NOT NULL, proof TEXT NOT NULL);",
    )?;
    Ok(())
}
/// Every collection this universe has ever been through, oldest first. The tombstone is the permanent
/// identity protection and keeps the first proof; this is the occurrence history, and a universe that comes
/// back through a verified restore and is collected again adds a row rather than replacing anything.
pub(crate) fn collection_history(db: &Connection, uuid: &str) -> Result<Vec<Value>, Error> {
    let mut stmt = db.prepare(
        "SELECT class,class_number,container_id,container_absent_at_collection,checkpoint_operation_id,collected_by_operation,
         collected_at FROM migration_collection_history WHERE universe_uuid=?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([uuid], |r| {
        Ok(json!({"class": r.get::<_, String>(0)?, "class_number": r.get::<_, i64>(1)?,
            "container_id": r.get::<_, String>(2)?, "container_absent_at_collection": r.get::<_, i64>(3)? != 0,
            "checkpoint_operation_id": r.get::<_, String>(4)?, "collected_by_operation": r.get::<_, String>(5)?,
            "collected_at": r.get::<_, i64>(6)?}))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
/// Records one collection: an append-only occurrence row, and the tombstone itself the first time only, so
/// that a second collection of the same identity never overwrites the proof the first one rested on. The
/// caller holds the transaction.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_collection(
    db: &Connection,
    uuid: &str,
    class: &str,
    class_number: i64,
    container_id: &str,
    absent: bool,
    checkpoint_operation: &str,
    operation: &str,
    at: i64,
    proof: &Value,
) -> Result<bool, Error> {
    let values = params![
        uuid,
        class,
        class_number,
        container_id,
        absent as i64,
        checkpoint_operation,
        operation,
        at,
        proof.to_string()
    ];
    db.execute(
        "INSERT INTO migration_collection_history(universe_uuid,class,class_number,container_id,container_absent_at_collection,
         checkpoint_operation_id,collected_by_operation,collected_at,proof) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        values,
    )?;
    // OR IGNORE, never OR REPLACE: the first tombstone's proof is history and is not overwritten.
    let first = db.execute(
        "INSERT OR IGNORE INTO migration_universe_tombstones VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        values,
    )?;
    Ok(first > 0)
}
/// The tombstone a garbage collection left for a universe, if any. It is never removed: it is the history
/// that keeps a collected identity from silently coming back (docs/GARBAGE-COLLECTION.md, "History retention").
pub(crate) fn tombstone(db: &Connection, uuid: &str) -> Result<Option<Value>, Error> {
    ensure_schema(db)?;
    Ok(db
        .query_row(
            "SELECT class,class_number,container_id,container_absent_at_collection,checkpoint_operation_id,collected_by_operation,
             collected_at,proof FROM migration_universe_tombstones WHERE universe_uuid=?1",
            [uuid],
            |r| {
                Ok(
                    json!({"universe_uuid": uuid, "class": r.get::<_, String>(0)?, "class_number": r.get::<_, i64>(1)?,
                    "container_id": r.get::<_, String>(2)?, "container_absent_at_collection": r.get::<_, i64>(3)? != 0,
                    "checkpoint_operation_id": r.get::<_, String>(4)?, "collected_by_operation": r.get::<_, String>(5)?,
                    "collected_at": r.get::<_, i64>(6)?,
                    "proof": serde_json::from_str::<Value>(&r.get::<_, String>(7)?).unwrap_or(Value::Null)}),
                )
            },
        )
        .optional()?
        .map(|mut t| {
            // The tombstone is the first collection; the occurrences are every collection of that identity.
            t["occurrences"] = json!(collection_history(db, uuid).unwrap_or_default());
            t
        }))
}
/// A collected universe UUID is never created again blindly: only a verified handoff restore, or an explicit
/// replacement procedure, may give that identity a meaning on this host after a collection.
pub(crate) fn refuse_identity_reuse(db: &Connection, uuid: &str, operation: &str) -> Result<(), Error> {
    ensure_schema(db)?;
    if let Some(t) = tombstone(db, uuid)? {
        return Err(failure(
            format!(
                "Universe {uuid} was collected on this host by garbage collection operation {} (class {}); {operation} with this universe UUID is refused, because reusing a collected identity requires a verified handoff restore or an explicit replacement procedure",
                t["collected_by_operation"].as_str().unwrap_or(""),
                t["class"].as_str().unwrap_or("")
            ),
            json!({"tombstone": {"universe_uuid": uuid, "class": t["class"], "class_number": t["class_number"],
                "collected_by_operation": t["collected_by_operation"], "collected_at": t["collected_at"]}}),
        ));
    }
    Ok(())
}
/// Whether any collection of this universe recorded this exact container as absent. Such a container can
/// only have come back out of band, so its original creation never owns it again — and this holds for every
/// container ever proved absent for that identity, not only for the one the tombstone kept.
pub(crate) fn collected_absent(db: &Connection, uuid: &str, container_id: &str) -> Result<bool, Error> {
    ensure_schema(db)?;
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM migration_universe_tombstones WHERE universe_uuid=?1 AND container_id=?2
         AND container_absent_at_collection=1)
         OR EXISTS(SELECT 1 FROM migration_collection_history WHERE universe_uuid=?1 AND container_id=?2
         AND container_absent_at_collection=1)",
        params![uuid, container_id],
        |r| r.get::<_, bool>(0),
    )?)
}

pub(crate) struct Reservation {
    pub operation_id: String,
    pub container_id: String,
    pub image_id: String,
    pub source_host: String,
    pub destination: String,
    pub started_at: String,
    pub state: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub detail: Option<String>,
}
impl Reservation {
    pub(crate) fn view(&self) -> Value {
        json!({"operation_id": self.operation_id, "container_id": self.container_id, "image_id": self.image_id,
            "source_host_uuid": self.source_host, "destination_host_uuid": self.destination,
            "container_started_at": self.started_at, "state": self.state, "created_at": self.created_at,
            "updated_at": self.updated_at, "detail": self.detail.as_deref().and_then(|d| serde_json::from_str::<Value>(d).ok())})
    }
    /// The recorded detail as a JSON object (empty when absent or not an object).
    pub(crate) fn detail_value(&self) -> Value {
        self.detail
            .as_deref()
            .and_then(|d| serde_json::from_str::<Value>(d).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}))
    }
}
pub(crate) fn reservation(db: &Connection, uuid: &str) -> Result<Option<Reservation>, Error> {
    Ok(db
        .query_row(
            "SELECT operation_id,container_id,image_id,source_host_uuid,destination_host_uuid,container_started_at,state,created_at,updated_at,detail
             FROM migration_reservations WHERE universe_uuid=?1",
            [uuid],
            |r| {
                Ok(Reservation {
                    operation_id: r.get(0)?,
                    container_id: r.get(1)?,
                    image_id: r.get(2)?,
                    source_host: r.get(3)?,
                    destination: r.get(4)?,
                    started_at: r.get(5)?,
                    state: r.get(6)?,
                    created_at: r.get(7)?,
                    updated_at: r.get(8)?,
                    detail: r.get(9)?,
                })
            },
        )
        .optional()?)
}
/// Every reservation this host holds, oldest first. The garbage collector is the only host-wide reader:
/// every other operation names the universe it acts on.
pub(crate) fn reservations(db: &Connection) -> Result<Vec<(String, Reservation)>, Error> {
    let mut stmt = db.prepare(
        "SELECT universe_uuid,operation_id,container_id,image_id,source_host_uuid,destination_host_uuid,container_started_at,
         state,created_at,updated_at,detail FROM migration_reservations ORDER BY created_at, universe_uuid",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            Reservation {
                operation_id: r.get(1)?,
                container_id: r.get(2)?,
                image_id: r.get(3)?,
                source_host: r.get(4)?,
                destination: r.get(5)?,
                started_at: r.get(6)?,
                state: r.get(7)?,
                created_at: r.get(8)?,
                updated_at: r.get(9)?,
                detail: r.get(10)?,
            },
        ))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
pub(crate) fn set_state(db: &Connection, uuid: &str, state: &str, detail: &Value) -> Result<(), Error> {
    db.execute(
        "UPDATE migration_reservations SET state=?2, updated_at=?3, detail=?4 WHERE universe_uuid=?1",
        params![uuid, state, crate::now() as i64, detail.to_string()],
    )?;
    Ok(())
}
/// Changes the reservation state while keeping the recorded detail fields, such as the artifact hashes.
pub(crate) fn merge_state(db: &Connection, uuid: &str, state: &str, patch: &Value) -> Result<(), Error> {
    let r = reservation(db, uuid)?.ok_or("Reservation not found")?;
    let mut detail = r.detail_value();
    if let (Some(d), Some(p)) = (detail.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            d.insert(k.clone(), v.clone());
        }
    }
    set_state(db, uuid, state, &detail)
}
/// Generic lifecycle operations must not bypass a migration reservation or an unresolved restore claim.
pub(crate) fn refuse_if_reserved(db: &Connection, uuid: &str, operation: &str) -> Result<(), Error> {
    ensure_schema(db)?;
    if let Some(r) = reservation(db, uuid)? {
        // A released reservation holds nothing: migration_release lifted the gate deliberately, and the
        // row stays only so that migration_restore_local can still resume its preserved memory. A collected
        // one is the same decision taken on proof by the garbage collector; its tombstone, not the
        // reservation, is what still refuses a blind create of that universe UUID.
        if r.state == RELEASED || r.state == COLLECTED {
            return Ok(());
        }
        return Err(failure(
            format!(
                "Universe {uuid} is reserved by migration operation {} (state {}); {operation} is refused while the reservation exists",
                r.operation_id, r.state
            ),
            json!({"reservation": r.view()}),
        ));
    }
    // A restore that is neither verified nor closed may have created a container for this universe.
    if let Some(claim) = crate::restore::unresolved_claim(db, uuid)? {
        return Err(failure(
            format!(
                "Universe {uuid} has an unresolved restore claim for authorization {} (state {}); {operation} is refused until migration_restore verifies it or migration_restore_abort closes it",
                claim["authorization_id"].as_str().unwrap_or(""),
                claim["state"].as_str().unwrap_or("")
            ),
            json!({"restore_claim": claim}),
        ));
    }
    Ok(())
}
/// Whether this host transferred the given container away through a completed migration, in the current or an
/// archived reservation. Such a container never regains ownership through its original creation.
pub(crate) fn transferred_away(db: &Connection, uuid: &str, container_id: &str) -> Result<bool, Error> {
    ensure_schema(db)?;
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM migration_reservations WHERE universe_uuid=?1 AND container_id=?2 AND state='transferred')
         OR EXISTS(SELECT 1 FROM migration_reservation_history WHERE universe_uuid=?1 AND container_id=?2 AND state='transferred')",
        params![uuid, container_id],
        |r| r.get::<_, bool>(0),
    )?)
}
/// Moves a reservation in one given state to history; the caller holds the transaction. Returns whether a row
/// was archived. Used when a verified restore brings a `transferred` universe back to this host, when a
/// verified local restore ends a `released` reservation, and when a new checkpoint supersedes a released one.
pub(crate) fn archive_reservation(db: &Connection, uuid: &str, operation: &str, state: &str) -> Result<bool, Error> {
    let moved = db.execute(
        "INSERT INTO migration_reservation_history(universe_uuid,operation_id,container_id,image_id,source_host_uuid,destination_host_uuid,
         container_started_at,state,created_at,updated_at,detail,archived_at,archived_by_operation)
         SELECT universe_uuid,operation_id,container_id,image_id,source_host_uuid,destination_host_uuid,container_started_at,state,created_at,
         updated_at,detail,?3,?4 FROM migration_reservations WHERE universe_uuid=?1 AND state=?2",
        params![uuid, state, crate::now() as i64, operation],
    )?;
    db.execute(
        "DELETE FROM migration_reservations WHERE universe_uuid=?1 AND state=?2",
        params![uuid, state],
    )?;
    Ok(moved > 0)
}
pub(crate) fn archive_transferred(db: &Connection, uuid: &str, operation: &str) -> Result<bool, Error> {
    archive_reservation(db, uuid, operation, "transferred")
}
fn history_view(db: &Connection, uuid: &str) -> Result<Vec<Value>, Error> {
    let mut stmt = db.prepare(
        "SELECT operation_id,container_id,state,created_at,updated_at,detail,archived_at,archived_by_operation
         FROM migration_reservation_history WHERE universe_uuid=?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([uuid], |r| {
        Ok(
            json!({"operation_id": r.get::<_, String>(0)?, "container_id": r.get::<_, String>(1)?, "state": r.get::<_, String>(2)?,
            "created_at": r.get::<_, i64>(3)?, "updated_at": r.get::<_, i64>(4)?,
            "detail": r.get::<_, Option<String>>(5)?.and_then(|d| serde_json::from_str::<Value>(&d).ok()),
            "archived_at": r.get::<_, i64>(6)?, "archived_by_operation": r.get::<_, String>(7)?}),
        )
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
pub(crate) fn host_uuid(db: &Connection) -> Result<String, Error> {
    Ok(db.query_row("SELECT value FROM metadata WHERE key='host_uuid'", [], |r| r.get(0))?)
}
/// SHA-256 of a service-controlled path, computed by coreutils with fixed arguments.
pub(crate) fn sha256(path: &Path) -> Result<String, Error> {
    let out = Command::new("/usr/bin/sha256sum").arg("--").arg(path).output()?;
    if !out.status.success() {
        return Err(format!("sha256sum failed for {}", path.display()).into());
    }
    let text = String::from_utf8(out.stdout)?;
    Ok(text.split_whitespace().next().ok_or("Empty sha256sum output")?.to_string())
}
/// SHA-256 of bytes held in memory, computed by coreutils from standard input.
pub(crate) fn sha256_bytes(bytes: &[u8]) -> Result<String, Error> {
    let mut child = Command::new("/usr/bin/sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    child.stdin.take().ok_or("sha256sum input unavailable")?.write_all(bytes)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err("sha256sum failed".into());
    }
    let text = String::from_utf8(out.stdout)?;
    Ok(text.split_whitespace().next().ok_or("Empty sha256sum output")?.to_string())
}
/// Copies a file into a service-owned path through a temporary name, private and synced; callers re-hash the copy.
pub(crate) fn copy_private(from: &Path, to: &Path) -> Result<u64, Error> {
    let temporary = to.with_extension("partial-write");
    let mut input = fs::File::open(from)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    let bytes = std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    fs::rename(&temporary, to)?;
    Ok(bytes)
}
/// Bytes available to root under a path, as reported by coreutils df; 0 when unreadable.
pub(crate) fn available_bytes(path: &Path) -> u64 {
    Command::new("/usr/bin/df")
        .args(["-B1", "--output=avail"])
        .arg(path)
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .last()
                .and_then(|l| l.trim().parse::<u64>().ok())
        })
        .unwrap_or(0)
}
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let temporary = path.with_extension("partial-write");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
}
// A failing runtime can write an unbounded log: a CRIU restore of a deliberately damaged archive produced a
// multi-gigabyte restore.log on the lab, and reading it whole cost the service an out-of-memory kill. Every
// copy of runtime output is therefore bounded, and a longer file is marked as truncated.
pub(crate) const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) fn read_bounded(path: &Path) -> Result<Vec<u8>, Error> {
    use std::io::Read;
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(MAX_LOG_BYTES).read_to_end(&mut bytes)?;
    if fs::metadata(path)?.len() > MAX_LOG_BYTES {
        bytes.extend_from_slice(format!("\n[truncated by PodMesh after {MAX_LOG_BYTES} bytes]\n").as_bytes());
    }
    Ok(bytes)
}
pub(crate) fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let start = text.len().saturating_sub(2000);
    text[text.char_indices().map(|(i, _)| i).find(|i| *i >= start).unwrap_or(0)..].to_string()
}
fn command_text(program: &str, args: &[&str]) -> (bool, String) {
    match Command::new(program)
        .args(args)
        .env("PATH", RUNTIME_PATH)
        .env_remove("INVOCATION_ID")
        .output()
    {
        Ok(o) => (
            o.status.success(),
            format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
                .trim()
                .to_string(),
        ),
        Err(e) => (false, e.to_string()),
    }
}

struct Assessment {
    container: Value,
    blockers: Vec<String>,
    facts: Value,
}
pub(crate) fn runtime_facts(blockers: &mut Vec<String>) -> Value {
    let mut hashes = json!({});
    for (path, expected) in [
        (RUNTIME_REAL, RUNTIME_REAL_SHA256),
        (RUNTIME_WRAPPER, RUNTIME_WRAPPER_SHA256),
        (RUNTIME_SHIM, RUNTIME_SHIM_SHA256),
    ] {
        match sha256(Path::new(path)) {
            Ok(h) => {
                if h != expected {
                    blockers.push(format!("{path} sha256 {h} differs from the qualified {expected}"));
                }
                hashes[path] = json!(h);
            }
            Err(e) => {
                blockers.push(format!("{path} is unavailable: {e}"));
                hashes[path] = Value::Null;
            }
        }
    }
    let (version_ok, version) = command_text(RUNTIME_SHIM, &["--version"]);
    if !version_ok || !version.contains(RUNTIME_GIT_ID) {
        blockers.push(format!("private runtime does not report {RUNTIME_GIT_ID}"));
    }
    let (check_ok, check) = command_text(RUNTIME_SHIM, &["check"]);
    if !check_ok {
        blockers.push("private runtime `criu check` failed".into());
    }
    json!({"shim": RUNTIME_SHIM, "wrapper": RUNTIME_WRAPPER, "binary": RUNTIME_REAL, "sha256": hashes,
        "version": version, "check": check, "podman_path": RUNTIME_PATH,
        "podman_version": command_text("/usr/bin/podman", &["--version"]).1,
        "crun_version": command_text("/usr/bin/crun", &["--version"]).1.lines().next().unwrap_or("").to_string(),
        "kernel": fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim()})
}
fn collect_processes(dir: &Path, out: &mut Vec<u32>) {
    if let Ok(procs) = fs::read_to_string(dir.join("cgroup.procs")) {
        out.extend(procs.lines().filter_map(|l| l.trim().parse::<u32>().ok()));
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                collect_processes(&entry.path(), out);
            }
        }
    }
}
/// Read-only process inspection; nothing is attached, frozen or signalled.
fn process_facts(c: &Value, blockers: &mut Vec<String>, allow_frozen: bool) -> (Value, u64) {
    let cgroup = c["State"]["CgroupPath"].as_str().unwrap_or("");
    if !cgroup.starts_with("/machine.slice/libpod-") || cgroup.contains("..") {
        blockers.push("container cgroup is not an observable Podman libpod scope".into());
        return (Value::Null, 0);
    }
    let root = PathBuf::from(format!("/sys/fs/cgroup{cgroup}"));
    let frozen = fs::read_to_string(root.join("cgroup.events"))
        .map(|e| e.lines().any(|l| l == "frozen 1"))
        .unwrap_or(false);
    if frozen && !allow_frozen {
        blockers.push("container cgroup is frozen".into());
    }
    let memory = match fs::read_to_string(root.join("memory.current"))
        .ok()
        .and_then(|m| m.trim().parse::<u64>().ok())
    {
        Some(m) => {
            if m > MAX_MEMORY_BYTES {
                blockers.push(format!(
                    "memory.current {m} bytes exceeds the qualified maximum of {MAX_MEMORY_BYTES}"
                ));
            }
            m
        }
        None => {
            blockers.push("container memory.current is unreadable; the dump cannot be bounded".into());
            0
        }
    };
    let mut pids = vec![];
    collect_processes(&root, &mut pids);
    pids.sort_unstable();
    pids.dedup();
    if pids.is_empty() {
        blockers.push("no container process observed".into());
    }
    if pids.len() > MAX_PROCESSES {
        blockers.push(format!("{} processes exceed the qualified maximum of {MAX_PROCESSES}", pids.len()));
    }
    let mut processes = vec![];
    for pid in pids.iter().take(MAX_PROCESSES) {
        let maps = fs::read_to_string(format!("/proc/{pid}/maps")).unwrap_or_default();
        let musl = maps.contains("/ld-musl-");
        let glibc = maps.contains("/libc.so.6");
        let rseq_disabled = fs::read(format!("/proc/{pid}/environ"))
            .map(|e| {
                e.split(|b| *b == 0)
                    .any(|v| v.starts_with(b"GLIBC_TUNABLES=") && String::from_utf8_lossy(v).contains("glibc.pthread.rseq=0"))
            })
            .unwrap_or(false);
        let libc = match (musl, glibc) {
            (true, false) => "musl",
            (_, true) => "glibc",
            _ => "unidentified",
        };
        // CRIU 3.15 predates rseq support: glibc registers rseq by default and a static binary
        // cannot be identified from its mappings. Refuse both before any suspension.
        if !(libc == "musl" || (libc == "glibc" && rseq_disabled)) {
            blockers.push(format!(
                "process {pid} uses {libc} libc; the qualified runtime cannot safely checkpoint rseq-registered or unidentified processes"
            ));
        }
        processes.push(json!({"pid": pid, "libc": libc, "glibc_rseq_disabled_by_tunable": rseq_disabled}));
    }
    (
        json!({"cgroup": cgroup, "frozen": frozen, "memory_current_bytes": memory, "processes": processes}),
        memory,
    )
}
fn space_facts(container_id: &str, memory: u64, blockers: &mut Vec<String>) -> Result<Value, Error> {
    let sized: Value = serde_json::from_str(&lc::podman(lc::QUICK, &["container", "inspect", "--size", container_id])?)?;
    let size_rw = sized[0]["SizeRw"].as_u64().unwrap_or(0);
    // The kept image files land in Podman's graph root. The qualified scope is the default rootful store,
    // so another graph root is refused rather than measured on a filesystem the dump does not use.
    let graph_root = lc::podman(lc::QUICK, &["info", "--format", "{{.Store.GraphRoot}}"])?
        .trim()
        .to_string();
    if graph_root != CONTAINER_STORAGE {
        blockers.push(format!(
            "Podman graph root {graph_root} is not the qualified default store {CONTAINER_STORAGE}"
        ));
    }
    // Uncompressed upper bound: memory twice (image files and export), writable layer, margin. The kept
    // image files land in the graph root and the export in the state directory; both are checked.
    let required = memory.saturating_mul(2).saturating_add(size_rw).saturating_add(SPACE_MARGIN_BYTES);
    let mut available_bytes = json!({});
    for path in [base()?.as_path(), Path::new(&graph_root)] {
        let out = Command::new("/usr/bin/df").args(["-B1", "--output=avail"]).arg(path).output()?;
        let available = String::from_utf8_lossy(&out.stdout)
            .lines()
            .last()
            .and_then(|l| l.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if available < required {
            blockers.push(format!("{available} bytes available under {}, {required} required", path.display()));
        }
        available_bytes[path.display().to_string()] = json!(available);
    }
    Ok(json!({"available_bytes": available_bytes, "required_bytes": required, "writable_layer_bytes": size_rw}))
}
/// Container shape rules for migration assess. Flat scope refuses privileged outer containers; nested requires them.
pub(crate) fn migration_shape_blockers(c: &Value, profile: MigrationProfile) -> Vec<String> {
    let mut blockers = vec![];
    match profile {
        MigrationProfile::Flat => {
            if c["HostConfig"]["Privileged"].as_bool() != Some(false) {
                blockers.push("container is privileged or its privilege mode is unknown".into());
            }
        }
        MigrationProfile::Nested => {
            if c["HostConfig"]["Privileged"].as_bool() != Some(true) {
                blockers.push(
                    "nested profile requires a privileged outer container (Rule 11 outer universe)"
                        .into(),
                );
            }
        }
    }
    if c["HostConfig"]["NetworkMode"].as_str() != Some("none") {
        blockers.push("network mode is not none".into());
    }
    if c["Mounts"].as_array().map(|m| !m.is_empty()).unwrap_or(true) {
        blockers.push("container has volumes or bind mounts".into());
    }
    if c["Config"]["Tty"].as_bool() == Some(true) {
        blockers.push("containers with a TTY are outside the qualified scope".into());
    }
    blockers
}
fn assess(db: &Connection, uuid: &str, b: &Binding, existing: Option<Value>, allow_frozen: bool) -> Result<Assessment, Error> {
    let c = existing.ok_or("Universe container not found")?;
    lc::owned(db, &c, uuid, "Migration source")?;
    let host = host_uuid(db)?;
    let mut blockers = vec![];
    if b.source_host != host {
        blockers.push(format!("source_host_uuid {} is not this host", b.source_host));
    }
    if b.destination == host {
        blockers.push("destination_host_uuid is this host".into());
    }
    if c["Id"].as_str() != Some(b.container_id) {
        blockers.push("container_id does not match the universe container".into());
    }
    let image = b.image.trim_start_matches("sha256:");
    if c["Image"].as_str().unwrap_or("").trim_start_matches("sha256:") != image {
        blockers.push("image does not match the universe container image".into());
    }
    if !lc::images()?.iter().any(|i| lc::image_id(i) == image) {
        blockers.push("image is not present in the local store".into());
    }
    let state = lc::status(&c).to_string();
    if state != "running" {
        blockers.push(format!(
            "container state {state} is not running; only a running process can be checkpointed"
        ));
    }
    blockers.extend(migration_shape_blockers(&c, b.profile));
    let runtime = runtime_facts(&mut blockers);
    let (processes, space) = if state == "running" {
        let (processes, memory) = process_facts(&c, &mut blockers, allow_frozen);
        let observed_id = c["Id"].as_str().ok_or("Universe container has no ID")?;
        (processes, space_facts(observed_id, memory, &mut blockers)?)
    } else {
        (Value::Null, Value::Null)
    };
    let facts = json!({"observed_at": crate::now(), "host_uuid": host, "migration_profile": b.profile.as_str(),
        "container": lc::state_view(&c), "image_id": image,
        "network_mode": c["HostConfig"]["NetworkMode"], "log_driver": c["HostConfig"]["LogConfig"]["Type"],
        "privileged": c["HostConfig"]["Privileged"], "runtime": runtime, "processes": processes, "space": space});
    Ok(Assessment {
        container: c,
        blockers,
        facts,
    })
}

pub(crate) fn preflight(db: &Connection, uuid: &str, b: &Binding, existing: Option<Value>) -> Result<Value, Error> {
    let mut a = assess(db, uuid, b, existing, false)?;
    let reserved = reservation(db, uuid)?;
    if let Some(ref r) = reserved {
        a.blockers.push(format!(
            "universe is already reserved by migration operation {} (state {})",
            r.operation_id, r.state
        ));
    }
    let scope = match b.profile {
        MigrationProfile::Flat => SCOPE,
        MigrationProfile::Nested => SCOPE_NESTED_PREFLIGHT,
    };
    Ok(
        json!({"status": "verified", "operation": "migration_preflight", "universe_uuid": uuid,
        "migration_profile": b.profile.as_str(),
        "compatible": a.blockers.is_empty(), "blockers": a.blockers, "facts": a.facts,
        "reservation": reserved.map(|r| r.view()),
        "effects": "none: preflight does not reserve, suspend, signal or write artifacts", "scope": scope}),
    )
}

fn scope_unit(id: &str) -> String {
    format!("podmesh-checkpoint-{id}.scope")
}
/// The checkpoint blockers of a RUNNING universe this journal owns, without a migration's two-host binding:
/// what a live recovery point (`recovery_point_prepare`, `capture: live`) must find true before anything is
/// suspended. Read-only; the same facts a preflight reports.
pub(crate) fn capture_assess(db: &Connection, uuid: &str, c: &Value) -> Result<(Vec<String>, Value), Error> {
    lc::owned(db, c, uuid, "Live capture source")?;
    let mut blockers = vec![];
    let state = lc::status(c).to_string();
    if !lc::process_active(c) {
        blockers.push(format!("container state {state} is not running; only a running process can be checkpointed"));
    }
    if c["HostConfig"]["NetworkMode"].as_str() != Some("none") {
        blockers.push("network mode is not none".into());
    }
    if c["Mounts"].as_array().map(|m| !m.is_empty()).unwrap_or(true) {
        blockers.push("container has volumes or bind mounts".into());
    }
    if c["HostConfig"]["Privileged"].as_bool() != Some(false) {
        blockers.push("container is privileged or its privilege mode is unknown".into());
    }
    if c["Config"]["Tty"].as_bool() == Some(true) {
        blockers.push("containers with a TTY are outside the qualified scope".into());
    }
    let runtime = runtime_facts(&mut blockers);
    let (processes, space) = if lc::process_active(c) {
        let (processes, memory) = process_facts(c, &mut blockers, false);
        let observed_id = c["Id"].as_str().ok_or("Universe container has no ID")?;
        (processes, space_facts(observed_id, memory, &mut blockers)?)
    } else {
        (Value::Null, Value::Null)
    };
    let facts = json!({"observed_at": crate::now(), "container": lc::state_view(c), "image_id": c["Image"],
        "network_mode": c["HostConfig"]["NetworkMode"], "runtime": runtime, "processes": processes, "space": space});
    Ok((blockers, facts))
}
/// One exported checkpoint of a running container, in a transient scope the caller names (see `scoped_podman`).
/// Without `--leave-running`: the processes are ended by the dump, so the writable layer Podman exports next is
/// the exact disk state the memory image was taken against.
pub(crate) fn checkpoint_export(unit: &str, id: &str, container_id: &str, archive: &Path, stdout: fs::File, stderr: fs::File) -> Result<ExitStatus, Error> {
    let export = format!("--export={}", archive.display());
    Ok(scoped_podman(
        unit,
        id,
        CHECKPOINT_SECONDS,
        &["container", "checkpoint", &export, "--compress=zstd", "--keep", "--file-locks", "--print-stats", container_id],
        stdout,
        stderr,
        None,
    )?
    .0)
}
/// The same container resumed in place from the checkpoint files Podman kept for it (`--keep`).
pub(crate) fn restore_kept(unit: &str, id: &str, container_id: &str, stdout: fs::File, stderr: fs::File) -> Result<ExitStatus, Error> {
    Ok(scoped_podman(
        unit,
        id,
        CHECKPOINT_SECONDS,
        &["container", "restore", "--keep", "--file-locks", "--print-stats", container_id],
        stdout,
        stderr,
        None,
    )?
    .0)
}
/// An exported checkpoint archive restored as a container named `name`, under the caller's bound on the graph root.
pub(crate) fn restore_import(unit: &str, id: &str, archive: &Path, name: &str, stdout: fs::File, stderr: fs::File, bound: Option<&Bound>) -> Result<(ExitStatus, Value), Error> {
    let import = format!("--import={}", archive.display());
    scoped_podman(
        unit,
        id,
        CHECKPOINT_SECONDS,
        &["container", "restore", &import, "--name", name, "--keep", "--file-locks", "--print-stats"],
        stdout,
        stderr,
        bound,
    )
}
/// Whether a transient scope may still hold its command. `is-active` reports a scope that is still
/// activating or deactivating as not active, so only a finished or absent unit counts as done; a query
/// that cannot be answered fails closed.
pub(crate) fn unit_busy(unit: &str) -> bool {
    match Command::new("/usr/bin/systemctl")
        .args(["show", "--property=ActiveState", "--value", unit])
        .output()
    {
        Ok(o) if o.status.success() => !matches!(String::from_utf8_lossy(&o.stdout).trim(), "inactive" | "failed"),
        _ => true,
    }
}
/// Runs one Podman command of a migration operation in its own transient systemd scope, outside the
/// service cgroup, with the private runtime first on PATH, bounded by GNU timeout inside the scope and
/// with output written to files, so that a service crash or restart neither interrupts it nor breaks its
/// output. Its temporary directory belongs to the operation and is never the shared scratch directory
/// that every service start empties.
///
/// systemd-run --scope sets INVOCATION_ID for the command it runs even when the caller has none (observed
/// with systemd 257). Podman then leaves conmon inside the scope, which stays active for as long as a
/// restored universe runs; the variable is therefore removed inside the scope as well, so that conmon moves
/// to its own libpod-conmon scope and this scope ends with the Podman command.
///
/// With a `bound`, the command is watched while it runs and stopped if it consumes more of the Podman
/// graph root than its own preflight required, or if that filesystem falls below the floor: a restore of
/// a damaged archive was measured writing about 20 MB/s without ever returning. The returned value
/// describes that measurement whether or not it had to act; without a bound it says `watched: false`.
pub(crate) fn scoped_podman(
    unit: &str,
    id: &str,
    seconds: u64,
    args: &[&str],
    stdout: fs::File,
    stderr: fs::File,
    bound: Option<&Bound>,
) -> Result<(ExitStatus, Value), Error> {
    let unit_argument = format!("--unit={unit}");
    let limit = seconds.to_string();
    let temporary = base()?.join(".tmp").join(id);
    fs::create_dir_all(&temporary)?;
    fs::set_permissions(base()?.join(".tmp"), fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700))?;
    let mut child = Command::new("/usr/bin/systemd-run")
        .env("PATH", RUNTIME_PATH)
        .env("TMPDIR", &temporary)
        .env_remove("INVOCATION_ID")
        .args([
            "--scope",
            "--quiet",
            "--collect",
            &unit_argument,
            "--",
            "/usr/bin/env",
            "-u",
            "INVOCATION_ID",
            "/usr/bin/timeout",
            "--signal=TERM",
            "--kill-after=5",
            &limit,
            "/usr/bin/podman",
        ])
        .args(args)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()?;
    let begin = Instant::now();
    let mut watch = bound.map(Watch::start);
    loop {
        if let Some(status) = child.try_wait()? {
            let measurement = match (&watch, bound) {
                (Some(w), Some(b)) => w.view(b),
                _ => json!({"watched": false}),
            };
            return Ok((status, measurement));
        }
        if let (Some(w), Some(b)) = (watch.as_mut(), bound) {
            w.step(b, begin.elapsed().as_secs_f64());
        }
        thread::sleep(Duration::from_millis(250));
    }
}
/// The checkpoint runs in its own scope (see `scoped_podman`): a service crash or restart must not kill
/// CRIU mid-dump, which was observed to destroy the application without producing an archive. The command
/// names the reserved container ID, never the universe name: a container replaced under that name after
/// the checks cannot be captured.
fn checkpoint_command(id: &str, container_id: &str, archive: &Path, stdout: fs::File, stderr: fs::File) -> Result<ExitStatus, Error> {
    let export = format!("--export={}", archive.display());
    // No space bound here: a dump writes what the preflight measured and was never observed running
    // away, unlike a restore of a damaged archive. Adding one would need its own measurement.
    Ok(scoped_podman(
        &scope_unit(id),
        id,
        CHECKPOINT_SECONDS,
        &[
            "container",
            "checkpoint",
            &export,
            "--compress=zstd",
            "--keep",
            "--file-locks",
            "--print-stats",
            container_id,
        ],
        stdout,
        stderr,
        None,
    )?
    .0)
}

fn archive_superseded_reservation(db: &Connection, uuid: &str, id: &str) -> Result<(), Error> {
    if let Some(r) = reservation(db, uuid)? {
        if (r.state == RELEASED || r.state == COLLECTED) && r.operation_id != id {
            let state = r.state.clone();
            archive_reservation(db, uuid, id, &state)?;
        }
    }
    Ok(())
}

fn prepare_empty_artifact_dir(id: &str) -> Result<PathBuf, Error> {
    let dir = base()?.join(id);
    match fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&dir)?,
        Err(e) => return Err(e.into()),
        Ok(m) if m.is_dir() && fs::read_dir(&dir)?.next().is_none() => {}
        Ok(_) => {
            return Err(
                "An artifact directory for this operation exists without a reservation and is not empty; refusing to reuse it"
                    .into(),
            )
        }
    }
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

fn persist_checkpoint_reservation(
    db: &Connection,
    uuid: &str,
    id: &str,
    b: &Binding,
    a: &Assessment,
    dir: &Path,
) -> Result<Reservation, Error> {
    let now = crate::now() as i64;
    let started_at = a.container["State"]["StartedAt"]
        .as_str()
        .unwrap_or("")
        .to_string();
    db.execute(
        "INSERT INTO migration_reservations VALUES(?1,?2,?3,?4,?5,?6,?7,'reserved',?8,?8,NULL)",
        params![
            uuid,
            id,
            b.container_id,
            b.image.trim_start_matches("sha256:"),
            b.source_host,
            b.destination,
            started_at,
            now
        ],
    )?;
    write_private(
        &dir.join("preflight.json"),
        serde_json::to_string_pretty(&a.facts)?.as_bytes(),
    )?;
    reservation(db, uuid)?.ok_or_else(|| Error::from("Reservation not persisted"))
}

fn nested_lab_detail(
    uuid: &str,
    facts: &Value,
    r: &Reservation,
    gaps: &[&str],
    effects: &str,
    checkpoint_phase: &str,
    extra: Value,
) -> Value {
    let mut detail = json!({
        "migration_profile": "nested",
        "universe_uuid": uuid,
        "scope": SCOPE_NESTED_LAB_CHECKPOINT,
        "checkpoint_phase": checkpoint_phase,
        "implementation_gaps": gaps,
        "facts": facts,
        "reservation": r.view(),
        "effects": effects,
        "reference": "docs/MIGRATION-INTEGRATION.md required separation",
    });
    if let Some(obj) = detail.as_object_mut() {
        if let Some(extra_obj) = extra.as_object() {
            for (k, v) in extra_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    detail
}

#[cfg_attr(not(test), allow(dead_code))]
fn nested_lab_capture_pending_detail(uuid: &str, facts: &Value, r: &Reservation) -> Value {
    nested_lab_detail(
        uuid,
        facts,
        r,
        &NESTED_LAB_CHECKPOINT_GAPS,
        "reservation and preflight.json only: no inner Podman reconciliation, suspension, archive or manifest",
        "reservation",
        json!({}),
    )
}

fn nested_lab_rule11_chain_refusal_detail(
    uuid: &str,
    facts: &Value,
    r: &Reservation,
    inner: &Value,
    binding: &Value,
    chain: &Value,
) -> Value {
    nested_lab_detail(
        uuid,
        facts,
        r,
        &NESTED_LAB_GAPS_AFTER_VFS_STORE_BINDING,
        "reservation, preflight.json, inner_podman_metadata.json and nested_vfs_store_binding.json: host default-store outer and inner VFS store binding assessed; Rule 11 destination restore chain not verified; no outer suspension, archive or manifest",
        "rule11_destination_restore_chain",
        json!({
            "inner_podman_metadata": inner,
            "nested_vfs_store_binding": binding,
            "rule11_destination_restore_chain": chain,
        }),
    )
}

fn nested_lab_outer_checkpoint_refusal_detail(
    uuid: &str,
    facts: &Value,
    r: &Reservation,
    inner: &Value,
    binding: &Value,
    chain: &Value,
    plan: &Value,
    blockers: &[String],
) -> Value {
    nested_lab_detail(
        uuid,
        facts,
        r,
        &NESTED_LAB_GAPS_AFTER_DESTINATION_RESTORE_CHAIN,
        "reservation, preflight.json, inner Podman sidecars and nested_outer_checkpoint_plan.json: outer checkpoint capture preconditions not met; no outer suspension, archive or manifest",
        "nested_outer_checkpoint_capture",
        json!({
            "inner_podman_metadata": inner,
            "nested_vfs_store_binding": binding,
            "rule11_destination_restore_chain": chain,
            "nested_outer_checkpoint_plan": plan,
            "outer_checkpoint_blockers": blockers,
        }),
    )
}

fn nested_lab_after_outer_checkpoint_detail(
    uuid: &str,
    facts: &Value,
    r: &Reservation,
    inner: &Value,
    binding: &Value,
    chain: &Value,
    plan: &Value,
) -> Value {
    nested_lab_detail(
        uuid,
        facts,
        r,
        &NESTED_LAB_GAPS_UNTIL_TWO_HOST_MOVE_COMPLETE,
        "reservation, preflight.json, nested sidecars, nested_outer_checkpoint_plan.json, checkpoint.tar.zst and manifest.json: outer checkpoint captured on the source; destination restore hooks are implemented on the peer at preflight; the two-host carry/restore/complete chain is not verified end-to-end",
        "nested_outer_checkpoint_captured",
        json!({
            "inner_podman_metadata": inner,
            "nested_vfs_store_binding": binding,
            "rule11_destination_restore_chain": chain,
            "nested_outer_checkpoint_plan": plan,
        }),
    )
}

fn outer_podman_exec(outer: &str, shell: &str) -> Result<String, String> {
    let out = Command::new("/usr/bin/podman")
        .args(["exec", outer, "sh", "-c", shell])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Parse the kit counter workload's last log line: `<epoch> <uuid> <counter>`.
pub(crate) fn parse_nested_counter_log_line(line: &str) -> Result<(String, u64), String> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() != 3 {
        return Err(format!("counter log line has {} fields, expected 3", fields.len()));
    }
    let counter = fields[2]
        .parse::<u64>()
        .map_err(|e| format!("counter field is not an integer: {e}"))?;
    Ok((fields[1].to_string(), counter))
}

fn nested_inner_counter_max_on_disk(outer: &str, inner_id: &str) -> u64 {
    let path = format!(
        "{INNER_PODMAN_GRAPH_ROOT}/vfs-containers/{inner_id}/userdata/ctr.log"
    );
    let text = outer_podman_exec(outer, &format!("test -r {path} && cat {path}")).unwrap_or_default();
    let mut max = 0u64;
    for line in text.lines().filter(|l| !l.is_empty()) {
        if let Ok((_, counter)) = parse_nested_counter_log_line(line) {
            max = max.max(counter);
        }
    }
    max
}

fn nested_inner_counter_poll_after_quiesced_resume(
    outer: &str,
    outer_container_id: &str,
    observed: u64,
    expected: u64,
) -> u64 {
    let mut counter = observed;
    if counter >= expected {
        return counter;
    }
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(expected.saturating_add(5));
    while counter < expected && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if let Ok(proof) = nested_inner_podman_proof(outer, outer_container_id) {
            counter = counter.max(
                proof
                    .get("counter")
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0),
            );
        }
    }
    counter
}

fn nested_inner_podman_proof(outer: &str, expected_outer_id: &str) -> Result<Value, Vec<String>> {
    let mut blockers = vec![];
    let inspect = outer_podman_exec(
        outer,
        &format!("{INNER_PODMAN_CLI} inspect {NESTED_COUNTER_INNER_NAME}"),
    );
    let inner = match inspect {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Array(items)) if !items.is_empty() => items[0].clone(),
            Ok(_) => {
                blockers.push(format!(
                    "inner container {NESTED_COUNTER_INNER_NAME} inspect returned no container"
                ));
                Value::Null
            }
            Err(e) => {
                blockers.push(format!("inner container inspect is not JSON: {e}"));
                Value::Null
            }
        },
        Err(e) => {
            blockers.push(format!(
                "inner Podman could not inspect container {NESTED_COUNTER_INNER_NAME}: {e}"
            ));
            Value::Null
        }
    };
    let logs = outer_podman_exec(
        outer,
        &format!("{INNER_PODMAN_CLI} logs --tail=3 {NESTED_COUNTER_INNER_NAME}"),
    );
    let (uuid, counter, log_lines) = match logs {
        Ok(text) => {
            let lines = text.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>();
            match lines.last() {
                Some(last) => match parse_nested_counter_log_line(last) {
                    Ok((uuid, counter)) => (uuid, counter, lines.iter().map(|l| l.to_string()).collect()),
                    Err(e) => {
                        blockers.push(format!("inner counter log tail is unusable: {e}"));
                        (String::new(), 0, vec![])
                    }
                },
                None => {
                    blockers.push("inner counter produced no log lines".into());
                    (String::new(), 0, vec![])
                }
            }
        }
        Err(e) => {
            blockers.push(format!("inner Podman could not read counter logs: {e}"));
            (String::new(), 0, vec![])
        }
    };
    let pid = inner["State"]["Pid"].as_i64().filter(|p| *p > 0);
    if pid.is_none() && blockers.is_empty() {
        blockers.push("inner counter container has no running PID".into());
    }
    if !blockers.is_empty() {
        return Err(blockers);
    }
    Ok(json!({
        "fixture": "vzcriu-kit-nested-counter",
        "outer_container_id": expected_outer_id,
        "inner_container_name": NESTED_COUNTER_INNER_NAME,
        "inner_id": inner["Id"],
        "pid": pid,
        "workload_uuid": uuid,
        "counter": counter,
        "log_tail": log_lines,
    }))
}

/// Blockers for restoring a nested exported checkpoint when the kit inner Podman fixture was checkpointed.
/// Lab observation (2026-10-07): CRIU restore fails replaying inner Podman overlay mounts (`fill_overlayfs_info`, exit -52)
/// while the inner container mount tree is active. Stopping the inner container before outer capture (`outer_capture_quiesce`)
/// was observed to allow restore on lab-a/b; in-memory counter continuity is not preserved across that quiesce.
pub(crate) fn nested_destination_outer_restore_blockers(inner_sidecar: &Value) -> Vec<String> {
    if inner_sidecar.get("status") == Some(&json!("verified"))
        && inner_sidecar.get("proof").is_some()
        && inner_sidecar
            .get("outer_capture_quiesce")
            .and_then(|q| q.get("status"))
            != Some(&json!("verified"))
    {
        return vec![
            "nested outer checkpoint restore is not qualified when inner Podman overlay mounts were active at outer capture: CRIU restore fails overlay mount replay on the lab (fill_overlayfs_info, exit -52); stop the inner fixture before outer checkpoint".into(),
        ];
    }
    vec![]
}

/// Stop the kit inner counter before outer CRIU capture so overlay mounts are not live in the dump.
fn nested_inner_podman_quiesce_for_outer_capture(outer: &str) -> Result<Value, Error> {
    if let Err(e) = outer_podman_exec(
        outer,
        &format!("{INNER_PODMAN_CLI} stop {NESTED_COUNTER_INNER_NAME}"),
    ) {
        return Err(format!(
            "inner Podman could not stop {NESTED_COUNTER_INNER_NAME} before outer capture: {e}"
        )
        .into());
    }
    let status = outer_podman_exec(
        outer,
        &format!(
            "{INNER_PODMAN_CLI} inspect {NESTED_COUNTER_INNER_NAME} --format '{{{{.State.Status}}}}'"
        ),
    )
    .map_err(|e| {
        Error::from(format!(
            "inner Podman could not inspect {NESTED_COUNTER_INNER_NAME} after stop: {e}"
        ))
    })?;
    let running = outer_podman_exec(
        outer,
        &format!(
            "{INNER_PODMAN_CLI} inspect {NESTED_COUNTER_INNER_NAME} --format '{{{{.State.Running}}}}'"
        ),
    )
    .map_err(|e| {
        Error::from(format!(
            "inner Podman could not read running state for {NESTED_COUNTER_INNER_NAME}: {e}"
        ))
    })?;
    if status.trim() == "running" || running.trim() == "true" {
        return Err(format!(
            "inner container {NESTED_COUNTER_INNER_NAME} is still running after stop (status={})",
            status.trim()
        )
        .into());
    }
    Ok(json!({
        "status": "verified",
        "checkpoint_phase": "inner_podman_quiesced_for_outer_capture",
        "inner_container_name": NESTED_COUNTER_INNER_NAME,
        "inner_state": status.trim(),
        "reason": "inner Podman uses overlay mounts even with vfs graph driver; CRIU cannot replay them while the inner container is running",
        "observed_at": crate::now(),
    }))
}

fn nested_quiesce_inner_sidecar_for_outer_capture(
    outer: &str,
    dir: &Path,
    sidecars: &mut Value,
) -> Result<(), Error> {
    let inner = sidecars
        .get("inner_podman_metadata")
        .cloned()
        .unwrap_or(Value::Null);
    if inner.get("status") != Some(&json!("verified")) || inner.get("proof").is_none() {
        return Ok(());
    }
    if inner
        .get("outer_capture_quiesce")
        .and_then(|q| q.get("status"))
        == Some(&json!("verified"))
    {
        return Ok(());
    }
    let quiesce = nested_inner_podman_quiesce_for_outer_capture(outer)?;
    let mut updated = inner;
    if let Some(obj) = updated.as_object_mut() {
        obj.insert("outer_capture_quiesce".into(), quiesce);
    }
    write_nested_inner_metadata(dir, &updated)?;
    if let Some(obj) = sidecars.as_object_mut() {
        obj.insert("inner_podman_metadata".into(), updated);
    }
    Ok(())
}

fn nested_inner_podman_reconcile_run_state_after_outer_restore(outer: &str) -> Result<Value, Error> {
    let script = format!("{NESTED_INNER_RUN_STATE_RECONCILE_PY}\n");
    let mut child = Command::new("/usr/bin/podman")
        .args(["exec", "-i", outer, "python3", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::from(format!("inner Podman run-state reconcile could not start: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| Error::from(format!("inner Podman run-state reconcile failed: {e}")))?;
    if !out.status.success() {
        return Err(Error::from(format!(
            "inner Podman run-state reconcile failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    match serde_json::from_str::<Value>(String::from_utf8_lossy(&out.stdout).trim()) {
        Ok(v) => Ok(v),
        Err(e) => Err(Error::from(format!("inner run-state reconcile output is not JSON: {e}"))),
    }
}

fn nested_inner_podman_resume_after_quiesced_capture(outer: &str) -> Result<Value, Error> {
    let run_state = nested_inner_podman_reconcile_run_state_after_outer_restore(outer)?;
    outer_podman_exec(
        outer,
        &format!("{INNER_PODMAN_CLI} start {NESTED_COUNTER_INNER_NAME}"),
    )
    .map_err(|e| {
        Error::from(format!(
            "inner Podman could not start {NESTED_COUNTER_INNER_NAME} after outer restore: {e}"
        ))
    })?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    Ok(run_state)
}

/// After a verified outer restore on the destination, reconcile inner Podman metadata and prove counter continuity.
pub(crate) fn nested_destination_post_outer_restore(
    outer_name: &str,
    outer_container_id: &str,
    source_inner_metadata: &Value,
) -> Result<Value, Error> {
    if source_inner_metadata.get("status") != Some(&json!("verified")) {
        return Err("source inner_podman_metadata sidecar is not verified".into());
    }
    let proof_doc = source_inner_metadata
        .get("proof")
        .ok_or_else(|| Error::from("source inner_podman_metadata has no proof"))?;
    let expected_counter = proof_doc
        .get("counter")
        .and_then(|c| c.as_u64())
        .ok_or_else(|| Error::from("source inner_podman_metadata proof has no counter"))?;
    let workload_uuid = proof_doc.get("workload_uuid").and_then(|u| u.as_str());
    let quiesced = source_inner_metadata
        .get("outer_capture_quiesce")
        .and_then(|q| q.get("status"))
        == Some(&json!("verified"));
    let run_state_reconcile = if quiesced {
        Some(nested_inner_podman_resume_after_quiesced_capture(outer_name)?)
    } else {
        None
    };
    let proof = nested_inner_podman_proof(outer_name, outer_container_id)
        .map_err(|blockers| Error::from(blockers.join("; ")))?;
    let live_counter = proof
        .get("counter")
        .and_then(|c| c.as_u64())
        .ok_or_else(|| Error::from("inner counter proof after restore has no counter"))?;
    let observed_counter = if quiesced {
        let inner_id = proof
            .get("inner_id")
            .and_then(|id| id.as_str())
            .ok_or_else(|| Error::from("inner counter proof after restore has no inner_id"))?;
        let baseline = live_counter.max(nested_inner_counter_max_on_disk(outer_name, inner_id));
        nested_inner_counter_poll_after_quiesced_resume(
            outer_name,
            outer_container_id,
            baseline,
            expected_counter,
        )
    } else {
        live_counter
    };
    let counter_continuity = if quiesced {
        if observed_counter < expected_counter {
            return Err(format!(
                "inner counter after quiesced restore did not reach the source checkpoint value: observed {observed_counter}, source checkpoint had {expected_counter}"
            )
            .into());
        }
        "inner_quiesced_before_capture: workload resumes from disk after outer restore; in-memory counter and workload UUID are not preserved"
    } else if observed_counter < expected_counter {
        return Err(format!(
            "inner counter regressed after restore: observed {observed_counter}, source checkpoint had {expected_counter}"
        )
        .into());
    } else {
        "observed_gte_source_checkpoint"
    };
    if !quiesced {
        if let Some(uuid) = workload_uuid {
            if proof.get("workload_uuid").and_then(|u| u.as_str()) != Some(uuid) {
                return Err("inner workload UUID does not match the source checkpoint proof".into());
            }
        }
    }
    let reconcile = if quiesced {
        Value::Null
    } else {
        nested_inner_podman_reconcile(outer_name, &proof)
            .map_err(|blockers| Error::from(blockers.join("; ")))?
    };
    Ok(json!({
        "status": "verified",
        "restore_phase": "destination_inner_podman_metadata_reconciliation",
        "source_counter_at_checkpoint": expected_counter,
        "observed_counter_after_outer_restore": observed_counter,
        "counter_continuity": counter_continuity,
        "outer_capture_quiesce": source_inner_metadata.get("outer_capture_quiesce").cloned().unwrap_or(Value::Null),
        "run_state_reconcile": run_state_reconcile.unwrap_or(Value::Null),
        "proof": proof,
        "reconcile": reconcile,
        "observed_at": crate::now(),
    }))
}

fn nested_inner_podman_reconcile(outer: &str, proof: &Value) -> Result<Value, Vec<String>> {
    let payload = serde_json::to_string(proof).unwrap_or_else(|_| "{}".to_string());
    let script = format!("{NESTED_INNER_RECONCILE_PY}\n");
    let mut child = Command::new("/usr/bin/podman")
        .args(["exec", "-i", outer, "python3", "-", &payload])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| vec![format!("inner Podman metadata reconcile could not start: {e}")])?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| vec![format!("inner Podman metadata reconcile failed: {e}")])?;
    if !out.status.success() {
        return Err(vec![format!(
            "inner Podman metadata reconcile failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )]);
    }
    match serde_json::from_str::<Value>(String::from_utf8_lossy(&out.stdout).trim()) {
        Ok(v) => Ok(v),
        Err(e) => Err(vec![format!("inner reconcile output is not JSON: {e}")]),
    }
}

fn read_nested_inner_metadata(dir: &Path) -> Option<Value> {
    let path = dir.join(INNER_PODMAN_METADATA);
    fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn write_nested_inner_metadata(dir: &Path, document: &Value) -> Result<(), Error> {
    write_private(
        &dir.join(INNER_PODMAN_METADATA),
        serde_json::to_string_pretty(document)?.as_bytes(),
    )
}

fn read_nested_vfs_store_binding(dir: &Path) -> Option<Value> {
    let path = dir.join(NESTED_VFS_STORE_BINDING);
    fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn read_rule11_destination_restore_chain(dir: &Path) -> Option<Value> {
    let path = dir.join(RULE11_DESTINATION_RESTORE_CHAIN);
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn write_rule11_destination_restore_chain(dir: &Path, document: &Value) -> Result<(), Error> {
    write_private(
        &dir.join(RULE11_DESTINATION_RESTORE_CHAIN),
        serde_json::to_string_pretty(document)?.as_bytes(),
    )
}

fn read_nested_outer_checkpoint_plan(dir: &Path) -> Option<Value> {
    let path = dir.join(NESTED_OUTER_CHECKPOINT_PLAN);
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn write_nested_outer_checkpoint_plan(dir: &Path, document: &Value) -> Result<(), Error> {
    write_private(
        &dir.join(NESTED_OUTER_CHECKPOINT_PLAN),
        serde_json::to_string_pretty(document)?.as_bytes(),
    )
}

/// Sidecar readiness for outer suspension: Rule 11 chain and inner artifacts must already be verified.
pub(crate) fn nested_outer_checkpoint_capture_assess(
    chain: &Value,
    binding: &Value,
    inner: &Value,
) -> Result<Value, Vec<String>> {
    let mut blockers = vec![];
    if chain.get("status") != Some(&json!("verified")) {
        blockers.push("Rule 11 destination restore chain is not verified".into());
    }
    if binding.get("status") != Some(&json!("verified")) {
        blockers.push("nested VFS store binding is not verified".into());
    }
    if inner.get("status") != Some(&json!("verified")) {
        blockers.push("inner Podman metadata is not verified".into());
    }
    if !blockers.is_empty() {
        return Err(blockers);
    }
    Ok(json!({
        "status": "verified",
        "checkpoint_phase": "nested_outer_checkpoint_capture",
        "scope": SCOPE_NESTED_OUTER_CHECKPOINT,
        "note": "Source-side plan only: outer suspension uses the same packaged runtime and scope isolation as flat-store migration_checkpoint; destination restore hooks are assessed on the peer host at preflight.",
        "observed_at": crate::now(),
    }))
}

pub(crate) fn nested_checkpoint_manifest(manifest: &Value) -> bool {
    manifest["format"].as_str() == Some(NESTED_MANIFEST_FORMAT)
        || manifest["migration_profile"].as_str() == Some("nested")
}

/// Destination-side assessment of the typed restore hooks Rule 11 nested moves require.
pub(crate) fn nested_destination_restore_hooks_assess(
    manifest: &Value,
    archive_config: &Value,
    sidecars_in_inbox: &Value,
) -> Result<Value, Vec<String>> {
    let mut blockers = vec![];
    if !nested_checkpoint_manifest(manifest) {
        blockers.push("manifest is not a nested-lab source checkpoint".into());
    }
    let privileged = archive_config
        .get("privileged")
        .and_then(|v| v.as_bool())
        .or_else(|| {
            archive_config
                .get("hostConfig")
                .and_then(|h| h.get("privileged"))
                .and_then(|v| v.as_bool())
        })
        .or_else(|| {
            archive_config
                .get("HostConfig")
                .and_then(|h| h.get("Privileged"))
                .and_then(|v| v.as_bool())
        });
    if privileged != Some(true) {
        blockers.push("archive configuration does not show a privileged outer universe".into());
    }
    let nested_sidecars = match manifest.get("nested_sidecars").and_then(|v| v.as_object()) {
        Some(o) => o,
        None => {
            blockers.push("manifest lacks nested_sidecars".into());
            return Err(blockers);
        }
    };
    for file in NESTED_CHECKPOINT_SIDECAR_FILES {
        let entry = match nested_sidecars
            .values()
            .find(|e| e.get("file").and_then(|f| f.as_str()) == Some(file))
        {
            Some(e) => e,
            None => {
                blockers.push(format!("manifest nested_sidecars does not name {file}"));
                continue;
            }
        };
        let expected = match entry
            .get("sha256")
            .and_then(|s| s.as_str())
            .filter(|h| h.len() == 64)
        {
            Some(h) => h,
            None => {
                blockers.push(format!("manifest nested_sidecars entry for {file} lacks sha256"));
                continue;
            }
        };
        let observed = sidecars_in_inbox
            .get(file)
            .and_then(|v| v.get("sha256"))
            .and_then(|s| s.as_str());
        if observed != Some(expected) {
            blockers.push(format!(
                "inbox sidecar {file} is missing or does not match the manifest (expected sha256 {expected})"
            ));
        }
    }
    if !blockers.is_empty() {
        return Err(blockers);
    }
    Ok(json!({
        "status": "verified",
        "checkpoint_phase": "nested_destination_restore_hooks",
        "binding_model": "two_host_serial_handoff",
        "note": "Destination-phase hook assessment: inbox sidecars match the nested manifest and the archive names a privileged outer universe. migration_restore runs outer checkpoint restore with the packaged runtime, then inner Podman metadata reconciliation for the kit counter fixture when present.",
        "required_operations": [
            "migration_destination_preflight",
            "migration_restore",
            "migration_complete_transfer"
        ],
        "api_hooks": {
            "migration_destination_preflight": {
                "status": "assessed_on_destination",
                "nested_profile": "sidecars_and_manifest_verified"
            },
            "migration_restore": {
                "status": "implementation_present_runtime_unqualified",
                "nested_profile": "privileged_outer_restore_plus_inner_podman_reconcile_after_outer_verified"
            }
        },
        "implementation_gaps": NESTED_LAB_GAPS_AFTER_DESTINATION_RESTORE_HOOKS,
        "observed_at": crate::now(),
    }))
}

fn write_nested_vfs_store_binding(dir: &Path, document: &Value) -> Result<(), Error> {
    write_private(
        &dir.join(NESTED_VFS_STORE_BINDING),
        serde_json::to_string_pretty(document)?.as_bytes(),
    )
}

fn host_podman_store_facts(container_id: &str) -> Result<Value, Vec<String>> {
    let mut blockers = vec![];
    let info = match Command::new("/usr/bin/podman")
        .args(["info", "--format", "json"])
        .output()
    {
        Ok(o) if o.status.success() => match serde_json::from_slice::<Value>(&o.stdout) {
            Ok(v) => v,
            Err(e) => {
                blockers.push(format!("host Podman info is not JSON: {e}"));
                Value::Null
            }
        },
        Ok(o) => {
            blockers.push(format!(
                "host Podman info failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
            Value::Null
        }
        Err(e) => {
            blockers.push(format!("host Podman info could not run: {e}"));
            Value::Null
        }
    };
    let inspect = match Command::new("/usr/bin/podman")
        .args(["container", "inspect", container_id, "--format", "json"])
        .output()
    {
        Ok(o) if o.status.success() => match serde_json::from_slice::<Value>(&o.stdout) {
            Ok(Value::Array(items)) if !items.is_empty() => items[0].clone(),
            Ok(_) => {
                blockers.push("host outer container inspect returned no container".into());
                Value::Null
            }
            Err(e) => {
                blockers.push(format!("host outer container inspect is not JSON: {e}"));
                Value::Null
            }
        },
        Ok(o) => {
            blockers.push(format!(
                "host outer container inspect failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
            Value::Null
        }
        Err(e) => {
            blockers.push(format!("host outer container inspect could not run: {e}"));
            Value::Null
        }
    };
    if !blockers.is_empty() {
        return Err(blockers);
    }
    let graph_root = info["store"]["graphRoot"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let graph_driver = info["store"]["graphDriverName"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if graph_root != CONTAINER_STORAGE {
        blockers.push(format!(
            "host Podman graph root {graph_root} is not the qualified default store {CONTAINER_STORAGE}"
        ));
    }
    Ok(json!({
        "role": "rule_11_outer_on_host_default_store",
        "graph_root": graph_root,
        "graph_driver": graph_driver,
        "run_root": info["store"]["runRoot"],
        "container_id": container_id,
        "static_dir": inspect["StaticDir"],
        "graph_root_matches_qualified_default": graph_root == CONTAINER_STORAGE,
    }))
}

fn nested_inner_podman_store_facts(outer: &str) -> Result<Value, Vec<String>> {
    let info_text = outer_podman_exec(outer, &format!("{INNER_PODMAN_CLI} info --format json"))
        .map_err(|e| vec![e])?;
    let info = match serde_json::from_str::<Value>(&info_text) {
        Ok(v) => v,
        Err(e) => return Err(vec![format!("inner Podman info is not JSON: {e}")]),
    };
    let graph_root = info["store"]["graphRoot"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let graph_driver = info["store"]["graphDriverName"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut blockers = vec![];
    if graph_driver != "vfs" {
        blockers.push(format!(
            "inner Podman graph driver {graph_driver} is not vfs"
        ));
    }
    if graph_root != INNER_PODMAN_GRAPH_ROOT {
        blockers.push(format!(
            "inner Podman graph root {graph_root} is not the expected {INNER_PODMAN_GRAPH_ROOT}"
        ));
    }
    if !blockers.is_empty() {
        return Err(blockers);
    }
    Ok(json!({
        "role": "kit_counter_fixture_inner_podman",
        "graph_root": graph_root,
        "graph_driver": graph_driver,
        "run_root": info["store"]["runRoot"],
    }))
}

/// Read-only assessment of how the host default-store outer relates to the inner VFS store.
pub(crate) fn nested_vfs_store_binding_assess(
    outer: &str,
    host_outer_container_id: &str,
    inner_metadata: &Value,
) -> Result<Value, Vec<String>> {
    let proof = inner_metadata
        .get("proof")
        .ok_or_else(|| vec!["inner Podman metadata has no proof".into()])?;
    if inner_metadata.get("status") != Some(&json!("verified")) {
        return Err(vec!["inner Podman metadata is not verified".into()]);
    }
    let inner_id = proof["inner_id"]
        .as_str()
        .ok_or_else(|| vec!["inner Podman proof has no inner_id".into()])?;
    let outer_store = host_podman_store_facts(host_outer_container_id)?;
    let inner_store = nested_inner_podman_store_facts(outer)?;
    let vfs_container_dir = format!("{INNER_PODMAN_GRAPH_ROOT}/vfs-containers/{inner_id}");
    let dir_probe = outer_podman_exec(
        outer,
        &format!("test -d {vfs_container_dir} && echo present"),
    );
    let vfs_dir_present = dir_probe.map(|t| t.trim() == "present").unwrap_or(false);
    if !vfs_dir_present {
        return Err(vec![format!(
            "inner VFS container directory {vfs_container_dir} is not present inside the outer universe"
        )]);
    }
    Ok(json!({
        "status": "verified",
        "checkpoint_phase": "nested_vfs_store_binding",
        "binding_model": "host_overlay_outer_contains_inner_vfs_store",
        "note": "The outer universe lives in the host default rootful overlay store; inner Podman uses an isolated VFS graph inside that outer. This is not the kit vzkit isolated outer store.",
        "outer": outer_store,
        "inner": inner_store,
        "inner_container_id": inner_id,
        "inner_vfs_container_dir": vfs_container_dir,
        "observed_at": crate::now(),
    }))
}

/// Source-side assessment of the typed destination restore chain Rule 11 nested moves require.
pub(crate) fn nested_rule11_destination_restore_chain_assess(
    source_host: &str,
    destination_host: &str,
    vfs_binding: &Value,
    inner_metadata: &Value,
) -> Result<Value, Vec<String>> {
    let mut blockers = vec![];
    if source_host.is_empty() {
        blockers.push("source_host_uuid is missing from the reservation".into());
    }
    if destination_host.is_empty() {
        blockers.push("destination_host_uuid is missing from the reservation".into());
    }
    if source_host == destination_host {
        blockers.push("destination_host_uuid must differ from source_host_uuid".into());
    }
    if vfs_binding.get("status") != Some(&json!("verified")) {
        blockers.push("nested VFS store binding is not verified".into());
    }
    if inner_metadata.get("status") != Some(&json!("verified")) {
        blockers.push("inner Podman metadata is not verified".into());
    }
    if !blockers.is_empty() {
        return Err(blockers);
    }
    Ok(json!({
        "status": "verified",
        "checkpoint_phase": "rule11_destination_restore_chain",
        "binding_model": "two_host_serial_handoff",
        "source_host_uuid": source_host,
        "destination_host_uuid": destination_host,
        "note": "Source-phase assessment only: documents the typed API chain the destination must honor for Rule 11 nested universes. Packaged flat-store migration_destination_preflight and migration_restore do not cover privileged outer plus inner VFS restore; nested destination hooks remain unimplemented.",
        "required_operations": [
            "migration_authorize_transfer",
            "migration_destination_preflight",
            "migration_restore",
            "migration_complete_transfer"
        ],
        "api_hooks": {
            "migration_destination_preflight": {
                "status": "not_invoked_on_source",
                "nested_profile": "unimplemented_on_destination"
            },
            "migration_restore": {
                "status": "not_invoked_on_source",
                "nested_profile": "unimplemented_on_destination"
            }
        },
        "observed_at": crate::now(),
    }))
}

fn nested_lab_inner_reconciliation(
    db: &Connection,
    attempt: i64,
    uuid: &str,
    outer: &str,
    facts: &Value,
    r: &Reservation,
    dir: &Path,
    b: &Binding,
    existing: Option<Value>,
) -> Result<Value, Error> {
    if r.state == NESTED_INNER_RECONCILED {
        let inner = read_nested_inner_metadata(dir).unwrap_or(Value::Null);
        return nested_lab_vfs_store_binding(db, attempt, uuid, outer, facts, r, dir, &inner, b, existing);
    }
    if r.state == NESTED_VFS_STORE_BOUND
        || r.state == NESTED_DESTINATION_RESTORE_CHAIN_ASSESSED
        || r.state == "checkpointing"
        || r.state == "checkpointed"
    {
        let inner = read_nested_inner_metadata(dir).unwrap_or(Value::Null);
        let binding = read_nested_vfs_store_binding(dir).unwrap_or(Value::Null);
        return nested_lab_destination_restore_chain(
            db,
            attempt,
            uuid,
            outer,
            facts,
            r,
            dir,
            &inner,
            &binding,
            b,
            existing,
        );
    }
    if let Some(verified_inner) = read_nested_inner_metadata(dir) {
        if verified_inner.get("status") == Some(&json!("verified")) {
            set_state(db, uuid, NESTED_INNER_RECONCILED, &verified_inner)?;
            let inner = read_nested_inner_metadata(dir).unwrap_or(verified_inner);
            return nested_lab_vfs_store_binding(db, attempt, uuid, outer, facts, r, dir, &inner, b, existing);
        }
    }
    let proof = match nested_inner_podman_proof(outer, &r.container_id) {
        Ok(p) => p,
        Err(blockers) => {
            let document = json!({
                "status": "refused",
                "checkpoint_phase": "inner_podman_metadata_reconciliation",
                "blockers": blockers,
                "observed_at": crate::now(),
            });
            write_nested_inner_metadata(dir, &document)?;
            let detail = nested_lab_detail(
                uuid,
                facts,
                r,
                &NESTED_LAB_CHECKPOINT_GAPS,
                "reservation, preflight.json and inner_podman_metadata.json refusal: no outer suspension",
                "inner_podman_metadata_reconciliation",
                json!({"inner_podman_metadata": document, "inner_reconciliation_blockers": blockers}),
            );
            set_state(db, uuid, "reserved", &detail)?;
            return Err(failure(
                "migration_checkpoint nested inner Podman metadata could not be reconciled; outer suspension refused",
                detail,
            ));
        }
    };
    let reconcile = match nested_inner_podman_reconcile(outer, &proof) {
        Ok(r) => r,
        Err(blockers) => {
            let document = json!({
                "status": "refused",
                "checkpoint_phase": "inner_podman_metadata_reconciliation",
                "proof": proof,
                "blockers": blockers,
                "observed_at": crate::now(),
            });
            write_nested_inner_metadata(dir, &document)?;
            let detail = nested_lab_detail(
                uuid,
                facts,
                r,
                &NESTED_LAB_CHECKPOINT_GAPS,
                "reservation, preflight.json and inner_podman_metadata.json refusal: no outer suspension",
                "inner_podman_metadata_reconciliation",
                json!({"inner_podman_metadata": document, "inner_reconciliation_blockers": blockers}),
            );
            set_state(db, uuid, "reserved", &detail)?;
            return Err(failure(
                "migration_checkpoint nested inner Podman metadata could not be reconciled; outer suspension refused",
                detail,
            ));
        }
    };
    let document = json!({
        "status": "verified",
        "checkpoint_phase": "inner_podman_metadata_reconciliation",
        "proof": proof,
        "reconcile": reconcile,
        "observed_at": crate::now(),
    });
    write_nested_inner_metadata(dir, &document)?;
    set_state(db, uuid, NESTED_INNER_RECONCILED, &document)?;
    let updated = reservation(db, uuid)?.ok_or_else(|| Error::from("Reservation not found after inner reconcile"))?;
    nested_lab_vfs_store_binding(db, attempt, uuid, outer, facts, &updated, dir, &document, b, existing)
}

fn nested_lab_vfs_store_binding(
    db: &Connection,
    attempt: i64,
    uuid: &str,
    outer: &str,
    facts: &Value,
    r: &Reservation,
    dir: &Path,
    inner: &Value,
    b: &Binding,
    existing: Option<Value>,
) -> Result<Value, Error> {
    if r.state == NESTED_VFS_STORE_BOUND
        || r.state == NESTED_DESTINATION_RESTORE_CHAIN_ASSESSED
        || r.state == "checkpointing"
        || r.state == "checkpointed"
    {
        let binding = read_nested_vfs_store_binding(dir).unwrap_or(Value::Null);
        return nested_lab_destination_restore_chain(
            db,
            attempt,
            uuid,
            outer,
            facts,
            r,
            dir,
            inner,
            &binding,
            b,
            existing,
        );
    }
    if let Some(existing_binding) = read_nested_vfs_store_binding(dir) {
        if existing_binding.get("status") == Some(&json!("verified")) {
            set_state(db, uuid, NESTED_VFS_STORE_BOUND, &existing_binding)?;
            let updated = reservation(db, uuid)?.ok_or_else(|| Error::from("Reservation not found after VFS binding"))?;
            return nested_lab_destination_restore_chain(
                db,
                attempt,
                uuid,
                outer,
                facts,
                &updated,
                dir,
                inner,
                &existing_binding,
                b,
                existing,
            );
        }
    }
    match nested_vfs_store_binding_assess(outer, &r.container_id, inner) {
        Ok(document) => {
            write_nested_vfs_store_binding(dir, &document)?;
            set_state(db, uuid, NESTED_VFS_STORE_BOUND, &document)?;
            let updated = reservation(db, uuid)?.ok_or_else(|| Error::from("Reservation not found after VFS binding"))?;
            nested_lab_destination_restore_chain(
                db,
                attempt,
                uuid,
                outer,
                facts,
                &updated,
                dir,
                inner,
                &document,
                b,
                existing,
            )
        }
        Err(blockers) => {
            let document = json!({
                "status": "refused",
                "checkpoint_phase": "nested_vfs_store_binding",
                "blockers": blockers,
                "observed_at": crate::now(),
            });
            write_nested_vfs_store_binding(dir, &document)?;
            let detail = nested_lab_detail(
                uuid,
                facts,
                r,
                &NESTED_LAB_GAPS_AFTER_INNER_RECONCILIATION,
                "reservation, preflight.json, inner_podman_metadata.json and nested_vfs_store_binding.json refusal: no outer suspension",
                "nested_vfs_store_binding",
                json!({
                    "inner_podman_metadata": inner,
                    "nested_vfs_store_binding": document,
                    "vfs_store_binding_blockers": blockers,
                }),
            );
            set_state(db, uuid, NESTED_INNER_RECONCILED, &detail)?;
            Err(failure(
                "migration_checkpoint nested VFS store binding could not be assessed; outer suspension refused",
                detail,
            ))
        }
    }
}

fn nested_lab_destination_restore_chain(
    db: &Connection,
    attempt: i64,
    uuid: &str,
    outer: &str,
    facts: &Value,
    r: &Reservation,
    dir: &Path,
    inner: &Value,
    binding: &Value,
    b: &Binding,
    existing: Option<Value>,
) -> Result<Value, Error> {
    if r.state == NESTED_DESTINATION_RESTORE_CHAIN_ASSESSED
        || r.state == "checkpointing"
        || r.state == "checkpointed"
    {
        let chain = read_rule11_destination_restore_chain(dir).unwrap_or(Value::Null);
        return nested_lab_outer_checkpoint(
            db,
            attempt,
            &r.operation_id,
            uuid,
            outer,
            b,
            existing,
            facts,
            r,
            dir,
            inner,
            binding,
            &chain,
        );
    }
    if let Some(existing_chain) = read_rule11_destination_restore_chain(dir) {
        if existing_chain.get("status") == Some(&json!("verified")) {
            set_state(db, uuid, NESTED_DESTINATION_RESTORE_CHAIN_ASSESSED, &existing_chain)?;
            let updated = reservation(db, uuid)?
                .ok_or_else(|| Error::from("Reservation not found after Rule 11 destination chain assess"))?;
            return nested_lab_outer_checkpoint(
                db,
                attempt,
                &updated.operation_id,
                uuid,
                outer,
                b,
                existing,
                facts,
                &updated,
                dir,
                inner,
                binding,
                &existing_chain,
            );
        }
    }
    match nested_rule11_destination_restore_chain_assess(
        &r.source_host,
        &r.destination,
        binding,
        inner,
    ) {
        Ok(document) => {
            write_rule11_destination_restore_chain(dir, &document)?;
            set_state(db, uuid, NESTED_DESTINATION_RESTORE_CHAIN_ASSESSED, &document)?;
            let updated = reservation(db, uuid)?
                .ok_or_else(|| Error::from("Reservation not found after Rule 11 destination chain assess"))?;
            nested_lab_outer_checkpoint(
                db,
                attempt,
                &updated.operation_id,
                uuid,
                outer,
                b,
                existing,
                facts,
                &updated,
                dir,
                inner,
                binding,
                &document,
            )
        }
        Err(blockers) => {
            let document = json!({
                "status": "refused",
                "checkpoint_phase": "rule11_destination_restore_chain",
                "blockers": blockers,
                "observed_at": crate::now(),
            });
            write_rule11_destination_restore_chain(dir, &document)?;
            let detail = nested_lab_rule11_chain_refusal_detail(
                uuid,
                facts,
                r,
                inner,
                binding,
                &document,
            );
            set_state(db, uuid, NESTED_VFS_STORE_BOUND, &detail)?;
            Err(failure(
                "migration_checkpoint nested Rule 11 destination restore chain could not be assessed; outer suspension refused",
                detail,
            ))
        }
    }
}

fn nested_sidecars(dir: &Path) -> (Value, Value, Value, Value) {
    (
        read_nested_inner_metadata(dir).unwrap_or(Value::Null),
        read_nested_vfs_store_binding(dir).unwrap_or(Value::Null),
        read_rule11_destination_restore_chain(dir).unwrap_or(Value::Null),
        read_nested_outer_checkpoint_plan(dir).unwrap_or(Value::Null),
    )
}

#[allow(clippy::too_many_arguments)]
fn nested_lab_outer_checkpoint(
    db: &Connection,
    attempt: i64,
    id: &str,
    uuid: &str,
    name: &str,
    b: &Binding,
    existing: Option<Value>,
    facts: &Value,
    r: &Reservation,
    dir: &Path,
    inner: &Value,
    binding: &Value,
    chain: &Value,
) -> Result<Value, Error> {
    if r.state == "checkpointing" || r.state == "checkpointed" {
        return nested_lab_outer_resume(db, attempt, id, uuid, name, b, existing, &r, dir);
    }
    let plan = match nested_outer_checkpoint_capture_assess(chain, binding, inner) {
        Ok(document) => document,
        Err(blockers) => {
            let document = json!({
                "status": "refused",
                "checkpoint_phase": "nested_outer_checkpoint_capture",
                "blockers": blockers,
                "observed_at": crate::now(),
            });
            write_nested_outer_checkpoint_plan(dir, &document)?;
            let detail = nested_lab_outer_checkpoint_refusal_detail(
                uuid, facts, r, inner, binding, chain, &document, &blockers,
            );
            set_state(db, uuid, NESTED_DESTINATION_RESTORE_CHAIN_ASSESSED, &detail)?;
            return Err(failure(
                "migration_checkpoint nested outer checkpoint capture sidecars are not verified; outer suspension refused",
                detail,
            ));
        }
    };
    write_nested_outer_checkpoint_plan(dir, &plan)?;
    let Some(c) = existing else {
        let detail = json!({"reason": "reserved source container is absent"});
        set_state(db, uuid, "checkpoint_failed", &detail)?;
        return Err(failure(
            "The reserved source container is absent; nothing can be finalized or recaptured",
            detail,
        ));
    };
    let a = assess(db, uuid, b, Some(c), false)?;
    if !a.blockers.is_empty() {
        let detail = nested_lab_outer_checkpoint_refusal_detail(
            uuid,
            &a.facts,
            r,
            inner,
            binding,
            chain,
            &plan,
            &a.blockers,
        );
        set_state(db, uuid, NESTED_DESTINATION_RESTORE_CHAIN_ASSESSED, &detail)?;
        return Err(failure(
            "migration_checkpoint nested outer checkpoint capture preconditions not met; outer suspension refused",
            detail,
        ));
    }
    let sidecars = json!({
        "inner_podman_metadata": inner,
        "nested_vfs_store_binding": binding,
        "rule11_destination_restore_chain": chain,
        "nested_outer_checkpoint_plan": plan,
    });
    capture_nested(db, attempt, id, uuid, name, dir, r, &sidecars)
}

#[allow(clippy::too_many_arguments)]
fn nested_lab_outer_resume(
    db: &Connection,
    attempt: i64,
    id: &str,
    uuid: &str,
    name: &str,
    b: &Binding,
    existing: Option<Value>,
    r: &Reservation,
    dir: &Path,
) -> Result<Value, Error> {
    if unit_busy(&scope_unit(id)) {
        return Err(failure(
            "The checkpoint scope of this operation has not finished, or its state cannot be queried; retry after it finishes",
            json!({"reservation": r.view(), "scope": scope_unit(id)}),
        ));
    }
    let (inner, binding, chain, plan) = nested_sidecars(dir);
    let Some(c) = existing else {
        let detail = json!({"reason": "reserved source container is absent"});
        set_state(db, uuid, "checkpoint_failed", &detail)?;
        return Err(failure(
            "The reserved source container is absent; nothing can be finalized or recaptured",
            detail,
        ));
    };
    let checkpointed_after_reservation = c["State"]["Checkpointed"] == true
        && c["State"]["CheckpointedAt"]
            .as_str()
            .and_then(lc::epoch)
            .is_some_and(|t| t >= r.created_at);
    if c["Id"].as_str() == Some(r.container_id.as_str()) && checkpointed_after_reservation && !lc::process_active(&c) {
        let sidecars = json!({
            "inner_podman_metadata": inner,
            "nested_vfs_store_binding": binding,
            "rule11_destination_restore_chain": chain,
            "nested_outer_checkpoint_plan": plan,
        });
        return finalize_nested(db, id, uuid, name, dir, &r, true, &sidecars);
    }
    let same_process = c["Id"].as_str() == Some(r.container_id.as_str())
        && lc::status(&c) == "running"
        && c["State"]["StartedAt"].as_str() == Some(r.started_at.as_str())
        && c["State"]["Checkpointed"] != true;
    if same_process {
        let a = assess(db, uuid, b, Some(c), true)?;
        if !a.blockers.is_empty() {
            let detail = nested_lab_outer_checkpoint_refusal_detail(
                uuid,
                &a.facts,
                &r,
                &inner,
                &binding,
                &chain,
                &plan,
                &a.blockers,
            );
            set_state(db, uuid, NESTED_DESTINATION_RESTORE_CHAIN_ASSESSED, &detail)?;
            return Err(failure(
                "migration_checkpoint nested outer checkpoint capture preconditions are no longer met; nothing was suspended",
                detail,
            ));
        }
        let sidecars = json!({
            "inner_podman_metadata": inner,
            "nested_vfs_store_binding": binding,
            "rule11_destination_restore_chain": chain,
            "nested_outer_checkpoint_plan": plan,
        });
        return capture_nested(db, attempt, id, uuid, name, dir, &r, &sidecars);
    }
    let detail = json!({"observed": lc::state_view(&c), "checkpointed": c["State"]["Checkpointed"], "checkpointed_at": c["State"]["CheckpointedAt"]});
    set_state(db, uuid, "checkpoint_failed", &detail)?;
    Err(failure(
        "The reserved source is neither checkpointed by this operation nor the same running process; nothing was restarted or recaptured",
        detail,
    ))
}

fn capture_nested(
    db: &Connection,
    attempt: i64,
    id: &str,
    uuid: &str,
    name: &str,
    dir: &Path,
    r: &Reservation,
    sidecars: &Value,
) -> Result<Value, Error> {
    set_state(db, uuid, "checkpointing", &json!({"attempt": attempt, "migration_profile": "nested"}))?;
    let mut sidecars = sidecars.clone();
    nested_quiesce_inner_sidecar_for_outer_capture(name, dir, &mut sidecars)?;
    let archive = dir.join(ARCHIVE);
    if archive.exists() {
        fs::rename(&archive, dir.join(format!("{ARCHIVE}.partial-before-attempt-{attempt}")))?;
    }
    let open = |path: &Path| fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path);
    let stdout_path = dir.join(format!("checkpoint-attempt-{attempt}.stdout"));
    let stderr_path = dir.join(format!("checkpoint-attempt-{attempt}.stderr"));
    let exit = checkpoint_command(id, &r.container_id, &archive, open(&stdout_path)?, open(&stderr_path)?)?;
    if !exit.success() {
        let stderr = read_bounded(&stderr_path).unwrap_or_default();
        let observed = lc::inspect(name)?;
        if let Some(ref c) = observed {
            copy_dump_log(c, &dir.join(format!("dump-attempt-{attempt}.log")));
        }
        let detail = json!({"attempt": attempt, "exit_code": exit.code(), "stderr_tail": tail(&stderr),
            "observed": observed.as_ref().map(lc::state_view),
            "checkpointed": observed.as_ref().map(|c| c["State"]["Checkpointed"].clone()),
            "migration_profile": "nested",
            "nested_sidecars": sidecars});
        write_private(
            &dir.join(format!("failure-attempt-{attempt}.json")),
            serde_json::to_string_pretty(&detail)?.as_bytes(),
        )?;
        set_state(db, uuid, "checkpoint_failed", &detail)?;
        return Err(failure(
            "Nested outer checkpoint failed; the source was not restarted and diagnostics are preserved",
            detail,
        ));
    }
    finalize_nested(db, id, uuid, name, dir, r, false, &sidecars)
}

/// Nested-lab checkpoint: assess, durable reservation (flat-store parity), then outer capture when ready.
fn checkpoint_nested_lab(
    db: &Connection,
    attempt: i64,
    id: &str,
    uuid: &str,
    name: &str,
    b: &Binding,
    existing: Option<Value>,
) -> Result<Value, Error> {
    archive_superseded_reservation(db, uuid, id)?;
    let dir = base()?.join(id);
    match reservation(db, uuid)? {
        Some(r) if r.operation_id != id => Err(failure(
            format!(
                "Universe is already reserved by migration operation {} (state {})",
                r.operation_id,
                r.state
            ),
            json!({"reservation": r.view()}),
        )),
        Some(r) => nested_lab_resume(db, attempt, uuid, name, b, existing, r, &dir),
        None => {
            let a = assess(db, uuid, b, existing.clone(), false)?;
            if !a.blockers.is_empty() {
                return Err(failure(
                    "Checkpoint preconditions not met; nothing was reserved, suspended or written",
                    json!({
                        "migration_profile": "nested",
                        "blockers": a.blockers,
                        "facts": a.facts,
                    }),
                ));
            }
            let dir = prepare_empty_artifact_dir(id)?;
            let r = persist_checkpoint_reservation(db, uuid, id, b, &a, &dir)?;
            nested_lab_inner_reconciliation(db, attempt, uuid, name, &a.facts, &r, &dir, b, existing)
        }
    }
}

fn nested_lab_resume(
    db: &Connection,
    attempt: i64,
    uuid: &str,
    name: &str,
    b: &Binding,
    existing: Option<Value>,
    r: Reservation,
    dir: &Path,
) -> Result<Value, Error> {
    if r.state == "checkpointing" || r.state == "checkpointed" {
        return nested_lab_outer_resume(db, attempt, &r.operation_id, uuid, name, b, existing, &r, dir);
    }
    if r.state != "reserved"
        && r.state != NESTED_INNER_RECONCILED
        && r.state != NESTED_VFS_STORE_BOUND
        && r.state != NESTED_DESTINATION_RESTORE_CHAIN_ASSESSED
    {
        return Err(failure(
            format!(
                "Nested-lab checkpoint cannot resume from reservation state {}; only reserved, nested_inner_reconciled, nested_vfs_store_bound, nested_destination_restore_chain_assessed, checkpointing or checkpointed is supported",
                r.state
            ),
            json!({"reservation": r.view()}),
        ));
    }
    if r.state == NESTED_VFS_STORE_BOUND || r.state == NESTED_DESTINATION_RESTORE_CHAIN_ASSESSED {
        let inner = read_nested_inner_metadata(dir).unwrap_or(Value::Null);
        let binding = read_nested_vfs_store_binding(dir).unwrap_or(Value::Null);
        let facts = json!({"migration_profile": "nested"});
        return nested_lab_destination_restore_chain(
            db,
            attempt,
            uuid,
            name,
            &facts,
            &r,
            dir,
            &inner,
            &binding,
            b,
            existing,
        );
    }
    if r.state == NESTED_INNER_RECONCILED {
        let inner = read_nested_inner_metadata(dir).unwrap_or(Value::Null);
        let facts = json!({"migration_profile": "nested"});
        return nested_lab_vfs_store_binding(db, attempt, uuid, name, &facts, &r, dir, &inner, b, existing);
    }
    let Some(c) = existing.clone() else {
        let detail = json!({"reason": "reserved source container is absent"});
        set_state(db, uuid, "checkpoint_failed", &detail)?;
        return Err(failure(
            "The reserved source container is absent; nothing can be finalized or recaptured",
            detail,
        ));
    };
    let a = assess(db, uuid, b, Some(c), false)?;
    if !a.blockers.is_empty() {
        let detail = json!({
            "migration_profile": "nested",
            "blockers": a.blockers,
            "facts": a.facts,
        });
        set_state(db, uuid, "checkpoint_failed", &detail)?;
        return Err(failure(
            "Checkpoint preconditions are no longer met; nothing was suspended",
            detail,
        ));
    }
    nested_lab_inner_reconciliation(db, attempt, uuid, name, &a.facts, &r, dir, b, existing)
}

fn checkpoint_flat_store(
    db: &Connection,
    attempt: i64,
    id: &str,
    uuid: &str,
    name: &str,
    b: &Binding,
    existing: Option<Value>,
) -> Result<Value, Error> {
    let dir = base()?.join(id);
    archive_superseded_reservation(db, uuid, id)?;
    match reservation(db, uuid)? {
        Some(r) if r.operation_id != id => Err(failure(
            format!(
                "Universe is already reserved by migration operation {} (state {})",
                r.operation_id, r.state
            ),
            json!({"reservation": r.view()}),
        )),
        Some(r) => resume(db, attempt, id, uuid, name, b, existing, r, &dir),
        None => {
            let a = assess(db, uuid, b, existing, false)?;
            if !a.blockers.is_empty() {
                return Err(failure(
                    "Checkpoint preconditions not met; nothing was reserved, suspended or written",
                    json!({"blockers": a.blockers, "facts": a.facts}),
                ));
            }
            let dir = prepare_empty_artifact_dir(id)?;
            let r = persist_checkpoint_reservation(db, uuid, id, b, &a, &dir)?;
            capture(db, attempt, id, uuid, name, &dir, &r)
        }
    }
}

pub(crate) fn checkpoint(
    db: &Connection,
    attempt: i64,
    id: &str,
    uuid: &str,
    name: &str,
    b: &Binding,
    existing: Option<Value>,
) -> Result<Value, Error> {
    match b.profile {
        MigrationProfile::Nested => checkpoint_nested_lab(db, attempt, id, uuid, name, b, existing),
        MigrationProfile::Flat => checkpoint_flat_store(db, attempt, id, uuid, name, b, existing),
    }
}

#[allow(clippy::too_many_arguments)]
fn resume(
    db: &Connection,
    attempt: i64,
    id: &str,
    uuid: &str,
    name: &str,
    b: &Binding,
    existing: Option<Value>,
    r: Reservation,
    dir: &Path,
) -> Result<Value, Error> {
    if unit_busy(&scope_unit(id)) {
        return Err(failure(
            "The checkpoint scope of this operation has not finished, or its state cannot be queried; retry after it finishes",
            json!({"reservation": r.view(), "scope": scope_unit(id)}),
        ));
    }
    let Some(c) = existing else {
        let detail = json!({"reason": "reserved source container is absent"});
        set_state(db, uuid, "checkpoint_failed", &detail)?;
        return Err(failure(
            "The reserved source container is absent; nothing can be finalized or recaptured",
            detail,
        ));
    };
    let checkpointed_after_reservation = c["State"]["Checkpointed"] == true
        && c["State"]["CheckpointedAt"]
            .as_str()
            .and_then(lc::epoch)
            .is_some_and(|t| t >= r.created_at);
    if c["Id"].as_str() == Some(r.container_id.as_str()) && checkpointed_after_reservation && !lc::process_active(&c) {
        // A previous attempt completed the checkpoint but was interrupted before recording it.
        return finalize(db, id, uuid, name, dir, &r, true);
    }
    let same_process = c["Id"].as_str() == Some(r.container_id.as_str())
        && lc::status(&c) == "running"
        && c["State"]["StartedAt"].as_str() == Some(r.started_at.as_str())
        && c["State"]["Checkpointed"] != true;
    if same_process {
        // The earlier attempt did not suspend this process: capture the same process, never a newer one.
        let a = assess(db, uuid, b, Some(c), true)?;
        if !a.blockers.is_empty() {
            let detail = json!({"blockers": a.blockers, "facts": a.facts});
            set_state(db, uuid, "checkpoint_failed", &detail)?;
            return Err(failure("Checkpoint preconditions are no longer met; nothing was suspended", detail));
        }
        return capture(db, attempt, id, uuid, name, dir, &r);
    }
    let detail = json!({"observed": lc::state_view(&c), "checkpointed": c["State"]["Checkpointed"], "checkpointed_at": c["State"]["CheckpointedAt"]});
    set_state(db, uuid, "checkpoint_failed", &detail)?;
    Err(failure(
        "The reserved source is neither checkpointed by this operation nor the same running process; nothing was restarted or recaptured",
        detail,
    ))
}

fn capture(db: &Connection, attempt: i64, id: &str, uuid: &str, name: &str, dir: &Path, r: &Reservation) -> Result<Value, Error> {
    set_state(db, uuid, "checkpointing", &json!({"attempt": attempt}))?;
    let archive = dir.join(ARCHIVE);
    if archive.exists() {
        // Preserve a partial archive of an earlier attempt as diagnostics; never overwrite it.
        fs::rename(&archive, dir.join(format!("{ARCHIVE}.partial-before-attempt-{attempt}")))?;
    }
    let open = |path: &Path| fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path);
    let stdout_path = dir.join(format!("checkpoint-attempt-{attempt}.stdout"));
    let stderr_path = dir.join(format!("checkpoint-attempt-{attempt}.stderr"));
    let exit = checkpoint_command(id, &r.container_id, &archive, open(&stdout_path)?, open(&stderr_path)?)?;
    if !exit.success() {
        let stderr = read_bounded(&stderr_path).unwrap_or_default();
        let observed = lc::inspect(name)?;
        if let Some(ref c) = observed {
            copy_dump_log(c, &dir.join(format!("dump-attempt-{attempt}.log")));
        }
        let detail = json!({"attempt": attempt, "exit_code": exit.code(), "stderr_tail": tail(&stderr),
            "observed": observed.as_ref().map(lc::state_view),
            "checkpointed": observed.as_ref().map(|c| c["State"]["Checkpointed"].clone())});
        write_private(
            &dir.join(format!("failure-attempt-{attempt}.json")),
            serde_json::to_string_pretty(&detail)?.as_bytes(),
        )?;
        set_state(db, uuid, "checkpoint_failed", &detail)?;
        return Err(failure(
            "Checkpoint failed; the source was not restarted and diagnostics are preserved",
            detail,
        ));
    }
    finalize(db, id, uuid, name, dir, r, false)
}

/// Copy one of Podman's CRIU logs (`State.<state_key>`), only from the container's own static directory.
pub(crate) fn copy_podman_log(c: &Value, state_key: &str, file: &str, target: &Path) -> bool {
    let expected = c["StaticDir"].as_str().map(|d| format!("{d}/{file}"));
    match (c["State"][state_key].as_str(), expected) {
        (Some(log), Some(expected)) if log == expected && log.starts_with("/var/lib/containers/storage/") => read_bounded(Path::new(log))
            .ok()
            .and_then(|bytes| write_private(target, &bytes).ok())
            .is_some(),
        _ => false,
    }
}
fn copy_dump_log(c: &Value, target: &Path) -> bool {
    copy_podman_log(c, "CheckpointLog", "dump.log", target)
}

fn finalize(db: &Connection, id: &str, uuid: &str, name: &str, dir: &Path, r: &Reservation, resumed: bool) -> Result<Value, Error> {
    let fail = |db: &Connection, reason: String, observed: Value| -> Result<Value, Error> {
        let detail = json!({"reason": reason, "observed": observed});
        set_state(db, uuid, "checkpoint_failed", &detail)?;
        Err(failure(
            format!("Checkpoint could not be verified: {reason}; the source was not restarted"),
            detail,
        ))
    };
    let Some(c) = lc::inspect(name)? else {
        return fail(db, "source container disappeared".into(), Value::Null);
    };
    if c["Id"].as_str() != Some(r.container_id.as_str()) || c["State"]["Checkpointed"] != true || lc::process_active(&c) {
        return fail(
            db,
            "source is not the reserved container in a checkpointed, stopped state".into(),
            lc::state_view(&c),
        );
    }
    let archive = dir.join(ARCHIVE);
    let bytes = fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
    if bytes == 0 {
        return fail(db, "archive is missing or empty".into(), lc::state_view(&c));
    }
    // Podman creates the archive; its mode is set here rather than inherited from a unit's umask.
    fs::set_permissions(&archive, fs::Permissions::from_mode(0o600))?;
    let listing = Command::new("/usr/bin/tar").arg("-tf").arg(&archive).output()?;
    let entries = String::from_utf8_lossy(&listing.stdout);
    if !listing.status.success()
        || !["config.dump", "spec.dump", "checkpoint/inventory.img"]
            .iter()
            .all(|e| entries.lines().any(|l| l == *e))
    {
        return fail(
            db,
            "archive is unreadable or lacks the expected checkpoint entries".into(),
            lc::state_view(&c),
        );
    }
    let log = dir.join("dump.log");
    if !copy_dump_log(&c, &log) {
        return fail(db, "CRIU dump log is unavailable".into(), lc::state_view(&c));
    }
    let log_text = fs::read_to_string(&log).unwrap_or_default();
    if !log_text.contains(&format!("(gitid {RUNTIME_GIT_ID})")) || !log_text.contains("Dumping finished successfully") {
        return fail(
            db,
            "dump log does not show a successful dump by the qualified private runtime".into(),
            lc::state_view(&c),
        );
    }
    let mut blockers = vec![];
    let runtime = runtime_facts(&mut blockers);
    let archive_sha256 = sha256(&archive)?;
    let manifest = json!({
        "format": "podmesh-source-checkpoint/1",
        "operation_id": id, "universe_uuid": uuid, "container_id": r.container_id, "image_id": r.image_id,
        "source_host_uuid": r.source_host, "destination_host_uuid": r.destination,
        "container_started_at": r.started_at, "reserved_at": r.created_at,
        "checkpointed_at": c["State"]["CheckpointedAt"],
        "archive": {"file": ARCHIVE, "bytes": bytes, "sha256": archive_sha256, "compression": "zstd"},
        "dump_log": {"file": "dump.log", "sha256": sha256(&log)?},
        "runtime": runtime, "runtime_blockers_at_finalization": blockers,
        "scope": SCOPE, "authority": AUTHORITY,
    });
    write_private(&dir.join(MANIFEST), serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    let manifest_sha256 = sha256(&dir.join(MANIFEST))?;
    set_state(
        db,
        uuid,
        "checkpointed",
        &json!({"archive_sha256": archive_sha256, "manifest_sha256": manifest_sha256}),
    )?;
    let mut source = lc::state_view(&c);
    source["checkpointed"] = json!(true);
    source["checkpointed_at"] = c["State"]["CheckpointedAt"].clone();
    Ok(json!({
        "status": "verified", "operation": "migration_checkpoint", "universe_uuid": uuid,
        "container_id": r.container_id, "image_id": r.image_id,
        "source_host_uuid": r.source_host, "destination_host_uuid": r.destination,
        "artifact_directory": dir,
        "archive": {"file": ARCHIVE, "bytes": bytes, "sha256": archive_sha256},
        "manifest": {"file": MANIFEST, "sha256": manifest_sha256},
        "runtime_git_id": RUNTIME_GIT_ID,
        "source_observed": source,
        "finalized_after_interruption": resumed,
        "reservation": {"state": "checkpointed"},
        "authority": AUTHORITY, "scope": SCOPE,
    }))
}

fn finalize_nested(
    db: &Connection,
    id: &str,
    uuid: &str,
    name: &str,
    dir: &Path,
    r: &Reservation,
    resumed: bool,
    sidecars: &Value,
) -> Result<Value, Error> {
    let fail = |db: &Connection, reason: String, observed: Value| -> Result<Value, Error> {
        let detail = json!({"reason": reason, "observed": observed, "migration_profile": "nested"});
        set_state(db, uuid, "checkpoint_failed", &detail)?;
        Err(failure(
            format!("Nested outer checkpoint could not be verified: {reason}; the source was not restarted"),
            detail,
        ))
    };
    let Some(c) = lc::inspect(name)? else {
        return fail(db, "source container disappeared".into(), Value::Null);
    };
    if c["Id"].as_str() != Some(r.container_id.as_str()) || c["State"]["Checkpointed"] != true || lc::process_active(&c) {
        return fail(
            db,
            "source is not the reserved container in a checkpointed, stopped state".into(),
            lc::state_view(&c),
        );
    }
    let archive = dir.join(ARCHIVE);
    let bytes = fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
    if bytes == 0 {
        return fail(db, "archive is missing or empty".into(), lc::state_view(&c));
    }
    fs::set_permissions(&archive, fs::Permissions::from_mode(0o600))?;
    let listing = Command::new("/usr/bin/tar").arg("-tf").arg(&archive).output()?;
    let entries = String::from_utf8_lossy(&listing.stdout);
    if !listing.status.success()
        || !["config.dump", "spec.dump", "checkpoint/inventory.img"]
            .iter()
            .all(|e| entries.lines().any(|l| l == *e))
    {
        return fail(
            db,
            "archive is unreadable or lacks the expected checkpoint entries".into(),
            lc::state_view(&c),
        );
    }
    let log = dir.join("dump.log");
    if !copy_dump_log(&c, &log) {
        return fail(db, "CRIU dump log is unavailable".into(), lc::state_view(&c));
    }
    let log_text = fs::read_to_string(&log).unwrap_or_default();
    if !log_text.contains(&format!("(gitid {RUNTIME_GIT_ID})")) || !log_text.contains("Dumping finished successfully") {
        return fail(
            db,
            "dump log does not show a successful dump by the qualified private runtime".into(),
            lc::state_view(&c),
        );
    }
    let mut blockers = vec![];
    let runtime = runtime_facts(&mut blockers);
    let archive_sha256 = sha256(&archive)?;
    let inner_sha = |file: &str| -> Value {
        let path = dir.join(file);
        if path.is_file() {
            json!({"file": file, "sha256": sha256(&path).unwrap_or_default()})
        } else {
            json!({"file": file, "present": false})
        }
    };
    let manifest = json!({
        "format": NESTED_MANIFEST_FORMAT,
        "migration_profile": "nested",
        "operation_id": id, "universe_uuid": uuid, "container_id": r.container_id, "image_id": r.image_id,
        "source_host_uuid": r.source_host, "destination_host_uuid": r.destination,
        "container_started_at": r.started_at, "reserved_at": r.created_at,
        "checkpointed_at": c["State"]["CheckpointedAt"],
        "archive": {"file": ARCHIVE, "bytes": bytes, "sha256": archive_sha256, "compression": "zstd"},
        "dump_log": {"file": "dump.log", "sha256": sha256(&log)?},
        "runtime": runtime, "runtime_blockers_at_finalization": blockers,
        "nested_sidecars": {
            "inner_podman_metadata": inner_sha(INNER_PODMAN_METADATA),
            "nested_vfs_store_binding": inner_sha(NESTED_VFS_STORE_BINDING),
            "rule11_destination_restore_chain": inner_sha(RULE11_DESTINATION_RESTORE_CHAIN),
            "nested_outer_checkpoint_plan": inner_sha(NESTED_OUTER_CHECKPOINT_PLAN),
        },
        "destination_restore": {
            "nested_profile": "hooks_assessed_on_destination_at_preflight",
            "implementation_gaps": NESTED_LAB_GAPS_AFTER_DESTINATION_RESTORE_HOOKS,
        },
        "scope": SCOPE_NESTED_OUTER_CHECKPOINT, "authority": AUTHORITY,
    });
    write_private(&dir.join(MANIFEST), serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    let manifest_sha256 = sha256(&dir.join(MANIFEST))?;
    set_state(
        db,
        uuid,
        "checkpointed",
        &json!({
            "archive_sha256": archive_sha256,
            "manifest_sha256": manifest_sha256,
            "migration_profile": "nested",
            "implementation_gaps": NESTED_LAB_GAPS_UNTIL_TWO_HOST_MOVE_COMPLETE,
        }),
    )?;
    let mut source = lc::state_view(&c);
    source["checkpointed"] = json!(true);
    source["checkpointed_at"] = c["State"]["CheckpointedAt"].clone();
    let facts = json!({"migration_profile": "nested"});
    let after = nested_lab_after_outer_checkpoint_detail(
        uuid,
        &facts,
        r,
        &sidecars["inner_podman_metadata"],
        &sidecars["nested_vfs_store_binding"],
        &sidecars["rule11_destination_restore_chain"],
        &sidecars["nested_outer_checkpoint_plan"],
    );
    Ok(json!({
        "status": "verified", "operation": "migration_checkpoint", "universe_uuid": uuid,
        "migration_profile": "nested",
        "container_id": r.container_id, "image_id": r.image_id,
        "source_host_uuid": r.source_host, "destination_host_uuid": r.destination,
        "artifact_directory": dir,
        "archive": {"file": ARCHIVE, "bytes": bytes, "sha256": archive_sha256},
        "manifest": {"file": MANIFEST, "sha256": manifest_sha256, "format": NESTED_MANIFEST_FORMAT},
        "runtime_git_id": RUNTIME_GIT_ID,
        "source_observed": source,
        "finalized_after_interruption": resumed,
        "reservation": {"state": "checkpointed"},
        "checkpoint_phase": "nested_outer_checkpoint_captured",
        "implementation_gaps": NESTED_LAB_GAPS_UNTIL_TWO_HOST_MOVE_COMPLETE,
        "nested_checkpoint_detail": after,
        "authority": AUTHORITY, "scope": SCOPE_NESTED_OUTER_CHECKPOINT,
    }))
}

/// Fresh verification of preserved artifacts, for historical replays and status.
pub(crate) fn verify_artifacts(id: &str, archive_sha256: Option<&str>, manifest_sha256: Option<&str>) -> Result<Value, Error> {
    let dir = base()?.join(id);
    let archive = dir.join(ARCHIVE);
    let manifest = dir.join(MANIFEST);
    let archive_now = if archive.is_file() { Some(sha256(&archive)?) } else { None };
    let manifest_now = if manifest.is_file() { Some(sha256(&manifest)?) } else { None };
    Ok(json!({
        "observed_at": crate::now(),
        "archive_present": archive_now.is_some(),
        "archive_sha256": archive_now,
        "archive_sha256_matches": archive_now.is_some() && archive_now.as_deref() == archive_sha256,
        "manifest_sha256_matches": manifest_now.is_some() && manifest_now.as_deref() == manifest_sha256,
    }))
}

/// Reservation states that are a finished record rather than a decision still owed to someone.
const SETTLED: [&str; 4] = [RELEASED, ABANDONED, COLLECTED, "transferred"];
/// What an outside observer needs in order to report on this universe without running anything: what is
/// unresolved and since when, what a failed restore left behind, and the room left where it would write.
///
/// Every fact carries the time it was observed. What cannot be established is reported as unknown, with
/// the reason: an absent or unreadable observation must never read as a healthy one. In particular, once a
/// container's cgroups are gone, the surviving-process question can only be answered by a command-line
/// scan, which is a hint and not proof, and it is labelled as such.
fn watch_view(r: Option<&Reservation>, claims: &[Value], authorizations: &[Value]) -> Result<Value, Error> {
    let now = crate::now();
    let unresolved: Vec<Value> = claims
        .iter()
        .filter(|k| matches!(k["state"].as_str(), Some("restoring") | Some("restore_failed")))
        .map(|k| {
            let container = k["container_id"].as_str();
            let claimed_at = k["created_at"].as_i64().unwrap_or(0);
            let processes = match container {
                Some(id) => crate::cleanup::runtime_processes(id, claimed_at),
                // A claim that never recorded a container may still have created one: nothing here can
                // say it did not, so the honest answer is unknown, not zero.
                None => json!({"observed_at": now, "known": false,
                    "reason": "this claim recorded no container, so no cgroup can be read for it; a container may still exist under the universe name"}),
            };
            json!({"authorization_id": k["authorization_id"], "state": k["state"], "since": k["updated_at"],
                "claimed_at": k["created_at"], "container_id": container, "runtime_processes": processes,
                "allowance_bytes": k["detail"]["prevention"]["allowance_bytes"],
                "prevention_stopped_the_attempt": k["detail"]["prevention"]["stopped"]})
        })
        .collect();
    let open: Vec<Value> = authorizations
        .iter()
        .filter(|a| a["state"].as_str() == Some("issued"))
        .map(|a| {
            json!({"authorization_id": a["authorization_id"], "state": a["state"], "since": a["updated_at"],
            "destination_host_uuid": a["destination_host_uuid"]})
        })
        .collect();
    // Where a restore writes, and how much room is left there against what an unresolved attempt was
    // allowed. A graph root that cannot be read is unknown, not empty.
    let graph_root = lc::podman(lc::QUICK, &["info", "--format", "{{.Store.GraphRoot}}"])
        .map(|g| g.trim().to_string())
        .ok();
    let graph = match graph_root {
        Some(ref path) => json!({"observed_at": now, "known": true, "path": path,
            "available_bytes": available_bytes(Path::new(path)),
            "is_the_qualified_default_store": path == CONTAINER_STORAGE,
            "required_bytes_for_unresolved_attempts": unresolved.iter().map(|k| k["allowance_bytes"].clone()).collect::<Vec<_>>()}),
        None => json!({"observed_at": now, "known": false, "reason": "Podman did not report its graph root"}),
    };
    Ok(json!({
        "observed_at": now,
        "reservation": r.map(|r| json!({"state": r.state, "since": r.updated_at, "created_at": r.created_at,
            "blocks_generic_operations": r.state != RELEASED && r.state != COLLECTED,
            "awaiting_decision": !SETTLED.contains(&r.state.as_str())})),
        "unresolved_restore_claims": unresolved,
        "open_transfer_authorizations": open,
        "graph_root": graph,
        "note": "read-only facts for an observer. A count of runtime processes is authoritative only when it comes from cgroup residency (source cgroup_residency, authorizes_reclaim true); a cmdline_fallback count is a hint about a container whose cgroups are already gone, and an unknown is not a zero.",
    }))
}

/// Read-only: reservation, fresh observation, artifact verification, transfer authorizations, restore claims,
/// archived reservations, release preconditions and the observer summary.
pub(crate) fn status(db: &Connection, request: &Value) -> Result<Value, Error> {
    let uuid = lc::text(request, "universe_uuid")?;
    if !lc::is_uuid(uuid) {
        return Err("Invalid universe UUID".into());
    }
    ensure_schema(db)?;
    let r = reservation(db, uuid)?;
    let current = lc::observe(uuid)?;
    let authorizations = crate::transfer::authorizations_view(db, uuid)?;
    let claims = crate::restore::claims_view(db, uuid)?;
    let history = history_view(db, uuid)?;
    let collected = tombstone(db, uuid)?;
    let Some(r) = r else {
        let watch = watch_view(None, &claims, &authorizations)?;
        return Ok(
            json!({"universe_uuid": uuid, "reservation": null, "current": current, "transfer_authorizations": authorizations,
            "restore_claims": claims, "reservation_history": history, "tombstone": collected, "watch": watch}),
        );
    };
    let issued = authorizations
        .iter()
        .filter(|a| a["checkpoint_operation_id"].as_str() == Some(r.operation_id.as_str()))
        .count();
    let detail: Value = r
        .detail
        .as_deref()
        .and_then(|d| serde_json::from_str(d).ok())
        .unwrap_or(Value::Null);
    let artifacts = verify_artifacts(
        &r.operation_id,
        detail["archive_sha256"].as_str(),
        detail["manifest_sha256"].as_str(),
    )?;
    let container = lc::inspect(&format!("podmesh-{uuid}"))?;
    let same = container
        .as_ref()
        .is_some_and(|c| c["Id"].as_str() == Some(r.container_id.as_str()));
    let observed = json!({
        "source_container_present": container.is_some(),
        "same_reserved_container": same,
        "source_not_running": container.as_ref().is_some_and(|c| !lc::process_active(c)),
        "source_checkpointed": container.as_ref().is_some_and(|c| c["State"]["Checkpointed"] == true),
        "transfer_authorizations_issued_for_reservation": issued,
    });
    let recovery = crate::recovery::availability(db, uuid, &r, container.as_ref(), issued, &artifacts)?;
    let watch = watch_view(Some(&r), &claims, &authorizations)?;
    Ok(json!({
        "universe_uuid": uuid,
        "reservation": r.view(),
        "current": current,
        "artifacts": artifacts,
        "transfer_authorizations": authorizations,
        "restore_claims": claims,
        "reservation_history": history,
        "tombstone": collected,
        "collection_history": collection_history(db, uuid)?,
        "recovery": recovery,
        "watch": watch,
        "release": {
            "permitted": recovery["release"]["permitted"],
            "operation_available": true,
            "reason": recovery["release"]["reason"],
            "blockers": recovery["release"]["blockers"],
            "preconditions_observed": observed,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        migration_shape_blockers, nested_lab_capture_pending_detail,
        nested_destination_restore_hooks_assess, nested_outer_checkpoint_capture_assess,
        nested_rule11_destination_restore_chain_assess, parse_nested_counter_log_line, MigrationProfile,
        NESTED_LAB_CHECKPOINT_GAPS, NESTED_LAB_GAPS_AFTER_DESTINATION_RESTORE_CHAIN,
        NESTED_LAB_GAPS_AFTER_DESTINATION_RESTORE_HOOKS, NESTED_LAB_GAPS_AFTER_INNER_RECONCILIATION,
        NESTED_LAB_GAPS_AFTER_OUTER_CHECKPOINT_CAPTURE, NESTED_LAB_GAPS_AFTER_VFS_STORE_BINDING,
        NESTED_MANIFEST_FORMAT, Reservation,
    };
    use serde_json::json;

    fn fixture(privileged: bool) -> serde_json::Value {
        json!({
            "HostConfig": {"NetworkMode": "none", "Privileged": privileged},
            "Mounts": [],
            "Config": {"Tty": false}
        })
    }

    #[test]
    fn flat_refuses_privileged_outer() {
        let blockers = migration_shape_blockers(&fixture(true), MigrationProfile::Flat);
        assert!(
            blockers
                .iter()
                .any(|b| b.contains("privileged") || b.contains("Privileged")),
            "{blockers:?}"
        );
        assert!(migration_shape_blockers(&fixture(false), MigrationProfile::Flat).is_empty());
    }

    #[test]
    fn nested_requires_privileged_outer() {
        assert!(migration_shape_blockers(&fixture(false), MigrationProfile::Nested)
            .iter()
            .any(|b| b.contains("privileged outer")));
        assert!(migration_shape_blockers(&fixture(true), MigrationProfile::Nested).is_empty());
    }

    #[test]
    fn nested_still_refuses_network_and_mounts() {
        let mut c = fixture(true);
        c["HostConfig"]["NetworkMode"] = json!("bridge");
        assert!(
            migration_shape_blockers(&c, MigrationProfile::Nested)
                .iter()
                .any(|b| b.contains("network"))
        );
        c = fixture(true);
        c["Mounts"] = json!([{"Type": "bind"}]);
        assert!(
            migration_shape_blockers(&c, MigrationProfile::Nested)
                .iter()
                .any(|b| b.contains("mount"))
        );
    }

    /// Outer shape from `podmesh-lab/records/slice-b-move-2026-10-07/nested-shape-inspect.txt`
    /// (`Privileged=true`, `network=none`, no mounts).
    #[test]
    fn slice_b_rule11_outer_shape_passes_nested_preflight_assess() {
        let c = fixture(true);
        assert!(migration_shape_blockers(&c, MigrationProfile::Nested).is_empty());
        assert!(!migration_shape_blockers(&c, MigrationProfile::Flat).is_empty());
    }

    #[test]
    fn nested_lab_pending_detail_lists_gaps_and_reservation() {
        let r = Reservation {
            operation_id: "op-nested-1".into(),
            container_id: "abc".repeat(21),
            image_id: "img".into(),
            source_host: "00000000-0000-0000-0000-000000000001".into(),
            destination: "00000000-0000-0000-0000-000000000002".into(),
            started_at: "2026-10-07T00:00:00Z".into(),
            state: "reserved".into(),
            created_at: 1,
            updated_at: 1,
            detail: None,
        };
        let facts = json!({"migration_profile": "nested"});
        let detail = nested_lab_capture_pending_detail("11111111-1111-1111-1111-111111111111", &facts, &r);
        assert_eq!(detail["universe_uuid"], "11111111-1111-1111-1111-111111111111");
        assert_eq!(detail["reservation"]["state"], "reserved");
        assert_eq!(
            detail["implementation_gaps"].as_array().map(|a| a.len()),
            Some(NESTED_LAB_CHECKPOINT_GAPS.len())
        );
        assert!(detail["effects"]
            .as_str()
            .unwrap_or("")
            .contains("preflight.json"));
    }

    #[test]
    fn nested_counter_log_line_parses_kit_tail() {
        let (uuid, counter) =
            parse_nested_counter_log_line("1759850000 550e8400-e29b-41d4-a716-446655440000 42")
                .unwrap();
        assert_eq!(uuid, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(counter, 42);
        assert!(parse_nested_counter_log_line("only two fields").is_err());
    }

    #[test]
    fn nested_lab_gaps_shrink_after_inner_reconciliation() {
        assert_eq!(NESTED_LAB_GAPS_AFTER_INNER_RECONCILIATION.len(), 3);
        assert!(!NESTED_LAB_GAPS_AFTER_INNER_RECONCILIATION
            .iter()
            .any(|g| *g == "inner_podman_metadata_reconciliation"));
        assert_eq!(
            NESTED_LAB_CHECKPOINT_GAPS.len(),
            NESTED_LAB_GAPS_AFTER_INNER_RECONCILIATION.len() + 1
        );
    }

    #[test]
    fn nested_lab_gaps_shrink_after_vfs_store_binding() {
        assert_eq!(NESTED_LAB_GAPS_AFTER_VFS_STORE_BINDING.len(), 2);
        assert_eq!(
            NESTED_LAB_GAPS_AFTER_VFS_STORE_BINDING[0],
            "rule11_destination_restore_chain"
        );
        assert!(!NESTED_LAB_GAPS_AFTER_VFS_STORE_BINDING
            .iter()
            .any(|g| *g == "nested_vfs_store_binding"));
        assert_eq!(
            NESTED_LAB_GAPS_AFTER_INNER_RECONCILIATION.len(),
            NESTED_LAB_GAPS_AFTER_VFS_STORE_BINDING.len() + 1
        );
    }

    #[test]
    fn nested_lab_gaps_shrink_after_destination_restore_chain() {
        assert_eq!(NESTED_LAB_GAPS_AFTER_DESTINATION_RESTORE_CHAIN.len(), 0);
        assert!(!NESTED_LAB_GAPS_AFTER_DESTINATION_RESTORE_CHAIN
            .iter()
            .any(|g| *g == "nested_outer_checkpoint_capture"));
        assert_eq!(NESTED_LAB_GAPS_AFTER_VFS_STORE_BINDING[1], "nested_outer_checkpoint_capture");
    }

    #[test]
    fn nested_lab_gaps_after_outer_checkpoint_name_destination_chain() {
        assert_eq!(NESTED_LAB_GAPS_AFTER_OUTER_CHECKPOINT_CAPTURE.len(), 2);
        assert_eq!(
            NESTED_LAB_GAPS_AFTER_OUTER_CHECKPOINT_CAPTURE[0],
            "nested_destination_restore_hooks"
        );
        assert_eq!(
            NESTED_LAB_GAPS_AFTER_OUTER_CHECKPOINT_CAPTURE[1],
            "two_host_checkpoint_carry_restore_complete"
        );
    }

    #[test]
    fn nested_lab_gaps_shrink_after_destination_restore_hooks() {
        assert_eq!(NESTED_LAB_GAPS_AFTER_DESTINATION_RESTORE_HOOKS.len(), 0);
        assert!(!NESTED_LAB_GAPS_AFTER_DESTINATION_RESTORE_HOOKS
            .iter()
            .any(|g| *g == "nested_destination_restore_hooks"));
    }

    #[test]
    fn nested_inner_run_state_reconcile_script_refreshes_libpod_alive() {
        assert!(super::NESTED_INNER_RUN_STATE_RECONCILE_PY.contains("/run/libpod/alive"));
        assert!(super::NESTED_INNER_RUN_STATE_RECONCILE_PY.contains("boot_id"));
    }

    #[test]
    fn nested_destination_outer_restore_blockers_name_inner_vfs_overlay_gap() {
        let verified = json!({"status": "verified", "proof": {"counter": 1}});
        assert!(!super::nested_destination_outer_restore_blockers(&verified).is_empty());
        let quiesced = json!({
            "status": "verified",
            "proof": {"counter": 1},
            "outer_capture_quiesce": {"status": "verified"},
        });
        assert!(super::nested_destination_outer_restore_blockers(&quiesced).is_empty());
        assert!(super::nested_destination_outer_restore_blockers(&json!({"status": "refused"})).is_empty());
    }

    #[test]
    fn nested_destination_hooks_assess_requires_manifest_sidecars_and_privileged_archive() {
        let manifest = json!({
            "format": NESTED_MANIFEST_FORMAT,
            "migration_profile": "nested",
            "nested_sidecars": {
                "inner_podman_metadata": {"file": "inner_podman_metadata.json", "sha256": "a".repeat(64)},
                "nested_vfs_store_binding": {"file": "nested_vfs_store_binding.json", "sha256": "b".repeat(64)},
                "rule11_destination_restore_chain": {"file": "rule11_destination_restore_chain.json", "sha256": "c".repeat(64)},
                "nested_outer_checkpoint_plan": {"file": "nested_outer_checkpoint_plan.json", "sha256": "d".repeat(64)},
            }
        });
        let archive = json!({"hostConfig": {"privileged": true}});
        let inbox = json!({
            "inner_podman_metadata.json": {"sha256": "a".repeat(64)},
            "nested_vfs_store_binding.json": {"sha256": "b".repeat(64)},
            "rule11_destination_restore_chain.json": {"sha256": "c".repeat(64)},
            "nested_outer_checkpoint_plan.json": {"sha256": "d".repeat(64)},
        });
        let doc = nested_destination_restore_hooks_assess(&manifest, &archive, &inbox).unwrap();
        assert_eq!(doc["status"], "verified");
        assert_eq!(doc["checkpoint_phase"], "nested_destination_restore_hooks");
        assert_eq!(
            doc["api_hooks"]["migration_restore"]["nested_profile"],
            "privileged_outer_restore_plus_inner_podman_reconcile_after_outer_verified"
        );
        assert!(nested_destination_restore_hooks_assess(
            &manifest,
            &json!({"hostConfig": {"privileged": false}}),
            &inbox,
        )
        .is_err());
    }

    #[test]
    fn nested_outer_checkpoint_capture_assess_requires_verified_sidecars() {
        let binding = json!({"status": "verified"});
        let inner = json!({"status": "verified"});
        let chain = json!({"status": "verified"});
        let doc = nested_outer_checkpoint_capture_assess(&chain, &binding, &inner).unwrap();
        assert_eq!(doc["status"], "verified");
        assert_eq!(doc["checkpoint_phase"], "nested_outer_checkpoint_capture");
        assert!(nested_outer_checkpoint_capture_assess(
            &json!({"status": "refused"}),
            &binding,
            &inner,
        )
        .is_err());
    }

    #[test]
    fn nested_rule11_destination_chain_assess_requires_verified_artifacts() {
        let binding = json!({"status": "verified"});
        let inner = json!({"status": "verified"});
        let doc = nested_rule11_destination_restore_chain_assess(
            "00000000-0000-0000-0000-000000000001",
            "00000000-0000-0000-0000-000000000002",
            &binding,
            &inner,
        )
        .unwrap();
        assert_eq!(doc["status"], "verified");
        assert_eq!(doc["checkpoint_phase"], "rule11_destination_restore_chain");
        assert!(doc["api_hooks"]["migration_restore"]["nested_profile"]
            .as_str()
            .unwrap()
            .contains("unimplemented"));
        assert!(nested_rule11_destination_restore_chain_assess(
            "00000000-0000-0000-0000-000000000001",
            "00000000-0000-0000-0000-000000000001",
            &binding,
            &inner,
        )
        .is_err());
    }
}
