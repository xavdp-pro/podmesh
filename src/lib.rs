mod cleanup;
mod activation;
mod boot_restore;
mod recovery_point;
mod retention;
mod network;
mod manager;
mod secrets;
mod publisher;
mod signing;
mod schema;
mod storage;
mod health;
pub use manager::control_relay;
pub use network::reconcile as reconcile_network;
pub use publisher::withdraw_at_startup as withdraw_unentitled_publishers_at_startup;
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
    manager::prepare_host_state(dir);
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
            | "pause"
            | "resume"
            | "resources"
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
            "collection_retention_declare" | "collection_hold_declare" | "collection_hold_release" | "collection_status" => retention::execute(db, request)?,
            "manager_status" | "manager_decision" | "manager_observe" => manager::execute(db, request)?,
            "secret_declare" | "secret_remove" | "secret_status" => secrets::execute(db, request)?,
            "publisher_declare" | "publisher_start" | "publisher_stop" | "publisher_status" | "publisher_observed" => publisher::execute(db, request)?,
            "network_declare" | "network_undeclare" | "network_route_publish" | "network_route_withdraw" | "network_route_resume" | "network_reapply" | "network_status" => network::execute(db, request)?,
            "activation_require" | "activation_acquire" | "activation_renew" | "activation_release"
            | "activation_supersede" | "activation_status" | "activation_fence" | "activation_fence_preview" => activation::execute(db, request)?,
            "recovery_point_prepare" | "recovery_point_status" | "recovery_point_restore" | "recovery_point_promote"
            | "recovery_point_stage" | "recovery_point_discard" | "recovery_point_resume" => recovery_point::execute(db, request)?,
            "boot_restore" | "boot_restore_status" => boot_restore::execute(db, request)?,
            "migration_status" => migration::status(db, request)?,
            "storage_status" => storage::status()?,
            "host_status" => health::host_status()?,
            "universe_stats" => health::universe_stats()?,
            "capabilities" => json!({
                "schemas": schema::all(),
                "schema_version": "podmesh-operation-schema/1",
                "version":option_env!("PODMESH_PACKAGE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
                "operations":["capabilities","identity","inventory","observations","activation_require","activation_acquire","activation_renew","activation_release","activation_supersede","activation_status","activation_fence","activation_fence_preview","recovery_point_prepare","recovery_point_status","recovery_point_restore","recovery_point_promote","recovery_point_stage","recovery_point_discard","recovery_point_resume","create","delete","clone","start","stop","pause","resume","resources","storage_status","host_status","universe_stats"],
                "experimental_operations":["migration_preflight","migration_checkpoint","migration_status","migration_authorize_transfer",
                    "migration_complete_transfer","migration_retire_source","migration_release","migration_abandon","migration_restore_local",
                    "migration_destination_preflight","migration_restore","migration_restore_abort",
                    "garbage_collect_plan","garbage_collect_apply","collection_retention_declare","collection_hold_declare","collection_hold_release","collection_status","network_declare","network_undeclare","network_route_publish","network_route_withdraw","network_route_resume","network_reapply","network_status","boot_restore","boot_restore_status","manager_status","manager_decision","manager_observe","secret_declare","secret_remove","secret_status","publisher_declare","publisher_start","publisher_stop","publisher_status","publisher_observed"],
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
                    "reservation":"a reservation or an unresolved restore claim blocks create, start, delete and clone for the universe; stop remains available, and a released or collected reservation blocks nothing",
                    "boot_restore":"host-wide, journaled, called at boot by a local unit under the operator's mandate or by an operator or agent: starts, at most once per boot each, the universes whose last journaled intent is to run, through the start gates, under the caller's authorization_ref; never a managed-network universe while the declaration is not effective, an epoch-gated one, a lease-gated one whose lease was not acquired or renewed during this boot, a universe with recovery points and no policy, a quarantined copy, a holding migration reservation or unresolved restore claim, or a universe with a capture still dumping or an unfinished live promotion recorded after its last intent",
                    "network_reapply":"host-wide, journaled, called at boot before boot_restore: re-applies the effects of this host's effective declaration that the kernel or Podman no longer shows (bridge, peer routes, NAT exemption) and withdraws every recorded /32 route whose kernel route is gone, with its alias, and every route row left resuming whatever the kernel shows; a /32 route is never re-applied from the ledger",
                    "network_route_resume":"host-wide, journaled (exclusive_resource): the recorded exclusive route and alias of a role this host still holds, put back in place after the carrier lost them (a stop, a restart, a roll at the same address): the row kept and marked resuming, the dead effects removed, the alias and route made again with the recorded ip, via and resource and verified; a failure or a crash leaves the row recorded for the next resume; only while the lease is live, held here, unsuperseded and was acquired or renewed during this boot, a universe runs at via, and the kernel holds no other route for the address; refused otherwise with a named reason (no_policy, no_recorded_route, route_incomplete, lease_not_entitled, lease_not_renewed_this_boot, declaration_not_effective, kernel_unknown, other_kernel_route, alias_unknown, no_carrier_at_via); an effective route answers already_effective; never changes the holder, acquires or renews",
                    "publisher_start":"every method of takeover proof is held until its eligible_after on this host's clock and accepted from it, and an expired one refused; the takeover proof is verified and recorded with the lease incarnation and boot it was verified under; without a proof, or with one refused, the start resumes under the recorded one (method resume_same_epoch, journaled with the original proof's identity) only while the lease is live, held here, unsuperseded, at the same epoch, generation and acquisition, in the same boot and under the same authority, key and quorum; refused otherwise with a named reason",
                    "authority_quorum":"a policy may name its authority as a quorum of replica keys (threshold a strict majority of 1 to 9 keys) instead of one key: acquisition, supersession and the takeover proof then take a certificate that many distinct keys of the quorum signed under the policy's digest, verified here with no network; a duplicate, unknown or malformed signature refuses the whole certificate by name; no permit is accepted, so the epoch screen moves forward on certificates only and never backwards; the authority set carries a serial its digest covers, one more at every change, and changes only under a certificate of the policy in place bound to that serial or the operator's re-declaration naming the digest it replaces; a single key is the 1-of-1 case and its documents and permits are accepted as before",
                    "publisher_startup_withdrawal":"not a request: the daemon's own journaled operation at startup, withdrawing, connector and mark, every declared publisher present without a live, unsuperseded lease held here",
                    "boot_restore_status":"read-only: this boot's passes, what a pass would decide now for every universe intended to run, and the operations a previous run of the service left pending"
                },
                "scope":"local rootful Podman; network-disabled universes created or cloned by this host's PodMesh journal; one request at a time",
                "contracts":{
                    "ownership":"delete, start, stop and clone sources require a verified create, clone, migration_restore or live recovery_point_promote in this host's journal for the same universe and container ID",
                    "recovery_point_prepare":"capture stopped (default) exports a universe already stopped (class quiescent); capture live checkpoints a running universe with the qualified private runtime and resumes it in place from the kept images (class memory-coherent): the universe is interrupted for the dump and the resume, about half a second on a small universe, and its memory continues; gated by the activation lease as a start is; refused for a universe with a network, mounts, a TTY, more than 1 GiB of memory or non-musl processes",
                    "recovery_point_stage":"a live point's archive held on this host, verified against its manifest, no container created: a memory checkpoint restores as running processes, so the copy is restored only by recovery_point_promote under the lease",
                    "recovery_point_resume":"brings a universe left stopped by a final live capture (capture live, resume false) back in place from the images it kept, under the lease gate; the way back when its promotion elsewhere did not happen",
                    "recovery_point_discard":"removes a staged point's archive and manifest from this host's inbox; refused for a point promoted here",
                    "start":"observe_seconds 0-30 (default 2); reports running or not running as observed, with exit code when not running",
                    "stop":"timeout_seconds 0-300 and on_timeout kill|leave_running are required; kill lets podman escalate to SIGKILL after the timeout, leave_running only sends the stop signal",
                    "pause":"freezes every process of a running universe; memory and address stay; never gated by the lease; a paused universe answers none_already_paused",
                    "resume":"thaws a paused universe; gated by the activation lease exactly as start; a running universe answers none_already_running",
                    "resources":"memory_bytes (32 MiB to this host's total) and/or cpus (0.1 to this host's cores); applied to the live cgroup and read back from the kernel when running, kept for the next start otherwise",
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
