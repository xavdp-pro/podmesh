mod cleanup;
mod collector;
mod lifecycle;
mod migration;
mod recovery;
mod restore;
mod transfer;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}
pub fn open_state(dir: &Path) -> Result<Connection, Box<dyn std::error::Error>> {
    fs::create_dir_all(dir)?;
    let db = Connection::open(dir.join("state.sqlite"))?;
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS observations(id INTEGER PRIMARY KEY,observed_at INTEGER NOT NULL,operation TEXT NOT NULL,result TEXT NOT NULL);")?;
    let machine = fs::read_to_string("/etc/machine-id")?.trim().to_string();
    let stored: Option<String> = db
        .query_row("SELECT value FROM metadata WHERE key='machine_id'", [], |r| r.get(0))
        .optional()?;
    if let Some(previous) = stored {
        if previous != machine {
            return Err("State belongs to a different host; explicit identity adoption required".into());
        }
    }
    let tx = db.unchecked_transaction()?;
    tx.execute("INSERT OR IGNORE INTO metadata VALUES('machine_id',?1)", [&machine])?;
    let uuid = fs::read_to_string("/proc/sys/kernel/random/uuid")?;
    tx.execute("INSERT OR IGNORE INTO metadata VALUES('host_uuid',?1)", [uuid.trim()])?;
    tx.commit()?;
    lifecycle::prepare_scratch(&dir.join("podman-tmp"))?;
    migration::prepare(&dir.join("migrations"))?;
    transfer::prepare(dir)?;
    Ok(db)
}
fn inventory() -> Result<Value, Box<dyn std::error::Error>> {
    // Fixed command; no caller-controlled shell or command arguments.
    let mut child = Command::new("/usr/bin/podman")
        .args(["ps", "--all", "--format", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    // Drain both pipes while waiting, bounding retained output without blocking the child.
    fn drain(mut input: impl std::io::Read) -> Vec<u8> {
        let mut out = Vec::new();
        let mut b = [0u8; 8192];
        loop {
            match input.read(&mut b) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out.len() + n <= 4 * 1024 * 1024 {
                        out.extend_from_slice(&b[..n]);
                    }
                }
            }
        }
        out
    }
    let a = thread::spawn(move || drain(stdout));
    let b = thread::spawn(move || drain(stderr));
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() > deadline {
            child.kill()?;
            child.wait()?;
            return Err("Podman inventory timeout".into());
        }
        thread::sleep(Duration::from_millis(50));
    };
    let output = a.join().map_err(|_| "Output reader failed")?;
    let errors = b.join().map_err(|_| "Error reader failed")?;
    if !status.success() {
        return Err(format!("Podman failed: {}", String::from_utf8_lossy(&errors)).into());
    }
    Ok(serde_json::from_slice(&output)?)
}
pub fn handle(db: &Connection, request: &Value) -> Value {
    let op = request.get("operation").and_then(Value::as_str).unwrap_or("");
    let result: Result<Value, Box<dyn std::error::Error>> = (|| {
        Ok(match op {
            "create"
            | "delete"
            | "clone"
            | "start"
            | "stop"
            | "migration_preflight"
            | "migration_checkpoint"
            | "migration_authorize_transfer"
            | "migration_complete_transfer"
            | "migration_retire_source"
            | "migration_release"
            | "migration_abandon"
            | "migration_restore_local"
            | "migration_destination_preflight"
            | "migration_restore"
            | "migration_restore_abort" => lifecycle::execute(db, request)?,
            // Host-wide by design: the collector is the only operation that does not name one universe.
            "garbage_collect_plan" | "garbage_collect_apply" => collector::execute(db, request)?,
            "migration_status" => migration::status(db, request)?,
            "capabilities" => json!({
                "version":option_env!("PODMESH_PACKAGE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
                "operations":["capabilities","identity","inventory","observations","create","delete","clone","start","stop"],
                "experimental_operations":["migration_preflight","migration_checkpoint","migration_status","migration_authorize_transfer",
                    "migration_complete_transfer","migration_retire_source","migration_release","migration_abandon","migration_restore_local",
                    "migration_destination_preflight","migration_restore","migration_restore_abort",
                    "garbage_collect_plan","garbage_collect_apply"],
                "experimental_contracts":{
                    "migration_preflight":"read-only compatibility report bound to universe UUID, container ID, image ID, source and destination host UUIDs; no reservation, suspension or artifact",
                    "migration_checkpoint":"source: fresh checks before suspension, durable reservation, checkpoint with the packaged podmesh-vzcriu runtime in its own scope, archive/manifest/hashes under the state directory; never an authorization to restore",
                    "migration_status":"read-only reservation, fresh observation, artifact re-hash, transfer authorizations, restore claims, archived reservations and which recovery operations the observed state permits",
                    "migration_authorize_transfer":"source: checkpointed -> transfer_authorized after re-hashing the artifacts and observing the checkpointed source; authorization recorded first, then archive, manifest and handoff in outbox/<authorization_id>/",
                    "migration_complete_transfer":"source: inbox/<authorization_id>/outcome.json bound to this handoff; restored -> transferred, not_restored -> checkpointed with the authorization ended; any mismatch refused without state change",
                    "migration_retire_source":"source: from transferred, removes only the stopped, checkpointed reserved container; reservation and evidence kept",
                    "migration_release":"source: checkpointed or checkpoint_failed -> released, only when no transfer authorization was ever issued and the same reserved container is observed stopped; lifts the generic-operation gate and starts nothing",
                    "migration_abandon":"source: reserved, checkpointing, checkpoint_failed or checkpointed with the reserved container absent -> abandoned, only when no authorization was ever issued; artifacts kept and the universe UUID stays refused",
                    "migration_restore_local":"source: from released, resumes the checkpointed memory here from the checkpoint files Podman kept (same container ID) or, when they are gone, from the preserved archive; verified like a destination restore, then the reservation is archived",
                    "migration_destination_preflight":"destination, read-only: inbox handoff names this host; name, label, reservation and claims free; image, runtime, kernel, archive, manifest and space checked",
                    "migration_restore":"destination: preflight, durable claim, restore of a private archive copy with the packaged runtime in its own scope; verified only when the universe runs restored and the CRIU restore log names the qualified runtime; records ownership and writes outbox/<authorization_id>/outcome.json",
                    "migration_restore_abort":"destination: never removes a running or verified universe; removes only a non-running container created by a held claim, or declines an unclaimed authorization, then records not_restored and writes the outcome. With the explicit reclaim_processes: true it first ends the processes it can prove belong to the failed attempt (membership of the container's own libpod or libpod-conmon cgroup, start time at or after the claim, both re-read immediately before the signal) and verifies that both cgroups disappeared; without it, surviving processes are reported and nothing is removed or signalled",
                    "restore_bound":"a restore attempt is watched while it runs: if it consumes more of the Podman graph root than its own preflight required, or that filesystem falls below the floor, its container cgroup is frozen (nothing is ended) and the transient scope is stopped",
                    "garbage_collect_plan":"host-wide and read-only: enumerates a bounded set of reservations and unresolved restore claims, their class, every proof fact observed and every blocker, and proposes an effect for each. No Podman mutation, no signal, no deletion, no state change beyond its own immutable run record. Age never justifies collection: proof does",
                    "garbage_collect_apply":"separately authorized: names the plan it applies, the candidates it may act on, and its own bounds (max_effects, max_runtime_reclaims, reclaim_processes). It repeats every proof immediately before each effect, stops at the first mismatch, and verifies each result from outside. Terminal reservation classes 1 and 2 become a collected reservation with a tombstone; a failed restore claim is delegated to migration_restore_abort. There is no timer and no artifact collection in this version",
                    "tombstone":"a collected universe UUID keeps refusing create and clone into that identity for good; a container proven absent at collection never regains ownership through its original creation. Only a verified handoff restore, or an explicit replacement procedure, gives that identity a meaning again",
                    "reservation":"a reservation or an unresolved restore claim blocks create, start, delete and clone for the universe; stop remains available, and a released or collected reservation blocks nothing"
                },
                "scope":"local rootful Podman; network-disabled universes created or cloned by this host's PodMesh journal; one request at a time",
                "contracts":{
                    "ownership":"delete, start, stop and clone sources require a verified create, clone or migration_restore in this host's journal for the same universe and container ID",
                    "start":"observe_seconds 0-30 (default 2); reports running or not running as observed, with exit code when not running",
                    "stop":"timeout_seconds 0-300 and on_timeout kill|leave_running are required; kill lets podman escalate to SIGKILL after the timeout, leave_running only sends the stop signal",
                    "retry":"a verified operation ID returns its historical result with a fresh observation; pending or failed operations are re-evaluated; no cancellation operation",
                    "clone":"stopped, mount-free source through a committed snapshot image"
                }
            }),
            "identity" => json!({"host_uuid":db.query_row("SELECT value FROM metadata WHERE key='host_uuid'",[],|r|r.get::<_,String>(0))?}),
            "inventory" => json!({"containers":inventory()?,"store":"default rootful Podman"}),
            "observations" => {
                let mut stmt = db.prepare("SELECT id,observed_at,operation FROM observations ORDER BY id DESC LIMIT 20")?;
                let rows = stmt.query_map([], |r| {
                    Ok(json!({"id":r.get::<_,i64>(0)?,"observed_at":r.get::<_,i64>(1)?,"operation":r.get::<_,String>(2)?}))
                })?;
                json!({"observations":rows.collect::<Result<Vec<_>,_>>()?})
            }
            _ => return Err("Unsupported operation".into()),
        })
    })();
    let response = match result {
        Ok(data) => json!({"ok":true,"observed_at":now(),"data":data}),
        Err(e) => {
            let mut failure = json!({"ok":false,"observed_at":now(),"error":e.to_string()});
            // Failures after an effect was attempted carry the freshly observed state.
            if let Some(f) = e.downcast_ref::<lifecycle::Failure>() {
                failure["details"] = f.details.clone();
            }
            failure
        }
    };
    if let Err(e) = db.execute(
        "INSERT INTO observations(observed_at,operation,result) VALUES(?1,?2,?3)",
        params![now() as i64, op, response.to_string()],
    ) {
        return json!({"ok":false,"error":format!("Observation persistence failed: {e}")});
    }
    response
}
