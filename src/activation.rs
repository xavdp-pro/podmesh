//! Exclusive activation leases: the safety half of high availability for a chosen universe.
//!
//! A universe may be marked as requiring a live activation lease. This host then refuses to
//! `start` it unless this host holds one that has not expired. A lease is taken for a bounded
//! period and must be renewed; when it lapses, the universe becomes startable by another
//! holder after a stated margin.
//!
//! WHAT THIS PROVES, AND WHAT IT DOES NOT. The lease lives in this host's own journal, so it
//! is a **self-restraint**: this host will not start a universe it has no lease for. That is
//! soundly implementable locally and it is worth having on its own -- an agent that names the
//! wrong host gets a refusal rather than a second writer. It is **not** mutual exclusion
//! across hosts, and nothing here should be read as if it were: a host that never asks, or
//! whose journal says something else, is not restrained by this table. Mutual exclusion needs
//! the lease replicated as a fact and a permit issued against a reconciled history, which is
//! the manager's job and a later lot. Until then a partitioned host is restrained only by its
//! own copy of this record.
//!
//! The takeover margin is what keeps two honest hosts apart. A different holder may acquire
//! only after the previous lease's expiry PLUS the margin, so the window in which the previous
//! holder still believes it is entitled and the window in which the new one starts cannot
//! overlap, even with clocks that disagree by less than the margin. Expiry is judged on wall
//! clock, because two hosts have no shared monotonic clock; the margin is therefore also the
//! clock-skew budget, and that is stated rather than assumed.
use crate::lifecycle as lc;
use rusqlite::{params, Connection, OptionalExtension};

type Error = Box<dyn std::error::Error>;

/// Smallest lease a caller may take. A lease shorter than this cannot be renewed reliably.
const MIN_LEASE_SECONDS: u64 = 5;
/// Largest lease a caller may take. A long lease is a long outage after a failure.
const MAX_LEASE_SECONDS: u64 = 3600;
/// Smallest takeover margin. It is the clock-skew budget between two hosts as well as the
/// guard between the old holder's belief and the new holder's start.
const MIN_TAKEOVER_MARGIN_SECONDS: u64 = 5;

pub fn ensure_schema(db: &Connection) -> Result<(), Error> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS activation_policy(
            universe_uuid TEXT PRIMARY KEY,
            lease_seconds INTEGER NOT NULL,
            takeover_margin_seconds INTEGER NOT NULL,
            declared_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS activation_leases(
            universe_uuid TEXT PRIMARY KEY,
            holder_host_uuid TEXT NOT NULL,
            generation INTEGER NOT NULL,
            acquired_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS activation_lease_history(
            id INTEGER PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            holder_host_uuid TEXT NOT NULL,
            generation INTEGER NOT NULL,
            event TEXT NOT NULL,
            at INTEGER NOT NULL,
            operation_id TEXT NOT NULL);",
    )?;
    Ok(())
}

pub struct Policy {
    pub lease_seconds: u64,
    pub takeover_margin_seconds: u64,
}

pub fn policy(db: &Connection, uuid: &str) -> Result<Option<Policy>, Error> {
    Ok(db
        .query_row(
            "SELECT lease_seconds,takeover_margin_seconds FROM activation_policy WHERE universe_uuid=?1",
            [uuid],
            |r| Ok(Policy { lease_seconds: r.get::<_, i64>(0)? as u64, takeover_margin_seconds: r.get::<_, i64>(1)? as u64 }),
        )
        .optional()?)
}

pub struct Lease {
    pub holder_host_uuid: String,
    pub generation: i64,
    pub expires_at: i64,
}

pub fn lease(db: &Connection, uuid: &str) -> Result<Option<Lease>, Error> {
    Ok(db
        .query_row(
            "SELECT holder_host_uuid,generation,expires_at FROM activation_leases WHERE universe_uuid=?1",
            [uuid],
            |r| Ok(Lease { holder_host_uuid: r.get(0)?, generation: r.get(1)?, expires_at: r.get(2)? }),
        )
        .optional()?)
}

