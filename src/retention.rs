//! Retention, holds and retained manifests: what class 5 of the collector contract needs declared.
//!
//! Age never justifies collection; the contract's class 5 lets an artifact go only after its
//! operation is terminal, nothing references it, a DECLARED retention has elapsed, no hold applies,
//! and a small retained manifest has been written first. This module holds the declarations --
//! how many recovery points to keep and how old one must be before it is a candidate, and the
//! holds that stop collection -- and the retained manifests that survive a collection.
//!
//! HOLDS, AND WHAT AN ABSENT ONE MEANS. The contract (amended 2026-09-13) names two scopes: an
//! `evidence_hold` blocks artifact deletion and runtime reclaim, and an `investigation_hold`
//! blocks every collector effect. A hold that cannot be read is a hold. The absence of a row is
//! read as "no hold declared" only because the table itself was read successfully; if it cannot
//! be, the collector treats the state as unknown and blocks. Nothing here expires a hold on its
//! own: a hold ends when someone releases it under their own operation, and the release is kept.
use crate::lifecycle as lc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

type Error = Box<dyn std::error::Error>;

pub const SCOPE_EVIDENCE: &str = "evidence_hold";
pub const SCOPE_INVESTIGATION: &str = "investigation_hold";
/// At least one point is always kept: a retention that keeps nothing would let the last point
/// go and leave the universe with no recovery at all.
const MIN_KEEP_LATEST: u64 = 1;
const MAX_KEEP_LATEST: u64 = 1000;
/// Ten years, in seconds: a bound so that a typo cannot declare a retention nobody will outlive.
const MAX_MINIMUM_AGE_SECONDS: u64 = 315_360_000;

pub fn ensure_schema(db: &Connection) -> Result<(), Error> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS recovery_point_retention(
            universe_uuid TEXT PRIMARY KEY,
            keep_latest INTEGER NOT NULL,
            minimum_age_seconds INTEGER NOT NULL,
            declared_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL,
            authorization_ref TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS collection_holds(
            hold_id TEXT PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            scope TEXT NOT NULL,
            reason TEXT NOT NULL,
            declared_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL,
            authorization_ref TEXT NOT NULL,
            released_at INTEGER,
            released_by_operation TEXT,
            release_authorization_ref TEXT);
         CREATE TABLE IF NOT EXISTS recovery_point_retained(
            recovery_point_uuid TEXT PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            generation INTEGER NOT NULL,
            path_class TEXT NOT NULL,
            manifest TEXT NOT NULL,
            manifest_sha256 TEXT NOT NULL,
            rootfs_sha256 TEXT NOT NULL,
            rootfs_bytes INTEGER NOT NULL,
            prepare_operation_id TEXT NOT NULL,
            terminal_state TEXT NOT NULL,
            collected_at INTEGER NOT NULL,
            collecting_operation_id TEXT NOT NULL,
            retention TEXT NOT NULL);",
    )?;
    Ok(())
}

pub struct Retention {
    pub keep_latest: u64,
    pub minimum_age_seconds: u64,
    pub declared_at: i64,
    pub authorization_ref: String,
}

impl Retention {
    pub fn view(&self) -> Value {
        json!({"keep_latest": self.keep_latest, "minimum_age_seconds": self.minimum_age_seconds,
               "declared_at": self.declared_at, "declared_by": self.authorization_ref})
    }
}

pub fn retention(db: &Connection, uuid: &str) -> Result<Option<Retention>, Error> {
    Ok(db
        .query_row(
            "SELECT keep_latest,minimum_age_seconds,declared_at,authorization_ref FROM recovery_point_retention WHERE universe_uuid=?1",
            [uuid],
            |r| Ok(Retention {
                keep_latest: r.get::<_, i64>(0)? as u64,
                minimum_age_seconds: r.get::<_, i64>(1)? as u64,
                declared_at: r.get(2)?,
                authorization_ref: r.get(3)?,
            }),
        )
        .optional()?)
}

pub struct Hold {
    pub hold_id: String,
    pub scope: String,
    pub reason: String,
    pub declared_at: i64,
}

impl Hold {
    pub fn view(&self) -> Value {
        json!({"hold_id": self.hold_id, "scope": self.scope, "reason": self.reason, "declared_at": self.declared_at})
    }
}

