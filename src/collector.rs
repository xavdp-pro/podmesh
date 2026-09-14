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
//! * **Class 5, for recovery points** — the archive a `recovery_point_prepare` left in this host's outbox,
//!   once the universe has a DECLARED retention, the point is outside the generations that retention keeps
//!   and older than its minimum age, no hold applies, and the archive still hashes to what the manifest bound.
//!   The effect writes a retained manifest first, in the same transaction that marks the point collected,
//!   and only then removes the archive; a run interrupted between the two finishes the removal on retry
//!   rather than repeating anything. Other operation artifacts (checkpoints) are NOT collected here.
//!
//! Both hold scopes of the contract are honoured for every class: an `investigation_hold` blocks every
//! effect, an `evidence_hold` blocks artifact deletion (class 5) and runtime reclaim (class 3 with
//! `reclaim_processes`) but not a history-preserving terminal transition. A hold that cannot be read blocks.
//!
//! Class 4 (a failed local restore) is not implemented here and nothing in this file assumes it exists.
use crate::cleanup;
use crate::retention as rt;
use crate::lifecycle::{self as lc, failure, Error};
use crate::migration::{self as mg, Reservation, COLLECTED};
use crate::restore as ds;
use crate::transfer as tr;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

const COLLECTOR_VERSION: &str = "podmesh-collector/2";
/// What this version is allowed to collect at all.
const POLICY_VERSION: &str = "podmesh-collection-policy/2: terminal reservation classes 1 and 2, failed restore claims through migration_restore_abort, and recovery point archives after a declared retention with both hold scopes; checkpoint artifacts are not collected";
const AUTHORITY: &str = "A plan is read-only and authorizes nothing. An apply is a separate operation that names the plan it applies, the candidates it may act on and its own bounds; it repeats every proof immediately before each effect and verifies the result from outside. There is no timer: nothing here ever runs on its own.";

/// Contract class names, as they appear in a plan, in an apply request and in a tombstone.
pub(crate) const CLASS_TERMINAL_NOT_RESTORED: &str = "terminal_reservation_all_authorizations_not_restored";
pub(crate) const CLASS_TERMINAL_ABSENT: &str = "terminal_reservation_container_absent";
pub(crate) const CLASS_FAILED_RESTORE_CLAIM: &str = "failed_restore_claim";
pub(crate) const CLASS_RECOVERY_POINT_AFTER_RETENTION: &str = "recovery_point_archive_after_retention";

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
/// Bytes of archive one apply run may remove. The default is small on purpose: a real universe's export is
/// gigabytes, and letting those go should be an explicit number in the request.
const DEFAULT_MAX_BYTES: usize = 4 << 30;
const LIMIT_MAX_BYTES: usize = 1 << 40;

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
         PRIMARY KEY(operation_id, candidate_key));",
    )?;
    mg::ensure_schema(db)?;
    rt::ensure_schema(db)?;
    crate::recovery_point::ensure_schema(db)?;
    Ok(())
}
/// Durable progress of one effect. Written inside the effect's own transaction with `verification: pending`
/// and updated with the full result once the verification from outside has been read; a lost update costs a
/// re-verification on recovery, never a repeated effect.
fn record_effect(db: &Connection, operation: &str, t: &Target, result: &Value) -> Result<(), Error> {
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
    Ok(db
        .query_row(
            "SELECT result FROM garbage_collection_effects WHERE operation_id=?1 AND candidate_key=?2",
            params![operation, key],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|t| serde_json::from_str(&t).ok()))
}
/// Whether this very operation already performed this candidate's effect, read from the domain tables
/// rather than from the collector's own progress row: the tombstone history for a reservation, the claim's
/// own closing operation for a failed restore. This is what makes a retry after an interruption safe even
/// if the progress row itself was lost.
fn already_applied(db: &Connection, operation: &str, t: &Target) -> Result<bool, Error> {
    if t.class == CLASS_FAILED_RESTORE_CLAIM {
        return claim_closed_by(db, &t.authorization_id, operation);
    }
    if t.class == CLASS_RECOVERY_POINT_AFTER_RETENTION {
        let by: Option<String> = db
            .query_row("SELECT collecting_operation_id FROM recovery_point_retained WHERE recovery_point_uuid=?1", [&t.recovery_point_uuid], |r| r.get(0))
            .optional()?;
        return Ok(by.as_deref() == Some(operation));
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
    let Some((state, detail)) = row else { return Ok(false) };
    let detail: Value = serde_json::from_str(detail.as_deref().unwrap_or("null")).unwrap_or(Value::Null);
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
    max_bytes: usize,
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
            max_candidates: bounded(request, "max_candidates", DEFAULT_MAX_CANDIDATES, LIMIT_MAX_CANDIDATES)?,
            max_effects: bounded(request, "max_effects", DEFAULT_MAX_EFFECTS, LIMIT_MAX_EFFECTS)?,
            max_runtime_reclaims: bounded(
                request,
                "max_runtime_reclaims",
                DEFAULT_MAX_RUNTIME_RECLAIMS,
                LIMIT_MAX_RUNTIME_RECLAIMS,
            )?,
            max_bytes: bounded(request, "max_bytes", DEFAULT_MAX_BYTES, LIMIT_MAX_BYTES)?,
        })
    }
    fn view(&self, mode: Mode) -> Value {
        match mode {
            Mode::Plan => json!({"max_candidates": self.max_candidates}),
            Mode::Apply => json!({"max_candidates": self.max_candidates, "max_effects": self.max_effects,
                "max_runtime_reclaims": self.max_runtime_reclaims, "max_bytes": self.max_bytes}),
        }
    }
}

