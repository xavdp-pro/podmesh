//! Garbage collection on proof, never on age: a read-only plan and a separately authorized, bounded apply.
//!
//! Contract: docs/GARBAGE-COLLECTION.md. Age never justifies collection. The collector enumerates a bounded
//! set of candidates, records every proof fact it observed and every blocker it found, and only then, in a
//! second and separately authorized operation, applies a bounded number of effects — each one preceded by a
//! fresh repetition of its own proofs and followed by a verification from outside.
//!
//! This version implements the contract's terminal reservation classes 1 and 2 and its class 3 by delegation:
//!
//! * **Class 1** — a checkpointed reservation every authorization of which ended `not_restored`. The
//!   destination proved, for each of them, that it did not restore; nothing else may hold the universe. The
//!   effect is a terminal `collected` reservation state plus a tombstone: it lifts the generic-operation gate
//!   so that the stopped, checkpointed container can be started, deleted or locally restored by a later
//!   explicit operation, and it never starts anything itself.
//! * **Class 2** — a reservation whose container disappeared before any authorization was ever issued. The
//!   same terminal state and tombstone; there is no container left to operate.
//! * **Class 3** — a failed restore claim and the runtime processes it may have left. The plan proposes it;
//!   the effect goes through `migration_restore_abort`'s own contract in `restore.rs`, including its explicit
//!   `reclaim_processes`. No process management is duplicated here.
//!
//! A tombstone is never removed. It keeps refusing a blind `create` (and a clone into that identity) for a
//! collected universe UUID, and, when the container was proven absent at collection, it refuses ownership of
//! that exact container ID if it ever comes back out of band. Reusing a collected identity needs a verified
//! handoff restore or an explicit replacement procedure, exactly as the contract requires.
//!
//! Class 4 (a failed local restore) and class 5 (operation artifacts after declared retention) are not
//! implemented here and nothing in this file assumes they exist.
use crate::cleanup;
use crate::lifecycle::{self as lc, failure, Error};
use crate::migration::{self as mg, Reservation, COLLECTED};
use crate::restore as ds;
use crate::transfer as tr;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const COLLECTOR_VERSION: &str = "podmesh-collector/1";
/// What this version is allowed to collect at all. A retention policy only becomes meaningful with the
/// artifact class, which this version does not implement.
const POLICY_VERSION: &str = "podmesh-collection-policy/1: terminal reservation classes 1 and 2, and failed restore claims through migration_restore_abort; no artifact collection and no retention interval in this version";
const AUTHORITY: &str = "A plan is read-only and authorizes nothing. An apply is a separate operation that names the plan it applies, the candidates it may act on and its own bounds; it repeats every proof immediately before each effect and verifies the result from outside. There is no timer: nothing here ever runs on its own.";
const TEST_BARRIER_ENV: &str = "PODMESH_TEST_COLLECTOR_BARRIER_DIR";
const TEST_BARRIER_FORMAT: &str = "podmesh-test-collector-barrier/1";
const TEST_BARRIER_MAX_WAIT: Duration = Duration::from_secs(60);

/// Contract class names, as they appear in a plan, in an apply request and in a tombstone.
pub(crate) const CLASS_TERMINAL_NOT_RESTORED: &str =
    "terminal_reservation_all_authorizations_not_restored";
pub(crate) const CLASS_TERMINAL_ABSENT: &str = "terminal_reservation_container_absent";
pub(crate) const CLASS_FAILED_RESTORE_CLAIM: &str = "failed_restore_claim";

/// Reservation states that still owe someone a decision. A settled reservation is a finished record: the
/// collector counts it and leaves it alone.
const OPEN_STATES: [&str; 5] = [
    "reserved",
    "checkpointing",
    "checkpointed",
    "checkpoint_failed",
    "transfer_authorized",
];
/// Restore claim states that are neither verified nor closed.
const UNRESOLVED_CLAIMS: [&str; 2] = ["restoring", "restore_failed"];

const DEFAULT_MAX_CANDIDATES: usize = 20;
const LIMIT_MAX_CANDIDATES: usize = 100;
const DEFAULT_MAX_EFFECTS: usize = 1;
const LIMIT_MAX_EFFECTS: usize = 10;
const DEFAULT_MAX_RUNTIME_RECLAIMS: usize = 0;
const LIMIT_MAX_RUNTIME_RECLAIMS: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Plan,
    Apply,
}
impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Plan => "plan",
            Mode::Apply => "apply",
        }
    }
    fn operation(self) -> &'static str {
        match self {
            Mode::Plan => "garbage_collect_plan",
            Mode::Apply => "garbage_collect_apply",
        }
    }
}

pub(crate) fn ensure_schema(db: &Connection) -> Result<(), Error> {
    // The run record is append-only and the tombstone table is never written by anything but a collection.
    // Each effect is recorded in the same transaction as the effect itself, so that a run interrupted after
    // an effect and before its report can recover what it did instead of doing it twice.
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS garbage_collection_runs(operation_id TEXT PRIMARY KEY, mode TEXT NOT NULL,
         authorization_ref TEXT NOT NULL, collector_version TEXT NOT NULL, policy_version TEXT NOT NULL,
         started_at INTEGER NOT NULL, finished_at INTEGER NOT NULL, record TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS garbage_collection_effects(operation_id TEXT NOT NULL, candidate_key TEXT NOT NULL,
         class TEXT NOT NULL, universe_uuid TEXT NOT NULL, applied_at INTEGER NOT NULL, result TEXT NOT NULL,
         PRIMARY KEY(operation_id, candidate_key));
         CREATE TABLE IF NOT EXISTS garbage_collection_reclaim_attempts(id INTEGER PRIMARY KEY,
         operation_id TEXT NOT NULL, candidate_key TEXT NOT NULL, reserved_at INTEGER NOT NULL);",
    )?;
    mg::ensure_schema(db)?;
    Ok(())
}
/// Durable progress of one effect. Written inside the effect's own transaction with `verification: pending`
/// and updated with the full result once the verification from outside has been read; a lost update costs a
/// re-verification on recovery, never a repeated effect.
fn record_effect(
    db: &Connection,
    operation: &str,
    t: &Target,
    result: &Value,
) -> Result<(), Error> {
    db.execute(
        "INSERT OR REPLACE INTO garbage_collection_effects VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            operation,
            t.key(),
            t.class,
            t.universe_uuid,
            crate::now() as i64,
            result.to_string()
        ],
    )?;
    Ok(())
}
fn recorded_effect(db: &Connection, operation: &str, key: &str) -> Result<Option<Value>, Error> {
    let raw = db
        .query_row(
            "SELECT result FROM garbage_collection_effects WHERE operation_id=?1 AND candidate_key=?2",
            params![operation, key],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    // A corrupt progress row is evidence of unknown state, never evidence that no effect happened.
    raw.map(|text| serde_json::from_str(&text).map_err(Error::from))
        .transpose()
}
/// A reclaim allowance is consumed durably before entering an abort that is authorized to signal. It is an
/// attempt allowance, not a prediction based on the plan's process observation: an interrupted or failed
/// delegated call keeps its reservation, because the caller cannot prove that the call sent no signal.
fn reserve_reclaim(
    db: &Connection,
    operation: &str,
    key: &str,
    limit: usize,
) -> Result<Option<usize>, Error> {
    let tx = db.unchecked_transaction()?;
    let used: usize = tx.query_row(
        "SELECT COUNT(*) FROM garbage_collection_reclaim_attempts WHERE operation_id=?1",
        [operation],
        |r| r.get(0),
    )?;
    if used >= limit {
        return Ok(None);
    }
    tx.execute(
        "INSERT INTO garbage_collection_reclaim_attempts(operation_id,candidate_key,reserved_at) VALUES(?1,?2,?3)",
        params![operation, key, crate::now() as i64],
    )?;
    tx.commit()?;
    Ok(Some(used + 1))
}
fn reclaim_reservations(db: &Connection, operation: &str) -> Result<usize, Error> {
    Ok(db.query_row(
        "SELECT COUNT(*) FROM garbage_collection_reclaim_attempts WHERE operation_id=?1",
        [operation],
        |r| r.get(0),
    )?)
}
fn progress_prevents_repeating_effect(progress: &Value) -> bool {
    matches!(progress["effect_state"].as_str(), Some("committed" | "uncertain"))
        // Compatibility with a pending progress row written by the first M4 implementation.
        || progress["verification"] == "pending"
}
fn effect_requires_stop(result: &Value) -> bool {
    result["verified"] != true
}
/// Whether this very operation already performed this candidate's effect, read from the domain tables
/// rather than from the collector's own progress row: the tombstone history for a reservation, the claim's
/// own closing operation for a failed restore. This is what makes a retry after an interruption safe even
/// if the progress row itself was lost.
fn already_applied(db: &Connection, operation: &str, t: &Target) -> Result<bool, Error> {
    if t.class == CLASS_FAILED_RESTORE_CLAIM {
        return claim_closed_by(db, &t.authorization_id, operation);
    }
    Ok(mg::collection_history(db, &t.universe_uuid)?
        .iter()
        .any(|c| c["collected_by_operation"].as_str() == Some(operation)))
}
fn claim_closed_by(db: &Connection, authorization: &str, operation: &str) -> Result<bool, Error> {
    let row: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT state,detail FROM migration_restore_claims WHERE authorization_id=?1",
            [authorization],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((state, detail)) = row else {
        return Ok(false);
    };
    let detail: Value =
        serde_json::from_str(detail.as_deref().unwrap_or("null")).unwrap_or(Value::Null);
    Ok(state == "not_restored" && detail["closed_by_operation"].as_str() == Some(operation))
}
/// An observation that can never fail the operation that made it: a Podman error becomes a recorded fact.
fn observe_quietly(name: &str) -> (Option<Value>, Option<String>) {
    match lc::inspect(name) {
        Ok(c) => (c, None),
        Err(e) => (None, Some(e.to_string())),
    }
}

/// What one run may touch. Every bound is explicit in the record, whether the caller named it or not.
struct Limits {
    max_candidates: usize,
    max_effects: usize,
    max_runtime_reclaims: usize,
}
fn bounded(request: &Value, key: &str, default: usize, cap: usize) -> Result<usize, Error> {
    match request.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .map(|v| v as usize)
            .filter(|v| *v <= cap)
            .ok_or_else(|| format!("{key} must be an integer from 0 to {cap}").into()),
    }
}
impl Limits {
    fn parse(request: &Value) -> Result<Limits, Error> {
        Ok(Limits {
            max_candidates: bounded(
                request,
                "max_candidates",
                DEFAULT_MAX_CANDIDATES,
                LIMIT_MAX_CANDIDATES,
            )?,
            max_effects: bounded(
                request,
                "max_effects",
                DEFAULT_MAX_EFFECTS,
                LIMIT_MAX_EFFECTS,
            )?,
            max_runtime_reclaims: bounded(
                request,
                "max_runtime_reclaims",
                DEFAULT_MAX_RUNTIME_RECLAIMS,
                LIMIT_MAX_RUNTIME_RECLAIMS,
            )?,
        })
    }
    fn view(&self, mode: Mode) -> Value {
        match mode {
            Mode::Plan => json!({"max_candidates": self.max_candidates}),
            Mode::Apply => {
                json!({"max_candidates": self.max_candidates, "max_effects": self.max_effects,
                "max_runtime_reclaims": self.max_runtime_reclaims})
            }
        }
    }
}

/// One thing an apply request names. A candidate is never discovered by an apply: it repeats the
/// classification of something a recorded plan already found collectable.
struct Target {
    class: String,
    universe_uuid: String,
    authorization_id: String,
}
impl Target {
    fn key(&self) -> &str {
        if self.class == CLASS_FAILED_RESTORE_CLAIM {
            &self.authorization_id
        } else {
            &self.universe_uuid
        }
    }
    fn view(&self) -> Value {
        if self.class == CLASS_FAILED_RESTORE_CLAIM {
            json!({"class": self.class, "authorization_id": self.authorization_id, "universe_uuid": self.universe_uuid})
        } else {
            json!({"class": self.class, "universe_uuid": self.universe_uuid})
        }
    }
}
fn parse_targets(request: &Value, limits: &Limits) -> Result<Vec<Target>, Error> {
    let listed = request
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or("candidates must be an array naming what this apply may collect")?;
    if listed.is_empty() {
        return Err("candidates must name at least one candidate".into());
    }
    if listed.len() > limits.max_candidates {
        return Err(format!(
            "{} candidates exceed the max_candidates bound of {} for this run",
            listed.len(),
            limits.max_candidates
        )
        .into());
    }
    let mut targets = vec![];
    for entry in listed {
        let class = lc::text(entry, "class")?.to_string();
        if ![
            CLASS_TERMINAL_NOT_RESTORED,
            CLASS_TERMINAL_ABSENT,
            CLASS_FAILED_RESTORE_CLAIM,
        ]
        .contains(&class.as_str())
        {
            return Err(format!("{class} is not a collection class of this version").into());
        }
        let universe_uuid = lc::text(entry, "universe_uuid")?.to_string();
        if !lc::is_uuid(&universe_uuid) {
            return Err("universe_uuid must be a UUID".into());
        }
        let authorization_id = if class == CLASS_FAILED_RESTORE_CLAIM {
            let value = lc::text(entry, "authorization_id")?.to_string();
            if !lc::is_uuid(&value) {
                return Err("authorization_id must be a UUID".into());
            }
            value
        } else {
            String::new()
        };
        targets.push(Target {
            class,
            universe_uuid,
            authorization_id,
        });
    }
    Ok(targets)
}

