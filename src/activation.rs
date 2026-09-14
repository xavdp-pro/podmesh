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
//! THE EPOCH HALF (lot H8). `experiments/manager-fencing` in the web tree models exclusion
//! the other way round: not a lease that expires, but an EPOCH issued by one external gate,
//! rotated only by an explicit trusted action, with each maker keeping a durable screen that
//! refuses any epoch it has already seen superseded. A policy may name that gate's
//! `authority_id`; acquisition then requires a permit in the laboratory's exact form, bound to
//! this host and to this boot, and the screen below refuses stale ones. The agent that names
//! the host is the laboratory's rotation controller; PodMesh is its maker. What PodMesh cannot
//! do is verify a permit's origin -- there is no signature and it never contacts the gate --
//! so a permit is provenance from a root-only channel, and the asymmetry is stated: a forged
//! HIGHER epoch can stop a universe here (availability), never start a second one (safety).
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
/// A standby per remaining node and no more; beyond that the number describes nothing.
const MAX_STANDBYS: u64 = 16;
/// The fencing laboratory's permit bounds, taken as they are: an identifier is
/// `[A-Za-z0-9][A-Za-z0-9_.:-]{0,95}`, an epoch is 1 to 2^31-1, a permit is at most 4096 bytes.
const MAX_IDENTIFIER: usize = 96;
const MAX_EPOCH: i64 = i32::MAX as i64;
const MAX_PERMIT_BYTES: usize = 4096;
const PERMIT_FIELDS: [&str; 6] = ["authority_id", "resource", "epoch", "replica_id", "instance_id", "grant_id"];

/// An identifier as the fencing laboratory defines one.
fn identifier(value: &str, field: &str) -> Result<(), Error> {
    let mut bytes = value.bytes();
    let head = bytes.next().ok_or_else(|| format!("{field} must not be empty"))?;
    if !head.is_ascii_alphanumeric() || value.len() > MAX_IDENTIFIER
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b':' || b == b'-')
    {
        return Err(format!("{field} must be 1-{MAX_IDENTIFIER} ASCII characters from [A-Za-z0-9_.:-], starting alphanumeric").into());
    }
    Ok(())
}

/// This boot's identity. A permit is bound to the replica's current incarnation, and a host
/// that has rebooted must be authorised again rather than resume under a permit it held
/// before -- whatever it was doing then, nobody has re-decided it since.
fn boot_id() -> Result<String, Error> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim().to_string())
}

/// The laboratory's permit, exactly six fields, no more and no less.
pub struct Permit {
    pub authority_id: String,
    pub resource: String,
    pub epoch: i64,
    pub replica_id: String,
    pub instance_id: String,
    pub grant_id: String,
}

fn permit(request: &serde_json::Value) -> Result<Permit, Error> {
    let value = request.get("permit").ok_or("This universe is gated by an authority: activation requires a permit")?;
    let object = value.as_object().ok_or("permit must be an object")?;
    // Defence in depth, unreachable through the API: a whole request is refused at the same
    // size before this runs. Kept because the bound is part of the laboratory's definition of
    // a permit, and stated so it is not mistaken for a tested rule.
    if serde_json::to_string(value)?.len() > MAX_PERMIT_BYTES {
        return Err(format!("permit exceeds {MAX_PERMIT_BYTES} bytes").into());
    }
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected = PERMIT_FIELDS.to_vec();
    expected.sort_unstable();
    if keys != expected {
        return Err(format!("permit must carry exactly the fields {}", PERMIT_FIELDS.join(", ")).into());
    }
    let text = |field: &str| -> Result<String, Error> {
        let v = object[field].as_str().ok_or_else(|| format!("permit.{field} must be a string"))?;
        identifier(v, &format!("permit.{field}"))?;
        Ok(v.to_string())
    };
    let epoch = object["epoch"].as_i64().ok_or("permit.epoch must be an integer")?;
    if epoch < 1 || epoch > MAX_EPOCH {
        return Err(format!("permit.epoch must be from 1 to {MAX_EPOCH}").into());
    }
    Ok(Permit {
        authority_id: text("authority_id")?,
        resource: text("resource")?,
        epoch,
        replica_id: text("replica_id")?,
        instance_id: text("instance_id")?,
        grant_id: text("grant_id")?,
    })
}

