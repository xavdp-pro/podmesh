//! Restoring, after a boot, the universes this host's journal says should run (vital target V1).
//!
//! PodMesh acts on nothing by itself, and this module does not change that: `boot_restore` is an
//! operation. What makes a host bring back its universes after a reboot without any peer, manager
//! or workstation is the local unit that calls it at boot, which runs only under a mandate
//! written on the host (`packaging/podmesh-restore`, shipped disabled, exactly as the self-fence),
//! and every start it causes names the caller's authorization reference.
//!
//! WHAT "SHOULD RUN" MEANS. The journal records intents, not a desired state. An operation that
//! names a universe either leaves it running when it is verified (start, resume, a destination or
//! local migration restore, a recovery point resume, a live promotion), or says that it should not
//! run (create, clone, stop, pause, delete, a checkpoint, a final live capture, a release, an
//! abandonment, a completed transfer, a fence) -- when it was verified, when it was interrupted, or
//! when it failed leaving the universe observed not running; a refusal before any effect, or a
//! failure that brought the universe back, expresses nothing. The latest intent, ordered by the
//! attempt that last expressed it, is the universe's last intent. Only a universe whose last intent
//! is to run is considered.
//!
//! WHAT IS NEVER RESTORED HERE, whatever the last intent says:
//! - a universe on the managed network while the host's network declaration is not effective: after
//!   a boot, `network_reapply` re-applies the declaration's bridge, peer routes and NAT exemption, and a
//!   universe restored without them would run unreachable; its /32 routes and alias are published again
//!   by whoever holds the role, never restored from the ledger;
//! - a universe whose activation is bound to an external authority's epoch: a host that rebooted
//!   must be authorised again (`activation.rs`, where a permit is bound to the boot);
//! - a universe under a lease, unless the lease was acquired or renewed during this boot: a host
//!   that was down cannot know whether its universe was taken over elsewhere while its lease still
//!   ran, and a takeover of a lost host waits only for that lease to lapse, so the entitlement must
//!   have been decided again since the boot; and never while the clock is not known synchronized;
//! - a universe with recovery points and no activation policy: its copies may have been promoted
//!   elsewhere, and nothing here says where it may run;
//! - a quarantined restore copy, which is evidence, not a service;
//! - a migration reservation in a state that still holds the universe, or an unresolved restore
//!   claim; a released or collected reservation holds nothing, and a start after it is the caller's
//!   explicit choice the protocol leaves to the caller;
//! - a universe with a live capture still dumping, or an unfinished live promotion, recorded after
//!   its last intent: each is settled only by retrying its own operation.
//!
//! AT MOST ONE START PER UNIVERSE PER BOOT. Each start this module issues carries an operation ID
//! derived from this boot's identity and the universe; a later pass in the same boot sends that
//! start's saved request again, whatever its own parameters, so a verified start is replayed instead
//! of repeated and an application that exits again is not restarted in a loop. Every start of a pass
//! is issued first, then observed once, so a pass stays short on a service that answers one request
//! at a time.
use crate::lifecycle as lc;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::process::Command;
use std::time::Duration;

type Error = Box<dyn std::error::Error>;

const MAX_OBSERVE_SECONDS: u64 = 30;
const DEFAULT_OBSERVE_SECONDS: u64 = 2;
const PENDING_LISTED: usize = 20;

/// What an operation says about whether its universe should run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Intent {
    Run,
    NotRun,
}