fn host_uuid(db: &Connection) -> Result<String, Error> {
    Ok(db.query_row("SELECT value FROM metadata WHERE key='host_uuid'", [], |r| r.get(0))?)
}

/// The gate. A universe under an activation policy may only be started by the host holding a
/// live lease, and the refusal says which of the three reasons applies rather than a single
/// opaque no -- an operator reading it has to be able to act on it.
pub fn refuse_if_not_activated(db: &Connection, uuid: &str, operation: &str) -> Result<(), Error> {
    ensure_schema(db)?;
    if policy(db, uuid)?.is_none() {
        return Ok(());
    }
    let now = crate::now() as i64;
    let held = lease(db, uuid)?;
    let this_host = host_uuid(db)?;
    match held {
        None => Err(format!(
            "{operation} refused: this universe requires an activation lease and none is held"
        )
        .into()),
        Some(l) if l.holder_host_uuid != this_host => Err(format!(
            "{operation} refused: the activation lease is held by another host"
        )
        .into()),
        Some(l) if l.expires_at <= now => Err(format!(
            "{operation} refused: this host's activation lease expired {} seconds ago",
            now - l.expires_at
        )
        .into()),
        Some(_) => Ok(()),
    }
}

fn record(db: &Connection, uuid: &str, holder: &str, generation: i64, event: &str, id: &str) -> Result<(), Error> {
    db.execute(
        "INSERT INTO activation_lease_history(universe_uuid,holder_host_uuid,generation,event,at,operation_id)
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![uuid, holder, generation, event, crate::now() as i64, id],
    )?;
    Ok(())
}

fn bounded(request: &serde_json::Value, field: &str, low: u64, high: u64) -> Result<u64, Error> {
    let value = request
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("{field} must be an integer"))?;
    if value < low || value > high {
        return Err(format!("{field} must be from {low} to {high}").into());
    }
    Ok(value)
}

fn view(db: &Connection, uuid: &str) -> Result<serde_json::Value, Error> {
    let now = crate::now() as i64;
    let policy = policy(db, uuid)?;
    let held = lease(db, uuid)?;
    Ok(serde_json::json!({
        "universe_uuid": uuid,
        "this_host_uuid": host_uuid(db)?,
        "requires_lease": policy.is_some(),
        "lease_seconds": policy.as_ref().map(|p| p.lease_seconds),
        "takeover_margin_seconds": policy.as_ref().map(|p| p.takeover_margin_seconds),
        "holder_host_uuid": held.as_ref().map(|l| l.holder_host_uuid.clone()),
        "generation": held.as_ref().map(|l| l.generation),
        "expires_at": held.as_ref().map(|l| l.expires_at),
        "seconds_remaining": held.as_ref().map(|l| l.expires_at - now),
        "live": held.as_ref().is_some_and(|l| l.expires_at > now),
        // Said in every answer, because a caller reading only this object must not mistake a
        // local self-restraint for cross-host exclusion.
        "scope": "this host's journal only; not mutual exclusion across hosts",
    }))
}