/// The highest epoch this host has seen for a universe: the maker's durable screen.
fn highest_epoch_seen(db: &Connection, uuid: &str) -> Result<Option<i64>, Error> {
    Ok(db
        .query_row("SELECT epoch FROM activation_epochs WHERE universe_uuid=?1", [uuid], |r| r.get(0))
        .optional()?)
}

fn screen(db: &Connection, uuid: &str, p: &Permit, id: &str) -> Result<(), Error> {
    db.execute(
        "INSERT INTO activation_epochs VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(universe_uuid) DO UPDATE SET epoch=MAX(epoch,excluded.epoch),
           authority_id=excluded.authority_id, grant_id=excluded.grant_id, replica_id=excluded.replica_id,
           seen_at=excluded.seen_at, operation_id=excluded.operation_id
         WHERE excluded.epoch>=epoch",
        params![uuid, p.authority_id, p.epoch, p.grant_id, p.replica_id, crate::now() as i64, id],
    )?;
    Ok(())
}

/// What this host has, for a caller deciding where a standby can go.
///
/// PodMesh measured disk and nothing else, so "does this host have room for a standby" could
/// not be answered honestly at all. Memory and CPU are read from the kernel rather than
/// inferred: `MemAvailable` is the kernel's own estimate of what a new workload can claim
/// without swapping, which is the question, and it is not `MemFree` -- free memory on a busy
/// host is small and says nothing, because page cache is reclaimable.
///
/// These are FACTS AND NOT A DECISION. Nothing here decides whether a standby fits: that
/// depends on what the universe needs and on whatever allowance the operator has granted it,
/// neither of which PodMesh knows. An admission rule that guessed would be worse than none.
fn host_resources() -> serde_json::Value {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb = |key: &str| -> Option<u64> {
        meminfo.lines().find(|l| l.starts_with(key)).and_then(|l| {
            l.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok())
        })
    };
    serde_json::json!({
        "memory_total_bytes": kb("MemTotal:").map(|v| v * 1024),
        "memory_available_bytes": kb("MemAvailable:").map(|v| v * 1024),
        "cpu_count": std::thread::available_parallelism().map(std::num::NonZeroUsize::get).ok(),
        "load_average_1m": std::fs::read_to_string("/proc/loadavg").ok()
            .and_then(|l| l.split_whitespace().next().and_then(|v| v.parse::<f64>().ok())),
        "state_directory_available_bytes": crate::migration::base().ok().map(|b| crate::migration::available_bytes(&b)),
        "note": "facts only; whether a standby fits depends on what the universe needs and on its allowance, neither of which PodMesh knows",
    })
}

pub fn ensure_schema(db: &Connection) -> Result<(), Error> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS activation_policy(
            universe_uuid TEXT PRIMARY KEY,
            lease_seconds INTEGER NOT NULL,
            takeover_margin_seconds INTEGER NOT NULL,
            declared_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL,
            desired_standbys INTEGER NOT NULL DEFAULT 0,
            eligible_hosts TEXT NOT NULL DEFAULT '[]',
            authorization_ref TEXT NOT NULL DEFAULT '');
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
            operation_id TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS activation_epochs(
            universe_uuid TEXT PRIMARY KEY,
            authority_id TEXT NOT NULL,
            epoch INTEGER NOT NULL,
            grant_id TEXT NOT NULL,
            replica_id TEXT NOT NULL,
            seen_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL);",
    )?;
    // A table created by an earlier version lacks the later columns, and CREATE TABLE IF NOT
    // EXISTS does not add them. Each carries a default that is the truthful reading of a row
    // written before the field existed: no standby, no authority, epoch zero.
    for (table, column) in [
        ("activation_policy", "desired_standbys INTEGER NOT NULL DEFAULT 0"),
        ("activation_policy", "eligible_hosts TEXT NOT NULL DEFAULT '[]'"),
        ("activation_policy", "authorization_ref TEXT NOT NULL DEFAULT ''"),
        ("activation_policy", "authority_id TEXT NOT NULL DEFAULT ''"),
        ("activation_leases", "epoch INTEGER NOT NULL DEFAULT 0"),
        ("activation_leases", "grant_id TEXT NOT NULL DEFAULT ''"),
    ] {
        let name = column.split(' ').next().unwrap_or_default();
        let present: bool = db.query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name=?1"),
            [name],
            |r| Ok(r.get::<_, i64>(0)? > 0),
        )?;
        if !present {
            db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column};"))?;
        }
    }
    Ok(())
}