/// The intent an operation expresses for the universe it names, or `None` when it expresses none.
/// A run intent counts only once verified: an interrupted start may have started the container, but
/// only a verified one is known to have, and a universe whose start was interrupted is left to its
/// caller. A not-run intent counts when verified, when interrupted, and when it failed leaving the
/// universe observed not running; a refusal before any effect expresses nothing.
pub(crate) fn classify(operation: &str, status: &str, request: &Value, result: Option<&Value>) -> Option<Intent> {
    let verified = status == "verified";
    let not_run = || match status {
        "failed" => (result.map(|r| r["details"]["observed"]["running"] == json!(false)) == Some(true)).then_some(Intent::NotRun),
        _ => Some(Intent::NotRun),
    };
    match operation {
        "start" | "resume" | "migration_restore" | "migration_restore_local" | "recovery_point_resume" => {
            verified.then_some(Intent::Run)
        }
        // A live promotion brings the universe back running; a promotion of a quarantined copy
        // creates it stopped, and starting it is the caller's.
        "recovery_point_promote" => match (verified, result.map(|r| r["started"] == json!(true))) {
            (true, Some(true)) => Some(Intent::Run),
            (true, _) => Some(Intent::NotRun),
            _ => None,
        },
        // A final live capture leaves the universe stopped with its images kept, possibly for a
        // promotion elsewhere; a live capture that resumes in place, or a stopped capture of a
        // universe already stopped, changes nothing about whether it should run.
        "recovery_point_prepare" if request["capture"] == json!("live") && request["resume"] == json!(false) => not_run(),
        "create" | "clone" | "stop" | "pause" | "delete" | "migration_checkpoint" | "migration_release"
        | "migration_abandon" | "migration_complete_transfer" | "migration_retire_source" | "migration_restore_abort" => not_run(),
        _ => None,
    }
}

/// A universe's last intent and where it sits in the journal.
#[derive(Clone, Debug)]
pub(crate) struct LastIntent {
    pub intent: Intent,
    /// The attempt that last expressed it: operations are ordered by it.
    pub position: i64,
    pub operation: String,
    pub operation_id: String,
}

fn table_exists(db: &Connection, name: &str) -> Result<bool, Error> {
    Ok(db
        .query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1", [name], |_| Ok(()))
        .optional()?
        .is_some())
}

/// The attempt that last expressed each operation. A replayed operation writes no attempt, so this
/// is the last time the operation was executed or re-evaluated.
fn last_attempts(db: &Connection) -> Result<HashMap<String, i64>, Error> {
    let mut stmt = db.prepare("SELECT operation_id, MAX(id) FROM operation_attempts GROUP BY operation_id")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Every universe's last intent, from the operations that name it and the fences that stopped it.
pub(crate) fn last_intents(db: &Connection) -> Result<BTreeMap<String, LastIntent>, Error> {
    let attempts = last_attempts(db)?;
    let mut intents: BTreeMap<String, LastIntent> = BTreeMap::new();
    let mut record = |uuid: &str, intent: Intent, position: i64, operation: &str, operation_id: &str| {
        let newer = intents.get(uuid).map_or(true, |known| position > known.position);
        if newer {
            intents.insert(
                uuid.to_string(),
                LastIntent { intent, position, operation: operation.to_string(), operation_id: operation_id.to_string() },
            );
        }
    };
    let mut stmt = db.prepare("SELECT id, request, status, result FROM operations")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?))
    })?;
    for row in rows {
        let (id, request, status, result) = row?;
        let Some(&position) = attempts.get(&id) else { continue };
        let Ok(request) = serde_json::from_str::<Value>(&request) else { continue };
        let (Some(operation), Some(uuid)) = (request["operation"].as_str(), request["universe_uuid"].as_str()) else { continue };
        let result = result.and_then(|r| serde_json::from_str::<Value>(&r).ok());
        if let Some(intent) = classify(operation, &status, &request, result.as_ref()) {
            record(uuid, intent, position, operation, &id);
        }
    }
    // A fence is host-wide and names no universe in its request; the lease history names the
    // universes it stopped.
    if table_exists(db, "activation_lease_history")? {
        let mut stmt = db.prepare("SELECT universe_uuid, operation_id FROM activation_lease_history WHERE event='fenced'")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (uuid, id) = row?;
            if let Some(&position) = attempts.get(&id) {
                record(&uuid, Intent::NotRun, position, "activation_fence", &id);
            }
        }
    }
    Ok(intents)
}

/// This boot's identity, as the kernel gives it.
fn boot_id() -> Result<String, Error> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim().to_string())
}

/// When this boot began, in seconds since the Unix epoch, on the wall clock as it reads now.
fn boot_time() -> Option<i64> {
    std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime "))
        .and_then(|v| v.trim().parse().ok())
}

/// The start a pass issues for a universe in a boot: 70 characters of `[a-z0-9-]`, the same in
/// every pass of that boot.
pub(crate) fn start_operation_id(boot_id: &str, uuid: &str) -> String {
    format!("boot-{}-{}", boot_id.replace('-', ""), uuid.replace('-', ""))
}

