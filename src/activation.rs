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
//! THE EPOCH HALF (lot H8). `experiments/manager-fencing` in the web tree models exclusion the
//! other way round: not a lease that expires, but an EPOCH issued by one external gate, rotated
//! only by an explicit trusted action, with each node (the model's "maker") keeping a durable
//! screen that refuses any epoch it has already seen superseded. A policy may name that gate's
//! `authority_id`; acquisition then requires a permit in the laboratory's exact form, bound to
//! this host and to this boot, and the screen below refuses stale ones. The agent that names
//! the host is the laboratory's rotation controller; PodMesh is its node. What PodMesh cannot
//! do is verify a permit's origin -- there is no signature and it never contacts the gate --
//! so a permit is provenance from a root-only channel, and the asymmetry is stated: a forged
//! HIGHER epoch can stop a universe here (availability), never start a second one (safety).
//!
//! THE QUORUM (V3-2). A policy may instead name its authority as a quorum of replica keys
//! (`authority_quorum`, `signing::Quorum`). Every grant is then a certificate that a strict majority
//! of those keys signed under the policy's digest, verified here with no network: acquisition,
//! supersession and the takeover proof all take one, and no permit is accepted, so the durable
//! screen moves forward on certificates only and never backwards. What the node still cannot see
//! is whether the signers kept their promise of one decision per epoch; that is the manager's.
//! The authority set itself changes only under a certificate of the set in place or the operator's
//! explicit re-declaration naming the digest it replaces (`authority_change`).
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
pub(crate) fn boot_id() -> Result<String, Error> {
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
    if !(1..=MAX_EPOCH).contains(&epoch) {
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

/// The highest epoch this host has seen for a universe: the node's durable epoch screen.
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

/// The binding of a takeover document's method, whatever its form (the gate's document at
/// `publisher_start`, a quorum certificate at acquisition): `first` (no previous epoch or holder),
/// `same_holder` (this host held the previous epoch), `fence_receipt` (the previous holder's fence,
/// its receipt bound to this resource) or `lease_barrier` (an `eligible_after` is named). `name` is
/// what the refusal calls the document.
pub(crate) fn takeover_method(proof: &serde_json::Value, resource: &str, this_host: &str, previous_epoch: i64, name: &str) -> Result<(), Error> {
    let previous_holder = proof["previous_holder"].as_str();
    match proof["method"].as_str().ok_or_else(|| format!("{name} lacks method"))? {
        "first" => {
            if previous_epoch != 0 || previous_holder.is_some() {
                return Err(format!("{name} says first, but a previous epoch or holder exists").into());
            }
        }
        "same_holder" => {
            if previous_holder != Some(this_host) {
                return Err(format!("{name} says same_holder, but the previous holder is not this host").into());
            }
        }
        "fence_receipt" => {
            let receipt = &proof["receipt"];
            let host = receipt["host"].as_str().ok_or("the fence receipt names no host")?;
            if Some(host) != previous_holder {
                return Err("the fence receipt is from a host that is not the previous holder".into());
            }
            if receipt["withdrawn"] != serde_json::json!(true) || receipt["operation_id"].as_str().is_none() {
                return Err("the fence receipt does not record a verified withdrawal".into());
            }
            if receipt["resource"].as_str() != Some(resource) {
                return Err("the fence receipt is for another resource".into());
            }
        }
        "lease_barrier" => {
            // Held by the caller like every method; a lease barrier without one is not a barrier.
            proof["eligible_after"].as_i64().ok_or_else(|| format!("{name} lacks eligible_after"))?;
        }
        other => return Err(format!("{name} method {other} is unknown").into()),
    }
    Ok(())
}

/// What a quorum certificate decides, as the grant the screen and the lease record (V3-2). Its origin
/// first -- k distinct keys of the policy's quorum signed this very payload, under this very policy
/// (`signing::Quorum::verify`, each refusal named) -- then its binding: this universe, an epoch that
/// follows the previous one it names, a grant. With `holder` (this host and this boot, at acquisition)
/// also: the new holder is this host in this boot, the certificate is live on this clock and its barrier
/// has passed, and its method binds. Without it (a supersession, delivered to any host), only origin and
/// epoch matter: learning that one is overtaken can stop things here, never start them, so an expired
/// certificate still teaches it.
fn certificate_grant(policy: &Policy, uuid: &str, document: &serde_json::Value, holder: Option<(&str, &str)>, now: i64) -> Result<Permit, Error> {
    let quorum = policy.authority()?.ok_or("This universe's policy names no key; a certificate here would be checked against nothing")?;
    quorum.verify(document, crate::signing::QUORUM_PROOF_KIND, crate::signing::TAKEOVER_FIELDS)?;
    let text = |f: &str| document[f].as_str().unwrap_or_default().to_string();
    let int = |f: &str| document[f].as_i64().unwrap_or_default();
    if text("resource") != uuid {
        return Err("The certificate is for a different resource than this universe".into());
    }
    let epoch = int("new_epoch");
    if !(1..=MAX_EPOCH).contains(&epoch) {
        return Err(format!("certificate.new_epoch must be from 1 to {MAX_EPOCH}").into());
    }
    let previous_epoch = int("previous_epoch");
    if previous_epoch != epoch - 1 {
        return Err(format!("The certificate names previous epoch {previous_epoch}, not the one before {epoch}").into());
    }
    for field in ["new_holder", "holder_boot_id", "grant_id"] {
        identifier(&text(field), &format!("certificate.{field}"))?;
    }
    if let Some((host, boot)) = holder {
        if text("new_holder") != host {
            return Err("The certificate names another host as the new holder".into());
        }
        if text("holder_boot_id") != boot {
            return Err("The certificate is bound to another incarnation of this host; a rebooted host must be decided for again".into());
        }
        if int("issued_at") > now + 30 {
            return Err("The certificate is issued in the future beyond the clock allowance".into());
        }
        if int("expires_at") < now {
            return Err(format!("The certificate expired {} seconds ago", now - int("expires_at")).into());
        }
        let eligible = int("eligible_after");
        if now < eligible {
            return Err(format!("The certificate's barrier is at {eligible}, {} seconds from now on this clock; refusing before it", eligible - now).into());
        }
        takeover_method(document, uuid, host, previous_epoch, "the certificate")?;
    }
    Ok(Permit {
        authority_id: quorum.authority_id,
        resource: uuid.to_string(),
        epoch,
        replica_id: text("new_holder"),
        instance_id: text("holder_boot_id"),
        grant_id: text("grant_id"),
    })
}

/// The grant a request presents: a quorum certificate (`certificate`) under any keyed policy, or the
/// gate's permit under a policy that is not a quorum. Under a quorum the screen moves on certificates
/// only, so a permit is refused there however well formed.
fn grant_of(request: &serde_json::Value, policy: &Policy, uuid: &str, holder: Option<(&str, &str)>, now: i64) -> Result<Permit, Error> {
    match (request.get("permit"), request.get("certificate")) {
        (Some(_), Some(_)) => Err("A request presents a permit or a certificate, not both".into()),
        (_, Some(c)) => certificate_grant(policy, uuid, c, holder, now),
        (_, None) if policy.quorum_form() => Err(
            "This universe's authority is a quorum: its epoch moves on a certificate its keys signed (certificate), never on a permit".into(),
        ),
        _ => permit(request),
    }
}

/// A change of a universe's authority set at `activation_require` (V3-2), and how it was authorised.
///
/// When neither the policy in place nor the one declared is a quorum, nothing is required: that is
/// today's behaviour for a single key or none, kept for the migration, and the change is recorded. When
/// either is a quorum, a change of the authority set (its digest: form, authority, threshold and keys)
/// needs one of two things, and nothing else moves it:
/// - `policy_change_certificate`, a certificate of the policy IN PLACE (k of its keys, or the single
///   key's 1-of-1) binding this universe and the new policy's digest, live on this clock, and never
///   applied here before;
/// - `replaces_policy_digest`, the operator's explicit re-declaration, naming the digest of the policy it
///   replaces, which must still be the current one: a stale re-declaration is refused, not applied.
///
/// The first authority set of a universe replaces nothing and is the operator's declaration. A
/// declaration that leaves the authority set as it is needs neither and records nothing. The epoch
/// screen is never touched by any of this: a new policy does not reset what this host has seen.
fn authority_change(
    db: &Connection,
    uuid: &str,
    request: &serde_json::Value,
    current: Option<&Policy>,
    proposed: &Policy,
    now: i64,
) -> Result<Option<(String, String, &'static str, String)>, Error> {
    let from = match current { Some(p) => p.authority_digest()?, None => None };
    let to = proposed.authority_digest()?;
    if from == to {
        return Ok(None);
    }
    let (from_text, to_text) = (from.clone().unwrap_or_default(), to.clone().unwrap_or_default());
    let quorum_involved = proposed.quorum_form() || current.is_some_and(Policy::quorum_form);
    let Some(from) = from.filter(|_| quorum_involved) else {
        let how = if from_text.is_empty() { "declared" } else { "redeclared_single_key" };
        return Ok(Some((from_text, to_text, how, String::new())));
    };
    match (request.get("replaces_policy_digest"), request.get("policy_change_certificate")) {
        (Some(_), Some(_)) => Err("A change of the authority set is authorised by a certificate or by the operator's re-declaration, not both".into()),
        (Some(named), None) => {
            if named.as_str() != Some(from.as_str()) {
                return Err(format!(
                    "replaces_policy_digest names {named}, but this universe's authority set is {from}: a re-declaration replaces the policy it names, and that one is not current"
                )
                .into());
            }
            Ok(Some((from_text, to_text, "operator_redeclaration", String::new())))
        }
        (None, Some(c)) => {
            let in_place = current.and_then(|p| p.authority().transpose()).transpose()?.ok_or("no authority in place")?;
            in_place.verify(c, crate::signing::POLICY_CHANGE_KIND, crate::signing::POLICY_CHANGE_FIELDS)?;
            if c["resource"].as_str() != Some(uuid) {
                return Err("The policy change certificate is for a different resource than this universe".into());
            }
            if to.is_none() {
                return Err("A certificate moves an authority set to another one; dropping the key is the operator's re-declaration".into());
            }
            if c["new_policy_digest"].as_str() != to.as_deref() {
                return Err(format!("The policy change certificate moves to {}, not to the policy declared here ({to_text})", c["new_policy_digest"]).into());
            }
            if c["issued_at"].as_i64().unwrap_or_default() > now + 30 {
                return Err("The policy change certificate is issued in the future beyond the clock allowance".into());
            }
            let expires = c["expires_at"].as_i64().unwrap_or_default();
            if expires < now {
                return Err(format!("The policy change certificate expired {} seconds ago", now - expires).into());
            }
            let digest = crate::signing::payload_digest(c)?;
            let used: i64 = db.query_row(
                "SELECT COUNT(*) FROM activation_policy_changes WHERE universe_uuid=?1 AND certificate_digest=?2",
                params![uuid, digest],
                |r| r.get(0),
            )?;
            if used > 0 {
                return Err("This policy change certificate was already applied here; a change it authorised once is not replayed".into());
            }
            Ok(Some((from_text, to_text, "certificate", digest)))
        }
        (None, None) => Err(format!(
            "This universe's authority set is {from}; changing it requires a certificate of that policy (policy_change_certificate) or the operator's explicit re-declaration (replaces_policy_digest: \"{from}\")"
        )
        .into()),
    }
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
        "state_directory_available_bytes": crate::migration::base().ok().map(|b| crate::migration::available_bytes(b)),
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
            operation_id TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS activation_policy_changes(
            id INTEGER PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            from_digest TEXT NOT NULL,
            to_digest TEXT NOT NULL,
            how TEXT NOT NULL,
            certificate_digest TEXT NOT NULL,
            authorization_ref TEXT NOT NULL,
            at INTEGER NOT NULL,
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
        ("activation_policy", "authority_key TEXT NOT NULL DEFAULT ''"),
        // The authority as a quorum (V3-2): the stored `{threshold, keys}`, empty for a single key or none.
        ("activation_policy", "authority_quorum TEXT NOT NULL DEFAULT ''"),
        ("activation_leases", "epoch INTEGER NOT NULL DEFAULT 0"),
        ("activation_leases", "grant_id TEXT NOT NULL DEFAULT ''"),
        // The boot a history row was written in; NULL for the rows written before the field existed,
        // which is the truthful reading: that boot was not recorded.
        ("activation_lease_history", "boot_id TEXT"),
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
    /// The authority's Ed25519 public key (32 bytes, lowercase hex); empty means the authority
    /// signs nothing yet and its takeover documents are laboratory ones, accepted on their
    /// binding alone and labelled so. Named, it makes every takeover document's signature
    /// required and verified here -- PodMesh holds no key of its own.
    pub authority_key: String,
    /// The authority as a quorum (V3-2): `{threshold, keys}` as stored, empty when the policy names a
    /// single key or none. Named, every exclusive decision is a certificate k of its n keys signed, and
    /// the epoch screen moves on certificates only: no permit is accepted.
    pub authority_quorum: String,
}

impl Policy {
    pub fn gated(&self) -> bool {
        !self.authority_id.is_empty()
    }

    /// Whether the authority is a declared quorum rather than a single key or none.
    pub fn quorum_form(&self) -> bool {
        !self.authority_quorum.is_empty()
    }

    /// The authority as a quorum: the declared one, or the 1-of-1 of a single key; none without a key.
    pub fn authority(&self) -> Result<Option<crate::signing::Quorum>, Error> {
        if self.quorum_form() {
            let v: serde_json::Value = serde_json::from_str(&self.authority_quorum)?;
            return Ok(Some(crate::signing::Quorum::declared(&self.authority_id, &v)?));
        }
        if !self.authority_key.is_empty() {
            return Ok(Some(crate::signing::Quorum::single(&self.authority_id, &self.authority_key)?));
        }
        Ok(None)
    }

    /// The digest of the authority set: what a certificate under it names, and what a change of it
    /// replaces. None for a policy without a key.
    pub fn authority_digest(&self) -> Result<Option<String>, Error> {
        Ok(self.authority()?.map(|q| q.digest()))
    }
}

pub fn policy(db: &Connection, uuid: &str) -> Result<Option<Policy>, Error> {
    Ok(db
        .query_row(
            "SELECT lease_seconds,takeover_margin_seconds,desired_standbys,eligible_hosts,authorization_ref,authority_id,authority_key,authority_quorum FROM activation_policy WHERE universe_uuid=?1",
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
                    authority_key: r.get(6)?,
                    authority_quorum: r.get(7)?,
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
    /// When this incarnation of the lease was taken: set at every acquisition, never at a renewal.
    /// A lapsed lease of this host's retaken keeps its generation and epoch; this is what differs.
    pub acquired_at: i64,
}

pub fn lease(db: &Connection, uuid: &str) -> Result<Option<Lease>, Error> {
    Ok(db
        .query_row(
            "SELECT holder_host_uuid,generation,expires_at,epoch,grant_id,acquired_at FROM activation_leases WHERE universe_uuid=?1",
            [uuid],
            |r| Ok(Lease { holder_host_uuid: r.get(0)?, generation: r.get(1)?, expires_at: r.get(2)?, epoch: r.get(3)?, grant_id: r.get(4)?, acquired_at: r.get(5)? }),
        )
        .optional()?)
}

/// The epoch that overtook this host's lease, if its screen has seen one: what the gate's fourth
/// refusal reads, for callers that name each refusal themselves.
pub(crate) fn superseded_by(db: &Connection, uuid: &str, l: &Lease) -> Result<Option<i64>, Error> {
    superseded(db, uuid, l)
}

/// Whether the lease was acquired or renewed during this boot: the rule `boot_restore` applies, and
/// anything else a host does again after its own restart. A host that was down cannot know what was
/// decided while it was, so only an entitlement decided again since the boot counts. A history row
/// written since 2026-09-18 carries the boot's identity and is compared by it, whatever the clock
/// says. A row written before carries none and is judged by the older rule, kept for those rows only:
/// written at or after the boot's start on the wall clock (`booted_at`, the kernel's `btime`), which
/// a clock stepped across the boot can mislead.
pub(crate) fn renewed_this_boot(db: &Connection, uuid: &str, boot: &str, booted_at: Option<i64>) -> Result<bool, Error> {
    ensure_schema(db)?;
    let exists = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> Result<bool, Error> { Ok(db.query_row(sql, p, |r| r.get::<_, i64>(0))? > 0) };
    if exists("SELECT COUNT(*) FROM activation_lease_history WHERE universe_uuid=?1 AND event IN ('acquired','renewed') AND boot_id=?2", &[&uuid, &boot])? {
        return Ok(true);
    }
    let Some(booted) = booted_at else { return Ok(false) };
    exists(
        "SELECT COUNT(*) FROM activation_lease_history WHERE universe_uuid=?1 AND event IN ('acquired','renewed') AND boot_id IS NULL AND at>=?2",
        &[&uuid, &booted],
    )
}

/// Whether this host's lease has been overtaken by an epoch it has seen: the node's epoch screen
/// says a newer grant exists, so whatever this lease says, this host is no longer entitled.
fn superseded(db: &Connection, uuid: &str, l: &Lease) -> Result<Option<i64>, Error> {
    Ok(highest_epoch_seen(db, uuid)?.filter(|&seen| seen > l.epoch))
}

pub(crate) fn host_uuid_public(db: &Connection) -> Result<String, Error> {
    host_uuid(db)
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
        // host's lease, live or not, no longer entitles it. This is the node's refusal.
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
        "INSERT INTO activation_lease_history(universe_uuid,holder_host_uuid,generation,event,at,operation_id,boot_id)
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        // A boot that could not be read is written as `unknown`, never NULL: NULL is how the rows written
        // before the field existed are recognised, and judged by the clock; `unknown` never matches a boot.
        params![uuid, holder, generation, event, crate::now() as i64, id, boot_id().unwrap_or_else(|_| "unknown".into())],
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
    let quorum = policy.as_ref().is_some_and(Policy::quorum_form);
    let mut v = serde_json::json!({
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
        "authority_key": policy.as_ref().map(|p| p.authority_key.clone()).filter(|k| !k.is_empty()),
        "takeover_proof_verification": if policy.as_ref().is_some_and(|p| !p.authority_key.is_empty()) {
            "the authority's takeover documents must be Ed25519-signed by the policy's key; signature verified here before any binding"
        } else {
            "laboratory takeover documents, unsigned: their binding is checked, their origin is not"
        },
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
    });
    // The authority as a set of keys (V3-2): its digest for every keyed policy, what a certificate names
    // and a change replaces; and, under a quorum, the keys, the threshold and what that changes.
    if let Some(p) = policy.as_ref() {
        v["authority_policy_digest"] = serde_json::json!(p.authority_digest()?);
    }
    if quorum {
        let p = policy.as_ref().ok_or("no policy")?;
        v["authority_quorum"] = serde_json::from_str(&p.authority_quorum)?;
        v["takeover_proof_verification"] = serde_json::json!(
            "a quorum certificate: at least the threshold's number of distinct keys of the policy's quorum signed its canonical form, under this policy's digest; verified here before any binding"
        );
        v["permit_verification"] = serde_json::json!(
            "no permit is accepted under a quorum: the epoch screen moves forward on certificates only, each verified here, and never backwards"
        );
        v["scope"] = serde_json::json!(
            "this host's journal only; not mutual exclusion across hosts by itself. The epoch screen refuses grants this host has seen superseded and moves only on certificates the quorum signed; whether two certificates can exist for one epoch is the signers' promise, not this host's"
        );
    }
    Ok(v)
}

pub fn execute(db: &Connection, request: &serde_json::Value) -> Result<serde_json::Value, Error> {
    let operation = lc::text(request, "operation")?;
    lc::ensure_schema(db)?;
    ensure_schema(db)?;
    if operation == "activation_fence_preview" {
        return fence_preview(db);
    }
    if operation == "activation_status" {
        let uuid = lc::text(request, "universe_uuid")?;
        lc::token(uuid)?;
        return view(db, uuid);
    }
    // Every mutation here keeps the journal contract of every other operation: one request per
    // operation ID, a verified one replayed and never repeated, an interrupted one re-evaluated.
    lc::journaled(db, request, |db| perform(db, request))
}

fn perform(db: &Connection, request: &serde_json::Value) -> Result<serde_json::Value, Error> {
    let operation = lc::text(request, "operation")?;
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
            // Optional with it: the authority's public key, checked to be a valid Ed25519 key now
            // so that a mistyped one is refused here and not at the first takeover.
            let authority_key = match request.get("authority_key") {
                None => String::new(),
                Some(v) => {
                    let k = v.as_str().ok_or("authority_key must be a string")?;
                    if authority.is_empty() {
                        return Err("authority_key without authority_id: a key belongs to a named authority".into());
                    }
                    crate::signing::verifying_key(k)?;
                    k.to_string()
                }
            };
            // Or the authority as a quorum of replica keys (V3-2): n public keys under stable key ids and
            // a threshold that is a strict majority of them. One form or the other, never both.
            let authority_quorum = match request.get("authority_quorum") {
                None => String::new(),
                Some(v) => {
                    if authority.is_empty() {
                        return Err("authority_quorum without authority_id: a quorum belongs to a named authority".into());
                    }
                    if !authority_key.is_empty() {
                        return Err("authority_key and authority_quorum are two forms of one authority; name one".into());
                    }
                    crate::signing::Quorum::declared(&authority, v)?.stored().to_string()
                }
            };
            let proposed = Policy {
                lease_seconds,
                takeover_margin_seconds: margin,
                desired_standbys: standbys,
                eligible_hosts: hosts.clone(),
                authorization_ref: reference.to_string(),
                authority_id: authority.clone(),
                authority_key: authority_key.clone(),
                authority_quorum: authority_quorum.clone(),
            };
            let change = authority_change(db, uuid, request, policy(db, uuid)?.as_ref(), &proposed, now)?;
            db.execute(
                "INSERT INTO activation_policy(universe_uuid,lease_seconds,takeover_margin_seconds,declared_at,operation_id,
                   desired_standbys,eligible_hosts,authorization_ref,authority_id,authority_key,authority_quorum)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                 ON CONFLICT(universe_uuid) DO UPDATE SET lease_seconds=excluded.lease_seconds,
                   takeover_margin_seconds=excluded.takeover_margin_seconds,
                   declared_at=excluded.declared_at, operation_id=excluded.operation_id,
                   desired_standbys=excluded.desired_standbys, eligible_hosts=excluded.eligible_hosts,
                   authorization_ref=excluded.authorization_ref, authority_id=excluded.authority_id,
                   authority_key=excluded.authority_key, authority_quorum=excluded.authority_quorum",
                params![uuid, lease_seconds as i64, margin as i64, now, id, standbys as i64,
                        serde_json::to_string(&hosts)?, reference, authority, authority_key, authority_quorum],
            )?;
            if let Some((from, to, how, certificate)) = change {
                db.execute(
                    "INSERT INTO activation_policy_changes(universe_uuid,from_digest,to_digest,how,certificate_digest,authorization_ref,at,operation_id)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![uuid, from, to, how, certificate, reference, now, id],
                )?;
            }
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
                let boot = boot_id()?;
                let p = grant_of(request, &policy, uuid, Some((&this_host, &boot)), now)?;
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
                if request.get("certificate").is_some() {
                    return Err("This universe's policy names no authority; a certificate here would be checked against nothing".into());
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
            let p = grant_of(request, &policy, uuid, None, now)?;
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
/// What a fence would do now, read-only and unjournaled: the universes under a policy this host
/// no longer holds a live lease for and that are running, the exclusive routes it no longer
/// holds, and whether the network ledger carries anything incomplete. A timer asks this first
/// and journals a fence only when there is something to fence (Codex, I2): the transition is
/// evidence, the empty tick is not.
/// The names of the universe containers Podman reports running (or still stopping), from one listing: a fence and
/// its preview ask Podman once, not once per policy row. A journal that has accumulated many policies (87 on the
/// laboratory's lab-a) otherwise spends seconds in child processes on every tick, and a daemon that serves one
/// request at a time is unavailable for that long. `None` when the listing cannot be read: the callers then fall
/// back to asking per universe, as before.
pub(crate) fn running_universe_names(list: &[serde_json::Value]) -> std::collections::HashSet<String> {
    list.iter()
        .filter(|c| matches!(c["State"].as_str(), Some("running") | Some("stopping")))
        .flat_map(|c| c["Names"].as_array().cloned().unwrap_or_default())
        .filter_map(|n| n.as_str().map(str::to_string))
        .filter(|n| n.starts_with("podmesh-"))
        .collect()
}

fn running_universes() -> Option<std::collections::HashSet<String>> {
    crate::transfer::all_containers().ok().map(|list| running_universe_names(&list))
}

/// The resources whose publishing connector unit is active or activating, from one `systemctl list-units`.
pub(crate) fn active_publisher_resources(listing: &str) -> std::collections::HashSet<String> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let unit = fields.next()?;
            let _load = fields.next()?;
            let active = fields.next()?;
            let resource = unit.strip_prefix("podmesh-publisher-")?.strip_suffix(".service")?;
            matches!(active, "active" | "activating").then(|| resource.to_string())
        })
        .collect()
}

fn active_publishers() -> Option<std::collections::HashSet<String>> {
    let out = std::process::Command::new("systemctl")
        .args(["list-units", "--all", "--plain", "--no-legend", "--no-pager", "podmesh-publisher-*.service"])
        .output()
        .ok()?;
    out.status.success().then(|| active_publisher_resources(&String::from_utf8_lossy(&out.stdout)))
}

pub(crate) fn fence_preview(db: &Connection) -> Result<serde_json::Value, Error> {
    let now = crate::now() as i64;
    let this_host = host_uuid(db)?;
    let mut statement = db.prepare("SELECT universe_uuid FROM activation_policy ORDER BY universe_uuid")?;
    let universes: Vec<String> = statement.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut pending = Vec::new();
    let running = running_universes();
    let publishers = active_publishers();
    for uuid in universes {
        let held = lease(db, &uuid)?;
        let overtaken = match held.as_ref() { Some(l) => superseded(db, &uuid, l)?, None => None };
        let entitled = held.as_ref().is_some_and(|l| l.holder_host_uuid == this_host && l.expires_at > now && overtaken.is_none());
        if entitled {
            continue;
        }
        // One listing decides who is not running; a universe it names is confirmed by inspection.
        let maybe_running = running.as_ref().is_none_or(|names| names.contains(&format!("podmesh-{uuid}")));
        if maybe_running {
            let observed = lc::observe(&uuid)?;
            if observed["present"] == serde_json::json!(true) && observed["running"] == serde_json::json!(true) {
                pending.push(serde_json::json!({"universe_uuid": uuid, "what": "running without entitlement"}));
            }
        }
        if crate::network::exclusive_route_held(db, &uuid)? {
            pending.push(serde_json::json!({"resource": uuid, "what": "exclusive route without entitlement"}));
        }
        let connector = match publishers.as_ref() {
            Some(active) => Some(active.contains(&uuid)),
            None => crate::publisher::connector_present(&uuid),
        };
        if connector == Some(true) {
            pending.push(serde_json::json!({"resource": uuid, "what": "publishing connector without entitlement"}));
        }
    }
    let incomplete = crate::network::incomplete_effects(db)?;
    Ok(serde_json::json!({
        "pending": pending,
        "incomplete_network_effects": incomplete,
        "nothing_to_fence": pending.is_empty() && incomplete == 0,
        "scope": "read-only, not journaled: what activation_fence would act on now, from this host's journal and Podman; a fence must follow to act",
    }))
}

fn fence(db: &Connection, id: &str, timeout: u64) -> Result<serde_json::Value, Error> {
    let now = crate::now() as i64;
    let this_host = host_uuid(db)?;
    let mut statement = db.prepare("SELECT universe_uuid FROM activation_policy ORDER BY universe_uuid")?;
    let universes: Vec<String> = statement
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut fenced = Vec::new();
    let mut left = Vec::new();
    // Every resource this fence found this host NOT entitled to, whether or not anything had to be
    // withdrawn for it: what a fence receipt binds a takeover proof to (Codex, P0).
    let mut unentitled = Vec::new();
    // For each of them, what the fence found: the epoch that overtook this host's lease, if one did. A
    // fence receipt is bound to a transition by it (third review of V3-1): the authority accepts a receipt
    // as the previous holder's account of THIS rotation only when that host had seen the rotation's epoch.
    let mut unentitled_detail = Vec::new();
    let listed = running_universes();
    for uuid in universes {
        let held = lease(db, &uuid)?;
        let overtaken = match held.as_ref() { Some(l) => superseded(db, &uuid, l)?, None => None };
        let entitled = held
            .as_ref()
            .is_some_and(|l| l.holder_host_uuid == this_host && l.expires_at > now && overtaken.is_none());
        if !entitled {
            unentitled.push(uuid.clone());
            unentitled_detail.push(serde_json::json!({
                "resource": uuid,
                "superseded_by_epoch": overtaken,
                "held_by": held.as_ref().map(|l| l.holder_host_uuid.clone()),
                "expired_seconds_ago": held.as_ref().map(|l| now - l.expires_at).filter(|&s| s >= 0),
            }));
        }
        // One listing decides who is not running; a universe it names (or every one, when it cannot be read) is
        // confirmed by inspection before anything is stopped.
        let maybe_running = listed.as_ref().is_none_or(|names| names.contains(&format!("podmesh-{uuid}")));
        let running = maybe_running && {
            let observed = lc::observe(&uuid)?;
            observed["present"] == serde_json::json!(true) && observed["running"] == serde_json::json!(true)
        };
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
    // Exclusive routes go the same way as universes: a route published under a resource this host
    // no longer holds a live, unsuperseded lease for is withdrawn, verified from the kernel -- after
    // reconciliation has undone whatever a crash left half-made, so that nothing is unowned.
    let network_reconciliation = crate::network::reconcile(db)?;
    // The publishing connector of a role this host no longer holds goes first -- stopped, the
    // active manager's mark removed -- so that nothing publishes an address about to be withdrawn.
    let mut publishers_withdrawn = crate::publisher::withdraw_unentitled(db, &|resource: &str| {
        lease(db, resource).ok().flatten().is_some_and(|l| {
            l.holder_host_uuid == this_host && l.expires_at > now && superseded(db, resource, &l).ok().flatten().is_none()
        })
    }, id)?;
    // The reconciliation just above withdraws a publisher of a resource this host no longer holds
    // before this step reaches it (the supersession was delivered before the fence): that
    // withdrawal is the fence's too, reported here, first, with the pass that made it named.
    let by_reconciliation: Vec<serde_json::Value> = network_reconciliation["publishers"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter(|r| r["resource"].as_str().is_some_and(|res| unentitled.iter().any(|u| u == res)))
                .map(|r| {
                    let mut r = r.clone();
                    r["by"] = serde_json::Value::from("reconciliation before the fence");
                    r
                })
                .collect()
        })
        .unwrap_or_default();
    publishers_withdrawn.splice(0..0, by_reconciliation);
    let routes_withdrawn = crate::network::withdraw_unentitled(db, &|resource: &str| {
        lease(db, resource).ok().flatten().is_some_and(|l| {
            l.holder_host_uuid == this_host && l.expires_at > now && superseded(db, resource, &l).ok().flatten().is_none()
        })
    })?;
    Ok(serde_json::json!({
        "this_host_uuid": this_host,
        "fenced": fenced,
        "unentitled": unentitled,
        "unentitled_detail": unentitled_detail,
        "publishers_withdrawn": publishers_withdrawn,
        "routes_withdrawn": routes_withdrawn,
        "network_reconciliation": network_reconciliation,
        "left_running_or_absent": left,
        "scope": "this host only; a host that never runs this operation is not fenced by it",
    }))
}

#[cfg(test)]
mod boot_tests {
    use super::*;

    const U: &str = "00000000-0000-4000-8000-000000000001";

    fn history(db: &Connection, event: &str, at: i64, boot: Option<&str>) {
        db.execute("INSERT INTO activation_lease_history(universe_uuid,holder_host_uuid,generation,event,at,operation_id,boot_id) VALUES(?1,'h',1,?2,?3,'o',?4)",
                   params![U, event, at, boot]).unwrap();
    }

    /// Renewed during this boot, by the boot's identity where a row carries one (review of V3-1): a
    /// row of another boot never counts, whatever its time says; a row of this boot counts, whatever
    /// the clock did; a row written before the field existed is judged by the wall clock, as before.
    #[test]
    fn this_boot_is_recognised_by_its_identity_and_older_rows_by_the_clock() {
        let db = Connection::open_in_memory().unwrap();
        ensure_schema(&db).unwrap();
        let booted = Some(1_000);
        assert!(!renewed_this_boot(&db, U, "this-boot", booted).unwrap());
        history(&db, "renewed", 5_000, Some("another-boot"));
        assert!(!renewed_this_boot(&db, U, "this-boot", booted).unwrap(), "a later time under another boot counts");
        history(&db, "policy_declared", 5_000, Some("this-boot"));
        assert!(!renewed_this_boot(&db, U, "this-boot", booted).unwrap(), "an event that is not a renewal counts");
        history(&db, "renewed", 5_000, Some("unknown"));
        assert!(!renewed_this_boot(&db, U, "this-boot", booted).unwrap(), "a row whose boot could not be read counts");
        history(&db, "renewed", 10, Some("this-boot"));
        assert!(renewed_this_boot(&db, U, "this-boot", booted).unwrap(), "a renewal of this boot stamped before btime does not count");
        let db = Connection::open_in_memory().unwrap();
        ensure_schema(&db).unwrap();
        history(&db, "acquired", 999, None);
        assert!(!renewed_this_boot(&db, U, "this-boot", booted).unwrap());
        history(&db, "acquired", 1_000, None);
        assert!(renewed_this_boot(&db, U, "this-boot", booted).unwrap());
        assert!(!renewed_this_boot(&db, U, "this-boot", None).unwrap(), "an older row without a known boot start counts");
    }

    #[test]
    fn every_history_row_written_now_carries_this_boot() {
        let db = Connection::open_in_memory().unwrap();
        ensure_schema(&db).unwrap();
        record(&db, U, "h", 1, "renewed", "o").unwrap();
        let boot: Option<String> = db.query_row("SELECT boot_id FROM activation_lease_history", [], |r| r.get(0)).unwrap();
        assert_eq!(boot, boot_id().ok());
        assert!(renewed_this_boot(&db, U, &boot_id().unwrap(), None).unwrap());
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;

    #[test]
    fn running_names_come_from_one_listing() {
        let list: Vec<serde_json::Value> = serde_json::from_str(r#"[
            {"Names":["podmesh-a"],"State":"running"},
            {"Names":["podmesh-b"],"State":"exited"},
            {"Names":["podmesh-c"],"State":"stopping"},
            {"Names":["other"],"State":"running"},
            {"Names":["podmesh-d"],"State":"paused"}
        ]"#).unwrap();
        let names = running_universe_names(&list);
        assert!(names.contains("podmesh-a") && names.contains("podmesh-c"));
        assert!(!names.contains("podmesh-b") && !names.contains("other") && !names.contains("podmesh-d"));
    }

    #[test]
    fn active_publishers_come_from_one_unit_listing() {
        let listing = "podmesh-publisher-91eeb6bf.service loaded active running cloudflared\n\
                       podmesh-publisher-aaaa.service loaded inactive dead x\n\
                       podmesh-publisher-bbbb.service loaded activating start y\n\
                       other.service loaded active running z\n";
        let active = active_publisher_resources(listing);
        assert!(active.contains("91eeb6bf") && active.contains("bbbb"));
        assert!(!active.contains("aaaa") && active.len() == 2);
    }
}

#[cfg(test)]
mod quorum_tests {
    //! V3-2 on the node: a quorum policy declared, its certificates at acquisition and supersession,
    //! the epoch screen, and the rule for changing the authority set -- through `execute`, as a request
    //! on the socket runs, journal included.
    use super::*;
    use crate::signing::testkit::{self, public, sign};
    use serde_json::{json, Value};

    const R: &str = "91eeb6bf-5489-405b-b77a-53105b0aff7a";
    const OTHER_RESOURCE: &str = "00000000-0000-4000-8000-000000000001";
    const HOST: &str = "5d1c0b8e-3f59-4d0e-9d7a-2a1e7c4b9f10";
    const OTHER: &str = "00000000-0000-4000-8000-00000000000b";
    const ABC: &[(&str, u8)] = &[("replica-a", 1), ("replica-b", 2), ("replica-c", 3)];
    const GATE_SEED: u8 = 42;

    fn next() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(1);
        N.fetch_add(1, Ordering::SeqCst)
    }

    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        db.execute("INSERT INTO metadata VALUES('host_uuid',?1)", [HOST]).unwrap();
        lc::ensure_schema(&db).unwrap();
        ensure_schema(&db).unwrap();
        db
    }

    fn act(db: &Connection, op: &str, extra: Value) -> Result<Value, String> {
        let mut r = json!({"operation": op, "universe_uuid": R, "operation_id": format!("q-{}", next()), "authorization_ref": "probe"});
        for (k, v) in extra.as_object().unwrap() {
            r[k] = v.clone();
        }
        execute(db, &r).map_err(|e| e.to_string())
    }

    fn boot() -> String {
        boot_id().unwrap()
    }

    fn quorum_policy(threshold: usize, keys: &[(&str, u8)]) -> Value {
        json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "replicas", "authority_quorum": testkit::policy(threshold, keys)})
    }

    /// A journal under a 2-of-3 quorum of replica keys.
    fn under_quorum() -> Connection {
        let db = db();
        act(&db, "activation_require", quorum_policy(2, ABC)).unwrap();
        db
    }

    fn digest(db: &Connection) -> String {
        policy(db, R).unwrap().unwrap().authority_digest().unwrap().unwrap()
    }

    /// A takeover certificate of this universe's current policy, for `epoch`, to `holder` in this boot,
    /// signed by `signers`.
    fn cert(db: &Connection, epoch: i64, holder: &str, signers: &[(&str, u8)]) -> Value {
        let authority = policy(db, R).unwrap().unwrap().authority_id;
        sign(&testkit::takeover(&authority, &digest(db), R, epoch, holder, &boot()), signers)
    }

    fn screen_of(db: &Connection) -> Option<i64> {
        highest_epoch_seen(db, R).unwrap()
    }

    fn permit(epoch: i64, host: &str) -> Value {
        json!({"authority_id": "replicas", "resource": R, "epoch": epoch, "replica_id": host, "instance_id": boot(), "grant_id": format!("g{epoch}")})
    }

    /// Proves: under a quorum, acquisition takes a certificate of k distinct keys and nothing less. A
    /// permit, however well formed, is refused; a single replica's certificate is refused; two
    /// replicas' certificate is accepted, the lease and the screen record its epoch and grant; the
    /// status reports the quorum, its digest and what the screen now moves on.
    #[test]
    fn under_a_quorum_a_majority_decides_and_a_minority_does_not() {
        let db = under_quorum();
        let e = act(&db, "activation_acquire", json!({"permit": permit(1, HOST)})).unwrap_err();
        assert!(e.contains("never on a permit"), "{e}");
        let e = act(&db, "activation_acquire", json!({})).unwrap_err();
        assert!(e.contains("never on a permit"), "{e}");
        let e = act(&db, "activation_acquire", json!({"certificate": cert(&db, 1, HOST, &ABC[..1])})).unwrap_err();
        assert!(e.contains("(below_threshold)"), "{e}");
        let e = act(&db, "activation_acquire", json!({"certificate": cert(&db, 1, HOST, &ABC[..2]), "permit": permit(1, HOST)})).unwrap_err();
        assert!(e.contains("not both"), "{e}");
        assert_eq!(screen_of(&db), None, "nothing refused moved the screen");
        let v = act(&db, "activation_acquire", json!({"certificate": cert(&db, 1, HOST, &ABC[1..])})).unwrap();
        assert_eq!((v["epoch"].clone(), v["grant_id"].clone(), v["highest_epoch_seen"].clone()), (json!(1), json!("g1"), json!(1)));
        assert_eq!(v["authority_policy_digest"], json!(digest(&db)));
        assert_eq!(v["authority_quorum"]["threshold"], json!(2));
        assert!(v["permit_verification"].as_str().unwrap().contains("certificates only"), "{v}");
        // The same decision presented again, by all three this time: the same grant, accepted.
        act(&db, "activation_acquire", json!({"certificate": cert(&db, 1, HOST, ABC)})).unwrap();
        // A same-holder re-issue at the next epoch.
        let mut same = cert(&db, 2, HOST, &[]);
        same["method"] = json!("same_holder");
        same["previous_holder"] = json!(HOST);
        act(&db, "activation_acquire", json!({"certificate": sign(&same, &ABC[..2])})).unwrap();
        assert_eq!(screen_of(&db), Some(2));
    }

    /// Proves: a certificate is bound to what it decides. Signed by a majority, but for another
    /// resource, another holder, another boot of this host, an epoch that does not follow the previous
    /// one it names, a second grant at an epoch already granted, an expired life, a barrier still ahead,
    /// or a method whose binding fails: each refused with its reason, and the screen unmoved.
    #[test]
    fn a_certificate_for_another_resource_holder_boot_or_epoch_is_refused() {
        let db = under_quorum();
        act(&db, "activation_acquire", json!({"certificate": cert(&db, 5, HOST, &ABC[..2])})).unwrap();
        let refused = |doc: Value, why: &str| {
            let e = act(&db, "activation_acquire", json!({"certificate": sign(&doc, &ABC[..2])})).unwrap_err();
            assert!(e.contains(why), "{why}: {e}");
            assert_eq!(screen_of(&db), Some(5), "{why}");
        };
        let base = cert(&db, 6, HOST, &[]);
        let edit = |f: &dyn Fn(&mut Value)| { let mut v = base.clone(); f(&mut v); v };
        refused(edit(&|v| v["resource"] = json!(OTHER_RESOURCE)), "different resource");
        refused(edit(&|v| v["new_holder"] = json!(OTHER)), "another host as the new holder");
        refused(edit(&|v| v["holder_boot_id"] = json!("00000000-0000-4000-8000-0000000000bb")), "another incarnation of this host");
        refused(edit(&|v| v["previous_epoch"] = json!(4)), "not the one before 6");
        refused(edit(&|v| { v["new_epoch"] = json!(4); v["previous_epoch"] = json!(3); }), "superseded: this host has already seen epoch 5");
        refused(edit(&|v| { v["new_epoch"] = json!(5); v["previous_epoch"] = json!(4); v["grant_id"] = json!("another"); }), "already granted here under another grant");
        refused(edit(&|v| { v["issued_at"] = json!(1_700_000_000); v["expires_at"] = json!(1_700_003_600); }), "expired");
        refused(edit(&|v| v["issued_at"] = json!(crate::now() as i64 + 600)), "issued in the future");
        refused(edit(&|v| v["eligible_after"] = json!(crate::now() as i64 + 600)), "barrier is at");
        refused(edit(&|v| v["method"] = json!("same_holder")), "the certificate says same_holder");
        refused(edit(&|v| v["method"] = json!("first")), "the certificate says first");
        refused(edit(&|v| v["method"] = json!("fence_receipt")), "the fence receipt names no host");
        refused(edit(&|v| v["method"] = json!("elected")), "method elected is unknown");
        refused(edit(&|v| v["new_epoch"] = json!(i64::from(i32::MAX) + 1)), "new_epoch must be from 1");
        // A certificate for another resource cannot supersede this one either.
        let e = act(&db, "activation_supersede", json!({"certificate": sign(&edit(&|v| v["resource"] = json!(OTHER_RESOURCE)), &ABC[..2])})).unwrap_err();
        assert!(e.contains("different resource"), "{e}");
        assert_eq!(screen_of(&db), Some(5));
    }

    /// Proves the invariant the V3 plan states for every later lot: the durable epoch screen moves
    /// forward on certificates only, never on anything else, and never backwards. A deterministic
    /// sequence of 400 attempts (valid and invalid certificates at epochs around the screen, permits,
    /// supersessions, acquisitions, policy re-declarations) is run; after each, the screen is compared
    /// with what it was: it never decreased, and it changed only on a successful operation that
    /// presented a certificate verified here.
    #[test]
    fn the_screen_never_moves_backwards_and_moves_on_certificates_only() {
        let db = under_quorum();
        act(&db, "activation_acquire", json!({"certificate": cert(&db, 10, HOST, &ABC[..2])})).unwrap();
        assert_eq!(screen_of(&db), Some(10));
        // The SQL guard itself: a lower epoch written straight to the screen changes nothing.
        let low = Permit { authority_id: "replicas".into(), resource: R.into(), epoch: 3, replica_id: OTHER.into(), instance_id: "b".into(), grant_id: "g3".into() };
        screen(&db, R, &low, "direct").unwrap();
        assert_eq!(screen_of(&db), Some(10));
        // And the operation says so rather than succeeding quietly: a valid certificate at or below the
        // screen is refused as a supersession and as an acquisition.
        for epoch in [9, 10] {
            let e = act(&db, "activation_supersede", json!({"certificate": cert(&db, epoch, OTHER, &ABC[..2])})).unwrap_err();
            assert!(e.contains(&format!("Epoch {epoch} does not supersede epoch 10")), "{e}");
        }
        let e = act(&db, "activation_acquire", json!({"certificate": cert(&db, 9, HOST, &ABC[..2])})).unwrap_err();
        assert!(e.contains("superseded: this host has already seen epoch 10"), "{e}");
        let mut seed: u64 = 0x5eed;
        let mut rand = move |n: u64| { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (seed >> 33) % n };
        let mut moved = 0;
        for _ in 0..400 {
            let before = screen_of(&db).unwrap();
            let epoch = (before + rand(5) as i64 - 2).max(1);
            let signers: &[(&str, u8)] = match rand(4) { 0 => &ABC[..1], 1 => &[("replica-a", 1), ("replica-a", 1)], _ => &ABC[..2] };
            let holder = if rand(2) == 0 { HOST } else { OTHER };
            let (op, body, by_certificate) = match rand(6) {
                0 => ("activation_supersede", json!({"permit": permit(epoch, holder)}), false),
                1 => ("activation_acquire", json!({"permit": permit(epoch, HOST)}), false),
                2 => ("activation_require", quorum_policy(2, &[ABC[2], ABC[0], ABC[1]]), false),
                3 => ("activation_acquire", json!({"certificate": cert(&db, epoch, HOST, signers)}), true),
                _ => ("activation_supersede", json!({"certificate": cert(&db, epoch, holder, signers)}), true),
            };
            let ok = act(&db, op, body).is_ok();
            let after = screen_of(&db).unwrap();
            assert!(after >= before, "{op} moved the screen back from {before} to {after}");
            if after != before {
                assert!(ok && by_certificate && signers.len() == 2 && signers[0] != signers[1], "{op} moved the screen without a valid certificate");
                moved += 1;
            }
        }
        assert!(moved >= 20, "the sequence moved the screen only {moved} times");
    }

    /// Proves: under a quorum, a supersession is a certificate of k keys too -- so a forged higher epoch
    /// cannot even stop a universe here any more -- and it teaches the host it is overtaken even when
    /// the certificate's own life is over, since learning it can only stop things.
    #[test]
    fn a_supersession_is_a_certificate_and_an_expired_one_still_teaches() {
        let db = under_quorum();
        act(&db, "activation_acquire", json!({"certificate": cert(&db, 1, HOST, &ABC[..2])})).unwrap();
        let e = act(&db, "activation_supersede", json!({"permit": permit(2, OTHER)})).unwrap_err();
        assert!(e.contains("never on a permit"), "{e}");
        let e = act(&db, "activation_supersede", json!({"certificate": cert(&db, 2, OTHER, &ABC[2..])})).unwrap_err();
        assert!(e.contains("(below_threshold)"), "{e}");
        let mut old = cert(&db, 2, OTHER, &[]);
        old["issued_at"] = json!(1_700_000_000);
        old["expires_at"] = json!(1_700_003_600);
        old["holder_boot_id"] = json!("00000000-0000-4000-8000-0000000000bb");
        let v = act(&db, "activation_supersede", json!({"certificate": sign(&old, &ABC[1..])})).unwrap();
        assert_eq!((v["highest_epoch_seen"].clone(), v["superseded"].clone()), (json!(2), json!(true)));
        assert!(refuse_if_not_activated(&db, R, "start").unwrap_err().to_string().contains("superseded by epoch 2"));
        let e = act(&db, "activation_renew", json!({})).unwrap_err();
        assert!(e.contains("superseded by epoch 2"), "{e}");
    }

    /// Proves: the single key's policies behave as before -- the gate's permits still acquire and
    /// supersede -- and accept a certificate under their 1-of-1 digest, signed by the gate's key: the
    /// migration can move the gate to certificates before it moves the policy to the replicas.
    #[test]
    fn a_single_key_policy_keeps_its_permits_and_accepts_its_one_of_one_certificate() {
        let db = db();
        act(&db, "activation_require", json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "lab-gate", "authority_key": public(GATE_SEED)})).unwrap();
        let mut p = permit(1, HOST);
        p["authority_id"] = json!("lab-gate");
        act(&db, "activation_acquire", json!({"permit": p})).unwrap();
        let single = policy(&db, R).unwrap().unwrap().authority().unwrap().unwrap();
        assert!(single.single_key);
        let c = sign(&testkit::takeover("lab-gate", &single.digest(), R, 2, HOST, &boot()), &[(crate::signing::SINGLE_KEY_ID, GATE_SEED)]);
        let v = act(&db, "activation_acquire", json!({"certificate": c})).unwrap();
        assert_eq!(v["highest_epoch_seen"], json!(2));
        assert!(v.get("authority_quorum").is_none() && v["permit_verification"].as_str().unwrap().contains("not verified"), "{v}");
        let mut p = permit(3, OTHER);
        p["authority_id"] = json!("lab-gate");
        act(&db, "activation_supersede", json!({"permit": p})).unwrap();
        assert_eq!(screen_of(&db), Some(3));
        // A policy with an authority and no key takes no certificate: nothing to check it against.
        let unkeyed = self::db();
        act(&unkeyed, "activation_require", json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "lab-gate"})).unwrap();
        let e = act(&unkeyed, "activation_acquire", json!({"certificate": c_unkeyed()})).unwrap_err();
        assert!(e.contains("names no key"), "{e}");
    }

    fn c_unkeyed() -> Value {
        sign(&testkit::takeover("lab-gate", "none", R, 1, HOST, &boot()), &[(crate::signing::SINGLE_KEY_ID, GATE_SEED)])
    }

    fn changes(db: &Connection) -> Vec<(String, String, String)> {
        let mut s = db.prepare("SELECT from_digest,to_digest,how FROM activation_policy_changes WHERE universe_uuid=?1 ORDER BY id").unwrap();
        s.query_map([R], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap()
    }

    fn change_certificate(db: &Connection, to: &str, signers: &[(&str, u8)]) -> Value {
        let p = policy(db, R).unwrap().unwrap();
        let now = crate::now() as i64;
        sign(&json!({"kind": crate::signing::POLICY_CHANGE_KIND, "authority_id": p.authority_id, "policy_digest": p.authority_digest().unwrap().unwrap(),
                     "resource": R, "new_policy_digest": to, "issued_at": now, "expires_at": now + 600}), signers)
    }

    fn digest_of(authority: &str, v: &Value) -> String {
        crate::signing::Quorum::declared(authority, v).unwrap().digest()
    }

    /// Proves the rule for changing the authority set. From the gate's single key to a 2-of-3 quorum:
    /// refused with neither authorisation, refused under a stale digest, accepted under the operator's
    /// explicit re-declaration naming the current digest. From that quorum to another: accepted under a
    /// certificate two of the CURRENT keys signed; refused when signed by one of them, by the new
    /// policy's keys, for another resource, or for another target. A certificate applied once is not
    /// replayed after the operator moved the policy back. Re-declaring the same quorum in another key
    /// order needs nothing. Dropping the quorum is the operator's only. Every change is recorded, and
    /// none touches the screen.
    #[test]
    fn the_authority_set_changes_under_a_certificate_of_the_current_policy_or_the_operator_only() {
        let db = db();
        act(&db, "activation_require", json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "lab-gate", "authority_key": public(GATE_SEED)})).unwrap();
        let mut p = permit(7, HOST);
        p["authority_id"] = json!("lab-gate");
        act(&db, "activation_acquire", json!({"permit": p})).unwrap();
        let gate = digest(&db);
        let e = act(&db, "activation_require", quorum_policy(2, ABC)).unwrap_err();
        assert!(e.contains("requires a certificate of that policy") && e.contains(&gate), "{e}");
        let mut stale = quorum_policy(2, ABC);
        stale["replaces_policy_digest"] = json!("0".repeat(64));
        let e = act(&db, "activation_require", stale).unwrap_err();
        assert!(e.contains("is not current"), "{e}");
        let mut by_operator = quorum_policy(2, ABC);
        by_operator["replaces_policy_digest"] = json!(gate);
        act(&db, "activation_require", by_operator.clone()).unwrap();
        let first = digest(&db);
        assert_eq!(changes(&db).last().unwrap(), &(gate.clone(), first.clone(), "operator_redeclaration".to_string()));
        assert_eq!(screen_of(&db), Some(7), "a new policy does not reset the screen");
        // The same quorum, keys listed in another order: not a change, nothing required.
        act(&db, "activation_require", quorum_policy(2, &[ABC[2], ABC[1], ABC[0]])).unwrap();
        assert_eq!(changes(&db).len(), 2);
        // To another quorum (replica-c's key replaced by replica-d's), by certificate.
        let next_keys: &[(&str, u8)] = &[ABC[0], ABC[1], ("replica-d", 4)];
        let target = digest_of("replicas", &testkit::policy(2, next_keys));
        let with_cert = |c: Value| { let mut r = quorum_policy(2, next_keys); r["policy_change_certificate"] = c; r };
        let e = act(&db, "activation_require", with_cert(change_certificate(&db, &target, &ABC[..1]))).unwrap_err();
        assert!(e.contains("(below_threshold)"), "{e}");
        let e = act(&db, "activation_require", with_cert(change_certificate(&db, &target, &[("replica-a", 1), ("replica-d", 4)]))).unwrap_err();
        assert!(e.contains("(unknown_key)"), "the new policy's keys cannot authorise their own admission: {e}");
        let mut elsewhere = change_certificate(&db, &target, &[]);
        elsewhere["resource"] = json!(OTHER_RESOURCE);
        let e = act(&db, "activation_require", with_cert(sign(&elsewhere, &ABC[..2]))).unwrap_err();
        assert!(e.contains("different resource"), "{e}");
        let e = act(&db, "activation_require", with_cert(change_certificate(&db, &"f".repeat(64), &ABC[..2]))).unwrap_err();
        assert!(e.contains("not to the policy declared here"), "{e}");
        let mut late = change_certificate(&db, &target, &[]);
        late["expires_at"] = json!(1_700_000_000);
        let e = act(&db, "activation_require", with_cert(sign(&late, &ABC[..2]))).unwrap_err();
        assert!(e.contains("expired"), "{e}");
        let e = act(&db, "activation_require", {
            let mut r = with_cert(change_certificate(&db, &target, &ABC[..2]));
            r["replaces_policy_digest"] = json!(first);
            r
        })
        .unwrap_err();
        assert!(e.contains("not both"), "{e}");
        assert_eq!(digest(&db), first, "no refused request changed the policy");
        let applied = change_certificate(&db, &target, &ABC[1..]);
        act(&db, "activation_require", with_cert(applied.clone())).unwrap();
        assert_eq!(digest(&db), target);
        assert_eq!(changes(&db).last().unwrap(), &(first.clone(), target.clone(), "certificate".to_string()));
        // The operator moves it back; the certificate applied once does not move it forward again.
        let mut back = quorum_policy(2, ABC);
        back["replaces_policy_digest"] = json!(target);
        act(&db, "activation_require", back).unwrap();
        let e = act(&db, "activation_require", with_cert(applied)).unwrap_err();
        assert!(e.contains("already applied"), "{e}");
        // Dropping the quorum: never by certificate, by the operator's re-declaration only.
        let lease_only = json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "replicas"});
        let e = act(&db, "activation_require", lease_only.clone()).unwrap_err();
        assert!(e.contains("requires a certificate"), "{e}");
        let mut drop = lease_only.clone();
        drop["policy_change_certificate"] = change_certificate(&db, "", &ABC[..2]);
        let e = act(&db, "activation_require", drop).unwrap_err();
        assert!(e.contains("dropping the key is the operator's"), "{e}");
        let mut drop = lease_only;
        drop["replaces_policy_digest"] = json!(first);
        act(&db, "activation_require", drop).unwrap();
        assert_eq!(policy(&db, R).unwrap().unwrap().authority_digest().unwrap(), None);
        assert_eq!(screen_of(&db), Some(7), "no change of policy moved the screen");
    }

    /// Proves: the migration's starting point stays open -- the gate's single key can certify the move
    /// to the replicas' quorum itself, in the certificate form under its 1-of-1 digest -- and that a
    /// change between single keys, or from no key, keeps today's behaviour: the operator's declaration,
    /// recorded, with nothing more asked.
    #[test]
    fn the_gate_can_certify_its_own_replacement_and_single_key_changes_keep_todays_behaviour() {
        let db = db();
        act(&db, "activation_require", json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "lab-gate"})).unwrap();
        act(&db, "activation_require", json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "lab-gate", "authority_key": public(GATE_SEED)})).unwrap();
        act(&db, "activation_require", json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "lab-gate", "authority_key": public(7)})).unwrap();
        act(&db, "activation_require", json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "lab-gate", "authority_key": public(GATE_SEED)})).unwrap();
        let hows: Vec<String> = changes(&db).into_iter().map(|(_, _, h)| h).collect();
        assert_eq!(hows, ["declared", "redeclared_single_key", "redeclared_single_key"]);
        let mut to_replicas = json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "lab-gate", "authority_quorum": testkit::policy(2, ABC)});
        let target = digest_of("lab-gate", &testkit::policy(2, ABC));
        to_replicas["policy_change_certificate"] = change_certificate(&db, &target, &[(crate::signing::SINGLE_KEY_ID, GATE_SEED)]);
        act(&db, "activation_require", to_replicas).unwrap();
        assert_eq!(digest(&db), target);
        assert_eq!(changes(&db).last().unwrap().2, "certificate");
        let e = act(&db, "activation_require", json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "lab-gate", "authority_key": public(GATE_SEED),
                                                       "authority_quorum": testkit::policy(2, ABC)})).unwrap_err();
        assert!(e.contains("name one"), "{e}");
        let e = act(&db, "activation_require", json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_quorum": testkit::policy(2, ABC)})).unwrap_err();
        assert!(e.contains("without authority_id"), "{e}");
    }
}