/// A candidate as the record describes it: what it is, which class it belongs to if any, every proof fact
/// observed for it, and every blocker. An unknown fact is a blocker; nothing is guessed.
struct Candidate {
    kind: &'static str,
    key: String,
    universe_uuid: String,
    class: Option<&'static str>,
    class_number: Option<u64>,
    proofs: Value,
    blockers: Vec<String>,
    effect: Value,
}
impl Candidate {
    fn collectable(&self) -> bool {
        self.class.is_some() && self.blockers.is_empty()
    }
    fn view(&self) -> Value {
        json!({"kind": self.kind, "key": self.key, "universe_uuid": self.universe_uuid, "class": self.class,
            "class_number": self.class_number, "collectable": self.collectable(), "proofs": self.proofs,
            "blockers": self.blockers, "proposed_effect": self.effect})
    }
}

/// Every authorization ever recorded for a universe, with the documents themselves, so that a proof can be
/// re-checked against bytes rather than against a state column.
fn authorizations(db: &Connection, uuid: &str) -> Result<Vec<Value>, Error> {
    let mut stmt = db.prepare(
        "SELECT authorization_id,operation_id,checkpoint_operation_id,destination_host_uuid,handoff,handoff_sha256,state,
         created_at,updated_at,outcome,outcome_sha256,completed_by_operation FROM migration_authorizations
         WHERE universe_uuid=?1 ORDER BY created_at, authorization_id",
    )?;
    let rows = stmt.query_map([uuid], |r| {
        Ok(
            json!({"authorization_id": r.get::<_, String>(0)?, "operation_id": r.get::<_, String>(1)?,
            "checkpoint_operation_id": r.get::<_, String>(2)?, "destination_host_uuid": r.get::<_, String>(3)?,
            "handoff": r.get::<_, String>(4)?, "handoff_sha256": r.get::<_, String>(5)?, "state": r.get::<_, String>(6)?,
            "created_at": r.get::<_, i64>(7)?, "updated_at": r.get::<_, i64>(8)?, "outcome": r.get::<_, Option<String>>(9)?,
            "outcome_sha256": r.get::<_, Option<String>>(10)?, "completed_by_operation": r.get::<_, Option<String>>(11)?}),
        )
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
/// Every restore claim recorded for a universe on this host.
fn claims(db: &Connection, uuid: &str) -> Result<Vec<Value>, Error> {
    let mut stmt = db.prepare(
        "SELECT authorization_id,operation_id,state,container_id,created_at,updated_at FROM migration_restore_claims
         WHERE universe_uuid=?1 ORDER BY created_at, authorization_id",
    )?;
    let rows = stmt.query_map([uuid], |r| {
        Ok(
            json!({"authorization_id": r.get::<_, String>(0)?, "operation_id": r.get::<_, String>(1)?,
            "state": r.get::<_, String>(2)?, "container_id": r.get::<_, Option<String>>(3)?,
            "created_at": r.get::<_, i64>(4)?, "updated_at": r.get::<_, i64>(5)?}),
        )
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
/// Unresolved restore claims of this host, oldest first: the class 3 candidates.
fn unresolved_claims(db: &Connection) -> Result<Vec<Value>, Error> {
    let mut stmt = db.prepare(
        "SELECT authorization_id,operation_id,universe_uuid,state,container_id,created_at,updated_at,detail
         FROM migration_restore_claims WHERE state IN ('restoring','restore_failed') ORDER BY created_at, authorization_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(
            json!({"authorization_id": r.get::<_, String>(0)?, "operation_id": r.get::<_, String>(1)?,
            "universe_uuid": r.get::<_, String>(2)?, "state": r.get::<_, String>(3)?,
            "container_id": r.get::<_, Option<String>>(4)?, "created_at": r.get::<_, i64>(5)?,
            "updated_at": r.get::<_, i64>(6)?,
            "detail": r.get::<_, Option<String>>(7)?.and_then(|d| serde_json::from_str::<Value>(&d).ok())}),
        )
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
fn one_claim(db: &Connection, authorization: &str) -> Result<Option<Value>, Error> {
    Ok(unresolved_claims(db)?
        .into_iter()
        .find(|k| k["authorization_id"].as_str() == Some(authorization)))
}

/// The freshly observed universe container, and what it proves. `all` is one bounded inventory read.
fn container_facts(
    observed: Option<&Value>,
    r: &Reservation,
    uuid: &str,
    all: &[Value],
    blockers: &mut Vec<String>,
) -> Value {
    let elsewhere = all
        .iter()
        .any(|c| c["Id"].as_str() == Some(r.container_id.as_str()));
    let Some(c) = observed else {
        return json!({"observed_at": crate::now(), "present": false, "recorded_container_id": r.container_id,
            "recorded_container_present_under_another_name": elsewhere});
    };
    let id = c["Id"].as_str().unwrap_or("");
    let state = lc::status(c);
    let frozen = cleanup::frozen(id);
    if id != r.container_id {
        blockers.push(format!(
            "the container under the universe name is {id}, not the reserved {}: the source was replaced",
            r.container_id
        ));
    }
    if c["Config"]["Labels"]["io.podmesh.universe"].as_str() != Some(uuid) {
        blockers.push(
            "the container under the universe name does not carry this universe label".into(),
        );
    }
    if !lc::STOPPED.contains(&state) {
        blockers.push(format!(
            "the universe container is in state {state}: only a container observed created, exited or stopped is collectable"
        ));
    }
    if frozen {
        blockers.push(
            "the universe container's cgroup is frozen: its state cannot be observed reliably"
                .into(),
        );
    }
    json!({"observed_at": crate::now(), "present": true, "recorded_container_id": r.container_id,
        "observed_container_id": id, "same_reserved_container": id == r.container_id, "state": state,
        "checkpointed": c["State"]["Checkpointed"], "cgroup_frozen": frozen,
        "universe_label": c["Config"]["Labels"]["io.podmesh.universe"], "observed": lc::state_view(c)})
}

/// What the reservation's artifact directory still holds. The collector does not re-hash an archive: this
/// version collects no artifact, and a plan over many reservations must not read hundreds of megabytes to
/// answer a question it is not asking. Sizes and presence are recorded; nothing is claimed about content.
fn artifact_facts(operation: &str) -> Value {
    let Ok(dir) = mg::base().map(|b| b.join(operation)) else {
        return json!({"known": false, "reason": "the migration state directory is not prepared"});
    };
    let file = |name: &str| match std::fs::symlink_metadata(dir.join(name)) {
        Ok(m) if m.is_file() => json!({"present": true, "bytes": m.len()}),
        _ => json!({"present": false}),
    };
    json!({"directory": dir, "directory_present": dir.is_dir(), "archive": file(mg::ARCHIVE),
        "manifest": file(mg::MANIFEST),
        "note": "presence and size only: this version collects no artifact and re-hashes none"})
}

/// Whether an authorization's recorded outcome really binds to it: re-hashed, re-parsed and re-checked
/// field by field. A journal row that says `ended_not_restored` is not by itself a proof of anything.
fn outcome_binding(a: &Value, uuid: &str, host: &str) -> (Value, Vec<String>) {
    let id = a["authorization_id"].as_str().unwrap_or("");
    let mut blockers = vec![];
    let handoff_rehash = mg::sha256_bytes(a["handoff"].as_str().unwrap_or("").as_bytes()).ok();
    if handoff_rehash.as_deref() != a["handoff_sha256"].as_str() {
        blockers.push(format!(
            "the handoff recorded for authorization {id} no longer hashes to the value recorded when it was issued"
        ));
    }
    let Some(text) = a["outcome"].as_str() else {
        blockers.push(format!(
            "authorization {id} has no recorded outcome document"
        ));
        return (
            json!({"authorization_id": id, "outcome_present": false}),
            blockers,
        );
    };
    let rehash = mg::sha256_bytes(text.as_bytes()).ok();
    if rehash.as_deref() != a["outcome_sha256"].as_str() {
        blockers.push(format!(
            "the outcome recorded for authorization {id} no longer hashes to the value recorded when it was completed"
        ));
    }
    let outcome: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    if !outcome.is_object() {
        blockers.push(format!(
            "the outcome recorded for authorization {id} is not a JSON object"
        ));
        return (
            json!({"authorization_id": id, "outcome_present": true, "outcome_malformed": true, "outcome_sha256_now": rehash}),
            blockers,
        );
    }
    let s = |k: &str| outcome[k].as_str().unwrap_or("");
    for (field, observed, expected) in [
        ("format", s("format"), tr::OUTCOME_FORMAT),
        ("authorization_id", s("authorization_id"), id),
        (
            "handoff_sha256",
            s("handoff_sha256"),
            a["handoff_sha256"].as_str().unwrap_or(""),
        ),
        ("universe_uuid", s("universe_uuid"), uuid),
        ("source_host_uuid", s("source_host_uuid"), host),
        (
            "destination_host_uuid",
            s("destination_host_uuid"),
            a["destination_host_uuid"].as_str().unwrap_or(""),
        ),
        ("result", s("result"), "not_restored"),
    ] {
        if observed != expected {
            blockers.push(format!(
                "the outcome of authorization {id} names {field} {observed:?} instead of {expected:?}: it does not bind to this authorization"
            ));
        }
    }
    if !outcome["restored_container_id"].is_null() {
        blockers.push(format!(
            "the outcome of authorization {id} names a restored container: it is not a proof of non-restoration"
        ));
    }
    if a["completed_by_operation"].as_str().is_none() {
        blockers.push(format!(
            "authorization {id} records no completing operation"
        ));
    }
    (
        json!({"authorization_id": id, "outcome_present": true, "outcome_sha256_now": rehash,
            "handoff_sha256_now": handoff_rehash, "destination_host_uuid": a["destination_host_uuid"],
            "result": s("result"), "decided_at": outcome["decided_at"], "completed_by_operation": a["completed_by_operation"],
            "binds_to_this_authorization": blockers.is_empty()}),
        blockers,
    )
}

/// Classifies one reservation against the contract's terminal classes, from freshly observed facts.
fn classify_reservation(
    db: &Connection,
    uuid: &str,
    r: &Reservation,
    observed: Option<&Value>,
    all: &[Value],
    host: &str,
) -> Result<Candidate, Error> {
    let mut blockers = vec![];
    let recorded = authorizations(db, uuid)?;
    let own: Vec<&Value> = recorded
        .iter()
        .filter(|a| a["checkpoint_operation_id"].as_str() == Some(r.operation_id.as_str()))
        .collect();
    let open: Vec<&Value> = recorded
        .iter()
        .filter(|a| a["state"].as_str() == Some("issued"))
        .collect();
    let restored: Vec<&Value> = recorded
        .iter()
        .filter(|a| a["state"].as_str() == Some("completed_restored"))
        .collect();
    let live_claims = claims(db, uuid)?;
    let unresolved: Vec<&Value> = live_claims
        .iter()
        .filter(|k| UNRESOLVED_CLAIMS.contains(&k["state"].as_str().unwrap_or("")))
        .collect();
    let container = container_facts(observed, r, uuid, all, &mut blockers);
    let artifacts = artifact_facts(&r.operation_id);
    let detail = r.detail_value();

    // Facts every class shares, whatever the verdict is.
    for a in &open {
        blockers.push(format!(
            "transfer authorization {} is still open (state issued) for this universe: only a verified destination outcome bound to it can end it",
            a["authorization_id"].as_str().unwrap_or("")
        ));
    }
    for k in &unresolved {
        blockers.push(format!(
            "restore claim {} of this universe is unresolved (state {}) on this host",
            k["authorization_id"].as_str().unwrap_or(""),
            k["state"].as_str().unwrap_or("")
        ));
    }
    if !detail["authorization_id"].is_null() {
        blockers.push(format!(
            "the reservation still holds authorization {}",
            detail["authorization_id"].as_str().unwrap_or("")
        ));
    }
    let present = container["present"] == true;
    let same = container["same_reserved_container"] == true;
    let checkpointed = container["checkpointed"] == true;
    let mut outcomes = vec![];
    let (class, class_number) = if !own.is_empty() {
        // Class 1: every authorization this reservation emitted must be terminal and proven not_restored.
        for a in &own {
            let state = a["state"].as_str().unwrap_or("");
            if state != "ended_not_restored" {
                blockers.push(format!(
                    "authorization {} of this reservation is in state {state}, not ended_not_restored",
                    a["authorization_id"].as_str().unwrap_or("")
                ));
                outcomes.push(json!({"authorization_id": a["authorization_id"], "state": state, "binds_to_this_authorization": false}));
                continue;
            }
            let (view, mut found) = outcome_binding(a, uuid, host);
            let mut view = view;
            view["state"] = json!(state);
            outcomes.push(view);
            blockers.append(&mut found);
        }
        if r.state != "checkpointed" {
            blockers.push(format!(
                "the reservation is in state {}: class 1 collects a checkpointed reservation whose authorizations all ended not_restored",
                r.state
            ));
        }
        if !present {
            blockers.push(
                "the reserved container is absent: class 1 requires a fresh observation of the stopped, checkpointed source it releases"
                    .into(),
            );
        } else if same && !checkpointed {
            blockers.push("the reserved container is no longer in its checkpointed state".into());
        }
        (Some(CLASS_TERMINAL_NOT_RESTORED), Some(1))
    } else if !present && container["recorded_container_present_under_another_name"] != true {
        // Class 2: the container disappeared and no authorization was ever issued for this reservation.
        if !OPEN_STATES.contains(&r.state.as_str()) {
            blockers.push(format!(
                "the reservation is in state {}: it is already settled",
                r.state
            ));
        }
        if !restored.is_empty() {
            blockers.push(
                "an authorization of this universe completed with a verified restore elsewhere"
                    .into(),
            );
        }
        (Some(CLASS_TERMINAL_ABSENT), Some(2))
    } else {
        // Neither class fits: say why, and never widen a class to make it fit.
        if !present {
            blockers.push("the reserved container is absent under its name but still exists under another name".into());
        } else if own.is_empty() {
            blockers.push(
                "the reserved container is still present and no authorization was ever issued for this reservation: this is a release or an abandonment decision (migration_release, migration_abandon), not a collection"
                    .into(),
            );
        }
        (None, None)
    };
    let effect = match class {
        Some(CLASS_TERMINAL_NOT_RESTORED) => {
            json!({"operation": "garbage_collect_apply", "action": "collect_reservation",
            "reservation_state": COLLECTED, "tombstone": true,
            "lifts": "create is refused for this universe UUID by the tombstone; start, stop, delete, clone and migration_restore_local of the recorded container become available again",
            "starts_nothing": true})
        }
        Some(CLASS_TERMINAL_ABSENT) => {
            json!({"operation": "garbage_collect_apply", "action": "collect_reservation",
            "reservation_state": COLLECTED, "tombstone": true,
            "lifts": "nothing to operate: the container is gone; create stays refused for this universe UUID and the absent container ID never owns it again",
            "starts_nothing": true})
        }
        _ => json!({"operation": null, "action": "none"}),
    };
    Ok(Candidate {
        kind: "reservation",
        key: uuid.to_string(),
        universe_uuid: uuid.to_string(),
        class,
        class_number,
        proofs: json!({"reservation": r.view(), "container": container, "artifacts": artifacts,
            "authorizations_of_this_reservation": own.len(), "authorizations_of_this_universe": recorded.len(),
            "open_authorizations": open.len(), "outcomes": outcomes,
            "restore_claims": live_claims.iter().map(|k| json!({"authorization_id": k["authorization_id"], "state": k["state"]}))
                .collect::<Vec<_>>(),
            "unresolved_restore_claims": unresolved.len()}),
        blockers,
        effect,
    })
}

/// Classifies one unresolved restore claim. The collector proposes; the effect is `migration_restore_abort`.
fn classify_claim(
    db: &Connection,
    k: &Value,
    observed: Option<&Value>,
) -> Result<Candidate, Error> {
    let authorization = k["authorization_id"].as_str().unwrap_or("").to_string();
    let uuid = k["universe_uuid"].as_str().unwrap_or("").to_string();
    let claimed_at = k["created_at"].as_i64().unwrap_or(0);
    let operation = k["operation_id"].as_str().unwrap_or("");
    let mut blockers = vec![];
    let scope = format!("podmesh-restore-{operation}.scope");
    let busy = mg::unit_busy(&scope);
    if busy {
        blockers.push(format!(
            "the restore scope {scope} of this claim has not finished, or its state cannot be queried"
        ));
    }
    // The processes a failed attempt may have left, by cgroup residency; a command-line count authorizes nothing.
    let processes = match k["container_id"].as_str() {
        Some(id) => cleanup::runtime_processes(id, claimed_at),
        None => json!({"observed_at": crate::now(), "known": false,
            "reason": "this claim recorded no container, so no cgroup can be read for it"}),
    };
    let mut container = json!({"observed_at": crate::now(), "present": false});
    if let Some(c) = observed {
        let id = c["Id"].as_str().unwrap_or("");
        let created_after = c["Created"]
            .as_str()
            .and_then(lc::epoch)
            .is_some_and(|t| t >= claimed_at);
        let labelled = c["Config"]["Labels"]["io.podmesh.universe"].as_str() == Some(uuid.as_str());
        let owner = ds::verified_owner(db, id)?;
        container = json!({"observed_at": crate::now(), "present": true, "container_id": id, "state": lc::status(c),
            "created_after_claim": created_after, "carries_universe_label": labelled,
            "owned_by_a_verified_operation": owner, "cgroup_frozen": cleanup::frozen(id), "observed": lc::state_view(c)});
        if lc::process_active(c) {
            blockers
                .push("the container under the universe name is running: a failed restore's abort never touches a running universe".into());
        }
        if !labelled {
            blockers
                .push("the container under the universe name does not carry this universe label, so this claim did not create it".into());
        }
        if !created_after {
            blockers.push("the container under the universe name predates this claim".into());
        }
        if owner {
            blockers.push("the container under the universe name is owned by a verified operation of this host".into());
        }
    }
    let surviving = processes["count"].as_u64().unwrap_or(0);
    let authoritative = processes["authorizes_reclaim"] == true;
    Ok(Candidate {
        kind: "restore_claim",
        key: authorization.clone(),
        universe_uuid: uuid,
        class: Some(CLASS_FAILED_RESTORE_CLAIM),
        class_number: Some(3),
        proofs: json!({"claim": k, "restore_scope": {"unit": scope, "busy": busy}, "container": container,
            "runtime_processes": processes}),
        blockers,
        effect: json!({"operation": "garbage_collect_apply", "action": "abort_failed_restore",
            "delegates_to": "migration_restore_abort",
            "authorization_id": authorization,
            "requires_reclaim_processes": authoritative && surviving > 0,
            "surviving_processes": if authoritative { json!(surviving) } else { Value::Null },
            "note": "the effect is migration_restore_abort's own contract: it removes only a non-running container created by this claim, and ends processes only with an explicit reclaim_processes and only those it can prove belong to the attempt. The collector manages no process itself."}),
    })
}

/// One bounded inventory read and one bounded inspection of the candidate names that exist, so that a plan
/// over many reservations still costs two read-only Podman calls.
fn observe_names(all: &[Value], names: &[String]) -> Result<Vec<Value>, Error> {
    let present: Vec<&str> = names
        .iter()
        .filter(|n| {
            all.iter().any(|c| {
                c["Names"]
                    .as_array()
                    .map(|v| v.iter().any(|x| x.as_str() == Some(n.as_str())))
                    .unwrap_or(false)
            })
        })
        .map(String::as_str)
        .collect();
    if present.is_empty() {
        return Ok(vec![]);
    }
    let mut args = vec!["container", "inspect"];
    args.extend(present.iter().copied());
    // One call for the whole plan. A container removed between the inventory and the inspection makes
    // Podman refuse the batch; the fallback then observes each name on its own, and an absent one is
    // simply absent.
    match lc::podman(lc::QUICK, &args) {
        Ok(text) => Ok(serde_json::from_str::<Value>(&text)?
            .as_array()
            .cloned()
            .unwrap_or_default()),
        Err(_) => {
            let mut found = vec![];
            for name in present {
                if let Some(c) = lc::inspect(name)? {
                    found.push(c);
                }
            }
            Ok(found)
        }
    }
}
fn named<'a>(inspected: &'a [Value], name: &str) -> Option<&'a Value> {
    inspected.iter().find(|c| {
        c["Name"].as_str() == Some(name)
            || c["Names"]
                .as_array()
                .map(|v| v.iter().any(|x| x.as_str() == Some(name)))
                .unwrap_or(false)
    })
}

/// The universes an explicitly scoped plan may look at. Without one a plan is host-wide, and its bound then
/// decides what it reaches; with one it answers about exactly what the caller named, which is what an
/// operator preparing a specific collection, and a host whose journal is larger than any single bound, need.
fn parse_scope(request: &Value, limits: &Limits) -> Result<Option<Vec<String>>, Error> {
    let Some(listed) = request.get("universe_uuids") else {
        return Ok(None);
    };
    let listed = listed
        .as_array()
        .ok_or("universe_uuids must be an array of universe UUIDs")?;
    if listed.is_empty() || listed.len() > limits.max_candidates {
        return Err(format!(
            "universe_uuids must name from 1 to max_candidates ({}) universes",
            limits.max_candidates
        )
        .into());
    }
    let mut scope = vec![];
    for value in listed {
        let uuid = value
            .as_str()
            .filter(|v| lc::is_uuid(v))
            .ok_or("universe_uuids must be UUIDs")?;
        scope.push(uuid.to_string());
    }
    Ok(Some(scope))
}

/// Read-only. Enumerates a bounded set of candidates, their class, every proof fact observed and every
/// blocker. No Podman mutation, no signal, no state change beyond this run's own record.
fn plan(db: &Connection, limits: &Limits, scope: Option<&Vec<String>>) -> Result<Value, Error> {
    let host = mg::host_uuid(db)?;
    let all = tr::all_containers()?;
    let mut reservations = mg::reservations(db)?;
    if let Some(scope) = scope {
        reservations.retain(|(u, _)| scope.contains(u));
    }
    let settled: Vec<Value> = reservations
        .iter()
        .filter(|(_, r)| !OPEN_STATES.contains(&r.state.as_str()))
        .map(|(u, r)| json!({"universe_uuid": u, "state": r.state}))
        .collect();
    let open: Vec<&(String, Reservation)> = reservations
        .iter()
        .filter(|(_, r)| OPEN_STATES.contains(&r.state.as_str()))
        .collect();
    let mut claims = unresolved_claims(db)?;
    if let Some(scope) = scope {
        claims.retain(|k| scope.contains(&k["universe_uuid"].as_str().unwrap_or("").to_string()));
    }
    // One bound for the whole run, shared so that neither kind starves the other: each is guaranteed half
    // of it and may use whatever the other leaves. A host with many dead reservations must not hide the
    // failed restore claim that is holding processes and disk right now.
    let half = limits.max_candidates.div_ceil(2);
    let claims_budget = limits.max_candidates.saturating_sub(open.len().min(half));
    let examined_claims: Vec<&Value> = claims.iter().take(claims_budget).collect();
    let examined_reservations: Vec<&&(String, Reservation)> = open
        .iter()
        .take(limits.max_candidates.saturating_sub(examined_claims.len()))
        .collect();
    let truncated =
        open.len() > examined_reservations.len() || claims.len() > examined_claims.len();

    let mut names: Vec<String> = examined_reservations
        .iter()
        .map(|(u, _)| format!("podmesh-{u}"))
        .collect();
    names.extend(
        examined_claims
            .iter()
            .map(|k| format!("podmesh-{}", k["universe_uuid"].as_str().unwrap_or(""))),
    );
    names.sort();
    names.dedup();
    let inspected = observe_names(&all, &names)?;

    let mut candidates = vec![];
    for (uuid, r) in &examined_reservations {
        let observed = named(&inspected, &format!("podmesh-{uuid}"));
        candidates.push(classify_reservation(db, uuid, r, observed, &all, &host)?);
    }
    for k in &examined_claims {
        let observed = named(
            &inspected,
            &format!("podmesh-{}", k["universe_uuid"].as_str().unwrap_or("")),
        );
        candidates.push(classify_claim(db, k, observed)?);
    }
    let collectable = candidates.iter().filter(|c| c.collectable()).count();
    let mut by_class = json!({});
    for c in candidates.iter().filter(|c| c.collectable()) {
        let key = c.class.unwrap_or("");
        by_class[key] = json!(by_class[key].as_u64().unwrap_or(0) + 1);
    }
    Ok(json!({
        "host_uuid": host,
        "scope": match scope {
            None => json!({"universes": "every reservation and unresolved restore claim on this host, up to the bound"}),
            Some(s) => json!({"universe_uuids": s}),
        },
        "limits": {"max_candidates": limits.max_candidates, "reservations_open": open.len(),
            "reservations_examined": examined_reservations.len(), "unresolved_restore_claims": claims.len(),
            "restore_claims_examined": examined_claims.len(), "truncated": truncated,
            "note": "a settled reservation (released, abandoned, transferred, collected) is a finished record, not a decision owed to someone: it is counted, never examined. Only the number of candidates is bounded: the counts of settled rows, and the size of one candidate's proofs, are not"},
        "settled_reservations": {"count": settled.len(), "states": settled},
        "candidates": candidates.iter().map(Candidate::view).collect::<Vec<_>>(),
        "counts": {"examined": candidates.len(), "collectable": collectable,
            "blocked": candidates.len() - collectable, "collectable_by_class": by_class},
        "effects": "none: a plan makes no Podman mutation, sends no signal, deletes nothing, changes no reservation, claim, authorization or artifact, and writes only its own immutable run record",
    }))
}

/// The record of the plan an apply names. An apply collects only what a recorded plan of this host already
/// found collectable, and only if the proofs still hold when it repeats them.
fn plan_record(db: &Connection, plan_operation: &str) -> Result<Value, Error> {
    let row: Option<(String, String)> = db
        .query_row(
            "SELECT mode,record FROM garbage_collection_runs WHERE operation_id=?1",
            [plan_operation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (mode, record) = row.ok_or_else(|| {
        failure(
            format!("No garbage collection run {plan_operation} is recorded on this host; an apply names the plan it applies"),
            json!({"plan_operation_id": plan_operation}),
        )
    })?;
    if mode != Mode::Plan.name() {
        return Err(failure(
            format!("Run {plan_operation} was recorded in {mode} mode, not as a plan"),
            json!({"plan_operation_id": plan_operation}),
        ));
    }
    Ok(serde_json::from_str(&record)?)
}
fn planned(record: &Value, t: &Target) -> Result<Value, Error> {
    let found = record["candidates"]
        .as_array()
        .map(|v| v.as_slice())
        .unwrap_or(&[])
        .iter()
        .find(|c| c["key"].as_str() == Some(t.key()));
    let Some(c) = found else {
        return Err(failure(
            format!(
                "The plan did not examine {}; nothing was collected",
                t.key()
            ),
            json!({"candidate": t.view()}),
        ));
    };
    if c["class"].as_str() != Some(t.class.as_str()) {
        return Err(failure(
            format!(
                "The plan classified {} as {}, not as {}; nothing was collected",
                t.key(),
                c["class"],
                t.class
            ),
            json!({"candidate": t.view(), "planned": c}),
        ));
    }
    if c["collectable"] != true {
        return Err(failure(
            format!("The plan found {} blocked; nothing was collected", t.key()),
            json!({"candidate": t.view(), "planned_blockers": c["blockers"]}),
        ));
    }
    Ok(c.clone())
}

/// Whether the container a collection released is still the one it was proven against. `before` is absent
/// when a recovered effect is re-verified: there is nothing left to compare against, and the honest answer
/// is that this particular check no longer applies, not that it passed.
fn container_unchanged(
    before: Option<&Value>,
    after: Option<&Value>,
    error: Option<&str>,
) -> Option<bool> {
    if error.is_some() {
        return None;
    }
    let before = before?;
    Some(match (after, before["present"] == true) {
        (Some(c), true) => {
            c["Id"].as_str() == before["observed_container_id"].as_str()
                && lc::status(c) == before["state"].as_str().unwrap_or("")
                && c["State"]["Checkpointed"] == before["checkpointed"]
                && !lc::process_active(c)
        }
        (None, false) => true,
        _ => false,
    })
}
/// The verdict on what a collection left behind. Pure, so that every branch — including an observation that
/// could not be made at all — is decided in one place and can be tested without a laboratory.
fn reservation_verdict(
    state: &str,
    tombstone: bool,
    create_refused: bool,
    unchanged: Option<bool>,
    error: Option<&str>,
) -> Vec<String> {
    let mut blockers = vec![];
    if state != COLLECTED {
        blockers.push(format!(
            "the reservation is in state {state} after the collection"
        ));
    }
    if !tombstone {
        blockers.push("no tombstone is recorded for the collected universe".into());
    }
    if !create_refused {
        blockers
            .push("the tombstone does not refuse a create of the collected universe UUID".into());
    }
    if let Some(e) = error {
        blockers.push(format!(
            "the universe container could not be observed after the collection, so its state is unknown: {e}"
        ));
    }
    if unchanged == Some(false) {
        blockers.push(
            "the universe container is not the one observed immediately before the collection"
                .into(),
        );
    }
    blockers
}
/// The verdict on what a delegated abort left behind. An unknown observation is unknown, never a success.
fn claim_verdict(
    claim_unresolved: bool,
    claim_error: Option<&str>,
    container_present: bool,
    error: Option<&str>,
    processes: Option<&Value>,
) -> Vec<String> {
    let mut blockers = vec![];
    if let Some(e) = claim_error {
        blockers.push(format!(
            "the restore claim could not be read after the abort, so its terminal state is unknown: {e}"
        ));
    }
    if claim_unresolved {
        blockers.push("the restore claim is still unresolved after the abort".into());
    }
    if let Some(e) = error {
        blockers.push(format!(
            "the universe container could not be observed after the abort, so its absence is unknown: {e}"
        ));
    } else if container_present {
        blockers
            .push("a container is still present under the universe name after the abort".into());
    }
    match processes {
        None => {}
        Some(p) if p["authorizes_reclaim"] == true => {
            let count = p["count"].as_u64().unwrap_or(0);
            if count > 0 {
                blockers.push(format!(
                    "{count} process(es) of the failed attempt are still in the claimed container's cgroups after the abort"
                ));
            }
        }
        Some(p) if p["known"] == false => blockers.push(format!(
            "whether processes of the failed attempt survive could not be established: {}",
            p["reason"].as_str().unwrap_or("no reason recorded")
        )),
        // The cgroups are gone, so the only remaining reading is the command-line fallback, which the
        // contract says authorizes nothing. Its count is reported, never treated as a survivor.
        Some(_) => {}
    }
    blockers
}

/// Refusals that are completely knowable before `migration_restore_abort` is entered. Keeping these ahead
/// of the write-ahead uncertainty marker distinguishes a proven no-effect refusal from any error returned
/// after the delegated operation was allowed to act.
fn predelegation_abort_check(processes: &Value, reclaim: bool) -> Result<(), String> {
    if processes["known"] != true {
        return Err(format!(
            "Whether processes of the failed restore survive could not be established: {}",
            processes["reason"].as_str().unwrap_or("no reason recorded")
        ));
    }
    let count = processes["count"]
        .as_u64()
        .ok_or("The failed restore process observation has no valid count")?;
    let authorizes = processes["authorizes_reclaim"]
        .as_bool()
        .ok_or("The failed restore process observation has no valid authority verdict")?;
    if authorizes && count > 0 && !reclaim {
        return Err(format!(
            "{count} process(es) of the failed restore are still in its container cgroups. They are reported, not ended; retry with reclaim_processes: true"
        ));
    }
    Ok(())
}

/// The delegated abort's disk observation is part of its externally checked result. Missing, unknown or
/// internally inconsistent measurements can never be upgraded to a verified reclaim by the collector.
fn graph_measurement_verdict(outcome: &Value) -> Vec<String> {
    let graph = &outcome["graph_root"];
    if graph["known"] != true {
        return vec![format!(
            "graph-root recovery could not be verified because its measurement is unknown: {}",
            graph["error"]
                .as_str()
                .unwrap_or("no known graph-root measurement was recorded")
        )];
    }
    let (Some(before), Some(after), Some(recovered)) = (
        graph["available_bytes_before"].as_u64(),
        graph["available_bytes_after"].as_u64(),
        graph["recovered_bytes"].as_i64(),
    ) else {
        return vec!["graph-root recovery could not be verified because before, after, or recovered bytes are missing or invalid".into()];
    };
    if i128::from(recovered) != i128::from(after) - i128::from(before) {
        return vec![
            "graph-root recovered bytes do not match the recorded before and after measurements"
                .into(),
        ];
    }
    vec![]
}

/// Re-reads what a collection left behind, and never fails the operation that made it. Used immediately
/// after the effect and again when a run recovers an effect it had already committed.
fn verify_collection(
    db: &Connection,
    t: &Target,
    class_number: Option<u64>,
    before: Option<&Value>,
    from_state: &Value,
) -> Value {
    let uuid = t.universe_uuid.as_str();
    let (after, error) = observe_quietly(&format!("podmesh-{uuid}"));
    let (state_now, reservation_error) = match mg::reservation(db, uuid) {
        Ok(Some(r)) => (r.state, None),
        Ok(None) => (String::new(), None),
        Err(e) => (String::new(), Some(e.to_string())),
    };
    let (tombstone, tombstone_error) = match mg::tombstone(db, uuid) {
        Ok(value) => (value, None),
        Err(e) => (None, Some(e.to_string())),
    };
    let (history, history_error) = match mg::collection_history(db, uuid) {
        Ok(value) => (json!(value), None),
        Err(e) => (Value::Null, Some(e.to_string())),
    };
    let (create_refused, create_refusal_error) = match mg::refuse_identity_reuse(db, uuid, "create")
    {
        Ok(()) => (false, None),
        Err(e) if e.downcast_ref::<lc::Failure>().is_some() => (true, None),
        Err(e) => (false, Some(e.to_string())),
    };
    let (generic_refused, generic_refusal_error) = match mg::refuse_if_reserved(db, uuid, "start") {
        Ok(()) => (false, None),
        Err(e) if e.downcast_ref::<lc::Failure>().is_some() => (true, None),
        Err(e) => (false, Some(e.to_string())),
    };
    let unchanged = container_unchanged(before, after.as_ref(), error.as_deref());
    let mut blockers = reservation_verdict(
        &state_now,
        tombstone.is_some(),
        create_refused,
        unchanged,
        error.as_deref(),
    );
    for (fact, failure) in [
        ("reservation", reservation_error.as_deref()),
        ("tombstone", tombstone_error.as_deref()),
        ("collection history", history_error.as_deref()),
        ("create refusal", create_refusal_error.as_deref()),
        ("generic-operation gate", generic_refusal_error.as_deref()),
    ] {
        if let Some(e) = failure {
            blockers.push(format!(
                "the {fact} could not be verified from the durable state: {e}"
            ));
        }
    }
    json!({"action": "collected_reservation", "universe_uuid": uuid, "class": t.class, "class_number": class_number,
        "collected_from_state": from_state, "reservation": {"state": state_now}, "tombstone": tombstone,
        "collection_history": history,
        "verified": blockers.is_empty(), "verification_blockers": blockers,
        "verified_from_outside": {"observed_at": crate::now(), "reservation_state": state_now,
            "tombstone_present": tombstone.is_some(), "create_of_this_universe_uuid_refused": create_refused,
            "generic_operations_refused": generic_refused, "container_unchanged_by_the_collection": unchanged,
            "container_before": before, "container": after.as_ref().map(lc::state_view),
            "observation_error": error,
            "durable_observation_errors": {"reservation": reservation_error, "tombstone": tombstone_error,
                "collection_history": history_error, "create_refusal": create_refusal_error,
                "generic_operation_gate": generic_refusal_error}},
        "starts_nothing": "the collection changed a recorded decision; it did not start, stop or remove anything",
        "history": "the reservation row, its artifacts, every authorization and every outcome are kept; the tombstone keeps the first proof and the collection history keeps every occurrence"})
}

fn secure_barrier_directory(path: &Path, owner_uid: u32) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("the collector barrier directory is not absolute".into());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("the collector barrier directory cannot be inspected: {e}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("the collector barrier path is not a non-symlink directory".into());
    }
    if metadata.uid() != owner_uid || metadata.mode() & 0o777 != 0o700 {
        return Err(format!(
            "the collector barrier directory must be owned by UID {owner_uid} with mode 0700"
        ));
    }
    Ok(())
}

fn secure_control(path: &Path, owner_uid: u32) -> Result<Option<Value>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "{} is not a non-symlink regular file",
            path.display()
        ));
    }
    if metadata.uid() != owner_uid || metadata.mode() & 0o777 != 0o600 {
        return Err(format!(
            "{} must be owned by UID {owner_uid} with mode 0600",
            path.display()
        ));
    }
    if metadata.len() > 4096 {
        return Err(format!("{} exceeds 4096 bytes", path.display()));
    }
    let raw = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_slice(&raw)
        .map(Some)
        .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
}

fn exact_barrier_control(value: &Value, operation_id: &str, phase: &str) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == 3
            && value["format"] == TEST_BARRIER_FORMAT
            && value["operation_id"] == operation_id
            && value["phase"] == phase
    })
}