pub struct Policy {
    pub lease_seconds: u64,
    pub takeover_margin_seconds: u64,
    /// How many standbys the operator wants for this universe, beyond the one that runs.
    /// With three nodes that is one or two, and it is a per-universe choice rather than a
    /// cluster-wide setting: a standby costs storage and reserved headroom, so a universe
    /// cheap to rebuild does not want one and a universe that must not stop wants two.
    pub desired_standbys: u64,
    /// The hosts the operator will accept as a placement. Empty means none has been named,
    /// which is recorded as an absence rather than read as "anywhere".
    pub eligible_hosts: Vec<String>,
    /// Who decided this allocation, recorded verbatim. Provenance, never a credential.
    pub authorization_ref: String,
    /// The external gate whose epochs bind activation here; empty means the universe is
    /// under leases alone, with no epoch screen.
    pub authority_id: String,
}

impl Policy {
    pub fn gated(&self) -> bool {
        !self.authority_id.is_empty()
    }
}

pub fn policy(db: &Connection, uuid: &str) -> Result<Option<Policy>, Error> {
    Ok(db
        .query_row(
            "SELECT lease_seconds,takeover_margin_seconds,desired_standbys,eligible_hosts,authorization_ref,authority_id FROM activation_policy WHERE universe_uuid=?1",
            [uuid],
            |r| {
                let hosts: String = r.get(3)?;
                Ok(Policy {
                    lease_seconds: r.get::<_, i64>(0)? as u64,
                    takeover_margin_seconds: r.get::<_, i64>(1)? as u64,
                    desired_standbys: r.get::<_, i64>(2)? as u64,
                    eligible_hosts: serde_json::from_str(&hosts).unwrap_or_default(),
                    authorization_ref: r.get(4)?,
                    authority_id: r.get(5)?,
                })
            },
        )
        .optional()?)
}

pub struct Lease {
    pub holder_host_uuid: String,
    pub generation: i64,
    pub expires_at: i64,
    /// The epoch this lease was acquired under; zero for a lease under no authority.
    pub epoch: i64,
    pub grant_id: String,
}

pub fn lease(db: &Connection, uuid: &str) -> Result<Option<Lease>, Error> {
    Ok(db
        .query_row(
            "SELECT holder_host_uuid,generation,expires_at,epoch,grant_id FROM activation_leases WHERE universe_uuid=?1",
            [uuid],
            |r| Ok(Lease { holder_host_uuid: r.get(0)?, generation: r.get(1)?, expires_at: r.get(2)?, epoch: r.get(3)?, grant_id: r.get(4)? }),
        )
        .optional()?)
}