/// The holds in force for a universe. An error here is an UNKNOWN hold state, which the caller
/// must treat as a hold; it is never turned into an empty list.
pub fn holds(db: &Connection, uuid: &str) -> Result<Vec<Hold>, Error> {
    let mut s = db.prepare(
        "SELECT hold_id,scope,reason,declared_at FROM collection_holds WHERE universe_uuid=?1 AND released_at IS NULL ORDER BY declared_at,hold_id",
    )?;
    let rows = s
        .query_map([uuid], |r| Ok(Hold { hold_id: r.get(0)?, scope: r.get(1)?, reason: r.get(2)?, declared_at: r.get(3)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    for h in &rows {
        if h.scope != SCOPE_EVIDENCE && h.scope != SCOPE_INVESTIGATION {
            return Err(format!("Hold {} has an unknown scope {:?}; an unknown hold is a hold", h.hold_id, h.scope).into());
        }
    }
    Ok(rows)
}

/// What a set of holds forbids for one effect: `deletes_artifact` and `reclaims_runtime` describe
/// the effect, and the answer is the blocker's own sentence or nothing. This is the contract's
/// scope table made executable, and it is the single place the two scopes are interpreted.
pub fn hold_blocks(held: &[Hold], deletes_artifact: bool, reclaims_runtime: bool) -> Option<String> {
    for h in held {
        if h.scope == SCOPE_INVESTIGATION {
            return Some(format!("investigation hold {} blocks every collector effect: {}", h.hold_id, h.reason));
        }
    }
    for h in held {
        if h.scope == SCOPE_EVIDENCE && (deletes_artifact || reclaims_runtime) {
            return Some(format!(
                "evidence hold {} blocks {}: {}",
                h.hold_id,
                if deletes_artifact { "artifact deletion" } else { "runtime reclaim" },
                h.reason
            ));
        }
    }
    None
}

fn view(db: &Connection, uuid: &str) -> Result<Value, Error> {
    let retention = retention(db, uuid)?;
    let held = holds(db, uuid)?;
    let released: Vec<Value> = {
        let mut s = db.prepare(
            "SELECT hold_id,scope,reason,declared_at,released_at,released_by_operation FROM collection_holds WHERE universe_uuid=?1 AND released_at IS NOT NULL ORDER BY released_at",
        )?;
        let rows = s
            .query_map([uuid], |r| Ok(json!({"hold_id": r.get::<_, String>(0)?, "scope": r.get::<_, String>(1)?,
                "reason": r.get::<_, String>(2)?, "declared_at": r.get::<_, i64>(3)?, "released_at": r.get::<_, i64>(4)?,
                "released_by_operation": r.get::<_, String>(5)?})))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let retained: Vec<Value> = {
        let mut s = db.prepare(
            "SELECT recovery_point_uuid,generation,rootfs_sha256,rootfs_bytes,collected_at,collecting_operation_id FROM recovery_point_retained WHERE universe_uuid=?1 ORDER BY generation",
        )?;
        let rows = s
            .query_map([uuid], |r| Ok(json!({"recovery_point_uuid": r.get::<_, String>(0)?, "generation": r.get::<_, i64>(1)?,
                "rootfs_sha256": r.get::<_, String>(2)?, "rootfs_bytes": r.get::<_, i64>(3)?, "collected_at": r.get::<_, i64>(4)?,
                "collecting_operation_id": r.get::<_, String>(5)?})))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    Ok(json!({
        "universe_uuid": uuid,
        "retention": retention.as_ref().map(Retention::view),
        "retention_declared": retention.is_some(),
        "holds": held.iter().map(Hold::view).collect::<Vec<_>>(),
        "released_holds": released,
        "retained_manifests": retained,
        "note": "age never justifies collection: a recovery point is a candidate only after its retention has elapsed AND it is outside the kept generations AND no hold applies; a hold that cannot be read is a hold",
    }))
}

pub fn execute(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    let uuid = lc::text(request, "universe_uuid")?;
    lc::token(uuid)?;
    lc::ensure_schema(db)?;
    ensure_schema(db)?;
    if operation == "collection_status" {
        return view(db, uuid);
    }
    // The journal contract of every other operation: a repeated declaration with the same
    // request is its own history, one with a different request is refused, never overwritten.
    lc::journaled(db, request, |db| perform(db, request))
}

fn perform(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    let uuid = lc::text(request, "universe_uuid")?;
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let reference = lc::text(request, "authorization_ref")?;
    let now = crate::now() as i64;
    match operation {
        "collection_retention_declare" => {
            let keep = request
                .get("keep_latest")
                .and_then(Value::as_u64)
                .filter(|k| (MIN_KEEP_LATEST..=MAX_KEEP_LATEST).contains(k))
                .ok_or(format!("keep_latest must be an integer from {MIN_KEEP_LATEST} to {MAX_KEEP_LATEST}"))?;
            let age = request
                .get("minimum_age_seconds")
                .and_then(Value::as_u64)
                .filter(|a| *a <= MAX_MINIMUM_AGE_SECONDS)
                .ok_or(format!("minimum_age_seconds must be an integer from 0 to {MAX_MINIMUM_AGE_SECONDS}"))?;
            db.execute(
                "INSERT INTO recovery_point_retention VALUES(?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(universe_uuid) DO UPDATE SET keep_latest=excluded.keep_latest,
                   minimum_age_seconds=excluded.minimum_age_seconds, declared_at=excluded.declared_at,
                   operation_id=excluded.operation_id, authorization_ref=excluded.authorization_ref",
                params![uuid, keep as i64, age as i64, now, id, reference],
            )?;
        }
        "collection_hold_declare" => {
            let scope = lc::text(request, "scope")?;
            if scope != SCOPE_EVIDENCE && scope != SCOPE_INVESTIGATION {
                return Err(format!("scope must be {SCOPE_EVIDENCE} or {SCOPE_INVESTIGATION}").into());
            }
            let reason = lc::text(request, "reason")?;
            if reason.is_empty() || reason.len() > 1000 {
                return Err("reason must be 1-1000 characters".into());
            }
            // The hold's identity is the operation that declared it, so a replayed declaration is
            // the same hold and never a second one.
            db.execute(
                "INSERT OR IGNORE INTO collection_holds(hold_id,universe_uuid,scope,reason,declared_at,operation_id,authorization_ref)
                 VALUES(?1,?2,?3,?4,?5,?1,?6)",
                params![id, uuid, scope, reason, now, reference],
            )?;
        }
        "collection_hold_release" => {
            let hold = lc::text(request, "hold_id")?;
            lc::token(hold)?;
            let changed = db.execute(
                "UPDATE collection_holds SET released_at=?3, released_by_operation=?4, release_authorization_ref=?5
                 WHERE hold_id=?1 AND universe_uuid=?2 AND released_at IS NULL",
                params![hold, uuid, now, id, reference],
            )?;
            if changed == 0 {
                let released_by: Option<String> = db
                    .query_row("SELECT released_by_operation FROM collection_holds WHERE hold_id=?1 AND universe_uuid=?2", params![hold, uuid], |r| r.get(0))
                    .optional()?
                    .flatten();
                match released_by {
                    Some(by) if by == id => {}
                    Some(by) => return Err(format!("Hold {hold} was already released by operation {by}").into()),
                    None => return Err(format!("No hold {hold} is in force for this universe").into()),
                }
            }
        }
        _ => return Err("Unsupported collection declaration".into()),
    }
    view(db, uuid)
}

#[cfg(test)]
mod tests {
    use super::{hold_blocks, Hold, SCOPE_EVIDENCE, SCOPE_INVESTIGATION};

    fn hold(scope: &str) -> Hold {
        Hold { hold_id: "h".into(), scope: scope.into(), reason: "r".into(), declared_at: 0 }
    }

    #[test]
    fn an_investigation_hold_blocks_every_effect() {
        let held = [hold(SCOPE_INVESTIGATION)];
        assert!(hold_blocks(&held, false, false).is_some());
        assert!(hold_blocks(&held, true, false).is_some());
        assert!(hold_blocks(&held, false, true).is_some());
    }
    #[test]
    fn an_evidence_hold_blocks_deletion_and_reclaim_but_not_a_terminal_transition() {
        let held = [hold(SCOPE_EVIDENCE)];
        assert!(hold_blocks(&held, false, false).is_none());
        assert!(hold_blocks(&held, true, false).unwrap().contains("artifact deletion"));
        assert!(hold_blocks(&held, false, true).unwrap().contains("runtime reclaim"));
    }
    #[test]
    fn no_hold_blocks_nothing() {
        assert!(hold_blocks(&[], true, true).is_none());
    }
}