pub fn execute(db: &Connection, request: &serde_json::Value) -> Result<serde_json::Value, Error> {
    let operation = lc::text(request, "operation")?;
    lc::ensure_schema(db)?;
    ensure_schema(db)?;
    // Fencing is the one operation that is not about one universe: it acts on every universe
    // this host is not entitled to run, so it takes no universe and says so by refusing one.
    if operation == "activation_fence" {
        if request.get("universe_uuid").is_some() {
            return Err("activation_fence acts on every universe under a policy and takes no universe_uuid".into());
        }
        let id = lc::text(request, "operation_id")?;
        lc::token(id)?;
        let _ = lc::text(request, "authorization_ref")?;
        let timeout = bounded(request, "timeout_seconds", 0, 300)?;
        return fence(db, id, timeout);
    }
    let uuid = lc::text(request, "universe_uuid")?;
    lc::token(uuid)?;
    if operation == "activation_status" {
        return view(db, uuid);
    }
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let _ = lc::text(request, "authorization_ref")?;
    let now = crate::now() as i64;
    let this_host = host_uuid(db)?;

    match operation {
        "activation_require" => {
            let lease_seconds = bounded(request, "lease_seconds", MIN_LEASE_SECONDS, MAX_LEASE_SECONDS)?;
            let margin = bounded(
                request,
                "takeover_margin_seconds",
                MIN_TAKEOVER_MARGIN_SECONDS,
                MAX_LEASE_SECONDS,
            )?;
            db.execute(
                "INSERT INTO activation_policy VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(universe_uuid) DO UPDATE SET lease_seconds=excluded.lease_seconds,
                   takeover_margin_seconds=excluded.takeover_margin_seconds,
                   declared_at=excluded.declared_at, operation_id=excluded.operation_id",
                params![uuid, lease_seconds as i64, margin as i64, now, id],
            )?;
            record(db, uuid, &this_host, 0, "policy_declared", id)?;
        }
        "activation_acquire" => {
            let Some(policy) = policy(db, uuid)? else {
                return Err("This universe has no activation policy; declare one first".into());
            };
            let previous = lease(db, uuid)?;
            let generation = match previous {
                // Renewing our own live lease is an acquisition of the same generation: it is
                // idempotent by design, so a repeated request is not a takeover.
                Some(ref l) if l.holder_host_uuid == this_host => l.generation,
                // A lapsed lease of ours is ours to retake immediately: no other host can have
                // started in the meantime without this host's journal saying so.
                Some(ref l) if l.expires_at + (policy.takeover_margin_seconds as i64) > now => {
                    return Err(format!(
                        "Another host holds the activation lease; it may be taken over {} seconds from now",
                        l.expires_at + (policy.takeover_margin_seconds as i64) - now
                    )
                    .into());
                }
                Some(ref l) => l.generation + 1,
                None => 1,
            };
            db.execute(
                "INSERT INTO activation_leases VALUES(?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(universe_uuid) DO UPDATE SET holder_host_uuid=excluded.holder_host_uuid,
                   generation=excluded.generation, acquired_at=excluded.acquired_at,
                   expires_at=excluded.expires_at, operation_id=excluded.operation_id",
                params![uuid, this_host, generation, now, now + policy.lease_seconds as i64, id],
            )?;
            record(db, uuid, &this_host, generation, "acquired", id)?;
        }
        "activation_renew" => {
            let Some(policy) = policy(db, uuid)? else {
                return Err("This universe has no activation policy".into());
            };
            let Some(l) = lease(db, uuid)? else {
                return Err("No activation lease to renew".into());
            };
            if l.holder_host_uuid != this_host {
                return Err("The activation lease is held by another host".into());
            }
            // An expired lease is NOT renewed. Renewal would silently extend an entitlement
            // that had already lapsed, and another host may have begun its takeover wait.
            // Retaking it is an acquisition, which is visible as such in the history.
            if l.expires_at <= now {
                return Err("The activation lease has expired; acquire it again rather than renewing".into());
            }
            db.execute(
                "UPDATE activation_leases SET expires_at=?2, operation_id=?3 WHERE universe_uuid=?1",
                params![uuid, now + policy.lease_seconds as i64, id],
            )?;
            record(db, uuid, &this_host, l.generation, "renewed", id)?;
        }
        "activation_release" => {
            let Some(l) = lease(db, uuid)? else {
                return Err("No activation lease to release".into());
            };
            if l.holder_host_uuid != this_host {
                return Err("The activation lease is held by another host".into());
            }
            db.execute("DELETE FROM activation_leases WHERE universe_uuid=?1", [uuid])?;
            record(db, uuid, &this_host, l.generation, "released", id)?;
        }
        _ => return Err("Unsupported activation operation".into()),
    }
    view(db, uuid)
}