/// Whether the system clock is known to be synchronized: `Some(true)` or `Some(false)` from systemd,
/// `None` when it cannot be read, which is treated as not known.
fn clock_synchronized() -> Option<bool> {
    let out = Command::new("timedatectl").args(["show", "-p", "NTPSynchronized", "--value"]).output().ok()?;
    match String::from_utf8_lossy(&out.stdout).trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// The names of every container Podman holds, read once for a pass.
fn container_names() -> Result<HashSet<String>, Error> {
    let all: Value = serde_json::from_str(&lc::podman(lc::QUICK, &["ps", "--all", "--format", "json"])?)?;
    Ok(all
        .as_array()
        .ok_or("Invalid inventory")?
        .iter()
        .flat_map(|c| c["Names"].as_array().cloned().unwrap_or_default())
        .filter_map(|n| n.as_str().map(str::to_string))
        .collect())
}

/// What a pass knows for all its universes: read once.
struct Facts {
    boot: String,
    booted: Option<i64>,
    names: HashSet<String>,
    attempts: HashMap<String, i64>,
    clock: Option<bool>,
}

fn facts(db: &Connection) -> Result<Facts, Error> {
    Ok(Facts { boot: boot_id()?, booted: boot_time(), names: container_names()?, attempts: last_attempts(db)?, clock: clock_synchronized() })
}

/// The decision a pass reaches before issuing a start: a reason not to, or leave to the start.
enum Before {
    Absent,
    Skip { decision: &'static str, reason: String, detail: Value },
    Start,
}

fn skip(decision: &'static str, reason: impl Into<String>, detail: Value) -> Before {
    Before::Skip { decision, reason: reason.into(), detail }
}

fn exists(db: &Connection, table: &str, sql: &str, uuid: &str) -> Result<bool, Error> {
    Ok(table_exists(db, table)? && db.query_row(sql, [uuid], |_| Ok(())).optional()?.is_some())
}

/// Everything decided before a start is issued, in the order a reader should see the reasons.
fn before_start(db: &Connection, uuid: &str, last: &LastIntent, f: &Facts) -> Result<Before, Error> {
    let name = format!("podmesh-{uuid}");
    if !f.names.contains(&name) {
        return Ok(Before::Absent);
    }
    let c: Value = serde_json::from_str::<Value>(&lc::podman(lc::QUICK, &["container", "inspect", &name])?)?[0].clone();
    let state = lc::status(&c).to_string();
    if state == "running" {
        return Ok(skip("already_running", "running", json!({"observed": lc::state_view(&c)})));
    }
    if c["Config"]["Labels"][crate::network::LABEL_PROFILE].as_str() == Some(crate::network::PROFILE_MANAGED) {
        // A managed universe comes back only onto a network that is effective again: after a boot,
        // `network_reapply` restores the declaration's bridge, peer routes and NAT exemption first.
        match crate::network::declaration_effective(db)? {
            Some(true) => {}
            other => {
                return Ok(skip("not_restored", "managed_network_not_effective", json!({
                    "declaration_effective": other,
                    "note": "the host's network declaration must be effective, its effects re-applied by network_reapply after a boot, before a managed universe is restored",
                })));
            }
        }
    }
    if exists(db, "recovery_point_restores", "SELECT 1 FROM recovery_point_restores WHERE restored_universe_uuid=?1", uuid)? {
        return Ok(skip("not_restored", "quarantined_copy", json!({"note": "a restore copy is evidence, not a service; promote it to run it"})));
    }
    let policy = crate::activation::policy(db, uuid)?;
    match &policy {
        Some(p) if !p.authority_id.is_empty() => {
            return Ok(skip("not_restored", "epoch_gated", json!({"authority_id": p.authority_id, "note": "a host that rebooted must be authorised again by the authority"})));
        }
        Some(_) => {
            if f.clock != Some(true) {
                return Ok(skip("not_restored", "clock_not_synchronized", json!({"clock_synchronized": f.clock, "note": "a lease is judged on the wall clock"})));
            }
            // The entitlement must have been decided again since the boot: a host that was down cannot know
            // whether its universe was taken over elsewhere while its lease still ran.
            let renewed = match f.booted {
                Some(booted) => db
                    .query_row(
                        "SELECT MAX(at) FROM activation_lease_history WHERE universe_uuid=?1 AND event IN ('acquired','renewed') AND at>=?2",
                        rusqlite::params![uuid, booted],
                        |r| r.get::<_, Option<i64>>(0),
                    )?
                    .is_some(),
                None => false,
            };
            if !renewed {
                return Ok(skip("not_restored", "lease_not_renewed_since_boot", json!({
                    "booted_at": f.booted,
                    "note": "the lease must be acquired or renewed during this boot, by whoever decides where this universe runs, before a pass restores it",
                })));
            }
        }
        None => {
            if exists(db, "recovery_points", "SELECT 1 FROM recovery_points WHERE universe_uuid=?1", uuid)? {
                return Ok(skip("not_restored", "recovery_points_without_policy", json!({
                    "note": "copies of this universe may have been promoted elsewhere, and no activation policy says where it may run",
                })));
            }
        }
    }
    if let Some(r) = crate::migration::reservation(db, uuid)? {
        if r.state != crate::migration::RELEASED && r.state != crate::migration::COLLECTED {
            return Ok(skip("not_restored", format!("migration_{}", r.state), json!({"reservation": r.view()})));
        }
    }
    if let Some(claim) = crate::restore::unresolved_claim(db, uuid)? {
        return Ok(skip("not_restored", "restore_claim", json!({"restore_claim": claim})));
    }
    // A live capture the service did not see to the end, or a live promotion it launched and never recorded,
    // after the last intent: retrying that operation settles it and keeps the memory a plain start discards.
    let after_last = |id: &str| f.attempts.get(id).map_or(true, |p| *p >= last.position);
    if table_exists(db, "recovery_point_live_captures")? {
        let mut stmt = db.prepare("SELECT operation_id FROM recovery_point_live_captures WHERE universe_uuid=?1 AND state='dumping'")?;
        let open: Vec<String> = stmt.query_map([uuid], |r| r.get(0))?.collect::<Result<_, _>>()?;
        if let Some(id) = open.into_iter().find(|id| after_last(id)) {
            return Ok(skip("not_restored", "interrupted_live_capture", json!({"operation_id": id, "settle": "retry that operation"})));
        }
    }
    if table_exists(db, "recovery_point_live_promote_attempts")? {
        let mut stmt = db.prepare("SELECT operation_id FROM recovery_point_live_promote_attempts WHERE universe_uuid=?1 AND state='launched'")?;
        let open: Vec<String> = stmt.query_map([uuid], |r| r.get(0))?.collect::<Result<_, _>>()?;
        if let Some(id) = open.into_iter().find(|id| after_last(id)) {
            return Ok(skip("not_restored", "unfinished_live_promotion", json!({"operation_id": id, "settle": "retry that operation"})));
        }
    }
    if !lc::STOPPED.contains(&state.as_str()) {
        return Ok(skip("not_restored", format!("state_{state}"), json!({"observed": lc::state_view(&c)})));
    }
    Ok(Before::Start)
}

fn observe_seconds(request: &Value) -> Result<u64, Error> {
    match request.get("observe_seconds") {
        None => Ok(DEFAULT_OBSERVE_SECONDS),
        Some(v) => v
            .as_u64()
            .filter(|s| *s <= MAX_OBSERVE_SECONDS)
            .ok_or_else(|| "observe_seconds must be an integer from 0 to 30".into()),
    }
}

fn host_uuid(db: &Connection) -> Result<String, Error> {
    Ok(db.query_row("SELECT value FROM metadata WHERE key='host_uuid'", [], |r| r.get(0))?)
}

fn schemas(db: &Connection) -> Result<(), Error> {
    lc::ensure_schema(db)?;
    crate::activation::ensure_schema(db)?;
    crate::recovery_point::ensure_schema(db)?;
    Ok(())
}

pub fn execute(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    schemas(db)?;
    if request.get("universe_uuid").is_some() {
        return Err(format!("{operation} considers every universe of this host and takes no universe_uuid").into());
    }
    match operation {
        "boot_restore_status" => status(db),
        "boot_restore" => {
            lc::text(request, "authorization_ref")?;
            observe_seconds(request)?;
            lc::journaled(db, request, |db| restore(db, request))
        }
        _ => Err("Unsupported boot restore operation".into()),
    }
}

/// The start a pass sends for a universe: the saved request when this boot already has one, so that a
/// later pass replays it whatever its own parameters; otherwise a start under the caller's reference,
/// observed later with every other start of the pass.
fn start_request(db: &Connection, id: &str, uuid: &str, authorization_ref: &str) -> Result<Value, Error> {
    let saved: Option<String> = db.query_row("SELECT request FROM operations WHERE id=?1", [id], |r| r.get(0)).optional()?;
    Ok(match saved {
        Some(request) => serde_json::from_str(&request)?,
        None => json!({"operation": "start", "operation_id": id, "universe_uuid": uuid, "authorization_ref": authorization_ref, "observe_seconds": 0}),
    })
}

fn restore(db: &Connection, request: &Value) -> Result<Value, Error> {
    let authorization_ref = lc::text(request, "authorization_ref")?;
    let observe = observe_seconds(request)?;
    let intents = last_intents(db)?;
    let f = facts(db)?;
    let mut decisions: Vec<Value> = Vec::new();
    let mut absent = Vec::new();
    let mut issued = Vec::new();
    let mut considered = 0;
    for (uuid, last) in intents.iter().filter(|(_, l)| l.intent == Intent::Run) {
        considered += 1;
        let mut entry = json!({
            "universe_uuid": uuid,
            "last_intent": {"operation": last.operation, "operation_id": last.operation_id},
        });
        // One universe's failure is that universe's decision, never the pass's: every other universe is still decided.
        match before_start(db, uuid, last, &f) {
            Ok(Before::Absent) => {
                absent.push(json!(uuid));
                continue;
            }
            Ok(Before::Skip { decision, reason, detail }) => {
                entry["decision"] = json!(decision);
                entry["reason"] = json!(reason);
                entry["detail"] = detail;
            }
            Ok(Before::Start) => {
                let id = start_operation_id(&f.boot, uuid);
                entry["start_operation_id"] = json!(id);
                match start_request(db, &id, uuid, authorization_ref).and_then(|start| lc::execute(db, &start)) {
                    Ok(result) if result["replayed"] == json!(true) => {
                        entry["decision"] = json!("restored_earlier_this_boot");
                        entry["start"] = result;
                    }
                    Ok(result) => {
                        entry["decision"] = json!("started");
                        entry["start"] = result;
                        issued.push(decisions.len());
                    }
                    Err(e) => {
                        entry["decision"] = json!("not_restored");
                        entry["reason"] = json!("start_refused");
                        entry["detail"] = json!({"error": e.to_string()});
                    }
                }
            }
            Err(e) => {
                entry["decision"] = json!("not_restored");
                entry["reason"] = json!("decision_failed");
                entry["detail"] = json!({"error": e.to_string()});
            }
        }
        decisions.push(entry);
    }
    // One observation window for every start issued now.
    if !issued.is_empty() {
        std::thread::sleep(Duration::from_secs(observe));
        for i in issued {
            let name = format!("podmesh-{}", decisions[i]["universe_uuid"].as_str().unwrap_or_default());
            match lc::inspect(&name) {
                Ok(Some(c)) => {
                    let view = lc::state_view(&c);
                    decisions[i]["decision"] = json!(if view["running"] == json!(true) { "restored" } else { "started_not_running" });
                    decisions[i]["observed"] = view;
                }
                Ok(None) => decisions[i]["observed"] = json!({"present": false}),
                Err(e) => decisions[i]["observed"] = json!({"error": e.to_string()}),
            }
        }
    }
    let count = |d: &str| decisions.iter().filter(|e| e["decision"] == json!(d)).count();
    Ok(json!({
        "operation": "boot_restore",
        "boot_id": f.boot,
        "booted_at": f.booted,
        "host_uuid": host_uuid(db)?,
        "authorization_ref": authorization_ref,
        "clock_synchronized": f.clock,
        "observation_seconds": observe,
        "universes_intended_to_run": considered,
        "decisions": decisions,
        "absent_containers": absent,
        "counts": {
            "restored": count("restored"),
            "started_not_running": count("started_not_running"),
            "started_unobserved": count("started"),
            "restored_earlier_this_boot": count("restored_earlier_this_boot"),
            "already_running": count("already_running"),
            "not_restored": count("not_restored"),
            "absent_containers": absent.len(),
        },
        "note": "only universes whose last journaled intent is to run are considered; each is started at most once per boot, through the same gates as any start, and observed once at the end of the pass",
    }))
}

/// Read-only: this boot's passes, what a pass would decide now, and the operations a previous run of
/// the service left pending. Nothing is started, nothing is journaled.
fn status(db: &Connection) -> Result<Value, Error> {
    let intents = last_intents(db)?;
    let f = facts(db)?;
    let mut times: HashMap<String, (Option<i64>, Option<i64>)> = HashMap::new();
    {
        let mut stmt = db.prepare("SELECT operation_id, MAX(started_at), MAX(finished_at) FROM operation_attempts GROUP BY operation_id")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?)))? {
            let (id, started, finished) = row?;
            times.insert(id, (started, finished));
        }
    }
    let mut passes = Vec::new();
    let mut pending = Vec::new();
    let mut pending_count = 0;
    {
        let mut stmt = db.prepare("SELECT id, request, status, result FROM operations")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?))
        })?;
        for row in rows {
            let (id, request, state, result) = row?;
            if state == "pending" {
                pending_count += 1;
                if pending.len() < PENDING_LISTED {
                    pending.push(json!(id));
                }
            }
            if !request.contains("\"boot_restore\"") {
                continue;
            }
            let Ok(request) = serde_json::from_str::<Value>(&request) else { continue };
            if request["operation"] != json!("boot_restore") {
                continue;
            }
            let result = result.and_then(|r| serde_json::from_str::<Value>(&r).ok()).unwrap_or(Value::Null);
            let (started, finished) = times.get(&id).copied().unwrap_or((None, None));
            let this_boot = result["boot_id"] == json!(f.boot) || (result["boot_id"].is_null() && f.booted.zip(started).is_some_and(|(b, s)| s >= b));
            if this_boot {
                passes.push(json!({
                    "operation_id": id,
                    "status": state,
                    "authorization_ref": request["authorization_ref"],
                    "last_started_at": started,
                    "last_finished_at": finished,
                    "counts": result["counts"],
                    "error": result["error"],
                }));
            }
        }
    }

    let mut plan = Vec::new();
    for (uuid, last) in intents.iter().filter(|(_, l)| l.intent == Intent::Run) {
        let id = start_operation_id(&f.boot, uuid);
        let mut entry = json!({
            "universe_uuid": uuid,
            "last_intent": {"operation": last.operation, "operation_id": last.operation_id},
            "start_operation_id": id,
        });
        let start_status: Option<String> =
            db.query_row("SELECT status FROM operations WHERE id=?1", [&id], |r| r.get(0)).optional()?;
        entry["start_status_this_boot"] = json!(start_status);
        match before_start(db, uuid, last, &f) {
            Ok(Before::Absent) => {
                entry["would"] = json!("not_restored");
                entry["reason"] = json!("container_absent");
            }
            Ok(Before::Skip { decision, reason, detail }) => {
                entry["would"] = json!(decision);
                entry["reason"] = json!(reason);
                entry["detail"] = detail;
            }
            // A start this boot already verified is replayed by the next pass, never repeated.
            Ok(Before::Start) if start_status.as_deref() == Some("verified") => entry["would"] = json!("restored_earlier_this_boot"),
            Ok(Before::Start) => {
                // The start's own lease gate, read without effect; the other gates were read above. This is a
                // prediction: ownership and an earlier attempt of the same start are decided by the start itself.
                match crate::activation::refuse_if_not_activated(db, uuid, "boot_restore") {
                    Ok(()) => entry["would"] = json!("restore"),
                    Err(e) => {
                        entry["would"] = json!("not_restored");
                        entry["reason"] = json!("start_refused");
                        entry["detail"] = json!({"error": e.to_string()});
                    }
                }
            }
            Err(e) => {
                entry["would"] = json!("not_restored");
                entry["reason"] = json!("decision_failed");
                entry["detail"] = json!({"error": e.to_string()});
            }
        }
        plan.push(entry);
    }
    Ok(json!({
        "boot_id": f.boot,
        "booted_at": f.booted,
        "host_uuid": host_uuid(db)?,
        "clock_synchronized": f.clock,
        "passes_this_boot": passes,
        "plan": plan,
        "operations_left_pending": {"count": pending_count, "first": pending},
        "note": "read-only: nothing is started and nothing is journaled; `would` is a prediction; a pending operation is re-evaluated only when its operation ID is sent again",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(operation: &str, extra: Value) -> Value {
        let mut r = json!({"operation": operation, "operation_id": "x", "universe_uuid": "00000000-0000-4000-8000-000000000001", "authorization_ref": "t"});
        if let (Some(r), Some(e)) = (r.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                r.insert(k.clone(), v.clone());
            }
        }
        r
    }

    #[test]
    fn run_intents_count_only_when_verified() {
        for name in ["start", "resume", "migration_restore", "migration_restore_local", "recovery_point_resume"] {
            assert_eq!(classify(name, "verified", &op(name, json!({})), None), Some(Intent::Run), "{name}");
            assert_eq!(classify(name, "failed", &op(name, json!({})), None), None, "{name}");
            assert_eq!(classify(name, "pending", &op(name, json!({})), None), None, "{name}");
        }
    }

    #[test]
    fn not_run_intents_count_unless_a_failure_left_the_universe_running() {
        let stopped = json!({"error": "x", "details": {"observed": {"running": false}}});
        let running = json!({"error": "x", "details": {"observed": {"running": true}}});
        let refused = json!({"error": "refused before any effect"});
        for name in [
            "create", "clone", "stop", "pause", "delete", "migration_checkpoint", "migration_release", "migration_abandon",
            "migration_complete_transfer", "migration_retire_source", "migration_restore_abort",
        ] {
            assert_eq!(classify(name, "verified", &op(name, json!({})), None), Some(Intent::NotRun), "{name} verified");
            assert_eq!(classify(name, "pending", &op(name, json!({})), None), Some(Intent::NotRun), "{name} pending");
            assert_eq!(classify(name, "failed", &op(name, json!({})), Some(&stopped)), Some(Intent::NotRun), "{name} failed, observed stopped");
            assert_eq!(classify(name, "failed", &op(name, json!({})), Some(&running)), None, "{name} failed, observed running");
            assert_eq!(classify(name, "failed", &op(name, json!({})), Some(&refused)), None, "{name} refused");
            assert_eq!(classify(name, "failed", &op(name, json!({})), None), None, "{name} failed, no result");
        }
    }

    #[test]
    fn captures_and_promotions_by_form() {
        let final_capture = op("recovery_point_prepare", json!({"capture": "live", "resume": false}));
        assert_eq!(classify("recovery_point_prepare", "verified", &final_capture, None), Some(Intent::NotRun));
        assert_eq!(classify("recovery_point_prepare", "pending", &final_capture, None), Some(Intent::NotRun));
        // A final capture refused, or brought back after a failed dump, leaves the universe where it was.
        assert_eq!(classify("recovery_point_prepare", "failed", &final_capture, Some(&json!({"error": "refused"}))), None);
        let live_capture = op("recovery_point_prepare", json!({"capture": "live"}));
        assert_eq!(classify("recovery_point_prepare", "verified", &live_capture, None), None);
        let stopped_capture = op("recovery_point_prepare", json!({}));
        assert_eq!(classify("recovery_point_prepare", "verified", &stopped_capture, None), None);
        let promote = op("recovery_point_promote", json!({}));
        assert_eq!(classify("recovery_point_promote", "verified", &promote, Some(&json!({"started": true}))), Some(Intent::Run));
        assert_eq!(classify("recovery_point_promote", "verified", &promote, Some(&json!({"started": false}))), Some(Intent::NotRun));
        assert_eq!(classify("recovery_point_promote", "pending", &promote, None), None);
    }

    #[test]
    fn neutral_operations_express_nothing() {
        for name in ["resources", "migration_preflight", "migration_authorize_transfer", "migration_destination_preflight", "recovery_point_restore", "recovery_point_stage", "activation_acquire", "activation_release", "anything_else"] {
            assert_eq!(classify(name, "verified", &op(name, json!({})), None), None, "{name}");
        }
    }

    #[test]
    fn start_operation_ids_fit_the_token_rules() {
        let id = start_operation_id("0f3c2a1b-9d8e-4f7a-b6c5-d4e3f2a1b0c9", "c7a1f732-80b5-4421-9241-e08066f04cb8");
        assert_eq!(id.len(), 70);
        assert!(lc::token(&id).is_ok());
        assert_eq!(id, start_operation_id("0f3c2a1b-9d8e-4f7a-b6c5-d4e3f2a1b0c9", "c7a1f732-80b5-4421-9241-e08066f04cb8"));
    }

    fn journal() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE operations(id TEXT PRIMARY KEY, request TEXT NOT NULL, status TEXT NOT NULL, result TEXT);
             CREATE TABLE operation_attempts(id INTEGER PRIMARY KEY, operation_id TEXT NOT NULL, started_at INTEGER NOT NULL, finished_at INTEGER, outcome TEXT, detail TEXT);
             CREATE TABLE activation_lease_history(id INTEGER PRIMARY KEY, universe_uuid TEXT NOT NULL, holder_host_uuid TEXT NOT NULL, generation INTEGER NOT NULL, event TEXT NOT NULL, at INTEGER NOT NULL, operation_id TEXT NOT NULL);",
        )
        .unwrap();
        db
    }

    fn put(db: &Connection, id: &str, request: Value, status: &str, result: Option<Value>) {
        db.execute(
            "INSERT OR REPLACE INTO operations VALUES(?1,?2,?3,?4)",
            rusqlite::params![id, request.to_string(), status, result.map(|r| r.to_string())],
        )
        .unwrap();
        db.execute("INSERT INTO operation_attempts(operation_id,started_at) VALUES(?1,0)", [id]).unwrap();
    }

    const U: &str = "00000000-0000-4000-8000-000000000001";
    const V: &str = "00000000-0000-4000-8000-000000000002";

    #[test]
    fn the_latest_attempt_decides_and_a_retry_moves_an_operation_forward() {
        let db = journal();
        let with = |name: &str, uuid: &str| json!({"operation": name, "operation_id": name, "universe_uuid": uuid, "authorization_ref": "t"});
        put(&db, "c", with("create", U), "verified", None);
        put(&db, "s1", with("start", U), "pending", None);
        put(&db, "x", with("stop", U), "verified", None);
        assert_eq!(last_intents(&db).unwrap()[U].intent, Intent::NotRun);
        // The interrupted start is sent again after the stop and verified: its latest attempt is now the newest.
        db.execute("UPDATE operations SET status='verified' WHERE id='s1'", []).unwrap();
        db.execute("INSERT INTO operation_attempts(operation_id,started_at) VALUES('s1',0)", []).unwrap();
        let last = &last_intents(&db).unwrap()[U];
        assert_eq!((last.intent, last.operation_id.as_str()), (Intent::Run, "s1"));
        // A refused start after it changes nothing; a pause does.
        put(&db, "s2", with("start", U), "failed", None);
        assert_eq!(last_intents(&db).unwrap()[U].operation_id, "s1");
        put(&db, "p", with("pause", U), "verified", None);
        assert_eq!(last_intents(&db).unwrap()[U].intent, Intent::NotRun);
        // A resume, then a stop that timed out with the universe still running: the resume still decides.
        put(&db, "r", with("resume", U), "verified", None);
        put(&db, "x2", with("stop", U), "failed", Some(json!({"error": "timeout", "details": {"observed": {"running": true}}})));
        assert_eq!(last_intents(&db).unwrap()[U].operation_id, "r");
    }

    #[test]
    fn a_fence_stops_what_it_names_and_nothing_else() {
        let db = journal();
        let with = |name: &str, uuid: &str| json!({"operation": name, "operation_id": name, "universe_uuid": uuid, "authorization_ref": "t"});
        put(&db, "su", with("start", U), "verified", None);
        put(&db, "sv", with("start", V), "verified", None);
        put(&db, "f", json!({"operation": "activation_fence", "operation_id": "f", "authorization_ref": "t", "timeout_seconds": 2}), "verified", None);
        db.execute(
            "INSERT INTO activation_lease_history(universe_uuid,holder_host_uuid,generation,event,at,operation_id) VALUES(?1,'h',1,'fenced',0,'f')",
            [U],
        )
        .unwrap();
        let intents = last_intents(&db).unwrap();
        assert_eq!((intents[U].intent, intents[U].operation.as_str()), (Intent::NotRun, "activation_fence"));
        assert_eq!(intents[V].intent, Intent::Run);
    }
}