/// Whether this host's lease has been overtaken by an epoch it has seen: the maker's screen
/// says a newer grant exists, so whatever this lease says, this host is no longer entitled.
fn superseded(db: &Connection, uuid: &str, l: &Lease) -> Result<Option<i64>, Error> {
    Ok(highest_epoch_seen(db, uuid)?.filter(|&seen| seen > l.epoch))
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
        // The fourth reason, from the epoch screen: a newer grant has been seen here, so this
        // host's lease, live or not, no longer entitles it. This is the maker's refusal.
        Some(ref l) if superseded(db, uuid, l)?.is_some() => Err(format!(
            "{operation} refused: this host's activation was superseded by epoch {}",
            superseded(db, uuid, l)?.unwrap_or_default()
        )
        .into()),
        Some(_) => Ok(()),
    }
}

/// Surrender this host's lease because the universe was handed off and now runs elsewhere.
///
/// NOT REACHED BY THE CHECKS BESIDE THIS FILE, and said so rather than left to look tested:
/// it runs only after `migration_complete_transfer` succeeds, which needs a real checkpoint,
/// an authorized transfer, a destination that restored, and an outcome document back in the
/// inbox -- a completed two-host handoff. The single-host check cannot produce one. The
/// safety of a completed handoff does not rest on this line: the reservation already refuses
/// `start` here in every state but released or collected. What this line keeps true is the
/// journal, so a lease never outlives the universe it was for.
///
/// Idempotent and quiet when there is nothing to surrender: a universe under no policy, or one
/// whose lease this host does not hold, is left exactly as it was. The history records the
/// release under its own event so a reader can tell a handoff from an operator's release.
pub fn release_by_handoff(db: &Connection, uuid: &str, id: &str) -> Result<(), Error> {
    ensure_schema(db)?;
    let Some(l) = lease(db, uuid)? else { return Ok(()) };
    if l.holder_host_uuid != host_uuid(db)? {
        return Ok(());
    }
    db.execute("DELETE FROM activation_leases WHERE universe_uuid=?1", [uuid])?;
    record(db, uuid, &l.holder_host_uuid, l.generation, "released_by_handoff", id)?;
    Ok(())
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
        // The epoch half. A lease under an authority carries the epoch and grant it was taken
        // under; the screen is the highest epoch this host has seen, whoever it was granted to.
        "authority_id": policy.as_ref().map(|p| p.authority_id.clone()).filter(|a| !a.is_empty()),
        "epoch": held.as_ref().map(|l| l.epoch).filter(|&e| e > 0),
        "grant_id": held.as_ref().map(|l| l.grant_id.clone()).filter(|g| !g.is_empty()),
        "highest_epoch_seen": highest_epoch_seen(db, uuid)?,
        "superseded": match held.as_ref() { Some(l) => superseded(db, uuid, l)?.is_some(), None => false },
        "permit_verification": "a permit is provenance from a root-only channel and is not verified: PodMesh never contacts the gate and holds no key; a forged higher epoch can stop a universe here, never start a second one",
        "desired_standbys": policy.as_ref().map(|p| p.desired_standbys),
        "eligible_hosts": policy.as_ref().map(|p| p.eligible_hosts.clone()),
        // The allocation is a judgement made against criteria PodMesh cannot see. It records
        // who made it and reports it back, and it models nothing about how they decided.
        "allocation_decided_by": policy.as_ref().map(|p| p.authorization_ref.clone()),
        "allocation_is_a_judgement": true,
        // Declared, never observed. PodMesh sees one host -- this one -- so it cannot say how
        // many standbys exist, and a number here would be a claim about hosts it has never
        // contacted. The count is what the operator asked for, and the placement is unverified
        // until something that can see the other hosts verifies it.
        "standbys_placed": serde_json::Value::Null,
        "placement_verified": false,
        "host_resources": host_resources(),
        // Said in every answer, because a caller reading only this object must not mistake a
        // local self-restraint for cross-host exclusion.
        "scope": "this host's journal only; not mutual exclusion across hosts. Under an authority, an epoch screen refuses grants this host has seen superseded; the screen is fed by documents whose origin PodMesh cannot verify",
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
    // How much a universe is allowed is a judgement -- the administrator weighs criteria
    // PodMesh cannot see and would be wrong to model. What PodMesh owes that judgement is a
    // record of WHO made it, kept verbatim beside the policy it produced. It is provenance
    // and never a checked credential, exactly as PREPARE-A-HOST.md says of every
    // authorization_ref: validating it and then discarding it, which is what this did until
    // now, keeps the obligation and loses the only part worth keeping.
    let reference = lc::text(request, "authorization_ref")?;
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
            // Optional, and absent means zero rather than unlimited: a policy that says
            // nothing about standbys is declaring none, not declaring "as many as possible".
            let standbys = match request.get("desired_standbys") {
                None => 0,
                Some(_) => bounded(request, "desired_standbys", 0, MAX_STANDBYS)?,
            };
            let hosts: Vec<String> = match request.get("eligible_hosts") {
                None => Vec::new(),
                Some(v) => serde_json::from_value(v.clone())
                    .map_err(|_| "eligible_hosts must be a list of host UUIDs")?,
            };
            for host in &hosts {
                lc::token(host)?;
            }
            // A target no placement can satisfy is refused at declaration rather than
            // discovered later: naming two standbys among two eligible hosts, one of which
            // runs it, cannot be honoured and saying so now is cheaper than a surprise.
            if standbys > 0 && !hosts.is_empty() && (standbys as usize) + 1 > hosts.len() {
                return Err(format!(
                    "desired_standbys {standbys} plus the running host exceeds the {} eligible hosts named",
                    hosts.len()
                )
                .into());
            }
            // Optional: the external gate whose epochs bind activation here. Absent means
            // leases alone, which is what every policy declared before the field existed says.
            let authority = match request.get("authority_id") {
                None => String::new(),
                Some(v) => {
                    let a = v.as_str().ok_or("authority_id must be a string")?;
                    identifier(a, "authority_id")?;
                    a.to_string()
                }
            };
            db.execute(
                "INSERT INTO activation_policy VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(universe_uuid) DO UPDATE SET lease_seconds=excluded.lease_seconds,
                   takeover_margin_seconds=excluded.takeover_margin_seconds,
                   declared_at=excluded.declared_at, operation_id=excluded.operation_id,
                   desired_standbys=excluded.desired_standbys, eligible_hosts=excluded.eligible_hosts,
                   authorization_ref=excluded.authorization_ref, authority_id=excluded.authority_id",
                params![uuid, lease_seconds as i64, margin as i64, now, id, standbys as i64,
                        serde_json::to_string(&hosts)?, reference, authority],
            )?;
            record(db, uuid, &this_host, 0, "policy_declared", id)?;
        }
        "activation_acquire" => {
            let Some(policy) = policy(db, uuid)? else {
                return Err("This universe has no activation policy; declare one first".into());
            };
            let previous = lease(db, uuid)?;
            // The epoch half, when the policy names an authority. The permit is bound to this
            // universe, this host and this boot, and then screened: an epoch below the highest
            // seen here is superseded whatever it says, and a takeover from another holder
            // needs an epoch newer than the one that holder was granted -- the explicit
            // rotation the laboratory requires, never inferred from a lapse alone.
            let granted = if policy.gated() {
                let p = permit(request)?;
                if p.authority_id != policy.authority_id {
                    return Err("The permit names a different authority than this universe's policy".into());
                }
                if p.resource != uuid {
                    return Err("The permit is for a different resource than this universe".into());
                }
                if p.replica_id != this_host {
                    return Err("The permit is bound to another replica, not this host".into());
                }
                if p.instance_id != boot_id()? {
                    return Err("The permit is bound to another incarnation of this host; a rebooted host must be authorised again".into());
                }
                if let Some(seen) = highest_epoch_seen(db, uuid)?.filter(|&seen| seen > p.epoch) {
                    return Err(format!("The permit's epoch {} is superseded: this host has already seen epoch {seen}", p.epoch).into());
                }
                // One grant per epoch is the gate's rule. A permit at the epoch this host has
                // already recorded must be THAT grant, to the same replica; a second grant at
                // one epoch is something the gate never issues, so it can only be a forgery or
                // a copy, and is refused whatever it claims.
                let recorded: Option<(String, String)> = db
                    .query_row("SELECT grant_id,replica_id FROM activation_epochs WHERE universe_uuid=?1 AND epoch=?2",
                               params![uuid, p.epoch], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                if let Some((grant, replica)) = recorded {
                    if grant != p.grant_id || replica != p.replica_id {
                        return Err(format!("Epoch {} was already granted here under another grant; a second grant at one epoch is not something the gate issues", p.epoch).into());
                    }
                }
                if let Some(ref l) = previous {
                    if l.holder_host_uuid != this_host && l.epoch >= p.epoch {
                        return Err(format!(
                            "A takeover requires a newer epoch than the previous holder's {}; the permit carries {}",
                            l.epoch, p.epoch
                        )
                        .into());
                    }
                }
                Some(p)
            } else {
                if request.get("permit").is_some() {
                    return Err("This universe's policy names no authority; a permit here would be checked against nothing".into());
                }
                None
            };
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
            let (epoch, grant) = granted.as_ref().map_or((0, String::new()), |p| (p.epoch, p.grant_id.clone()));
            db.execute(
                "INSERT INTO activation_leases VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(universe_uuid) DO UPDATE SET holder_host_uuid=excluded.holder_host_uuid,
                   generation=excluded.generation, acquired_at=excluded.acquired_at,
                   expires_at=excluded.expires_at, operation_id=excluded.operation_id,
                   epoch=excluded.epoch, grant_id=excluded.grant_id",
                params![uuid, this_host, generation, now, now + policy.lease_seconds as i64, id, epoch, grant],
            )?;
            if let Some(p) = granted.as_ref() {
                screen(db, uuid, p, id)?;
            }
            record(db, uuid, &this_host, generation, "acquired", id)?;
        }
        "activation_supersede" => {
            // A newer grant, bound to whoever it was bound to, delivered here so that this host
            // learns it has been overtaken. The permit is not this host's to use and is not
            // used: it advances the screen, and this host's lease, if any, is left in place and
            // marked overtaken by the screen -- the gate, the renewal and the fence all read
            // it. Delivering a stale one is refused, so the screen only ever moves forward.
            let Some(policy) = policy(db, uuid)? else {
                return Err("This universe has no activation policy".into());
            };
            if !policy.gated() {
                return Err("This universe's policy names no authority; there is no epoch to supersede".into());
            }
            let p = permit(request)?;
            if p.authority_id != policy.authority_id {
                return Err("The permit names a different authority than this universe's policy".into());
            }
            if p.resource != uuid {
                return Err("The permit is for a different resource than this universe".into());
            }
            if let Some(seen) = highest_epoch_seen(db, uuid)?.filter(|&seen| seen >= p.epoch) {
                return Err(format!("Epoch {} does not supersede epoch {seen}, which this host has already seen", p.epoch).into());
            }
            screen(db, uuid, &p, id)?;
            if let Some(l) = lease(db, uuid)? {
                if l.holder_host_uuid == this_host {
                    record(db, uuid, &this_host, l.generation, "superseded", id)?;
                }
            }
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
            if let Some(seen) = superseded(db, uuid, &l)? {
                return Err(format!("This host's activation was superseded by epoch {seen}; it cannot be renewed").into());
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
        let overtaken = match held.as_ref() { Some(l) => superseded(db, &uuid, l)?, None => None };
        let entitled = held
            .as_ref()
            .is_some_and(|l| l.holder_host_uuid == this_host && l.expires_at > now && overtaken.is_none());
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
            "superseded_by_epoch": overtaken,
        }));
    }
    Ok(serde_json::json!({
        "this_host_uuid": this_host,
        "fenced": fenced,
        "left_running_or_absent": left,
        "scope": "this host only; a host that never runs this operation is not fenced by it",
    }))
}