/// Stop every universe this host is not entitled to run.
///
/// SELF-FENCING, AND WHY IT IS A TYPED OPERATION RATHER THAN A TIMER. PodMesh does not act on
/// its own -- the garbage collector carries the same constraint, and for the same reason: an
/// autonomous timer takes a decision nobody asked for. So the effect is an operation and its
/// TIMELINESS is the caller's obligation: whoever drives it must call it at least as often as
/// the shortest lease, or a lapsed lease leaves a universe running.
///
/// That dependence is exactly why self-fencing is the weakest of the three fencing
/// mechanisms. A host too sick to renew its lease may be too sick to run the fence, and then
/// nothing here stops it. It is defensible only through the takeover margin: the standby
/// waits strictly longer than the lease plus the margin, so an honest but slow host has
/// already been asked to stop before anyone else may start. A storage lease or out-of-band
/// fencing proves what this only asks for.
///
/// A universe is fenced when it is under a policy and this host has no live lease for it --
/// whether the lease lapsed, was never taken, or belongs to another host. A universe whose
/// lease is live is left alone, and that is a refusal to act rather than an omission.
fn fence(db: &Connection, id: &str, timeout: u64) -> Result<serde_json::Value, Error> {
    let now = crate::now() as i64;
    let this_host = host_uuid(db)?;
    let mut statement = db.prepare("SELECT universe_uuid FROM activation_policy ORDER BY universe_uuid")?;
    let universes: Vec<String> = statement
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut fenced = Vec::new();
    let mut left = Vec::new();
    for uuid in universes {
        let held = lease(db, &uuid)?;
        let entitled = held
            .as_ref()
            .is_some_and(|l| l.holder_host_uuid == this_host && l.expires_at > now);
        let observed = lc::observe(&uuid)?;
        let running = observed["present"] == serde_json::json!(true)
            && observed["running"] == serde_json::json!(true);
        if entitled {
            left.push(serde_json::json!({"universe_uuid": uuid, "reason": "lease is live", "running": running}));
            continue;
        }
        if !running {
            left.push(serde_json::json!({"universe_uuid": uuid, "reason": "not running", "running": false}));
            continue;
        }
        let name = format!("podmesh-{uuid}");
        let out = lc::run_podman(timeout + 5, &["stop", "--time", &timeout.to_string(), &name])?;
        // Podman reports its escalation to SIGKILL only as a warning on stderr, and a fence
        // that had to kill is a different fact from one that asked politely.
        let forced = String::from_utf8_lossy(&out.stderr).contains("resorting to SIGKILL");
        let after = lc::observe(&uuid)?;
        let stopped = after["running"] != serde_json::json!(true);
        // DEFENCE IN DEPTH, and not reachable by the check beside this file: `podman stop`
        // returning success while the container still runs is the case it guards, and the
        // check cannot manufacture it. Removing this line leaves the suite green, which is
        // why it is written down here rather than left to look like a tested safeguard. It
        // stays because a fence that reports a stop it did not perform is the one lie this
        // operation must never tell -- everything downstream reads the report, not the host.
        if !stopped {
            return Err(format!("Fencing {uuid} did not stop it; refusing to report a fence that did not happen").into());
        }
        record(db, &uuid, &this_host, held.as_ref().map_or(0, |l| l.generation), "fenced", id)?;
        fenced.push(serde_json::json!({
            "universe_uuid": uuid,
            "forced": forced,
            "held_by": held.as_ref().map(|l| l.holder_host_uuid.clone()),
            "expired_seconds_ago": held.as_ref().map(|l| now - l.expires_at),
        }));
    }
    Ok(serde_json::json!({
        "this_host_uuid": this_host,
        "fenced": fenced,
        "left_running_or_absent": left,
        "scope": "this host only; a host that never runs this operation is not fenced by it",
    }))
}
