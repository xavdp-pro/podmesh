//! Experimental source-side migration preparation: preflight, reservation and checkpoint.
//!
//! Scope: the default rootful Podman store; network-disabled, mount-free containers owned by this
//! host's journal whose processes are musl-based; checkpoint with the separately packaged
//! podmesh-vzcriu runtime through its private-path shim, never the distribution CRIU.
//! Transfer authorization and completion live in `transfer.rs`, destination restore in `restore.rs`
//! (docs/MIGRATION-PROTOCOL.md). Reservation release and abandonment are not implemented.
//! A checkpoint result never authorizes a restore: only a transfer authorization issues a handoff.
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
const AUTHORITY: &str = "This checkpoint does not authorize restore on any host and does not release the reservation. Only migration_authorize_transfer issues a handoff, and only a verified destination outcome bound to it ends the reservation.";
const RELEASE_GAP: &str = "No release operation exists in this version (lot M3). Releasing would let the source universe run again; the protocol permits it only when no transfer authorization was ever issued for the reservation. Restarting the application would start it fresh, not resume the checkpointed memory.";

/// Identity binding carried by every migration request.
pub(crate) struct Binding<'a> {
    pub container_id: &'a str,
    pub image: &'a str,
    pub source_host: &'a str,
    pub destination: &'a str,
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
         archived_at INTEGER NOT NULL, archived_by_operation TEXT NOT NULL);",
    )?;
    Ok(())
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
/// Moves a `transferred` reservation to history when a verified restore brings the universe back to this host;
/// the caller holds the transaction. Returns whether a row was archived.
pub(crate) fn archive_transferred(db: &Connection, uuid: &str, operation: &str) -> Result<bool, Error> {
    let moved = db.execute(
        "INSERT INTO migration_reservation_history(universe_uuid,operation_id,container_id,image_id,source_host_uuid,destination_host_uuid,
         container_started_at,state,created_at,updated_at,detail,archived_at,archived_by_operation)
         SELECT universe_uuid,operation_id,container_id,image_id,source_host_uuid,destination_host_uuid,container_started_at,state,created_at,
         updated_at,detail,?2,?3 FROM migration_reservations WHERE universe_uuid=?1 AND state='transferred'",
        params![uuid, crate::now() as i64, operation],
    )?;
    db.execute(
        "DELETE FROM migration_reservations WHERE universe_uuid=?1 AND state='transferred'",
        [uuid],
    )?;
    Ok(moved > 0)
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
    let (processes, space) = if state == "running" {
        let (processes, memory) = process_facts(&c, &mut blockers, allow_frozen);
        let observed_id = c["Id"].as_str().ok_or("Universe container has no ID")?;
        (processes, space_facts(observed_id, memory, &mut blockers)?)
    } else {
        (Value::Null, Value::Null)
    };
    let facts = json!({"observed_at": crate::now(), "host_uuid": host, "container": lc::state_view(&c), "image_id": image,
        "network_mode": c["HostConfig"]["NetworkMode"], "log_driver": c["HostConfig"]["LogConfig"]["Type"],
        "runtime": runtime, "processes": processes, "space": space});
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
    Ok(
        json!({"status": "verified", "operation": "migration_preflight", "universe_uuid": uuid,
        "compatible": a.blockers.is_empty(), "blockers": a.blockers, "facts": a.facts,
        "reservation": reserved.map(|r| r.view()),
        "effects": "none: preflight does not reserve, suspend, signal or write artifacts", "scope": SCOPE}),
    )
}

fn scope_unit(id: &str) -> String {
    format!("podmesh-checkpoint-{id}.scope")
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
pub(crate) fn scoped_podman(
    unit: &str,
    id: &str,
    seconds: u64,
    args: &[&str],
    stdout: fs::File,
    stderr: fs::File,
) -> Result<ExitStatus, Error> {
    let unit = format!("--unit={unit}");
    let limit = seconds.to_string();
    let temporary = base()?.join(".tmp").join(id);
    fs::create_dir_all(&temporary)?;
    fs::set_permissions(base()?.join(".tmp"), fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700))?;
    Ok(Command::new("/usr/bin/systemd-run")
        .env("PATH", RUNTIME_PATH)
        .env("TMPDIR", &temporary)
        .env_remove("INVOCATION_ID")
        .args([
            "--scope",
            "--quiet",
            "--collect",
            &unit,
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
        .status()?)
}
/// The checkpoint runs in its own scope (see `scoped_podman`): a service crash or restart must not kill
/// CRIU mid-dump, which was observed to destroy the application without producing an archive. The command
/// names the reserved container ID, never the universe name: a container replaced under that name after
/// the checks cannot be captured.
fn checkpoint_command(id: &str, container_id: &str, archive: &Path, stdout: fs::File, stderr: fs::File) -> Result<ExitStatus, Error> {
    let export = format!("--export={}", archive.display());
    scoped_podman(
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
    )
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
    let dir = base()?.join(id);
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
            match fs::symlink_metadata(&dir) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&dir)?,
                Err(e) => return Err(e.into()),
                // A service crash between creating the directory and persisting the reservation leaves it
                // empty; nothing was suspended, so it is reused. Anything else is refused.
                Ok(m) if m.is_dir() && fs::read_dir(&dir)?.next().is_none() => {}
                Ok(_) => {
                    return Err(
                        "An artifact directory for this operation exists without a reservation and is not empty; refusing to reuse it"
                            .into(),
                    )
                }
            }
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
            let now = crate::now() as i64;
            let started_at = a.container["State"]["StartedAt"].as_str().unwrap_or("").to_string();
            // The reservation is durable before anything can suspend the source.
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
            write_private(&dir.join("preflight.json"), serde_json::to_string_pretty(&a.facts)?.as_bytes())?;
            let r = reservation(db, uuid)?.ok_or("Reservation not persisted")?;
            capture(db, attempt, id, uuid, name, &dir, &r)
        }
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

/// Read-only: reservation, fresh observation, artifact verification, transfer authorizations, restore claims,
/// archived reservations and release preconditions.
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
    let Some(r) = r else {
        return Ok(
            json!({"universe_uuid": uuid, "reservation": null, "current": current, "transfer_authorizations": authorizations,
            "restore_claims": claims, "reservation_history": history}),
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
    Ok(json!({
        "universe_uuid": uuid,
        "reservation": r.view(),
        "current": current,
        "artifacts": artifacts,
        "transfer_authorizations": authorizations,
        "restore_claims": claims,
        "reservation_history": history,
        "release": {
            "permitted": false,
            "operation_available": false,
            "reason": RELEASE_GAP,
            "preconditions_observed": {
                "source_container_present": container.is_some(),
                "same_reserved_container": same,
                "source_not_running": container.as_ref().is_some_and(|c| !lc::process_active(c)),
                "source_checkpointed": container.as_ref().is_some_and(|c| c["State"]["Checkpointed"] == true),
                "transfer_authorizations_issued_for_reservation": issued,
            },
        },
    }))
}