/// One thing an apply request names. A candidate is never discovered by an apply: it repeats the
/// classification of something a recorded plan already found collectable.
struct Target {
    class: String,
    universe_uuid: String,
    authorization_id: String,
    recovery_point_uuid: String,
}
impl Target {
    fn key(&self) -> &str {
        if self.class == CLASS_FAILED_RESTORE_CLAIM {
            &self.authorization_id
        } else if self.class == CLASS_RECOVERY_POINT_AFTER_RETENTION {
            &self.recovery_point_uuid
        } else {
            &self.universe_uuid
        }
    }
    fn view(&self) -> Value {
        if self.class == CLASS_FAILED_RESTORE_CLAIM {
            json!({"class": self.class, "authorization_id": self.authorization_id, "universe_uuid": self.universe_uuid})
        } else if self.class == CLASS_RECOVERY_POINT_AFTER_RETENTION {
            json!({"class": self.class, "recovery_point_uuid": self.recovery_point_uuid, "universe_uuid": self.universe_uuid})
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
        if ![CLASS_TERMINAL_NOT_RESTORED, CLASS_TERMINAL_ABSENT, CLASS_FAILED_RESTORE_CLAIM, CLASS_RECOVERY_POINT_AFTER_RETENTION].contains(&class.as_str()) {
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
        let recovery_point_uuid = if class == CLASS_RECOVERY_POINT_AFTER_RETENTION {
            let value = lc::text(entry, "recovery_point_uuid")?.to_string();
            lc::token(&value)?;
            value
        } else {
            String::new()
        };
        targets.push(Target {
            class,
            universe_uuid,
            authorization_id,
            recovery_point_uuid,
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
fn container_facts(observed: Option<&Value>, r: &Reservation, uuid: &str, all: &[Value], blockers: &mut Vec<String>) -> Value {
    let elsewhere = all.iter().any(|c| c["Id"].as_str() == Some(r.container_id.as_str()));
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
        blockers.push("the container under the universe name does not carry this universe label".into());
    }
    if !lc::STOPPED.contains(&state) {
        blockers.push(format!(
            "the universe container is in state {state}: only a container observed created, exited or stopped is collectable"
        ));
    }
    if frozen {
        blockers.push("the universe container's cgroup is frozen: its state cannot be observed reliably".into());
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
        blockers.push(format!("authorization {id} has no recorded outcome document"));
        return (json!({"authorization_id": id, "outcome_present": false}), blockers);
    };
    let rehash = mg::sha256_bytes(text.as_bytes()).ok();
    if rehash.as_deref() != a["outcome_sha256"].as_str() {
        blockers.push(format!(
            "the outcome recorded for authorization {id} no longer hashes to the value recorded when it was completed"
        ));
    }
    let outcome: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    if !outcome.is_object() {
        blockers.push(format!("the outcome recorded for authorization {id} is not a JSON object"));
        return (
            json!({"authorization_id": id, "outcome_present": true, "outcome_malformed": true, "outcome_sha256_now": rehash}),
            blockers,
        );
    }
    let s = |k: &str| outcome[k].as_str().unwrap_or("");
    for (field, observed, expected) in [
        ("format", s("format"), tr::OUTCOME_FORMAT),
        ("authorization_id", s("authorization_id"), id),
        ("handoff_sha256", s("handoff_sha256"), a["handoff_sha256"].as_str().unwrap_or("")),
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
        blockers.push(format!("authorization {id} records no completing operation"));
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
    let open: Vec<&Value> = recorded.iter().filter(|a| a["state"].as_str() == Some("issued")).collect();
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
            blockers.push(format!("the reservation is in state {}: it is already settled", r.state));
        }
        if !restored.is_empty() {
            blockers.push("an authorization of this universe completed with a verified restore elsewhere".into());
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
        Some(CLASS_TERMINAL_NOT_RESTORED) => json!({"operation": "garbage_collect_apply", "action": "collect_reservation",
            "reservation_state": COLLECTED, "tombstone": true,
            "lifts": "create is refused for this universe UUID by the tombstone; start, stop, delete, clone and migration_restore_local of the recorded container become available again",
            "starts_nothing": true}),
        Some(CLASS_TERMINAL_ABSENT) => json!({"operation": "garbage_collect_apply", "action": "collect_reservation",
            "reservation_state": COLLECTED, "tombstone": true,
            "lifts": "nothing to operate: the container is gone; create stays refused for this universe UUID and the absent container ID never owns it again",
            "starts_nothing": true}),
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
fn classify_claim(db: &Connection, k: &Value, observed: Option<&Value>) -> Result<Candidate, Error> {
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
        let created_after = c["Created"].as_str().and_then(lc::epoch).is_some_and(|t| t >= claimed_at);
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
/// One prepared recovery point of this host, as the journal records it.
struct Point {
    recovery_point_uuid: String,
    universe_uuid: String,
    generation: i64,
    operation_id: String,
    state: String,
    manifest_sha256: String,
    rootfs_sha256: String,
    rootfs_bytes: i64,
    prepared_at: i64,
    outbox: String,
}
fn point_row(r: &rusqlite::Row) -> rusqlite::Result<Point> {
    Ok(Point {
        recovery_point_uuid: r.get(0)?,
        universe_uuid: r.get(1)?,
        generation: r.get(2)?,
        operation_id: r.get(3)?,
        state: r.get(4)?,
        manifest_sha256: r.get(5)?,
        rootfs_sha256: r.get(6)?,
        rootfs_bytes: r.get(7)?,
        prepared_at: r.get(8)?,
        outbox: r.get(9)?,
    })
}
const POINT_COLUMNS: &str = "recovery_point_uuid,universe_uuid,generation,operation_id,state,manifest_sha256,rootfs_sha256,rootfs_bytes,prepared_at,outbox";
/// Every prepared point, oldest generation first, so that the bound falls on the newest ones -- which the
/// retention keeps anyway.
fn prepared_points(db: &Connection) -> Result<Vec<Point>, Error> {
    let mut s = db.prepare(&format!(
        "SELECT {POINT_COLUMNS} FROM recovery_points WHERE state='prepared' ORDER BY universe_uuid,generation"
    ))?;
    let rows = s.query_map([], point_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
fn one_point(db: &Connection, point: &str) -> Result<Option<Point>, Error> {
    Ok(db
        .query_row(&format!("SELECT {POINT_COLUMNS} FROM recovery_points WHERE recovery_point_uuid=?1"), [point], point_row)
        .optional()?)
}
fn newest_generation(db: &Connection, uuid: &str) -> Result<i64, Error> {
    Ok(db.query_row("SELECT COALESCE(MAX(generation),0) FROM recovery_points WHERE universe_uuid=?1", [uuid], |r| r.get(0))?)
}

/// The class 5 candidate: every condition of the contract, each either a proof or a blocker, and the archive
/// re-hashed against what the manifest bound. `hash` is false in a plan, where the archive is only sized, and
/// true immediately before an effect, where nothing is removed that does not still hash to its record.
fn classify_point(db: &Connection, p: &Point, hash: bool) -> Result<Candidate, Error> {
    let now = crate::now() as i64;
    let mut blockers = vec![];
    let mut proofs = json!({
        "recovery_point_uuid": p.recovery_point_uuid, "universe_uuid": p.universe_uuid, "generation": p.generation,
        "prepare_operation_id": p.operation_id, "state": p.state, "prepared_at": p.prepared_at,
        "age_seconds": now - p.prepared_at, "recorded_rootfs_bytes": p.rootfs_bytes, "recorded_rootfs_sha256": p.rootfs_sha256,
    });
    // 1. The operation is terminal and its result is in the journal. A prepare keeps its own durable
    //    record -- the point row, written only once the export was digested -- and its terminal artifact is
    //    the manifest, which must still hash to what that record bound: that is what survives as the
    //    retained manifest, so it is checked before anything else.
    if p.state != "prepared" {
        blockers.push(format!("the point is in state {}, not prepared", p.state));
    }
    let manifest_path = std::path::Path::new(&p.outbox).join(crate::recovery_point::MANIFEST);
    match std::fs::read(&manifest_path) {
        Ok(bytes) => {
            let digest = mg::sha256_bytes(&bytes)?;
            proofs["manifest"] = json!({"present": true, "sha256_matches_record": digest == p.manifest_sha256});
            if digest != p.manifest_sha256 {
                blockers.push("the manifest on disk does not hash to the digest its record binds; nothing is collected on an inconsistent record".into());
            }
        }
        Err(e) => {
            proofs["manifest"] = json!({"present": false, "error": e.to_string()});
            blockers.push(format!("the manifest could not be read ({e}); a point without its manifest is an inconsistency to investigate"));
        }
    }
    // 2. Nothing references it: a newer generation only names it as lineage, each archive is a full export;
    //    what does reference it is the retention's own kept set, checked below.
    // 3. A declared retention has elapsed.
    match rt::retention(db, &p.universe_uuid)? {
        None => {
            proofs["retention"] = Value::Null;
            blockers.push("no retention is declared for this universe; without one there is no elapsed retention to prove".into());
        }
        Some(r) => {
            let newest = newest_generation(db, &p.universe_uuid)?;
            let kept = p.generation > newest - r.keep_latest as i64;
            let age = now - p.prepared_at;
            proofs["retention"] = r.view();
            proofs["newest_generation"] = json!(newest);
            proofs["within_kept_generations"] = json!(kept);
            proofs["minimum_age_elapsed"] = json!(age >= r.minimum_age_seconds as i64);
            if kept {
                blockers.push(format!("generation {} is among the newest {} the retention keeps (newest is {newest})", p.generation, r.keep_latest));
            }
            if age < r.minimum_age_seconds as i64 {
                blockers.push(format!("the point is {age} seconds old and the retention keeps every point younger than {}", r.minimum_age_seconds));
            }
        }
    }
    // 4. No hold. An unreadable hold state is a hold.
    match rt::holds(db, &p.universe_uuid) {
        Err(e) => {
            proofs["holds"] = json!({"readable": false, "error": e.to_string()});
            blockers.push(format!("the hold state could not be read ({e}); an unknown hold is a hold"));
        }
        Ok(held) => {
            proofs["holds"] = json!({"readable": true, "in_force": held.iter().map(rt::Hold::view).collect::<Vec<_>>()});
            if let Some(why) = rt::hold_blocks(&held, true, false) {
                blockers.push(why);
            }
        }
    }
    // 6. The path is this service's own outbox for this point, and nothing else: recomputed from the
    //    identifier, never taken from the record or the caller, and the record must agree. Not reachable
    //    by the check: every record this service writes agrees with itself, and the removal below derives
    //    the path again from the identifier, so a forged record could at most block, never redirect.
    let expected = tr::outbox(&p.recovery_point_uuid)?;
    let path_matches = expected.to_string_lossy() == p.outbox;
    proofs["outbox_path_recomputed_from_identifier"] = json!(path_matches);
    if !path_matches {
        blockers.push("the recorded outbox path is not the path this service derives for the point; nothing under it is touched".into());
    }
    let rootfs = expected.join(crate::recovery_point::ROOTFS);
    match tr::regular_file(&rootfs) {
        Err(e) => {
            proofs["archive"] = json!({"present": false, "error": e.to_string()});
            blockers.push(format!("the archive is not a regular file ({e})"));
        }
        Ok(None) => {
            proofs["archive"] = json!({"present": false});
            blockers.push("the archive is absent: a point whose archive is already gone is an inconsistency to investigate, not something to collect".into());
        }
        Ok(Some(size)) => {
            proofs["archive"] = json!({"present": true, "bytes": size, "bytes_match_record": size == p.rootfs_bytes as u64});
            if size != p.rootfs_bytes as u64 {
                blockers.push(format!("the archive is {size} bytes, not the {} its record binds", p.rootfs_bytes));
            } else if hash {
                let digest = mg::sha256(&rootfs)?;
                proofs["archive"]["sha256_rehashed"] = json!(digest);
                proofs["archive"]["sha256_matches_record"] = json!(digest == p.rootfs_sha256);
                if digest != p.rootfs_sha256 {
                    blockers.push("the archive no longer hashes to the digest its record binds; it is kept as evidence, never collected".into());
                }
            }
        }
    }
    Ok(Candidate {
        kind: "recovery_point",
        key: p.recovery_point_uuid.clone(),
        universe_uuid: p.universe_uuid.clone(),
        class: Some(CLASS_RECOVERY_POINT_AFTER_RETENTION),
        class_number: Some(5),
        proofs,
        blockers,
        effect: json!({"proposed": "write a retained manifest and mark the point collected in one transaction, then remove the archive and its directory; the manifest's content survives in the journal"}),
    })
}

/// Holds for the reservation and claim classes, added after their own classification: an investigation
/// hold blocks the terminal transition and the abort alike; an evidence hold blocks only a reclaim, which is
/// decided at apply time by the request, so a plan records the hold and lets the apply refuse the reclaim.
fn add_hold_blockers(db: &Connection, c: &mut Candidate, reclaims_runtime: bool) {
    match rt::holds(db, &c.universe_uuid) {
        Err(e) => {
            c.proofs["holds"] = json!({"readable": false, "error": e.to_string()});
            c.blockers.push(format!("the hold state could not be read ({e}); an unknown hold is a hold"));
        }
        Ok(held) => {
            c.proofs["holds"] = json!({"readable": true, "in_force": held.iter().map(rt::Hold::view).collect::<Vec<_>>()});
            if let Some(why) = rt::hold_blocks(&held, false, reclaims_runtime) {
                c.blockers.push(why);
            }
        }
    }
}

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
        Ok(text) => Ok(serde_json::from_str::<Value>(&text)?.as_array().cloned().unwrap_or_default()),
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
    let listed = listed.as_array().ok_or("universe_uuids must be an array of universe UUIDs")?;
    if listed.is_empty() || listed.len() > limits.max_candidates {
        return Err(format!(
            "universe_uuids must name from 1 to max_candidates ({}) universes",
            limits.max_candidates
        )
        .into());
    }
    let mut scope = vec![];
    for value in listed {
        let uuid = value.as_str().filter(|v| lc::is_uuid(v)).ok_or("universe_uuids must be UUIDs")?;
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
    let mut points = prepared_points(db)?;
    if let Some(scope) = scope {
        points.retain(|p| scope.contains(&p.universe_uuid));
    }
    // One bound for the whole run, shared so that no kind starves another: each is guaranteed a third of it
    // and may use whatever the others leave. A host with many dead reservations must not hide the failed
    // restore claim that is holding processes and disk right now, nor the archives filling the disk.
    let third = limits.max_candidates.div_ceil(3);
    let claims_take = claims.len().min(third);
    let points_take = points.len().min(third);
    let examined_reservations: Vec<&&(String, Reservation)> = open
        .iter()
        .take(limits.max_candidates.saturating_sub(claims_take + points_take))
        .collect();
    let examined_claims: Vec<&Value> = claims
        .iter()
        .take(limits.max_candidates.saturating_sub(examined_reservations.len() + points_take))
        .collect();
    let examined_points: Vec<&Point> = points
        .iter()
        .take(limits.max_candidates.saturating_sub(examined_reservations.len() + examined_claims.len()))
        .collect();
    let truncated = open.len() > examined_reservations.len()
        || claims.len() > examined_claims.len()
        || points.len() > examined_points.len();

    let mut names: Vec<String> = examined_reservations.iter().map(|(u, _)| format!("podmesh-{u}")).collect();
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
        let mut c = classify_reservation(db, uuid, r, observed, &all, &host)?;
        add_hold_blockers(db, &mut c, false);
        candidates.push(c);
    }
    for k in &examined_claims {
        let observed = named(&inspected, &format!("podmesh-{}", k["universe_uuid"].as_str().unwrap_or("")));
        let mut c = classify_claim(db, k, observed)?;
        add_hold_blockers(db, &mut c, false);
        candidates.push(c);
    }
    for p in &examined_points {
        candidates.push(classify_point(db, p, false)?);
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
            None => json!({"universes": "every reservation, unresolved restore claim and prepared recovery point on this host, up to the bound"}),
            Some(s) => json!({"universe_uuids": s}),
        },
        "limits": {"max_candidates": limits.max_candidates, "reservations_open": open.len(),
            "reservations_examined": examined_reservations.len(), "unresolved_restore_claims": claims.len(),
            "restore_claims_examined": examined_claims.len(), "prepared_recovery_points": points.len(),
            "recovery_points_examined": examined_points.len(), "truncated": truncated,
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
            format!("The plan did not examine {}; nothing was collected", t.key()),
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
fn container_unchanged(before: Option<&Value>, after: Option<&Value>, error: Option<&str>) -> Option<bool> {
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
fn reservation_verdict(state: &str, tombstone: bool, create_refused: bool, unchanged: Option<bool>, error: Option<&str>) -> Vec<String> {
    let mut blockers = vec![];
    if state != COLLECTED {
        blockers.push(format!("the reservation is in state {state} after the collection"));
    }
    if !tombstone {
        blockers.push("no tombstone is recorded for the collected universe".into());
    }
    if !create_refused {
        blockers.push("the tombstone does not refuse a create of the collected universe UUID".into());
    }
    if let Some(e) = error {
        blockers.push(format!(
            "the universe container could not be observed after the collection, so its state is unknown: {e}"
        ));
    }
    if unchanged == Some(false) {
        blockers.push("the universe container is not the one observed immediately before the collection".into());
    }
    blockers
}
/// The verdict on what a delegated abort left behind. An unknown observation is unknown, never a success.
fn claim_verdict(claim_unresolved: bool, container_present: bool, error: Option<&str>, processes: Option<&Value>) -> Vec<String> {
    let mut blockers = vec![];
    if claim_unresolved {
        blockers.push("the restore claim is still unresolved after the abort".into());
    }
    if let Some(e) = error {
        blockers.push(format!(
            "the universe container could not be observed after the abort, so its absence is unknown: {e}"
        ));
    } else if container_present {
        blockers.push("a container is still present under the universe name after the abort".into());
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

/// Re-reads what a collection left behind, and never fails the operation that made it. Used immediately
/// after the effect and again when a run recovers an effect it had already committed.
fn verify_collection(db: &Connection, t: &Target, class_number: Option<u64>, before: Option<&Value>, from_state: &Value) -> Value {
    let uuid = t.universe_uuid.as_str();
    let (after, error) = observe_quietly(&format!("podmesh-{uuid}"));
    let reservation = mg::reservation(db, uuid).ok().flatten();
    let state_now = reservation.map(|r| r.state).unwrap_or_default();
    let tombstone = mg::tombstone(db, uuid).ok().flatten();
    let history = mg::collection_history(db, uuid).unwrap_or_default();
    let create_refused = mg::refuse_identity_reuse(db, uuid, "create").is_err();
    let generic_refused = mg::refuse_if_reserved(db, uuid, "start").is_err();
    let unchanged = container_unchanged(before, after.as_ref(), error.as_deref());
    let blockers = reservation_verdict(&state_now, tombstone.is_some(), create_refused, unchanged, error.as_deref());
    json!({"action": "collected_reservation", "universe_uuid": uuid, "class": t.class, "class_number": class_number,
        "collected_from_state": from_state, "reservation": {"state": state_now}, "tombstone": tombstone,
        "collection_history": history,
        "verified": blockers.is_empty(), "verification_blockers": blockers,
        "verified_from_outside": {"observed_at": crate::now(), "reservation_state": state_now,
            "tombstone_present": tombstone.is_some(), "create_of_this_universe_uuid_refused": create_refused,
            "generic_operations_refused": generic_refused, "container_unchanged_by_the_collection": unchanged,
            "container_before": before, "container": after.as_ref().map(lc::state_view),
            "observation_error": error},
        "starts_nothing": "the collection changed a recorded decision; it did not start, stop or remove anything",
        "history": "the reservation row, its artifacts, every authorization and every outcome are kept; the tombstone keeps the first proof and the collection history keeps every occurrence"})
}

/// The terminal state and the tombstone of a collected reservation, written in one transaction with the
/// run's own progress row, and then verified from outside. Nothing after the commit may fail this call: a
/// committed effect is a fact, and a verification that cannot be made is an unknown, never a refusal.
fn collect_reservation(db: &Connection, id: &str, reference: &str, t: &Target, candidate: &Candidate) -> Result<Value, Error> {
    let uuid = t.universe_uuid.as_str();
    let r = mg::reservation(db, uuid)?.ok_or("The reservation disappeared between the proof and the effect")?;
    let before = candidate.proofs["container"].clone();
    let now = crate::now() as i64;
    let absent = before["present"] != true;
    let from_state = json!(r.state);
    let proof = json!({"class": t.class, "class_number": candidate.class_number, "collected_by_operation": id,
        "authorization_ref": reference, "collected_at": now, "collected_from_state": r.state,
        "collector_version": COLLECTOR_VERSION, "policy_version": POLICY_VERSION, "proofs": candidate.proofs});
    let pending = json!({"action": "collected_reservation", "universe_uuid": uuid, "class": t.class,
        "class_number": candidate.class_number, "collected_from_state": from_state, "collected_at": now,
        "container_before": before, "verification": "pending"});
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
    let mut done = verify_collection(db, t, candidate.class_number, Some(&before), &from_state);
    done["tombstone_written_by_this_collection"] = json!(first);
    // Best effort: the progress row already exists, and losing this update costs a re-verification, not an
    // effect. It must never turn a committed collection into an error.
    let _ = record_effect(db, id, t, &done);
    Ok(done)
}

/// Re-reads what a delegated abort left behind, and never fails the operation that made it.
fn verify_abort(db: &Connection, t: &Target, class_number: Option<u64>, claim_before: &Value, reclaim: bool, outcome: Value) -> Value {
    let uuid = t.universe_uuid.as_str();
    let (after, error) = observe_quietly(&format!("podmesh-{uuid}"));
    let claim = one_claim(db, &t.authorization_id).ok().flatten();
    let processes = claim_before["container_id"]
        .as_str()
        .map(|cid| cleanup::runtime_processes(cid, claim_before["created_at"].as_i64().unwrap_or(0)));
    let signalled = outcome["reclaim"]["signalled"]
        .as_array()
        .map(|s| s.iter().filter(|e| e["decision"] == "sigkill").count())
        .unwrap_or(0);
    let blockers = claim_verdict(claim.is_some(), after.is_some(), error.as_deref(), processes.as_ref());
    json!({"action": "aborted_failed_restore", "universe_uuid": uuid, "authorization_id": t.authorization_id,
        "class": t.class, "class_number": class_number, "reclaim_processes": reclaim,
        "delegated_to": "migration_restore_abort", "result": outcome,
        "reclaim_performed": outcome["reclaim"]["requested"] == true, "processes_signalled": signalled,
        "verified": blockers.is_empty(), "verification_blockers": blockers,
        "verified_from_outside": {"observed_at": crate::now(), "claim_unresolved": claim.is_some(),
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
    match ds::abort(db, id, uuid, &name, &t.authorization_id, reference, reclaim, existing) {
        Ok(outcome) => {
            let done = verify_abort(db, t, candidate.class_number, &claim_before, reclaim, outcome);
            let _ = record_effect(db, id, t, &done);
            Ok(done)
        }
        Err(e) => {
            // The abort's contract is to refuse without effect, and every refusal observed so far did.
            // If its own journal nevertheless shows that this operation closed the claim, the honest answer
            // is a committed effect whose verification failed, not a claim that nothing happened.
            if claim_closed_by(db, &t.authorization_id, id).unwrap_or(false) {
                let mut done = verify_abort(
                    db,
                    t,
                    candidate.class_number,
                    &claim_before,
                    reclaim,
                    json!({"error": e.to_string()}),
                );
                let blockers = done["verification_blockers"].as_array().cloned().unwrap_or_default();
                let mut blockers: Vec<Value> = blockers;
                blockers.push(json!(format!(
                    "the abort reported an error after it had already closed this claim: {e}"
                )));
                done["verified"] = json!(false);
                done["verification_blockers"] = json!(blockers);
                let _ = record_effect(db, id, t, &done);
                return Ok(done);
            }
            Err(e)
        }
    }
}

/// Separately authorized, bounded, idempotent by operation ID. Each effect repeats its own proofs
/// immediately before it acts and stops the run at the first mismatch.
fn apply(db: &Connection, id: &str, reference: &str, limits: &Limits, request: &Value, targets: &[Target]) -> Result<Value, Error> {
    let host = mg::host_uuid(db)?;
    let plan_operation = lc::text(request, "plan_operation_id")?;
    let reclaim = match request.get("reclaim_processes") {
        None | Some(Value::Null) => false,
        Some(v) => v.as_bool().ok_or("reclaim_processes must be true or false")?,
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
    let mut reserved_reclaims = 0usize;
    let mut performed_reclaims = 0usize;
    let mut signalled = 0usize;
    let mut bytes_removed = 0usize;
    let mut stopped: Option<Value> = None;
    for t in targets {
        // An effect this very operation already committed is recovered, never repeated: a run interrupted
        // between its effect and its report comes back to exactly what it did.
        let progress = recorded_effect(db, id, t.key())?;
        let done_before = progress.is_some() || already_applied(db, id, t)?;
        if done_before {
            let mut entry = if t.class == CLASS_RECOVERY_POINT_AFTER_RETENTION {
                // The retained manifest is committed; the removal may not have happened. Finishing it is
                // completing the recorded effect, not repeating one: what is removed is what the journal
                // already says is collected.
                finish_point_removal(db, id, t)?
            } else {
                match (progress, t.class == CLASS_FAILED_RESTORE_CLAIM) {
                    (Some(p), false) if p["verification"] != "pending" => p,
                    (_, false) => verify_collection(db, t, None, None, &Value::Null),
                    (Some(p), true) => p,
                    (None, true) => json!({"action": "aborted_failed_restore", "universe_uuid": t.universe_uuid,
                        "authorization_id": t.authorization_id, "class": t.class,
                        "note": "recovered from the claim this operation closed; the collector's own progress row was lost"}),
                }
            };
            entry["applied"] = json!(true);
            entry["recovered"] = json!(true);
            entry["recovery_note"] =
                json!("this effect was already committed by this operation ID; it was recovered from the journal, not repeated");
            effects += 1;
            recovered += 1;
            results.push(entry);
            continue;
        }
        if effects >= limits.max_effects {
            stopped = Some(json!({"candidate": t.view(), "reason": format!(
                "the maximum of {} effect(s) for this run was reached; the remaining candidates were not acted on",
                limits.max_effects)}));
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
            let details = e.downcast_ref::<lc::Failure>().map(|f| f.details.clone()).unwrap_or(Value::Null);
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
            break;
        }
        let candidate = candidate.ok_or("Candidate lost between its proof and its effect")?;
        // The budget is reserved *before* a delegated call that is authorized to signal, not counted after
        // it: the abort observes the processes itself, so what this run predicted cannot bound what it may
        // do. Every attempt authorized to signal costs one unit, whether it ends up signalling or not.
        let may_signal = reclaim && t.class == CLASS_FAILED_RESTORE_CLAIM;
        // The holds, read again right now for the effect this request actually asks for: an evidence hold
        // that a plan could only record becomes a refusal here once the request says it may reclaim.
        //
        // For class 5 this is defence in depth: the fresh classification above already carries the hold as
        // a blocker, and removing this guard leaves the retention check green -- which is why that is
        // written here. For class 3 with `reclaim_processes` it is THE rule (an evidence hold blocks a
        // runtime reclaim and nothing upstream knows what the request asked), and the single-host check
        // cannot reach it: `retention::hold_blocks` is unit-tested for that case instead.
        let hold_refusal = match rt::holds(db, &t.universe_uuid) {
            Err(e) => Some(format!("the hold state could not be read ({e}); an unknown hold is a hold")),
            Ok(held) => rt::hold_blocks(&held, t.class == CLASS_RECOVERY_POINT_AFTER_RETENTION, may_signal),
        };
        if let Some(why) = hold_refusal {
            let e = failure(format!("A hold blocks this effect: {why}"), json!({"candidate": t.view()}));
            if effects == 0 {
                return Err(failure(format!("Collection refused: {e}"), json!({"candidate": t.view(), "applied": results})));
            }
            results.push(json!({"candidate": t.view(), "applied": false, "refused": e.to_string()}));
            stopped = Some(json!({"candidate": t.view(), "reason": e.to_string()}));
            break;
        }
        // The bytes bound, reserved before the removal like the reclaim budget: an archive larger than what
        // the run has left is not removed, and the run says so rather than exceeding its own number.
        if t.class == CLASS_RECOVERY_POINT_AFTER_RETENTION {
            let size = candidate.proofs["archive"]["bytes"].as_u64().unwrap_or(u64::MAX) as usize;
            if bytes_removed.saturating_add(size) > limits.max_bytes {
                stopped = Some(json!({"candidate": t.view(), "reason": format!(
                    "removing this {size}-byte archive would exceed the run's max_bytes of {} ({bytes_removed} already removed); it was not acted on",
                    limits.max_bytes)}));
                if effects == 0 {
                    return Err(failure(
                        format!("Collection refused: the archive of {} is {size} bytes and the run's max_bytes is {}; nothing was collected", t.key(), limits.max_bytes),
                        json!({"candidate": t.view(), "limits": limits.view(Mode::Apply)}),
                    ));
                }
                break;
            }
        }
        if may_signal && reserved_reclaims >= limits.max_runtime_reclaims {
            stopped = Some(json!({"candidate": t.view(), "reason": format!(
                "the run's allowance of {} runtime reclaim(s) is spent; this candidate was not acted on, because a delegated abort authorized to signal must have budget reserved before it is called",
                limits.max_runtime_reclaims)}));
            break;
        }
        if may_signal {
            reserved_reclaims += 1;
        }
        let outcome = if t.class == CLASS_FAILED_RESTORE_CLAIM {
            abort_failed_restore(db, id, reference, t, &candidate, reclaim)
        } else if t.class == CLASS_RECOVERY_POINT_AFTER_RETENTION {
            collect_point(db, id, t, &candidate)
        } else {
            collect_reservation(db, id, reference, t, &candidate)
        };
        match outcome {
            Ok(mut done) => {
                effects += 1;
                bytes_removed += done["bytes_removed"].as_u64().unwrap_or(0) as usize;
                if done["reclaim_performed"] == true {
                    performed_reclaims += 1;
                }
                signalled += done["processes_signalled"].as_u64().unwrap_or(0) as usize;
                done["applied"] = json!(true);
                done["proofs_repeated_before_the_effect"] = candidate.proofs.clone();
                // An effect whose own verification from outside did not hold stops the run: the contract
                // stops at the first unexpected identity or state, and this one is already recorded.
                let verified = done["verified"] != false;
                let blockers = done["verification_blockers"].clone();
                results.push(done);
                if !verified {
                    stopped = Some(json!({"candidate": t.view(),
                        "reason": "the verification from outside did not hold for the effect that was just applied",
                        "verification_blockers": blockers}));
                    break;
                }
            }
            Err(e) => {
                let details = e.downcast_ref::<lc::Failure>().map(|f| f.details.clone()).unwrap_or(Value::Null);
                if effects == 0 {
                    return Err(failure(
                        format!("Collection refused: {e}"),
                        json!({"candidate": t.view(), "detail": details, "applied": results}),
                    ));
                }
                results.push(json!({"candidate": t.view(), "applied": false, "refused": e.to_string(), "details": details}));
                stopped = Some(json!({"candidate": t.view(), "reason": e.to_string()}));
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
        "processes_signalled": signalled,
        "bytes_removed": bytes_removed,
        "verified": unverified.is_empty(),
        "unverified_effects": unverified,
        "stopped_before_the_rest": stopped,
        "results": results,
    }))
}

/// What the removal must leave: nothing at the point's outbox path. Read from outside the journal.
fn point_removal_verdict(dir: &std::path::Path) -> (bool, Vec<String>) {
    let mut blockers = vec![];
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => blockers.push(format!("the outbox directory could not be observed after the removal: {e}")),
        Ok(_) => blockers.push("the outbox directory is still present after the removal".into()),
    }
    (blockers.is_empty(), blockers)
}
/// Removes what the retained manifest already covers: the archive, the manifest file, the directory. Bounded
/// to the point's own directory, which is recomputed from its identifier; nothing is traversed.
fn remove_point_files(point: &str) -> Result<u64, Error> {
    let dir = tr::outbox(point)?;
    let mut removed = 0u64;
    for name in [crate::recovery_point::ROOTFS, crate::recovery_point::MANIFEST, "rootfs.tar.partial"] {
        let path = dir.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
            Ok(m) if m.is_file() => {
                removed += m.len();
                std::fs::remove_file(&path)?;
            }
            Ok(_) => return Err(format!("{} is not a regular file; nothing under the directory is removed", path.display()).into()),
        }
    }
    match std::fs::remove_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("the point's directory could not be removed ({e}); anything else it holds was not written by this service").into()),
    }
    Ok(removed)
}
/// The class 5 effect. The retained manifest and the terminal state are committed together BEFORE any byte
/// is removed, so that a crash between the two leaves a journal that already says what was collected and a
/// retry that finishes the removal rather than repeating a decision. The ORDER is not reachable by the
/// check -- it simulates the crash from a copy of the files, which passes whichever side of the commit the
/// removal sits on -- and is stated here as the contract's own requirement rather than a tested one.
fn collect_point(db: &Connection, id: &str, t: &Target, candidate: &Candidate) -> Result<Value, Error> {
    let p = one_point(db, &t.recovery_point_uuid)?.ok_or("The point vanished between its proof and its effect")?;
    let dir = tr::outbox(&p.recovery_point_uuid)?;
    let manifest_bytes = std::fs::read(dir.join(crate::recovery_point::MANIFEST))?;
    let manifest_digest = mg::sha256_bytes(&manifest_bytes)?;
    if manifest_digest != p.manifest_sha256 {
        return Err(failure(
            "The manifest on disk does not hash to the digest its record binds; it is kept as evidence and nothing was collected",
            json!({"candidate": t.view()}),
        ));
    }
    let manifest_text = String::from_utf8(manifest_bytes)?;
    let tx = db.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO recovery_point_retained VALUES(?1,?2,?3,'outbox',?4,?5,?6,?7,?8,'collected',?9,?10,?11)",
        params![
            p.recovery_point_uuid, p.universe_uuid, p.generation, manifest_text, p.manifest_sha256, p.rootfs_sha256,
            p.rootfs_bytes, p.operation_id, crate::now() as i64, id, candidate.proofs["retention"].to_string()
        ],
    )?;
    tx.execute("UPDATE recovery_points SET state='collected' WHERE recovery_point_uuid=?1", [&p.recovery_point_uuid])?;
    record_effect(&tx, id, t, &json!({"action": "collected_recovery_point", "class": t.class, "universe_uuid": t.universe_uuid,
        "recovery_point_uuid": t.recovery_point_uuid, "verification": "pending"}))?;
    tx.commit()?;
    // Nothing after the commit may fail this call: the point is collected and its manifest
    // retained, which is a fact, and a removal that did not finish is a verification blocker
    // the retry of this same operation finishes -- never a refusal of something that happened.
    let (removed, mut blockers) = match remove_point_files(&p.recovery_point_uuid) {
        Ok(n) => (n, vec![]),
        Err(e) => (0, vec![format!("the removal did not finish after the record was committed: {e}")]),
    };
    let (clean, more) = point_removal_verdict(&dir);
    blockers.extend(more);
    let verified = clean && blockers.is_empty();
    let done = json!({
        "action": "collected_recovery_point", "class": t.class, "universe_uuid": t.universe_uuid,
        "recovery_point_uuid": t.recovery_point_uuid, "generation": p.generation,
        "retained_manifest": {"manifest_sha256": p.manifest_sha256, "rootfs_sha256": p.rootfs_sha256, "rootfs_bytes": p.rootfs_bytes,
            "prepare_operation_id": p.operation_id, "terminal_state": "collected", "collecting_operation_id": id},
        "bytes_removed": removed, "verified": verified, "verification_blockers": blockers,
        "verification": "recorded",
    });
    let _ = record_effect(db, id, t, &done);
    Ok(done)
}
/// A recovered class 5 effect: the journal already says the point is collected under this operation; what
/// may be left is the removal itself, which is finished here and verified from outside.
fn finish_point_removal(db: &Connection, id: &str, t: &Target) -> Result<Value, Error> {
    let (removed, mut blockers) = match remove_point_files(&t.recovery_point_uuid) {
        Ok(n) => (n, vec![]),
        Err(e) => (0, vec![format!("the removal did not finish: {e}")]),
    };
    let dir = tr::outbox(&t.recovery_point_uuid)?;
    let (clean, more) = point_removal_verdict(&dir);
    blockers.extend(more);
    let verified = clean && blockers.is_empty();
    let done = json!({
        "action": "collected_recovery_point", "class": t.class, "universe_uuid": t.universe_uuid,
        "recovery_point_uuid": t.recovery_point_uuid, "bytes_removed": removed,
        "verified": verified, "verification_blockers": blockers, "verification": "recorded",
        "note": "the retained manifest was already committed by this operation; the removal was finished, not repeated",
    });
    let _ = record_effect(db, id, t, &done);
    Ok(done)
}

/// The fresh classification of one named candidate, immediately before its effect.
fn fresh(db: &Connection, t: &Target, host: &str) -> Result<Result<Candidate, Error>, Error> {
    let uuid = t.universe_uuid.as_str();
    if t.class == CLASS_RECOVERY_POINT_AFTER_RETENTION {
        let Some(p) = one_point(db, &t.recovery_point_uuid)? else {
            return Ok(Err(failure(
                format!("No recovery point {} is recorded on this host", t.recovery_point_uuid),
                json!({"candidate": t.view()}),
            )));
        };
        if p.universe_uuid != uuid {
            return Ok(Err(failure("The recovery point belongs to another universe", json!({"candidate": t.view()}))));
        }
        let candidate = classify_point(db, &p, true)?;
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
        return Ok(Ok(candidate));
    }
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

/// A replay never repeats an effect: the persisted record comes back with a fresh observation beside it.
fn replay(db: &Connection, id: &str, record: Option<String>) -> Result<Value, Error> {
    let original: Value = serde_json::from_str(&record.ok_or("Missing persisted result")?)?;
    let verified_at: Option<i64> = db.query_row(
        "SELECT MAX(finished_at) FROM operation_attempts WHERE operation_id=?1 AND outcome='verified'",
        [id],
        |r| r.get(0),
    )?;
    let mut current = vec![];
    let keys: Vec<Value> = match original["mode"].as_str() {
        // An applied effect names its universe directly; a refused candidate carries it under `candidate`.
        Some("apply") => original["results"]
            .as_array()
            .map(|v| {
                v.iter()
                    .map(|r| {
                        if r["universe_uuid"].is_string() {
                            json!({"class": r["class"], "universe_uuid": r["universe_uuid"], "recovery_point_uuid": r["recovery_point_uuid"]})
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
        let Some(uuid) = k["universe_uuid"].as_str() else { continue };
        if k["class"].as_str() == Some(CLASS_RECOVERY_POINT_AFTER_RETENTION) {
            let point = k["recovery_point_uuid"].as_str().or(k["key"].as_str()).unwrap_or("");
            let state: Option<String> = db
                .query_row("SELECT state FROM recovery_points WHERE recovery_point_uuid=?1", [point], |r| r.get(0))
                .optional()?;
            let present = tr::outbox(point).ok().map(|d| d.exists());
            current.push(json!({"universe_uuid": uuid, "recovery_point_uuid": point, "state": state, "outbox_present": present}));
            continue;
        }
        let reservation = mg::reservation(db, uuid)?.map(|r| json!({"state": r.state, "updated_at": r.updated_at}));
        current.push(
            json!({"universe_uuid": uuid, "reservation": reservation, "tombstone": mg::tombstone(db, uuid)?,
            "container": lc::observe(uuid)?}),
        );
    }
    Ok(json!({
        "replayed": true, "historical": true,
        "notice": "original_result is the record persisted when this collection was verified, not current state; current is a fresh observation. A replayed collection repeats no effect.",
        "verified_at": verified_at, "original_result": original,
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
    let scope = if mode == Mode::Plan { parse_scope(request, &limits)? } else { None };
    lc::ensure_schema(db)?;
    ensure_schema(db)?;
    let canonical = request.to_string();
    let previous: Option<(String, String, Option<String>)> = db
        .query_row("SELECT request,status,result FROM operations WHERE id=?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    if let Some((saved, status, result)) = previous {
        if saved != canonical {
            return Err("Operation ID already belongs to a different request".into());
        }
        if status == "verified" {
            return replay(db, id, result);
        }
    } else {
        db.execute("INSERT INTO operations VALUES(?1,?2,'pending',NULL)", params![id, canonical])?;
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
            let status = if body["verified"] == false {
                "completed_with_unverified_effects"
            } else {
                "verified"
            };
            let mut record = json!({
                "status": status, "operation": mode.operation(), "mode": mode.name(),
                "collection_operation_id": id, "requester": {"authorization_ref": reference},
                "collector_version": COLLECTOR_VERSION,
                "build_version": option_env!("PODMESH_PACKAGE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
                "policy_version": POLICY_VERSION,
                "started_at": started, "finished_at": finished,
                "authority": AUTHORITY,
                "scope": "docs/GARBAGE-COLLECTION.md classes 1 and 2 (terminal reservations, with tombstones), class 3 by delegation to migration_restore_abort, and class 5 for recovery point archives after a declared retention with both hold scopes; checkpoint artifacts are not collected; no timer",
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
                "UPDATE operations SET status='verified', result=?2 WHERE id=?1",
                params![id, record.to_string()],
            )?;
            db.execute(
                "UPDATE operation_attempts SET finished_at=?2, outcome='verified' WHERE id=?1",
                params![attempt, finished],
            )?;
            Ok(record)
        }
        Err(e) => {
            let details = e.downcast_ref::<lc::Failure>().map(|f| f.details.clone()).unwrap_or(Value::Null);
            let failed = json!({"error": e.to_string(), "details": details}).to_string();
            db.execute("UPDATE operations SET status='failed', result=?2 WHERE id=?1", params![id, failed])?;
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
    use super::{claim_verdict, container_unchanged, reservation_verdict, COLLECTED};
    use serde_json::json;

    fn before(id: &str, state: &str, checkpointed: bool) -> serde_json::Value {
        json!({"present": true, "observed_container_id": id, "state": state, "checkpointed": checkpointed})
    }
    fn after(id: &str, state: &str, checkpointed: bool) -> serde_json::Value {
        json!({"Id": id, "State": {"Status": state, "Checkpointed": checkpointed, "Pid": 0}})
    }

    #[test]
    fn a_collection_that_held_verifies() {
        let unchanged = container_unchanged(Some(&before("a", "exited", true)), Some(&after("a", "exited", true)), None);
        assert_eq!(unchanged, Some(true));
        assert!(reservation_verdict(COLLECTED, true, true, unchanged, None).is_empty());
    }
    #[test]
    fn an_observation_that_could_not_be_made_is_unknown_not_success() {
        // The effect is committed; the container could not be read. That is a blocker, never a pass, and it
        // is never reported as a refusal.
        let unchanged = container_unchanged(Some(&before("a", "exited", true)), None, Some("Podman ps exceeded its bound"));
        assert_eq!(unchanged, None);
        let blockers = reservation_verdict(COLLECTED, true, true, unchanged, Some("Podman ps exceeded its bound"));
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("could not be observed"), "{blockers:?}");
    }
    #[test]
    fn a_changed_or_unprotected_collection_does_not_verify() {
        let replaced = container_unchanged(Some(&before("a", "exited", true)), Some(&after("b", "exited", true)), None);
        assert_eq!(replaced, Some(false));
        assert_eq!(reservation_verdict(COLLECTED, true, true, replaced, None).len(), 1);
        let started = container_unchanged(Some(&before("a", "exited", true)), Some(&after("a", "running", false)), None);
        assert_eq!(started, Some(false));
        // A reservation that is not collected, without a tombstone and without the create refusal: three.
        assert_eq!(reservation_verdict("checkpointed", false, false, Some(true), None).len(), 3);
    }
    #[test]
    fn a_recovered_effect_cannot_compare_a_container_it_never_saw() {
        assert_eq!(container_unchanged(None, Some(&after("a", "exited", true)), None), None);
        assert!(reservation_verdict(COLLECTED, true, true, None, None).is_empty());
    }
    #[test]
    fn an_abort_that_ended_its_claim_verifies() {
        let gone = json!({"authorizes_reclaim": true, "count": 0});
        assert!(claim_verdict(false, false, None, Some(&gone)).is_empty());
        // No container was ever recorded for the claim, so there is no cgroup to read: not a blocker.
        assert!(claim_verdict(false, false, None, None).is_empty());
    }
    #[test]
    fn an_abort_that_left_something_does_not_verify() {
        let surviving = json!({"authorizes_reclaim": true, "count": 3});
        let blockers = claim_verdict(true, true, None, Some(&surviving));
        assert_eq!(blockers.len(), 3, "{blockers:?}");
        assert!(blockers.iter().any(|b| b.contains("still unresolved")));
        assert!(blockers.iter().any(|b| b.contains("still present")));
        assert!(blockers.iter().any(|b| b.contains("3 process(es)")));
    }
    #[test]
    fn an_unreadable_post_abort_observation_is_a_blocker() {
        let blockers = claim_verdict(false, false, Some("Podman inspect failed"), None);
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("absence is unknown"), "{blockers:?}");
        // An unknown process reading is unknown, not zero.
        let unknown = json!({"known": false, "reason": "this claim recorded no container"});
        let blockers = claim_verdict(false, false, None, Some(&unknown));
        assert_eq!(blockers.len(), 1);
        assert!(blockers[0].contains("could not be established"), "{blockers:?}");
        // The command-line fallback authorizes nothing and is never counted as a survivor.
        let fallback = json!({"authorizes_reclaim": false, "count": 2, "source": "cmdline_fallback"});
        assert!(claim_verdict(false, false, None, Some(&fallback)).is_empty());
    }
}