fn write_reached_marker(
    directory: &Path,
    operation_id: &str,
    target: &Target,
    owner_uid: u32,
) -> Result<Value, String> {
    let reached = directory.join(format!("reached-{operation_id}.json"));
    if fs::symlink_metadata(&reached).is_ok() {
        return Err(format!(
            "the collector barrier reached marker already exists: {}",
            reached.display()
        ));
    }
    let marker = json!({
        "format": TEST_BARRIER_FORMAT,
        "operation_id": operation_id,
        "candidate_key": target.key(),
        "universe_uuid": target.universe_uuid,
        "class": target.class,
        "phase": "effect_committed_verification_pending",
    });
    let encoded = serde_json::to_vec(&marker)
        .map_err(|e| format!("the collector barrier marker cannot be encoded: {e}"))?;
    let temporary = directory.join(format!(
        ".reached-{operation_id}.tmp-{}",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| format!("cannot create {}: {e}", temporary.display()))?;
    let result = (|| {
        file.write_all(&encoded)
            .map_err(|e| format!("cannot write {}: {e}", temporary.display()))?;
        file.write_all(b"\n")
            .map_err(|e| format!("cannot finish {}: {e}", temporary.display()))?;
        file.sync_all()
            .map_err(|e| format!("cannot fsync {}: {e}", temporary.display()))?;
        let metadata = file
            .metadata()
            .map_err(|e| format!("cannot inspect {}: {e}", temporary.display()))?;
        if metadata.uid() != owner_uid || metadata.mode() & 0o777 != 0o600 {
            return Err(format!(
                "{} was not created with the required UID {owner_uid} and mode 0600",
                temporary.display()
            ));
        }
        fs::rename(&temporary, &reached).map_err(|e| {
            format!(
                "cannot atomically publish {} as {}: {e}",
                temporary.display(),
                reached.display()
            )
        })?;
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| format!("cannot fsync {}: {e}", directory.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(marker)
}

fn post_commit_barrier_in(
    directory: &Path,
    operation_id: &str,
    target: &Target,
    owner_uid: u32,
    max_wait: Duration,
) -> Result<Option<Value>, String> {
    secure_barrier_directory(directory, owner_uid)?;
    let arm_path = directory.join(format!("arm-{operation_id}.json"));
    let Some(arm) = secure_control(&arm_path, owner_uid)? else {
        return Ok(None);
    };
    if !exact_barrier_control(&arm, operation_id, "arm") {
        return Err(format!(
            "{} does not contain the exact arm control",
            arm_path.display()
        ));
    }
    let marker = write_reached_marker(directory, operation_id, target, owner_uid)?;
    let release_path = directory.join(format!("release-{operation_id}.json"));
    let bounded_wait = max_wait.min(TEST_BARRIER_MAX_WAIT);
    let deadline = Instant::now() + bounded_wait;
    loop {
        if let Some(release) = secure_control(&release_path, owner_uid)? {
            if !exact_barrier_control(&release, operation_id, "release") {
                return Err(format!(
                    "{} does not contain the exact release control",
                    release_path.display()
                ));
            }
            return Ok(Some(json!({
                "armed": true,
                "released": true,
                "reached_marker": marker,
            })));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the collector test barrier was not released within {} seconds",
                bounded_wait.as_secs_f64()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn post_commit_test_barrier(operation_id: &str, target: &Target) -> Result<Option<Value>, String> {
    if target.class != CLASS_TERMINAL_ABSENT {
        return Ok(None);
    }
    let Some(directory) = env::var_os(TEST_BARRIER_ENV).map(PathBuf::from) else {
        return Ok(None);
    };
    if !lc::is_uuid(operation_id) {
        return Err("the collector test barrier requires an exact UUID operation ID".into());
    }
    post_commit_barrier_in(&directory, operation_id, target, 0, TEST_BARRIER_MAX_WAIT)
}

fn failed_barrier_evidence(error: &str) -> Value {
    json!({
        "configured": true,
        "armed": Value::Null,
        "reached": Value::Null,
        "released": false,
        "error": error,
    })
}

/// The terminal state and the tombstone of a collected reservation, written in one transaction with the
/// run's own progress row, and then verified from outside. Nothing after the commit may fail this call: a
/// committed effect is a fact, and a verification that cannot be made is an unknown, never a refusal.
fn collect_reservation(
    db: &Connection,
    id: &str,
    reference: &str,
    t: &Target,
    candidate: &Candidate,
) -> Result<Value, Error> {
    let uuid = t.universe_uuid.as_str();
    let r = mg::reservation(db, uuid)?
        .ok_or("The reservation disappeared between the proof and the effect")?;
    let before = candidate.proofs["container"].clone();
    let now = crate::now() as i64;
    let absent = before["present"] != true;
    let from_state = json!(r.state);
    let proof = json!({"class": t.class, "class_number": candidate.class_number, "collected_by_operation": id,
        "authorization_ref": reference, "collected_at": now, "collected_from_state": r.state,
        "collector_version": COLLECTOR_VERSION, "policy_version": POLICY_VERSION, "proofs": candidate.proofs});
    let pending = json!({"action": "collected_reservation", "universe_uuid": uuid, "class": t.class,
        "class_number": candidate.class_number, "collected_from_state": from_state, "collected_at": now,
        "container_before": before, "effect_state": "committed", "verification": "pending"});
    let tx = db.unchecked_transaction()?;
    mg::merge_state(
        &tx,
        uuid,
        COLLECTED,
        &json!({"collected_by_operation": id, "collected_at": now, "collected_from_state": r.state,
            "collection_class": t.class, "collection_class_number": candidate.class_number,
            "collection_authorization_ref": reference}),
    )?;
    let first = mg::record_collection(
        &tx,
        uuid,
        &t.class,
        candidate.class_number.unwrap_or(0) as i64,
        &r.container_id,
        absent,
        &r.operation_id,
        id,
        now,
        &proof,
    )?;
    record_effect(&tx, id, t, &pending)?;
    tx.commit()?;
    let test_barrier = match post_commit_test_barrier(id, t) {
        Ok(barrier) => barrier,
        Err(error) => {
            // The domain effect and pending progress row are already committed. A malformed or timed-out
            // qualification gate therefore stops before outside verification without pretending that the
            // effect did not happen. A retry observes and reconciles this pending effect; it never reapplies it.
            return Ok(json!({
                "action": "collected_reservation",
                "universe_uuid": uuid,
                "class": t.class,
                "class_number": candidate.class_number,
                "collected_from_state": from_state,
                "container_before": before,
                "effect_state": "committed",
                "verification": "pending",
                "verified": false,
                "verification_blockers": [format!("the test-only post-commit barrier failed closed: {error}")],
                "tombstone_written_by_this_collection": first,
                "test_post_commit_barrier": failed_barrier_evidence(&error),
            }));
        }
    };
    let mut done = verify_collection(db, t, candidate.class_number, Some(&before), &from_state);
    done["effect_state"] = json!("committed");
    done["tombstone_written_by_this_collection"] = json!(first);
    if let Some(barrier) = test_barrier {
        done["test_post_commit_barrier"] = barrier;
    }
    // Best effort: the progress row already exists, and losing this update costs a re-verification, not an
    // effect. It must never turn a committed collection into an error.
    let _ = record_effect(db, id, t, &done);
    Ok(done)
}

/// Re-reads what a delegated abort left behind, and never fails the operation that made it.
fn verify_abort(
    db: &Connection,
    t: &Target,
    class_number: Option<u64>,
    claim_before: &Value,
    reclaim: bool,
    outcome: Value,
) -> Value {
    let uuid = t.universe_uuid.as_str();
    let (after, error) = observe_quietly(&format!("podmesh-{uuid}"));
    let (claim, claim_error) = match one_claim(db, &t.authorization_id) {
        Ok(value) => (value, None),
        Err(e) => (None, Some(e.to_string())),
    };
    let processes = claim_before["container_id"].as_str().map(|cid| {
        cleanup::runtime_processes(cid, claim_before["created_at"].as_i64().unwrap_or(0))
    });
    let signal_entries = outcome["reclaim"]["signalled"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let signal_metrics = cleanup::signal_metrics(&signal_entries);
    let mut blockers = claim_verdict(
        claim.is_some(),
        claim_error.as_deref(),
        after.is_some(),
        error.as_deref(),
        processes.as_ref(),
    );
    blockers.extend(graph_measurement_verdict(&outcome));
    json!({"action": "aborted_failed_restore", "universe_uuid": uuid, "authorization_id": t.authorization_id,
        "class": t.class, "class_number": class_number, "reclaim_processes": reclaim,
        "claim_before": claim_before,
        "delegated_to": "migration_restore_abort", "result": outcome,
        "reclaim_performed": outcome["reclaim"]["requested"] == true,
        "signal_candidates": signal_metrics["signal_candidates"],
        "signal_attempts": signal_metrics["signal_attempts"],
        "signals_delivered": signal_metrics["signals_delivered"],
        "processes_signalled": signal_metrics["processes_signalled"],
        "processes_already_gone": signal_metrics["processes_already_gone"],
        "signals_refused": signal_metrics["signals_refused"],
        "verified": blockers.is_empty(), "verification_blockers": blockers,
        "verified_from_outside": {"observed_at": crate::now(), "claim_unresolved": claim.is_some(),
            "claim_observation_error": claim_error,
            "universe_container_present": after.is_some(), "observation_error": error,
            "runtime_processes": processes}}
    )
}

/// The class 3 effect: `migration_restore_abort`'s own contract, called with this run's operation ID and the
/// explicit `reclaim_processes` the request carried. No process management is duplicated here, and the
/// verdict on what the abort left is this collector's own, not the abort's.
fn abort_failed_restore(
    db: &Connection,
    id: &str,
    reference: &str,
    t: &Target,
    candidate: &Candidate,
    reclaim: bool,
) -> Result<Value, Error> {
    let uuid = t.universe_uuid.as_str();
    let name = format!("podmesh-{uuid}");
    let claim_before = candidate.proofs["claim"].clone();
    // Before the effect: an observation that cannot be made is a refusal, because nothing has happened yet.
    let existing = lc::inspect(&name).map_err(|e| {
        failure(
            format!("The universe container could not be observed before the abort: {e}; nothing was collected"),
            json!({"candidate": t.view()}),
        )
    })?;
    let processes = match claim_before["container_id"].as_str() {
        Some(container_id) => cleanup::runtime_processes(
            container_id,
            claim_before["created_at"].as_i64().unwrap_or(0),
        ),
        None => json!({"known": false, "count": Value::Null, "authorizes_reclaim": false,
            "reason": "the failed restore claim records no immutable container ID"}),
    };
    if let Err(reason) = predelegation_abort_check(&processes, reclaim) {
        return Err(failure(
            format!("{reason}; nothing was collected and migration_restore_abort was not entered"),
            json!({"candidate": t.view(), "runtime_processes": processes,
                "delegated": false, "effects_applied": 0}),
        ));
    }
    // Write-ahead uncertainty closes the crash window between the first possible external effect and the
    // claim's own durable close. Recovery reconciles this marker; it never delegates the same effect again.
    record_effect(
        db,
        id,
        t,
        &json!({"action": "aborted_failed_restore", "universe_uuid": uuid,
            "authorization_id": t.authorization_id, "class": t.class,
            "class_number": candidate.class_number, "claim_before": claim_before,
            "reclaim_processes": reclaim, "effect_state": "uncertain", "verification": "pending",
            "verified": false,
            "verification_blockers": ["the delegated abort was entered; its terminal state has not yet been reconciled"]}),
    )?;
    match ds::abort(
        db,
        id,
        uuid,
        &name,
        &t.authorization_id,
        reference,
        reclaim,
        existing,
    ) {
        Ok(outcome) => {
            let mut done = verify_abort(
                db,
                t,
                candidate.class_number,
                &claim_before,
                reclaim,
                outcome,
            );
            done["effect_state"] = json!("committed");
            let _ = record_effect(db, id, t, &done);
            Ok(done)
        }
        Err(e) => {
            // Once delegation began, an error cannot prove that no effect happened: the abort may have
            // signalled a process or removed the container before a later observation or journal write
            // failed. Persist uncertainty, stop this run, and reconcile on replay without delegating again.
            let (closed, close_observation_error) =
                match claim_closed_by(db, &t.authorization_id, id) {
                    Ok(value) => (Some(value), None),
                    Err(error) => (None, Some(error.to_string())),
                };
            let failure_details = e
                .downcast_ref::<lc::Failure>()
                .map(|failure| failure.details.clone())
                .unwrap_or(Value::Null);
            let graph_root = failure_details["graph_root"].clone();
            let mut done = verify_abort(
                db,
                t,
                candidate.class_number,
                &claim_before,
                reclaim,
                json!({
                    "error": e.to_string(),
                    "details": failure_details,
                    "graph_root": graph_root,
                    "claim_closed_by_this_operation": closed,
                    "claim_close_observation_error": close_observation_error,
                }),
            );
            let mut blockers = done["verification_blockers"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            blockers.push(json!(format!(
                "the delegated abort returned an error after it was allowed to act, so whether it committed an external effect is unknown: {e}"
            )));
            done["effect_state"] = json!("uncertain");
            done["verified"] = json!(false);
            done["verification_blockers"] = json!(blockers);
            let _ = record_effect(db, id, t, &done);
            Ok(done)
        }
    }
}

/// Separately authorized, bounded, idempotent by operation ID. Each effect repeats its own proofs
/// immediately before it acts and stops the run at the first mismatch.
fn apply(
    db: &Connection,
    id: &str,
    reference: &str,
    limits: &Limits,
    request: &Value,
    targets: &[Target],
) -> Result<Value, Error> {
    let host = mg::host_uuid(db)?;
    let plan_operation = lc::text(request, "plan_operation_id")?;
    let reclaim = match request.get("reclaim_processes") {
        None | Some(Value::Null) => false,
        Some(v) => v
            .as_bool()
            .ok_or("reclaim_processes must be true or false")?,
    };
    if reclaim && limits.max_runtime_reclaims == 0 {
        return Err(failure(
            "reclaim_processes: true needs an explicit max_runtime_reclaims of at least 1 for this run; nothing was collected",
            json!({"limits": limits.view(Mode::Apply)}),
        ));
    }
    let record = plan_record(db, plan_operation)?;
    if record["host_uuid"].as_str() != Some(host.as_str()) {
        return Err(failure(
            "The named plan was recorded for another host UUID; nothing was collected",
            json!({"plan_operation_id": plan_operation}),
        ));
    }
    let mut results: Vec<Value> = vec![];
    let mut effects = 0usize;
    let mut recovered = 0usize;
    let mut reserved_reclaims = reclaim_reservations(db, id)?;
    let mut performed_reclaims = 0usize;
    let mut signalled = 0usize;
    let mut signal_candidates = 0usize;
    let mut signal_attempts = 0usize;
    let mut already_gone = 0usize;
    let mut signals_refused = 0usize;
    let mut stopped: Option<Value> = None;
    let mut stop_kind: Option<&str> = None;
    for t in targets {
        // An effect this very operation already committed is recovered, never repeated: a run interrupted
        // between its effect and its report comes back to exactly what it did.
        let progress = recorded_effect(db, id, t.key())?;
        let done_before = progress
            .as_ref()
            .is_some_and(progress_prevents_repeating_effect)
            || already_applied(db, id, t)?;
        if done_before {
            let planned_candidate = planned(&record, t).ok();
            let mut entry = match (progress, t.class == CLASS_FAILED_RESTORE_CLAIM) {
                (Some(p), false) if p["verified"] == true => p,
                (Some(p), false) => {
                    let before = p.get("container_before").filter(|v| !v.is_null());
                    let from_state = p.get("collected_from_state").unwrap_or(&Value::Null);
                    let mut observed =
                        verify_collection(db, t, p["class_number"].as_u64(), before, from_state);
                    observed["effect_state"] = json!("committed");
                    let _ = record_effect(db, id, t, &observed);
                    observed
                }
                (None, false) => {
                    let mut observed = verify_collection(db, t, None, None, &Value::Null);
                    observed["effect_state"] = json!("committed");
                    let _ = record_effect(db, id, t, &observed);
                    observed
                }
                (Some(p), true) if p["verified"] == true => p,
                (Some(p), true) => {
                    let claim_before = p
                        .get("claim_before")
                        .filter(|v| v.is_object())
                        .or_else(|| planned_candidate.as_ref().map(|c| &c["proofs"]["claim"]));
                    match claim_before {
                        Some(claim) => {
                            let mut observed = verify_abort(
                                db,
                                t,
                                p["class_number"].as_u64(),
                                claim,
                                p["reclaim_processes"] == true,
                                p["result"].clone(),
                            );
                            observed["effect_state"] = p["effect_state"].clone();
                            let _ = record_effect(db, id, t, &observed);
                            observed
                        }
                        None => p,
                    }
                }
                (None, true) => match planned_candidate.as_ref().map(|c| &c["proofs"]["claim"]) {
                    Some(claim) if claim.is_object() => {
                        let mut observed = verify_abort(
                            db,
                            t,
                            Some(3),
                            claim,
                            reclaim,
                            json!({"recovered": true, "note": "the collector progress row was lost"}),
                        );
                        observed["effect_state"] = json!("committed");
                        let _ = record_effect(db, id, t, &observed);
                        observed
                    }
                    _ => {
                        json!({"action": "aborted_failed_restore", "universe_uuid": t.universe_uuid,
                        "authorization_id": t.authorization_id, "class": t.class, "effect_state": "committed",
                        "verified": false,
                        "verification_blockers": ["the claim was closed by this operation, but the collector progress row and the claim facts needed for outside verification are unavailable"],
                        "note": "recovered from the claim this operation closed; the collector's own progress row was lost"})
                    }
                },
            };
            entry["applied"] = json!(true);
            entry["recovered"] = json!(true);
            entry["recovery_note"] =
                json!("this effect was already committed by this operation ID; it was recovered from the journal, not repeated");
            effects += 1;
            recovered += 1;
            if entry["reclaim_performed"] == true {
                performed_reclaims += 1;
            }
            signalled += entry["processes_signalled"].as_u64().unwrap_or(0) as usize;
            signal_candidates += entry["signal_candidates"].as_u64().unwrap_or(0) as usize;
            signal_attempts += entry["signal_attempts"].as_u64().unwrap_or(0) as usize;
            already_gone += entry["processes_already_gone"].as_u64().unwrap_or(0) as usize;
            signals_refused += entry["signals_refused"].as_u64().unwrap_or(0) as usize;
            let verified = !effect_requires_stop(&entry);
            let blockers = entry["verification_blockers"].clone();
            results.push(entry);
            if !verified {
                stopped = Some(json!({"candidate": t.view(),
                    "reason": "the recovered effect is not verified from outside; no later candidate was acted on",
                    "verification_blockers": blockers}));
                stop_kind = Some("verification_failed");
                break;
            }
            continue;
        }
        if effects >= limits.max_effects {
            stopped = Some(json!({"candidate": t.view(), "reason": format!(
                "the maximum of {} effect(s) for this run was reached; the remaining candidates were not acted on",
                limits.max_effects)}));
            stop_kind = Some("effect_bound_reached");
            break;
        }
        // The plan is an audit chain, not a proof: what it found is checked again, from scratch, right now.
        let refusal = planned(&record, t).err();
        let mut candidate = None;
        let refusal = match refusal {
            Some(e) => Some(e),
            // An observation that cannot be made before an effect is a refusal of that candidate, and never
            // an error that would discard what earlier candidates already achieved.
            None => match fresh(db, t, &host) {
                Ok(Ok(c)) => {
                    candidate = Some(c);
                    None
                }
                Ok(Err(e)) => Some(e),
                Err(e) => Some(failure(
                    format!(
                        "The proofs for {} could not be established: {e}; nothing was collected for it",
                        t.key()
                    ),
                    json!({"candidate": t.view()}),
                )),
            },
        };
        if let Some(e) = refusal {
            let details = e
                .downcast_ref::<lc::Failure>()
                .map(|f| f.details.clone())
                .unwrap_or(Value::Null);
            let entry = json!({"candidate": t.view(), "applied": false, "refused": e.to_string(), "details": details});
            // Nothing has been collected yet: the whole request is refused, and no run record is written.
            if effects == 0 {
                return Err(failure(
                    format!("Collection refused: {e}"),
                    json!({"candidate": t.view(), "detail": details, "applied": results}),
                ));
            }
            results.push(entry);
            stopped = Some(json!({"candidate": t.view(), "reason": e.to_string()}));
            stop_kind = Some("candidate_refused_after_effect");
            break;
        }
        let candidate = candidate.ok_or("Candidate lost between its proof and its effect")?;
        // The budget is reserved *before* a delegated call that is authorized to signal, not counted after
        // it: the abort observes the processes itself, so what this run predicted cannot bound what it may
        // do. Every attempt authorized to signal costs one unit, whether it ends up signalling or not.
        let may_signal = reclaim && t.class == CLASS_FAILED_RESTORE_CLAIM;
        if may_signal {
            match reserve_reclaim(db, id, t.key(), limits.max_runtime_reclaims)? {
                Some(total) => reserved_reclaims = total,
                None => {
                    stopped = Some(json!({"candidate": t.view(), "reason": format!(
                        "the run's allowance of {} runtime reclaim(s) is spent; this candidate was not acted on, because every delegated abort authorized to signal consumes a durable attempt reservation before it is called",
                        limits.max_runtime_reclaims)}));
                    stop_kind = Some("reclaim_bound_reached");
                    break;
                }
            }
        }
        let outcome = if t.class == CLASS_FAILED_RESTORE_CLAIM {
            abort_failed_restore(db, id, reference, t, &candidate, reclaim)
        } else {
            collect_reservation(db, id, reference, t, &candidate)
        };
        match outcome {
            Ok(mut done) => {
                effects += 1;
                if done["reclaim_performed"] == true {
                    performed_reclaims += 1;
                }
                signalled += done["processes_signalled"].as_u64().unwrap_or(0) as usize;
                signal_candidates += done["signal_candidates"].as_u64().unwrap_or(0) as usize;
                signal_attempts += done["signal_attempts"].as_u64().unwrap_or(0) as usize;
                already_gone += done["processes_already_gone"].as_u64().unwrap_or(0) as usize;
                signals_refused += done["signals_refused"].as_u64().unwrap_or(0) as usize;
                done["applied"] = json!(true);
                done["proofs_repeated_before_the_effect"] = candidate.proofs.clone();
                // An effect whose own verification from outside did not hold stops the run: the contract
                // stops at the first unexpected identity or state, and this one is already recorded.
                let verified = !effect_requires_stop(&done);
                let blockers = done["verification_blockers"].clone();
                results.push(done);
                if !verified {
                    stopped = Some(json!({"candidate": t.view(),
                        "reason": "the verification from outside did not hold for the effect that was just applied",
                        "verification_blockers": blockers}));
                    stop_kind = Some("verification_failed");
                    break;
                }
            }
            Err(e) => {
                let details = e
                    .downcast_ref::<lc::Failure>()
                    .map(|f| f.details.clone())
                    .unwrap_or(Value::Null);
                if effects == 0 {
                    return Err(failure(
                        format!("Collection refused: {e}"),
                        json!({"candidate": t.view(), "detail": details, "applied": results}),
                    ));
                }
                results.push(json!({"candidate": t.view(), "applied": false, "refused": e.to_string(), "details": details}));
                stopped = Some(json!({"candidate": t.view(), "reason": e.to_string()}));
                stop_kind = Some("candidate_refused_after_effect");
                break;
            }
        }
    }
    // A run is verified only when every effect it carries was verified from outside. A report that was
    // written proves nothing about the state it describes.
    let unverified: Vec<Value> = results
        .iter()
        .filter(|r| r["applied"] == true && r["verified"] != true)
        .map(|r| {
            json!({"candidate": {"class": r["class"], "universe_uuid": r["universe_uuid"]},
            "verification_blockers": r["verification_blockers"], "recovered": r["recovered"]})
        })
        .collect();
    let completion = if !unverified.is_empty() {
        "unverified"
    } else {
        match stop_kind {
            Some("candidate_refused_after_effect") => "partial",
            Some("verification_failed") => "unverified",
            Some("effect_bound_reached" | "reclaim_bound_reached") => "bounded",
            _ => "complete",
        }
    };
    Ok(json!({
        "host_uuid": host,
        "plan_operation_id": plan_operation,
        "limits": limits.view(Mode::Apply),
        "reclaim_processes": reclaim,
        "candidates_named": targets.len(),
        "effects_applied": effects,
        "effects_recovered": recovered,
        "runtime_reclaims_reserved": reserved_reclaims,
        "runtime_reclaims_performed": performed_reclaims,
        "runtime_reclaims": performed_reclaims,
        "signal_candidates": signal_candidates,
        "signal_attempts": signal_attempts,
        "signals_delivered": signalled,
        "processes_signalled": signalled,
        "processes_already_gone": already_gone,
        "signals_refused": signals_refused,
        "completion": completion,
        "verified": unverified.is_empty(),
        "unverified_effects": unverified,
        "stopped_before_the_rest": stopped,
        "results": results,
    }))
}

/// The fresh classification of one named candidate, immediately before its effect.
fn fresh(db: &Connection, t: &Target, host: &str) -> Result<Result<Candidate, Error>, Error> {
    let uuid = t.universe_uuid.as_str();
    let name = format!("podmesh-{uuid}");
    let observed = lc::inspect(&name)?;
    let candidate = if t.class == CLASS_FAILED_RESTORE_CLAIM {
        let Some(k) = one_claim(db, &t.authorization_id)? else {
            return Ok(Err(failure(
                format!(
                    "No unresolved restore claim {} is recorded on this host; a verified or closed claim is never collected",
                    t.authorization_id
                ),
                json!({"candidate": t.view()}),
            )));
        };
        if k["universe_uuid"].as_str() != Some(uuid) {
            return Ok(Err(failure(
                "The restore claim names another universe",
                json!({"candidate": t.view(), "claim": k}),
            )));
        }
        classify_claim(db, &k, observed.as_ref())?
    } else {
        let Some(r) = mg::reservation(db, uuid)? else {
            return Ok(Err(failure(
                format!("Universe {uuid} has no migration reservation on this host"),
                json!({"candidate": t.view()}),
            )));
        };
        let all = tr::all_containers()?;
        classify_reservation(db, uuid, &r, observed.as_ref(), &all, host)?
    };
    if candidate.class != Some(t.class.as_str()) {
        return Ok(Err(failure(
            format!(
                "The fresh proofs classify {} as {:?}, not as {}; nothing was collected",
                t.key(),
                candidate.class,
                t.class
            ),
            json!({"candidate": t.view(), "class_now": candidate.class, "blockers": candidate.blockers,
                "proofs": candidate.proofs}),
        )));
    }
    if !candidate.blockers.is_empty() {
        return Ok(Err(failure(
            format!(
                "The proofs repeated immediately before the effect no longer hold for {}: {}",
                t.key(),
                candidate.blockers.join("; ")
            ),
            json!({"candidate": t.view(), "blockers": candidate.blockers, "proofs": candidate.proofs}),
        )));
    }
    Ok(Ok(candidate))
}

fn completed_status(mode: Mode, body: &Value) -> &'static str {
    if mode == Mode::Apply && body["verified"] == false {
        "completed_with_unverified_effects"
    } else if mode == Mode::Apply && body["completion"] == "partial" {
        "partially_applied"
    } else {
        "verified"
    }
}
fn terminal_status(status: &str) -> bool {
    matches!(
        status,
        "verified" | "partially_applied" | "completed_with_unverified_effects"
    )
}

/// A replay never repeats an effect: the persisted record comes back with a fresh observation beside it.
fn replay(db: &Connection, id: &str, record: Option<String>) -> Result<Value, Error> {
    let original: Value = serde_json::from_str(&record.ok_or("Missing persisted result")?)?;
    let persisted_at: Option<i64> = db.query_row(
        "SELECT MAX(finished_at) FROM operation_attempts WHERE operation_id=?1
         AND outcome IN ('verified','partially_applied','completed_with_unverified_effects')",
        [id],
        |r| r.get(0),
    )?;
    let mut current = vec![];
    let mut reconciliation = vec![];
    if let Some(results) = original["results"].as_array() {
        for result in results
            .iter()
            .filter(|r| r["applied"] == true && r["verified"] != true)
        {
            let Some(class) = result["class"].as_str() else {
                reconciliation.push(json!({"verified": false,
                    "verification_blockers": ["the persisted effect has no collection class, so it cannot be reconciled"]}));
                continue;
            };
            let Some(uuid) = result["universe_uuid"].as_str() else {
                reconciliation.push(json!({"verified": false,
                    "verification_blockers": ["the persisted effect has no universe UUID, so it cannot be reconciled"]}));
                continue;
            };
            let target = Target {
                class: class.to_string(),
                universe_uuid: uuid.to_string(),
                authorization_id: result["authorization_id"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            };
            let observed = if class == CLASS_FAILED_RESTORE_CLAIM {
                match result.get("claim_before").filter(|v| v.is_object()) {
                    Some(claim) => verify_abort(
                        db,
                        &target,
                        result["class_number"].as_u64(),
                        claim,
                        result["reclaim_processes"] == true,
                        result["result"].clone(),
                    ),
                    None => json!({"verified": false,
                        "verification_blockers": ["the persisted failed-restore effect has no pre-effect claim facts, so it cannot be reconciled"]}),
                }
            } else {
                verify_collection(
                    db,
                    &target,
                    result["class_number"].as_u64(),
                    result.get("container_before").filter(|v| !v.is_null()),
                    result.get("collected_from_state").unwrap_or(&Value::Null),
                )
            };
            reconciliation.push(observed);
        }
    }
    let keys: Vec<Value> = match original["mode"].as_str() {
        // An applied effect names its universe directly; a refused candidate carries it under `candidate`.
        Some("apply") => original["results"]
            .as_array()
            .map(|v| {
                v.iter()
                    .map(|r| {
                        if r["universe_uuid"].is_string() {
                            json!({"class": r["class"], "universe_uuid": r["universe_uuid"]})
                        } else {
                            r["candidate"].clone()
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => original["candidates"]
            .as_array()
            .map(|v| {
                v.iter()
                    .map(|c| json!({"class": c["class"], "universe_uuid": c["universe_uuid"], "key": c["key"]}))
                    .collect()
            })
            .unwrap_or_default(),
    };
    for k in keys {
        let Some(uuid) = k["universe_uuid"].as_str() else {
            continue;
        };
        let (reservation, reservation_error) = match mg::reservation(db, uuid) {
            Ok(value) => (
                value.map(|r| json!({"state": r.state, "updated_at": r.updated_at})),
                None,
            ),
            Err(e) => (None, Some(e.to_string())),
        };
        let (tombstone, tombstone_error) = match mg::tombstone(db, uuid) {
            Ok(value) => (value, None),
            Err(e) => (None, Some(e.to_string())),
        };
        let (container, container_error) = observe_quietly(&format!("podmesh-{uuid}"));
        let container = match container {
            None => json!({"observed_at": crate::now(), "present": false}),
            Some(c) => {
                let mut view = lc::state_view(&c);
                view["present"] = json!(true);
                view["managed_label"] =
                    json!(c["Config"]["Labels"]["io.podmesh.universe"].as_str() == Some(uuid));
                view
            }
        };
        current.push(
            json!({"universe_uuid": uuid, "reservation": reservation, "reservation_error": reservation_error,
            "tombstone": tombstone, "tombstone_error": tombstone_error,
            "container": container, "container_observation_error": container_error}),
        );
    }
    Ok(json!({
        "replayed": true, "historical": true,
        "notice": "original_result is the terminal record persisted by this collection attempt, including any partial or unverified outcome; current is a fresh observation. A replayed collection repeats no effect.",
        "persisted_at": persisted_at,
        "verified_at": if original["status"] == "verified" { json!(persisted_at) } else { Value::Null },
        "original_result": original,
        "reconciliation": {"effects_rechecked": reconciliation.len(), "results": reconciliation,
            "note": "reconciliation repeats observations only; it never repeats a collection or delegated abort"},
        "current": {"observed_at": crate::now(), "candidates": current},
    }))
}

/// Entry point for both modes. Validation, then the journal contract of every other operation: a stable
/// operation ID, one request per ID, a verified run replayed as history, and a durable record.
pub fn execute(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    let mode = match operation {
        "garbage_collect_plan" => Mode::Plan,
        "garbage_collect_apply" => Mode::Apply,
        _ => return Err("Unsupported collection operation".into()),
    };
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let reference = lc::text(request, "authorization_ref")?;
    let limits = Limits::parse(request)?;
    let targets = if mode == Mode::Apply {
        parse_targets(request, &limits)?
    } else {
        vec![]
    };
    let scope = if mode == Mode::Plan {
        parse_scope(request, &limits)?
    } else {
        None
    };
    lc::ensure_schema(db)?;
    ensure_schema(db)?;
    let canonical = request.to_string();
    let previous: Option<(String, String, Option<String>)> = db
        .query_row(
            "SELECT request,status,result FROM operations WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((saved, status, result)) = previous {
        if saved != canonical {
            return Err("Operation ID already belongs to a different request".into());
        }
        if terminal_status(&status) {
            return replay(db, id, result);
        }
    } else {
        db.execute(
            "INSERT INTO operations VALUES(?1,?2,'pending',NULL)",
            params![id, canonical],
        )?;
    }
    db.execute(
        "INSERT INTO operation_attempts(operation_id,started_at) VALUES(?1,?2)",
        params![id, crate::now() as i64],
    )?;
    let attempt = db.last_insert_rowid();
    let started = crate::now() as i64;
    let outcome = match mode {
        Mode::Plan => plan(db, &limits, scope.as_ref()),
        Mode::Apply => apply(db, id, reference, &limits, request, &targets),
    };
    let finished = crate::now() as i64;
    match outcome {
        Ok(body) => {
            // The status of a run says what the run achieved, never merely that it finished reporting.
            let status = completed_status(mode, &body);
            let mut record = json!({
                "status": status, "operation": mode.operation(), "mode": mode.name(),
                "collection_operation_id": id, "requester": {"authorization_ref": reference},
                "collector_version": COLLECTOR_VERSION,
                "build_version": option_env!("PODMESH_PACKAGE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
                "policy_version": POLICY_VERSION,
                "started_at": started, "finished_at": finished,
                "authority": AUTHORITY,
                "scope": "docs/GARBAGE-COLLECTION.md classes 1 and 2 (terminal reservations, with tombstones) and class 3 by delegation to migration_restore_abort; no artifact collection, no retention interval, no timer",
            });
            if let (Some(target), Some(source)) = (record.as_object_mut(), body.as_object()) {
                for (k, v) in source {
                    target.insert(k.clone(), v.clone());
                }
            }
            db.execute(
                "INSERT OR REPLACE INTO garbage_collection_runs VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    id,
                    mode.name(),
                    reference,
                    COLLECTOR_VERSION,
                    POLICY_VERSION,
                    started,
                    finished,
                    record.to_string()
                ],
            )?;
            db.execute(
                "UPDATE operations SET status=?2, result=?3 WHERE id=?1",
                params![id, status, record.to_string()],
            )?;
            db.execute(
                "UPDATE operation_attempts SET finished_at=?2, outcome=?3 WHERE id=?1",
                params![attempt, finished, status],
            )?;
            Ok(record)
        }
        Err(e) => {
            let details = e
                .downcast_ref::<lc::Failure>()
                .map(|f| f.details.clone())
                .unwrap_or(Value::Null);
            let failed = json!({"error": e.to_string(), "details": details}).to_string();
            db.execute(
                "UPDATE operations SET status='failed', result=?2 WHERE id=?1",
                params![id, failed],
            )?;
            db.execute(
                "UPDATE operation_attempts SET finished_at=?2, outcome='failed', detail=?3 WHERE id=?1",
                params![attempt, finished, failed],
            )?;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct BarrierDirectory(PathBuf);
    impl BarrierDirectory {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "podmesh-collector-barrier-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
        fn owner_uid(&self) -> u32 {
            fs::metadata(&self.0).unwrap().uid()
        }
        fn write_control(&self, operation_id: &str, phase: &str) {
            let path = self.0.join(format!("{phase}-{operation_id}.json"));
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .unwrap();
            serde_json::to_writer(
                &mut file,
                &json!({"format": TEST_BARRIER_FORMAT, "operation_id": operation_id, "phase": phase}),
            )
            .unwrap();
            file.sync_all().unwrap();
        }
    }
    impl Drop for BarrierDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn target(key: &str) -> Target {
        Target {
            class: CLASS_TERMINAL_ABSENT.to_string(),
            universe_uuid: key.to_string(),
            authorization_id: String::new(),
        }
    }

    fn before(id: &str, state: &str, checkpointed: bool) -> Value {
        json!({"present": true, "observed_container_id": id, "state": state, "checkpointed": checkpointed})
    }

    #[test]
    fn post_commit_barrier_publishes_exact_marker_and_requires_exact_release() {
        let directory = BarrierDirectory::new();
        let operation_id = "00000000-0000-4000-8000-000000000004";
        let candidate = target("00000000-0000-4000-8000-000000000005");
        let expected_key = candidate.key().to_string();
        let expected_uuid = candidate.universe_uuid.clone();
        directory.write_control(operation_id, "arm");
        let release_directory = directory.0.clone();
        let release = std::thread::spawn(move || {
            let reached = release_directory.join(format!("reached-{operation_id}.json"));
            let deadline = Instant::now() + Duration::from_secs(1);
            while !reached.exists() {
                assert!(
                    Instant::now() < deadline,
                    "the reached marker was not published"
                );
                thread::sleep(Duration::from_millis(5));
            }
            let marker: Value = serde_json::from_slice(&fs::read(&reached).unwrap()).unwrap();
            assert_eq!(marker["operation_id"], operation_id);
            assert_eq!(marker["candidate_key"], expected_key);
            assert_eq!(marker["universe_uuid"], expected_uuid);
            assert_eq!(marker["class"], CLASS_TERMINAL_ABSENT);
            assert_eq!(marker["phase"], "effect_committed_verification_pending");
            let path = release_directory.join(format!("release-{operation_id}.json"));
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .unwrap();
            serde_json::to_writer(
                &mut file,
                &json!({"format": TEST_BARRIER_FORMAT, "operation_id": operation_id, "phase": "release"}),
            )
            .unwrap();
            file.sync_all().unwrap();
        });
        let result = post_commit_barrier_in(
            &directory.0,
            operation_id,
            &candidate,
            directory.owner_uid(),
            Duration::from_secs(1),
        )
        .unwrap()
        .unwrap();
        release.join().unwrap();
        assert_eq!(result["armed"], true);
        assert_eq!(result["released"], true);
    }

    #[test]
    fn post_commit_barrier_fails_closed_on_malformed_arm() {
        let directory = BarrierDirectory::new();
        let operation_id = "00000000-0000-4000-8000-000000000006";
        let path = directory.0.join(format!("arm-{operation_id}.json"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        serde_json::to_writer(
            &mut file,
            &json!({"format": TEST_BARRIER_FORMAT, "operation_id": operation_id,
                "phase": "arm", "unexpected": true}),
        )
        .unwrap();
        file.sync_all().unwrap();
        let error = post_commit_barrier_in(
            &directory.0,
            operation_id,
            &target("00000000-0000-4000-8000-000000000007"),
            directory.owner_uid(),
            Duration::from_millis(10),
        )
        .unwrap_err();
        assert!(error.contains("exact arm control"));
        assert!(!directory
            .0
            .join(format!("reached-{operation_id}.json"))
            .exists());

        let evidence = failed_barrier_evidence(&error);
        assert_eq!(evidence["configured"], true);
        assert!(evidence["armed"].is_null());
        assert!(evidence["reached"].is_null());
        assert_eq!(evidence["released"], false);
        assert_eq!(evidence["error"], error);
    }

    fn after(id: &str, state: &str, checkpointed: bool) -> Value {
        json!({"Id": id, "State": {"Status": state, "Checkpointed": checkpointed, "Pid": 0}})
    }

    #[test]
    fn committed_progress_survives_failed_observation_and_is_terminal_for_replay() {
        let db = Connection::open_in_memory().unwrap();
        ensure_schema(&db).unwrap();
        let candidate = target("00000000-0000-0000-0000-000000000001");
        let pending = json!({
            "effect_state": "committed",
            "verification": "pending",
            "container_before": {"present": false}
        });
        record_effect(&db, "apply-one", &candidate, &pending).unwrap();

        let recovered = recorded_effect(&db, "apply-one", candidate.key())
            .unwrap()
            .unwrap();
        assert!(progress_prevents_repeating_effect(&recovered));
        let blockers = reservation_verdict(COLLECTED, true, true, None, Some("podman timed out"));
        assert!(blockers.iter().any(|b| b.contains("state is unknown")));

        let body = json!({"verified": false, "completion": "unverified"});
        let status = completed_status(Mode::Apply, &body);
        assert_eq!(status, "completed_with_unverified_effects");
        assert!(
            terminal_status(status),
            "a replay must return the terminal uncertainty without repeating the effect"
        );
    }

    #[test]
    fn corrupt_progress_is_unknown_instead_of_absent() {
        let db = Connection::open_in_memory().unwrap();
        ensure_schema(&db).unwrap();
        let candidate = target("00000000-0000-0000-0000-000000000002");
        db.execute(
            "INSERT INTO garbage_collection_effects VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                "apply-corrupt",
                candidate.key(),
                candidate.class,
                candidate.universe_uuid,
                1_i64,
                "{not-json"
            ],
        )
        .unwrap();
        assert!(recorded_effect(&db, "apply-corrupt", candidate.key()).is_err());
    }

    #[test]
    fn known_survivors_without_reclaim_refuse_before_delegation_or_effect() {
        let db = Connection::open_in_memory().unwrap();
        ensure_schema(&db).unwrap();
        let candidate = Target {
            class: CLASS_FAILED_RESTORE_CLAIM.to_string(),
            universe_uuid: "00000000-0000-0000-0000-000000000003".to_string(),
            authorization_id: "authorization-three".to_string(),
        };
        let processes = json!({"known": true, "authorizes_reclaim": true, "count": 3});
        let mut delegated = 0;

        let result = predelegation_abort_check(&processes, false).map(|_| {
            delegated += 1;
        });

        assert!(result.unwrap_err().contains("They are reported, not ended"));
        assert_eq!(delegated, 0, "migration_restore_abort must not be entered");
        assert!(
            recorded_effect(&db, "apply-no-reclaim", candidate.key())
                .unwrap()
                .is_none(),
            "a pre-effect refusal must not create a progress marker"
        );
        assert!(predelegation_abort_check(&processes, true).is_ok());

        let unknown = json!({"known": false, "reason": "cgroup observation unreadable"});
        assert!(predelegation_abort_check(&unknown, false)
            .unwrap_err()
            .contains("could not be established"));
    }

    #[test]
    fn reclaim_budget_is_reserved_durably_before_a_delegated_signal() {
        let db = Connection::open_in_memory().unwrap();
        ensure_schema(&db).unwrap();

        assert_eq!(
            reserve_reclaim(&db, "apply-reclaim", "claim-one", 1).unwrap(),
            Some(1)
        );
        assert_eq!(reclaim_reservations(&db, "apply-reclaim").unwrap(), 1);
        assert_eq!(
            reserve_reclaim(&db, "apply-reclaim", "claim-two", 1).unwrap(),
            None,
            "the second delegated call must be stopped before it is authorized to signal"
        );
        assert_eq!(reclaim_reservations(&db, "apply-reclaim").unwrap(), 1);
    }

    #[test]
    fn outside_abort_verdict_blocks_remaining_candidates_on_present_or_unknown_state() {
        let remaining = json!({"known": true, "authorizes_reclaim": true, "count": 1});
        let blockers = claim_verdict(false, None, true, None, Some(&remaining));
        let result = json!({"verified": blockers.is_empty(), "verification_blockers": blockers});
        assert!(effect_requires_stop(&result));
        assert!(result["verification_blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b.as_str().unwrap().contains("container is still present")));

        let unknown = json!({"known": false, "reason": "cgroup observation unreadable"});
        let blockers = claim_verdict(
            false,
            Some("database unavailable"),
            false,
            Some("podman unavailable"),
            Some(&unknown),
        );
        let result = json!({"verified": blockers.is_empty(), "verification_blockers": blockers});
        assert!(effect_requires_stop(&result));
        assert_eq!(result["verification_blockers"].as_array().unwrap().len(), 3);

        assert!(
            effect_requires_stop(&json!({})),
            "a missing verdict is unknown, never success"
        );
    }

    #[test]
    fn partial_and_unverified_runs_are_not_labelled_verified() {
        assert_eq!(
            completed_status(
                Mode::Apply,
                &json!({"verified": true, "completion": "partial"})
            ),
            "partially_applied"
        );
        assert_eq!(
            completed_status(
                Mode::Apply,
                &json!({"verified": false, "completion": "unverified"})
            ),
            "completed_with_unverified_effects"
        );
        assert_eq!(completed_status(Mode::Plan, &json!({})), "verified");
    }

    #[test]
    fn a_collection_that_held_verifies() {
        let unchanged = container_unchanged(
            Some(&before("a", "exited", true)),
            Some(&after("a", "exited", true)),
            None,
        );
        assert_eq!(unchanged, Some(true));
        assert!(reservation_verdict(COLLECTED, true, true, unchanged, None).is_empty());
    }

    #[test]
    fn an_observation_that_could_not_be_made_is_unknown_not_success() {
        let unchanged = container_unchanged(
            Some(&before("a", "exited", true)),
            None,
            Some("Podman ps exceeded its bound"),
        );
        assert_eq!(unchanged, None);
        let blockers = reservation_verdict(
            COLLECTED,
            true,
            true,
            unchanged,
            Some("Podman ps exceeded its bound"),
        );
        assert_eq!(blockers.len(), 1);
        assert!(
            blockers[0].contains("could not be observed"),
            "{blockers:?}"
        );
    }

    #[test]
    fn a_changed_or_unprotected_collection_does_not_verify() {
        let replaced = container_unchanged(
            Some(&before("a", "exited", true)),
            Some(&after("b", "exited", true)),
            None,
        );
        assert_eq!(replaced, Some(false));
        assert_eq!(
            reservation_verdict(COLLECTED, true, true, replaced, None).len(),
            1
        );
        let started = container_unchanged(
            Some(&before("a", "exited", true)),
            Some(&after("a", "running", false)),
            None,
        );
        assert_eq!(started, Some(false));
        assert_eq!(
            reservation_verdict("checkpointed", false, false, Some(true), None).len(),
            3
        );
    }

    #[test]
    fn a_recovered_effect_cannot_compare_a_container_it_never_saw() {
        assert_eq!(
            container_unchanged(None, Some(&after("a", "exited", true)), None),
            None
        );
        assert!(reservation_verdict(COLLECTED, true, true, None, None).is_empty());
    }

    #[test]
    fn an_abort_that_ended_its_claim_verifies() {
        let gone = json!({"authorizes_reclaim": true, "count": 0});
        assert!(claim_verdict(false, None, false, None, Some(&gone)).is_empty());
        assert!(claim_verdict(false, None, false, None, None).is_empty());
    }

    #[test]
    fn an_abort_that_left_something_does_not_verify() {
        let surviving = json!({"authorizes_reclaim": true, "count": 3});
        let blockers = claim_verdict(true, None, true, None, Some(&surviving));
        assert_eq!(blockers.len(), 3, "{blockers:?}");
        assert!(blockers.iter().any(|b| b.contains("still unresolved")));
        assert!(blockers.iter().any(|b| b.contains("still present")));
        assert!(blockers.iter().any(|b| b.contains("3 process(es)")));
    }

    #[test]
    fn an_unreadable_post_abort_observation_is_a_blocker() {
        let blockers = claim_verdict(false, None, false, Some("Podman inspect failed"), None);
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("absence is unknown"), "{blockers:?}");
        let unknown = json!({"known": false, "reason": "this claim recorded no container"});
        let blockers = claim_verdict(false, None, false, None, Some(&unknown));
        assert_eq!(blockers.len(), 1);
        assert!(
            blockers[0].contains("could not be established"),
            "{blockers:?}"
        );
        let fallback =
            json!({"authorizes_reclaim": false, "count": 2, "source": "cmdline_fallback"});
        assert!(claim_verdict(false, None, false, None, Some(&fallback)).is_empty());
    }

    #[test]
    fn real_cgroup_producer_failure_reaches_the_abort_verdict() {
        let produced = cleanup::runtime_processes_for_test(
            std::path::Path::new("/dev/null"),
            std::path::Path::new("/definitely-missing-podmesh-cgroup"),
        );
        assert_eq!(produced["known"], false);
        assert!(!produced["observation_errors"]
            .as_array()
            .unwrap()
            .is_empty());
        let blockers = claim_verdict(false, None, false, None, Some(&produced));
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("could not be established"));
    }

    #[test]
    fn unknown_or_inconsistent_graph_measurements_block_abort_verification() {
        let unknown = json!({"graph_root": {"known": false, "error": "df failed"}});
        let blockers = graph_measurement_verdict(&unknown);
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("unknown"));

        let missing = json!({"graph_root": {"known": true, "available_bytes_before": 10}});
        assert!(graph_measurement_verdict(&missing)[0].contains("missing or invalid"));

        let inconsistent = json!({"graph_root": {"known": true, "available_bytes_before": 10,
            "available_bytes_after": 14, "recovered_bytes": 3}});
        assert!(graph_measurement_verdict(&inconsistent)[0].contains("do not match"));

        let measured_zero = json!({"graph_root": {"known": true, "available_bytes_before": 0,
            "available_bytes_after": 0, "recovered_bytes": 0}});
        assert!(graph_measurement_verdict(&measured_zero).is_empty());
    }

    #[test]
    fn real_df_producer_failure_reaches_the_abort_verdict() {
        let error = mg::available_bytes_for_test(
            std::path::Path::new("/bin/false"),
            std::path::Path::new("/"),
        )
        .unwrap_err()
        .to_string();
        let produced = json!({"graph_root": {"known": false, "available_bytes": Value::Null,
            "error": error}});
        let blockers = graph_measurement_verdict(&produced);
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("unknown"));
    }
}
