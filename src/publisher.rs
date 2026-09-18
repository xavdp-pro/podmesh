//! The publishing connector that follows the active manager role (the operator's decision of
//! 2026-09-15, `CLOUDFLARE-TUNNEL-MANAGER-HA-DECISION`).
//!
//! The logical manager has one public hostname, one logical Cloudflare tunnel and, in this first
//! candidate, exactly one publishing `cloudflared`, co-located with the active manager replica and
//! governed by the same resource, epoch and fence. `cloudflared` is transport: it decides nothing.
//!
//! What this module does, all journaled and all recorded in the network effects ledger before
//! they are made, so that a crash leaves nothing unowned:
//!
//! - `publisher_declare`: the hostname, the tunnel UUID, the credential's secret name and the
//!   origin port, recorded by reference only -- no token in this journal, in Git or in an image.
//! - `publisher_start`: refused unless, in this order, the resource's lease is live and
//!   unsuperseded here (the epoch gate), the exclusive route and the alias are effective on this
//!   host, the previous publisher is accounted for (fenced, or the lease plus margin waited: the
//!   agent's word, recorded as provenance and refused when absent), the active manager's mark is
//!   written inside the carrier universe, and the origin answers ready at the service address with
//!   the expected logical manager, replica and epoch. Then the connector runs as a transient unit
//!   from a root-only runtime copy of the credential, and the unit's activity is verified. The
//!   takeover proof verified there is recorded; a later start at the same epoch may resume under it
//!   (`resume_same_epoch`) while the lease is the same incarnation, live, held here and unsuperseded,
//!   in the same boot -- the conditions under which a connector that never stopped continues.
//! - `publisher_stop`: the connector stopped and the mark removed, verified.
//! - at the daemon's start (`withdraw_at_startup`): a connector present without entitlement -- a
//!   lease that lapsed while the daemon was down -- is withdrawn, connector and mark, journaled.
//! - the fence (`withdraw_unentitled`): for a resource this host no longer holds, the connector
//!   is stopped and the mark removed BEFORE the alias and the route go -- one transition, each
//!   step recorded, each verified.
//! - `publisher_status`: what is declared, what the unit does, the connector's identity from its
//!   journal, the lease and epoch, the origin's readiness now, `publisher_eligible` with the
//!   refusal reasons, the last start, stop, fence and externally observed request.
//! - `publisher_observed`: the agent records an external request's result, as provenance.
//!
//! What it does not decide: which replica is the active manager (the gate does), whether the
//! hostname resolves (Cloudflare's), and the manager's web interface (the origin here is the
//! universe's epoch-qualified readiness responder).
use crate::lifecycle as lc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::io::{Read, Write};

type Error = Box<dyn std::error::Error>;

pub const KIND_PUBLISHER: &str = "publisher";
// The active manager's mark: its effect kind in the ledger and its path inside the carrier universe.
pub const KIND_MARK: &str = "active_manager_mark";
const MARK_PATH: &str = "/run/podmesh-manager/active-manager.json";
// The names both had before 2026-09-17. The hosts' journals keep rows of the previous kind, some
// left `applying` or `removing` by a crash: the same effect, matched wherever the kind is matched
// (`is_mark`) until no journal holds one, and never rewritten. The previous path is still removed
// at every write and every removal, until no running manager image reads it: the origin falls back
// to it when the current path is absent, so a file left there would keep a withdrawn replica marked.
const KIND_MARK_PREVIOUS: &str = "governor_mark";
const MARK_PATH_PREVIOUS: &str = "/run/podmesh-manager/governor.json";

/// Whether a ledger row's kind is the active manager's mark, under its name or the previous one.
pub(crate) fn is_mark(kind: &str) -> bool {
    kind == KIND_MARK || kind == KIND_MARK_PREVIOUS
}

pub fn ensure_schema(db: &Connection) -> Result<(), Error> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS publishers(
            resource TEXT PRIMARY KEY,
            hostname TEXT NOT NULL,
            tunnel_uuid TEXT NOT NULL,
            credential TEXT NOT NULL,
            origin_port INTEGER NOT NULL,
            declared_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL,
            authorization_ref TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS publisher_events(
            id INTEGER PRIMARY KEY,
            resource TEXT NOT NULL,
            event TEXT NOT NULL,
            at INTEGER NOT NULL,
            operation_id TEXT NOT NULL,
            detail TEXT);
         CREATE TABLE IF NOT EXISTS publisher_transitions(
            resource TEXT PRIMARY KEY,
            state TEXT NOT NULL,
            epoch INTEGER NOT NULL,
            operation_id TEXT NOT NULL,
            changed_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS publisher_takeover_verified(
            resource TEXT NOT NULL,
            epoch INTEGER NOT NULL,
            generation INTEGER NOT NULL,
            acquired_at INTEGER NOT NULL,
            boot_id TEXT NOT NULL,
            authority_id TEXT NOT NULL,
            authority_key TEXT NOT NULL,
            proof TEXT NOT NULL,
            verified TEXT NOT NULL,
            verified_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL,
            PRIMARY KEY(resource, epoch));",
    )?;
    Ok(())
}

/// The publisher's own durable transition (Codex, P1): `starting` written before any effect,
/// `effective` only once the mark, the connector and its registration are verified, `stopping`
/// before a withdrawal, gone after it. Reconciliation withdraws whatever is `starting` or
/// `stopping` after a crash, and withdraws an `effective` publisher whose lease this host no
/// longer holds; a publisher from an operation reported failed never stays active.
fn transition(db: &Connection, resource: &str, state: &str, epoch: i64, id: &str) -> Result<(), Error> {
    db.execute(
        "INSERT INTO publisher_transitions(resource,state,epoch,operation_id,changed_at) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(resource) DO UPDATE SET state=excluded.state, epoch=excluded.epoch, operation_id=excluded.operation_id, changed_at=excluded.changed_at",
        params![resource, state, epoch, id, crate::now() as i64],
    )?;
    Ok(())
}

fn transition_of(db: &Connection, resource: &str) -> Result<Option<(String, i64)>, Error> {
    Ok(db.query_row("SELECT state,epoch FROM publisher_transitions WHERE resource=?1", [resource], |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
}

/// Called by the network reconciliation: every publisher not verified effective under a lease
/// this host holds is withdrawn -- connector, mark, transition -- and reported.
pub(crate) fn reconcile(db: &Connection) -> Result<Vec<Value>, Error> {
    ensure_schema(db)?;
    let mut s = db.prepare("SELECT resource,state,epoch FROM publisher_transitions")?;
    let rows: Vec<(String, String, i64)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    let mut report = vec![];
    let this_host = crate::activation::host_uuid_public(db)?;
    let now = crate::now() as i64;
    for (resource, state, epoch) in rows {
        let entitled = crate::activation::lease(db, &resource)?.is_some_and(|l| l.holder_host_uuid == this_host && l.expires_at > now && l.epoch == epoch)
            && crate::activation::refuse_if_not_activated(db, &resource, "reconcile").is_ok();
        let intact = connector_present(&resource) == Some(true) && connector_id(&resource).is_some();
        let why = if state != "effective" { format!("transition {state}") } else if !entitled { "no longer entitled".into() } else if !intact { "connector not intact".into() } else { continue };
        let r = withdraw(db, &resource, "reconcile", "reconciliation")?;
        report.push(json!({"resource": resource, "was": state, "epoch": epoch, "why": why, "withdrawn": r["withdrawn"], "steps": r["steps"]}));
    }
    Ok(report)
}

/// The connector's registration with Cloudflare, waited for a bounded time from its journal: a
/// unit that is active but never registers, or exits, is not a publication (Codex, P1).
fn wait_registered(resource: &str, seconds: u64) -> Result<String, Error> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    loop {
        let state = unit_state(resource).unwrap_or("unknown".into());
        if state != "active" && state != "activating" {
            return Err(format!("the connector unit is {state} before registering").into());
        }
        if let Some(id) = connector_id(resource) {
            if state == "active" {
                return Ok(id);
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("the connector did not register with Cloudflare within {seconds} seconds (unit {state})").into());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// The takeover proof (Codex, P0): a typed document from the external authority that advanced
/// the epoch, bound to the resource, both epochs, both holders and a method -- `first` (no
/// publisher epoch ever existed), `same_holder` (this host held the previous epoch too),
/// `fence_receipt` (the previous holder's fence, its receipt bound to the transition) or
/// `lease_barrier` (an unreachable previous holder: the authority's `eligible_after`, the
/// previous lease plus the margin, compared with this host's clock). Ed25519-signed by the
/// authority and verified here when the policy names its key (`src/signing.rs`); a laboratory
/// proof, unsigned and labelled so, only under a policy that names none. The agent's
/// `previous` narrative is kept beside it and decides nothing.
fn verify_takeover_proof(db: &Connection, resource: &str, proof: &Value, epoch: i64, this_host: &str) -> Result<Value, Error> {
    let now = crate::now() as i64;
    let policy = crate::activation::policy(db, resource)?.ok_or("no activation policy")?;
    let text = |k: &str| proof[k].as_str().map(str::to_string).ok_or_else(|| format!("takeover_proof lacks {k}"));
    // Origin first, then binding: under a policy that names the authority's key, the document
    // must be the signed kind and its signature must verify over its canonical form (altered,
    // unknown-key and unsigned documents are refused here, before any field is read); under a
    // policy without a key, only the laboratory kind is accepted, on its binding alone.
    let signed = !policy.authority_key.is_empty();
    let kind = text("kind")?;
    let origin = if signed {
        if kind != crate::signing::SIGNED_PROOF_KIND {
            return Err(format!("takeover_proof is {kind}; this resource's policy names the authority's key and requires {}", crate::signing::SIGNED_PROOF_KIND).into());
        }
        crate::signing::verify(proof, &policy.authority_key).map_err(|e| format!("takeover_proof: {e}"))?;
        "signed by the authority's key named in the policy; the signature verified over the document's canonical form"
    } else {
        if kind != crate::signing::UNSIGNED_PROOF_KIND {
            return Err(format!("takeover_proof is {kind}; this resource's policy names no authority key, and only a laboratory proof is accepted without one").into());
        }
        "laboratory proof, unsigned: its binding is checked, its origin is not"
    };
    if text("authority_id")? != policy.authority_id {
        return Err("takeover_proof names another authority than this resource's policy".into());
    }
    if text("resource")? != resource {
        return Err("takeover_proof is bound to another resource".into());
    }
    if text("new_holder")? != this_host {
        return Err("takeover_proof names another host as the new holder".into());
    }
    let new_epoch = proof["new_epoch"].as_i64().ok_or("takeover_proof lacks new_epoch")?;
    let previous_epoch = proof["previous_epoch"].as_i64().ok_or("takeover_proof lacks previous_epoch")?;
    if new_epoch != epoch {
        return Err(format!("takeover_proof is for epoch {new_epoch}, this host's lease is under epoch {epoch}").into());
    }
    if previous_epoch != epoch - 1 {
        return Err(format!("takeover_proof names previous epoch {previous_epoch}, not the one before {epoch}").into());
    }
    let issued = proof["issued_at"].as_i64().ok_or("takeover_proof lacks issued_at")?;
    let expires = proof["expires_at"].as_i64().ok_or("takeover_proof lacks expires_at")?;
    if issued > now + 30 {
        return Err("takeover_proof is issued in the future beyond the clock allowance".into());
    }
    if expires < now {
        return Err(format!("takeover_proof expired {} seconds ago", now - expires).into());
    }
    let previous_holder = proof["previous_holder"].as_str();
    match text("method")?.as_str() {
        "first" => {
            if previous_epoch != 0 || previous_holder.is_some() {
                return Err("takeover_proof says first, but a previous epoch or holder exists".into());
            }
        }
        "same_holder" => {
            if previous_holder != Some(this_host) {
                return Err("takeover_proof says same_holder, but the previous holder is not this host".into());
            }
        }
        "fence_receipt" => {
            let receipt = &proof["receipt"];
            let host = receipt["host"].as_str().ok_or("the fence receipt names no host")?;
            if Some(host) != previous_holder {
                return Err("the fence receipt is from a host that is not the previous holder".into());
            }
            if receipt["withdrawn"] != json!(true) || receipt["operation_id"].as_str().is_none() {
                return Err("the fence receipt does not record a verified withdrawal".into());
            }
            if receipt["resource"].as_str() != Some(resource) {
                return Err("the fence receipt is for another resource".into());
            }
        }
        "lease_barrier" => {
            let eligible = proof["eligible_after"].as_i64().ok_or("takeover_proof lacks eligible_after")?;
            if now < eligible {
                return Err(format!("takeover_proof: the authority's barrier is at {eligible}, {} seconds from now on this clock; refusing before it", eligible - now).into());
            }
        }
        other => return Err(format!("takeover_proof method {other} is unknown").into()),
    }
    Ok(json!({"method": proof["method"], "previous_epoch": previous_epoch, "new_epoch": new_epoch, "previous_holder": previous_holder,
              "verified_at": now, "signed": signed, "note": origin}))
}

/// A takeover proof this host verified, with the lease and the boot it was verified under: what a
/// later start at the same epoch may resume under, and nothing else (V3-1).
#[derive(Clone, Debug)]
pub(crate) struct Verified {
    epoch: i64,
    generation: i64,
    acquired_at: i64,
    boot_id: String,
    authority_id: String,
    authority_key: String,
    proof: Value,
    verified: Value,
    verified_at: i64,
    operation_id: String,
}

/// Kept at every verification, before any effect of the start: the document, what the verification
/// said, and the incarnation of the lease it was verified against. One row per resource and epoch; a
/// later verification at the same epoch (after a retake, say) replaces it.
#[allow(clippy::too_many_arguments)]
fn record_verified(db: &Connection, resource: &str, l: &crate::activation::Lease, boot: &str, policy: &crate::activation::Policy, proof: &Value, verified: &Value, id: &str) -> Result<(), Error> {
    db.execute(
        "INSERT INTO publisher_takeover_verified VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(resource,epoch) DO UPDATE SET generation=excluded.generation, acquired_at=excluded.acquired_at, boot_id=excluded.boot_id,
           authority_id=excluded.authority_id, authority_key=excluded.authority_key, proof=excluded.proof, verified=excluded.verified,
           verified_at=excluded.verified_at, operation_id=excluded.operation_id",
        params![resource, l.epoch, l.generation, l.acquired_at, boot, policy.authority_id, policy.authority_key, proof.to_string(), verified.to_string(),
                crate::now() as i64, id],
    )?;
    Ok(())
}

/// The latest proof this host verified for the resource, at the highest epoch it verified one.
fn last_verified(db: &Connection, resource: &str) -> Result<Option<Verified>, Error> {
    Ok(db
        .query_row(
            "SELECT epoch,generation,acquired_at,boot_id,authority_id,authority_key,proof,verified,verified_at,operation_id
             FROM publisher_takeover_verified WHERE resource=?1 ORDER BY epoch DESC LIMIT 1",
            [resource],
            |r| {
                let json = |s: String| serde_json::from_str::<Value>(&s).unwrap_or(Value::Null);
                Ok(Verified { epoch: r.get(0)?, generation: r.get(1)?, acquired_at: r.get(2)?, boot_id: r.get(3)?, authority_id: r.get(4)?,
                              authority_key: r.get(5)?, proof: json(r.get(6)?), verified: json(r.get(7)?), verified_at: r.get(8)?, operation_id: r.get(9)? })
            },
        )
        .optional()?)
}

/// Everything the same-epoch resume is judged on, read from this host's journal and kernel before
/// anything is decided.
pub(crate) struct ResumeFacts {
    recorded: Option<Verified>,
    lease: Option<crate::activation::Lease>,
    this_host: String,
    now: i64,
    superseded_by: Option<i64>,
    boot_id: String,
    /// The resource's policy now: its authority and that authority's key.
    authority: Option<(String, String)>,
}

fn resume_facts(db: &Connection, resource: &str, this_host: &str, boot: &str) -> Result<ResumeFacts, Error> {
    let lease = crate::activation::lease(db, resource)?;
    let superseded_by = match &lease { Some(l) => crate::activation::superseded_by(db, resource, l)?, None => None };
    Ok(ResumeFacts {
        recorded: last_verified(db, resource)?,
        lease,
        this_host: this_host.into(),
        now: crate::now() as i64,
        superseded_by,
        boot_id: boot.into(),
        authority: crate::activation::policy(db, resource)?.map(|p| (p.authority_id, p.authority_key)),
    })
}

/// THE SAME-EPOCH RESUME (V3-1). The takeover proof attests one past fact: the transition into this
/// epoch accounted for the previous holder. The gate stamps it with an hour's life whatever the lease,
/// while the lease it started is renewed without any proof; a connector that died after that hour
/// could only come back through a new rotation. A start may instead resume under the proof this host
/// already verified and recorded, when every condition below holds -- and each is one under which a
/// connector that never stopped is ALREADY allowed to continue (the reconciliation's entitlement, the
/// follow tick's eligibility), so a restart under them grants nothing that continuing did not.
/// Anything else needs a valid new proof. Returns the verification resumed from, or the named refusal.
fn resume_refusal(f: &ResumeFacts) -> Result<&Verified, (&'static str, String)> {
    // Live, held here, unsuperseded: the lease gate's four reasons, the very predicate under which the
    // reconciliation leaves a running connector alone and the fence does not withdraw it.
    let Some(l) = &f.lease else { return Err(("no_lease", "this host holds no activation lease for the resource".into())) };
    if l.holder_host_uuid != f.this_host {
        return Err(("lease_held_elsewhere", "the activation lease is held by another host".into()));
    }
    if l.expires_at <= f.now {
        return Err(("lease_expired", format!("this host's activation lease expired {} seconds ago", f.now - l.expires_at)));
    }
    if let Some(seen) = f.superseded_by {
        return Err(("lease_superseded", format!("this host's activation was superseded by epoch {seen}")));
    }
    let Some(r) = &f.recorded else {
        return Err(("no_verified_proof", "this host never verified a takeover proof for this resource".into()));
    };
    // Same epoch: the proof accounts for the transition INTO this epoch. A newer epoch is another
    // transition, with another previous holder to account for; a running connector under an older
    // epoch is withdrawn by the reconciliation, which compares the transition's epoch with the lease's.
    if r.epoch != l.epoch {
        return Err(("epoch_changed", format!("the proof this host verified is for epoch {}, the lease is under epoch {}", r.epoch, l.epoch)));
    }
    // Same generation and same acquisition: the lease is the very incarnation the proof was verified
    // against, carried forward by renewals only. A lapsed lease of this host's retaken keeps its
    // generation and epoch but not its `acquired_at`; across the lapse a running connector is withdrawn
    // (not live, so not entitled), so resuming after a retake would grant what continuing never did.
    if r.generation != l.generation {
        return Err(("generation_changed", format!("the proof was verified under lease generation {}, the lease is now generation {}", r.generation, l.generation)));
    }
    if r.acquired_at != l.acquired_at {
        return Err(("lease_reacquired", format!("the lease was acquired again at {} after the proof was verified under the acquisition of {}", l.acquired_at, r.acquired_at)));
    }
    // Same boot: a connector never outlives its host's boot (a transient unit, a runtime credential), and
    // after a boot the entitlement is decided again, never assumed -- the rule of boot_restore and of
    // the permit, which is bound to the boot.
    if r.boot_id != f.boot_id {
        return Err(("boot_changed", "the proof was verified during another boot of this host; after a boot the entitlement is decided again".into()));
    }
    // Same authority and key: the proof's origin was checked under the policy as it stood; a policy
    // naming another authority or another key would have refused that very document.
    match &f.authority {
        Some((id, key)) if *id == r.authority_id && *key == r.authority_key => {}
        _ => return Err(("authority_changed", "the resource's policy no longer names the authority and key the proof was verified under".into())),
    }
    Ok(r)
}

/// Step 4 of a start: the takeover. A presented proof that verifies is recorded and used; otherwise
/// (none presented, or one refused -- expired, say) the start resumes under the recorded one when the
/// resume's conditions hold, and the resume is journaled as such with the original proof's identity.
fn takeover(db: &Connection, resource: &str, request: &Value, epoch: i64, this_host: &str, boot: &str, id: &str) -> Result<Value, Error> {
    let mut refused = None;
    if let Some(proof) = request.get("takeover_proof") {
        match verify_takeover_proof(db, resource, proof, epoch, this_host) {
            Ok(verified) => {
                let l = crate::activation::lease(db, resource)?.ok_or("no activation lease")?;
                let policy = crate::activation::policy(db, resource)?.ok_or("no activation policy")?;
                record_verified(db, resource, &l, boot, &policy, proof, &verified, id)?;
                return Ok(verified);
            }
            Err(e) => refused = Some(e.to_string()),
        }
    }
    let facts = resume_facts(db, resource, this_host, boot)?;
    match resume_refusal(&facts) {
        Ok(r) => {
            let resumed = json!({
                "method": "resume_same_epoch", "new_epoch": epoch, "verified_at": facts.now, "signed": r.verified["signed"],
                "resumed_from": {"operation_id": r.operation_id, "verified_at": r.verified_at, "method": r.verified["method"],
                                 "previous_epoch": r.verified["previous_epoch"], "previous_holder": r.verified["previous_holder"],
                                 "issued_at": r.proof["issued_at"], "expires_at": r.proof["expires_at"], "signature": r.proof["signature"]},
                "presented_proof_refused": refused,
                "note": "no new proof: the lease is the same incarnation, live, held here and unsuperseded, in the same boot, under the same authority and key, as when this host verified the proof for this epoch",
            });
            event(db, resource, "takeover_resumed", id, Some(resumed.clone()))?;
            Ok(resumed)
        }
        Err((code, why)) => Err(match refused {
            Some(p) => format!("takeover_proof refused: {p}; and no same-epoch resume ({code}): {why}"),
            None => format!("publisher_start requires `takeover_proof`, the authority's document for this epoch (the tool's rotate prints it; attest-fence upgrades it), unless it resumes under the proof this host already verified for this epoch; no same-epoch resume ({code}): {why}"),
        }
        .into()),
    }
}


pub(crate) struct Publisher {
    resource: String,
    hostname: String,
    tunnel_uuid: String,
    credential: String,
    origin_port: u16,
}

fn declared(db: &Connection, resource: &str) -> Result<Option<Publisher>, Error> {
    ensure_schema(db)?;
    Ok(db
        .query_row("SELECT resource,hostname,tunnel_uuid,credential,origin_port FROM publishers WHERE resource=?1", [resource], |r| {
            Ok(Publisher { resource: r.get(0)?, hostname: r.get(1)?, tunnel_uuid: r.get(2)?, credential: r.get(3)?, origin_port: r.get::<_, i64>(4)? as u16 })
        })
        .optional()?)
}

fn event(db: &Connection, resource: &str, what: &str, id: &str, detail: Option<Value>) -> Result<(), Error> {
    db.execute(
        "INSERT INTO publisher_events(resource,event,at,operation_id,detail) VALUES(?1,?2,?3,?4,?5)",
        params![resource, what, crate::now() as i64, id, detail.map(|d| d.to_string())],
    )?;
    Ok(())
}

fn last_event(db: &Connection, resource: &str, what: &str) -> Result<Option<Value>, Error> {
    Ok(db
        .query_row("SELECT at,operation_id,detail FROM publisher_events WHERE resource=?1 AND event=?2 ORDER BY id DESC LIMIT 1", [resource, what], |r| {
            Ok(json!({"at": r.get::<_, i64>(0)?, "operation_id": r.get::<_, String>(1)?,
                      "detail": r.get::<_, Option<String>>(2)?.and_then(|s| serde_json::from_str::<Value>(&s).ok())}))
        })
        .optional()?)
}

pub fn unit_name(resource: &str) -> String {
    format!("podmesh-publisher-{resource}.service")
}

fn runtime_dir(resource: &str) -> std::path::PathBuf {
    let socket = std::env::var("PODMESH_SOCKET").unwrap_or("/run/podmesh/api.sock".into());
    std::path::Path::new(&socket).parent().unwrap_or(std::path::Path::new("/run/podmesh")).join("publisher").join(resource)
}

/// The unit's state from systemd: active, inactive, failed, activating..., or None if unknown.
pub(crate) fn unit_state(resource: &str) -> Option<String> {
    let out = std::process::Command::new("systemctl").args(["show", "-p", "ActiveState", "--value", &unit_name(resource)]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The unit's current run, as systemd numbers it (`InvocationID`): None when the unit is not loaded,
/// which for a transient unit collected after its stop means no run at all; an error when systemd
/// could not be asked.
fn invocation(resource: &str) -> Result<Option<String>, String> {
    let out = std::process::Command::new("systemctl").args(["show", "-p", "InvocationID", "--value", &unit_name(resource)]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("systemctl show: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if id.is_empty() {
        return Ok(None);
    }
    if !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("systemd names the run {id:?}, not an invocation identifier"));
    }
    Ok(Some(id))
}

/// The registration cloudflared logged, from lines of its journal: `connection=<id>` on the last
/// registration line.
fn registration(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .filter(|l| l.contains("Registered tunnel connection"))
        .find_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix("connection=").map(str::to_string)))
}

/// The connector identity cloudflared logged when it registered with Cloudflare, from the journal of
/// the unit's CURRENT run only: the unit's name carries the lines of every earlier run too (a
/// connector id was read on a host whose unit was inactive, 2026-09-18), and a registration read from
/// one of those would pass a connector that never registered for a live one. The registration lines
/// are asked for by pattern first, so that a long run does not push them out of a bounded tail; a
/// journal without pattern support is read from its last lines instead. `Ok(None)` when the run's
/// journal was read and holds no registration, or there is no run; an error when it could not be read
/// -- which is not the same answer, and a caller deciding to stop a connector must tell them apart.
fn registration_read(resource: &str) -> Result<Option<String>, String> {
    let Some(run) = invocation(resource)? else { return Ok(None) };
    let field = format!("_SYSTEMD_INVOCATION_ID={run}");
    let journal = |extra: &[&str]| std::process::Command::new("journalctl").arg(&field).args(["--no-pager", "-o", "cat"]).args(extra).output().map_err(|e| e.to_string());
    // With a pattern, no match exits 1 with nothing printed: not an error, the tail below decides.
    if let Some(id) = registration(&String::from_utf8_lossy(&journal(&["-g", "Registered tunnel connection", "-n", "50"])?.stdout)) {
        return Ok(Some(id));
    }
    let out = journal(&["-n", "2000"])?;
    if !out.status.success() {
        return Err(format!("journalctl: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(registration(&String::from_utf8_lossy(&out.stdout)))
}

/// The registration of the current run, for the callers that treat "could not be read" as "not
/// registered": the start's wait and the reconciliation's integrity check, which withdraw nothing
/// that an unread journal hides -- the start fails closed and retries, the reconciliation leaves an
/// effective publisher alone only on a registration it read.
fn connector_id(resource: &str) -> Option<String> {
    registration_read(resource).ok().flatten()
}

/// The route and alias the resource holds on this host, from the network tables, verified from
/// the kernel and the universe's namespace: the service address and the carrier universe.
fn service_here(db: &Connection, resource: &str) -> Result<Option<(String, String)>, Error> {
    let row: Option<(String, Option<String>, Option<String>)> = db
        .query_row("SELECT ip,alias_universe_uuid,state FROM network_routes WHERE exclusive_resource=?1", [resource], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let Some((ip, alias, state)) = row else { return Ok(None) };
    if state.as_deref().unwrap_or("effective") != "effective" {
        return Ok(None);
    }
    let Some(carrier) = alias else { return Ok(None) };
    let status = crate::network::execute(db, &json!({"operation": "network_status"}))?;
    let effective = status["effective"]["published_routes"]
        .as_array()
        .map(|rs| rs.iter().any(|r| r["ip"] == json!(ip) && r["effective"] == json!(true) && r["alias_effective"] == json!(true)))
        .unwrap_or(false);
    Ok(if effective { Some((ip, carrier)) } else { None })
}

/// One HTTP GET of the origin's readiness, by hand over a TCP stream: the status and the JSON
/// body, or the reason it could not be read.
fn origin_ready(ip: &str, port: u16) -> Result<(u16, Value), String> {
    let mut stream = std::net::TcpStream::connect_timeout(&format!("{ip}:{port}").parse().map_err(|e| format!("{e}"))?, std::time::Duration::from_secs(4))
        .map_err(|e| format!("connect {ip}:{port}: {e}"))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(4))).ok();
    stream.write_all(format!("GET /ready HTTP/1.0\r\nHost: {ip}\r\nConnection: close\r\n\r\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    stream.take(65536).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&raw);
    let status: u16 = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).ok_or("no HTTP status line")?;
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    Ok((status, serde_json::from_str(body).unwrap_or(json!({"raw": body}))))
}

/// The active manager's mark inside the carrier universe: written and removed with
/// `podman exec`, root-only. A write removes the previous path too.
pub(crate) fn mark_write(carrier: &str, resource: &str, epoch: i64) -> Result<(), Error> {
    // Lab-only: a mark one epoch behind, so that the readiness check has a lie to catch.
    let epoch = if std::env::var("PODMESH_FAULT").as_deref() == Ok("publisher-stale-mark") { epoch - 1 } else { epoch };
    let content = json!({"resource": resource, "epoch": epoch, "marked_at": crate::now()}).to_string();
    lc::podman(
        lc::QUICK,
        &["exec", &format!("podmesh-{carrier}"), "sh", "-c", &format!("umask 077; printf '%s' '{content}' > {MARK_PATH}.tmp && mv -f {MARK_PATH}.tmp {MARK_PATH} && rm -f {MARK_PATH_PREVIOUS}")],
    )?;
    Ok(())
}

/// Which of the mark's paths hold a file inside the carrier, the current and the previous, read
/// with one `podman exec`; `None` when that could not be read.
fn mark_files(carrier: &str) -> Option<(bool, bool)> {
    let name = format!("podmesh-{carrier}");
    let script = format!("for p in {MARK_PATH} {MARK_PATH_PREVIOUS}; do if test -f $p; then echo present; else echo absent; fi; done");
    let out = std::process::Command::new("podman").args(["exec", &name, "sh", "-c", &script]).output().ok()?;
    if out.status.success() {
        let text = String::from_utf8_lossy(&out.stdout);
        let seen: Vec<&str> = text.split_whitespace().collect();
        return match seen[..] {
            [current, previous] => Some((current == "present", previous == "present")),
            _ => None,
        };
    }
    // A universe that is not running carries no mark; exec on it fails for that reason. One that
    // runs and could not be read is unknown.
    match lc::inspect(&name) {
        Ok(Some(c)) if c["State"]["Running"] == json!(true) => None,
        Ok(_) => Some((false, false)),
        Err(_) => None,
    }
}

/// Whether a mark is there at either path, a file the origin would read: what the status reports,
/// what drift is judged on, and what a removal must leave false.
pub(crate) fn mark_present(carrier: &str) -> Option<bool> {
    mark_files(carrier).map(|(current, previous)| current || previous)
}

/// Whether the mark is as a write leaves it: at the current path, and nothing at the previous one.
pub(crate) fn mark_written(carrier: &str) -> Option<bool> {
    mark_files(carrier).map(|(current, previous)| current && !previous)
}

/// The mark inside the carrier as the origin reads it, with one `podman exec`: the current path, else
/// the previous one. `Ok(Some(epoch))` for a mark and the epoch it names, `Ok(None)` when neither path
/// holds a file -- both read, both positive answers -- and an error when it could not be read (the exec
/// failed, the file is not a mark): unknown, which a caller must not take for a wrong mark.
pub(crate) fn mark_read(carrier: &str) -> Result<Option<i64>, String> {
    let script = format!(
        "if test -f {MARK_PATH}; then cat {MARK_PATH}; elif test -f {MARK_PATH_PREVIOUS}; then cat {MARK_PATH_PREVIOUS}; else echo absent; fi"
    );
    let out = std::process::Command::new("podman").args(["exec", &format!("podmesh-{carrier}"), "sh", "-c", &script]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("podman exec: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    mark_answer(&String::from_utf8_lossy(&out.stdout))
}

/// What the carrier's answer says: `absent`, or a mark's JSON and its epoch.
fn mark_answer(text: &str) -> Result<Option<i64>, String> {
    if text.trim() == "absent" {
        return Ok(None);
    }
    serde_json::from_str::<Value>(text.trim()).ok().and_then(|v| v["epoch"].as_i64()).map(Some).ok_or_else(|| format!("the mark could not be read as one: {:?}", text.chars().take(80).collect::<String>()))
}

/// The origin's answer against what the start requires -- 200, ready, this logical manager, the
/// lease's epoch, the carrier's replica. `Some(true)` when all hold; `Some(false)` when the origin
/// answered and one of them does not (a positive mismatch); `None` when it could not be told: the
/// origin could not be asked, or it answered as required but the carrier's replica is not known.
fn origin_verdict(readiness: &Value, epoch: i64, resource: &str, replica: Option<&Value>) -> Option<bool> {
    if readiness.get("error").is_some() {
        return None;
    }
    let body = &readiness["body"];
    let answered = readiness["status"] == json!(200) && body["ready"] == json!(true) && body["epoch"] == json!(epoch) && body["logical_manager_id"] == json!(resource);
    if !answered {
        return Some(false);
    }
    match replica {
        Some(r) if !r.is_null() => Some(body["replica_id"] == *r),
        _ => None,
    }
}

pub(crate) fn mark_remove(carrier: &str) -> Result<(), Error> {
    if mark_present(carrier) == Some(true) {
        lc::podman(lc::QUICK, &["exec", &format!("podmesh-{carrier}"), "rm", "-f", MARK_PATH, MARK_PATH_PREVIOUS])?;
    }
    Ok(())
}

/// The connector as a transient unit: the credential taken from Podman's store into a root-only
/// runtime file (tmpfs), the ingress written beside it, the unit started and verified active.
pub(crate) fn connector_start(p: &Publisher, ip: &str) -> Result<(), Error> {
    let dir = runtime_dir(&p.resource);
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    let out = std::process::Command::new("podman").args(["secret", "inspect", "--showsecret", "--format", "{{.SecretData}}", &p.credential]).output()?;
    if !out.status.success() {
        return Err(format!("the credential {} could not be read from Podman's store: {}", p.credential, String::from_utf8_lossy(&out.stderr).trim()).into());
    }
    let credentials = dir.join("credentials.json");
    crate::migration::write_private(&credentials, String::from_utf8_lossy(&out.stdout).trim().as_bytes())?;
    let config = dir.join("config.yml");
    crate::migration::write_private(
        &config,
        format!("tunnel: {}\ncredentials-file: {}\nno-autoupdate: true\ningress:\n  - hostname: {}\n    service: http://{ip}:{}\n  - service: http_status:503\n", p.tunnel_uuid, credentials.display(), p.hostname, p.origin_port).as_bytes(),
    )?;
    let unit = unit_name(&p.resource);
    let out = std::process::Command::new("systemd-run")
        .args(["--quiet", "--unit", unit.trim_end_matches(".service"), "--property=Restart=no", "--collect", "/usr/local/bin/cloudflared", "--no-autoupdate", "tunnel", "--config"])
        .arg(&config)
        .args(["run", &p.tunnel_uuid])
        .output()?;
    if !out.status.success() {
        return Err(format!("the connector unit could not start: {}", String::from_utf8_lossy(&out.stderr).trim()).into());
    }
    Ok(())
}

/// The connector from a ledger intent, for reconciliation's apply (a start interrupted before
/// its verification is undone, never finished; this path serves `effect_do` only).
pub(crate) fn connector_start_by_intent(intent: &Value) -> Result<(), Error> {
    let resource = intent["resource"].as_str().ok_or("intent without resource")?;
    let ip = intent["ip"].as_str().ok_or("intent without ip")?;
    let db = crate::open_state(&std::path::PathBuf::from(std::env::var("PODMESH_STATE_DIR").unwrap_or("/var/lib/podmesh".into())))?;
    let p = declared(&db, resource)?.ok_or("no publisher declared")?;
    connector_start(&p, ip)
}

pub(crate) fn connector_stop(resource: &str) -> Result<(), Error> {
    let unit = unit_name(resource);
    if unit_state(resource).as_deref() == Some("active") || unit_state(resource).as_deref() == Some("activating") {
        let out = std::process::Command::new("systemctl").args(["stop", &unit]).output()?;
        if !out.status.success() {
            return Err(format!("systemctl stop {unit}: {}", String::from_utf8_lossy(&out.stderr).trim()).into());
        }
    }
    let _ = std::process::Command::new("systemctl").args(["reset-failed", &unit]).output();
    let dir = runtime_dir(resource);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

pub(crate) fn connector_present(resource: &str) -> Option<bool> {
    unit_state(resource).map(|s| s == "active" || s == "activating")
}

/// The gates of a start, evaluated without effect: what `publisher_status` reports as
/// `publisher_eligible`, and what `publisher_start` refuses on.
/// eligible, the refusal reasons, the service (ip, carrier) if effective, the epoch if entitled, and
/// each gate by name -- `lease`, `policy`, `service_address`, `credential` -- true when it passes, so
/// that a caller can tell "eligible but for the service address" from the rest without parsing text.
type Eligibility = (bool, Vec<String>, Option<(String, String)>, Option<i64>, Value);

fn eligibility(db: &Connection, p: &Publisher) -> Result<Eligibility, Error> {
    let mut reasons = vec![];
    let mut epoch = None;
    let lease_gate = match crate::activation::refuse_if_not_activated(db, &p.resource, "publisher_start") {
        Ok(()) => {
            epoch = crate::activation::lease(db, &p.resource)?.map(|l| l.epoch);
            true
        }
        Err(e) => {
            reasons.push(e.to_string());
            false
        }
    };
    let policy_gate = crate::activation::policy(db, &p.resource)?.is_some();
    if !policy_gate {
        reasons.push(format!("{} is under no activation policy on this host", p.resource));
    }
    let service = service_here(db, &p.resource)?;
    if service.is_none() {
        reasons.push("no effective exclusive route and alias for the resource on this host: publish the service address first".into());
    }
    let credential_gate = match crate::secrets::declared(db, &p.credential) {
        Ok(()) => true,
        Err(e) => {
            reasons.push(e.to_string());
            false
        }
    };
    let gates = json!({"lease": lease_gate, "policy": policy_gate, "service_address": service.is_some(), "credential": credential_gate});
    Ok((reasons.is_empty(), reasons, service, epoch, gates))
}

pub fn execute(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    let resource = lc::text(request, "resource")?;
    lc::token(resource)?;
    lc::ensure_schema(db)?;
    ensure_schema(db)?;
    if operation == "publisher_status" {
        return view(db, resource);
    }
    lc::journaled(db, request, |db| {
        let reconciliation = crate::network::reconcile(db)?;
        if reconciliation["remaining"].as_array().is_some_and(|r| !r.is_empty()) {
            return Err(format!("incomplete network effects remain; refusing: {}", reconciliation["remaining"]).into());
        }
        let mut answer = perform(db, request, resource)?;
        answer["reconciliation_before"] = reconciliation;
        Ok(answer)
    })
}

fn view(db: &Connection, resource: &str) -> Result<Value, Error> {
    let Some(p) = declared(db, resource)? else {
        return Ok(json!({"resource": resource, "declared": false, "publisher_eligible": false, "reasons": ["no publisher declared for this resource on this host"]}));
    };
    let (eligible, reasons, service, epoch, gates) = eligibility(db, &p)?;
    let lease = crate::activation::lease(db, resource)?;
    let readiness = service.as_ref().map(|(ip, _)| match origin_ready(ip, p.origin_port) {
        Ok((status, body)) => json!({"status": status, "body": body}),
        Err(e) => json!({"error": e}),
    });
    let carrier_identity = service.as_ref().and_then(|(_, carrier)| {
        crate::manager::execute(db, &json!({"operation": "manager_status", "universe_uuid": carrier})).ok().map(|s| s["resident_status"]["replica_id"].clone())
    });
    let mark = service.as_ref().and_then(|(_, carrier)| mark_present(carrier));
    // What a follow tick compares on a running connector, each with "could not be told" (null, or
    // `unknown`) kept apart from a wrong value: the tick stops a connector on a positive mismatch at
    // once, and on unknowns only when they persist (review of V3-1).
    let mark_now = service.as_ref().map(|(_, carrier)| mark_read(carrier));
    let (mark_epoch, mark_state) = match &mark_now {
        Some(Ok(Some(e))) => (json!(e), "present"),
        Some(Ok(None)) => (Value::Null, "absent"),
        Some(Err(_)) | None => (Value::Null, "unknown"),
    };
    let origin_at_epoch = match (&readiness, epoch) {
        (Some(r), Some(e)) => json!(origin_verdict(r, e, resource, carrier_identity.as_ref())),
        _ => Value::Null,
    };
    let registered = registration_read(resource);
    let connector = registered.as_ref().ok().cloned().flatten();
    let this_host = crate::activation::host_uuid_public(db)?;
    let resume = match crate::activation::boot_id() {
        Ok(boot) => match resume_refusal(&resume_facts(db, resource, &this_host, &boot)?) {
            Ok(r) => json!({"possible": true, "verified_epoch": r.epoch, "verified_by": r.operation_id}),
            Err((code, why)) => json!({"possible": false, "refusal": code, "why": why}),
        },
        Err(e) => json!({"possible": false, "refusal": "boot_unknown", "why": e.to_string()}),
    };
    Ok(json!({
        "resource": resource,
        "declared": {"hostname": p.hostname, "tunnel_uuid": p.tunnel_uuid, "credential": p.credential, "origin_port": p.origin_port},
        "unit": {"name": unit_name(resource), "state": unit_state(resource), "invocation_id": invocation(resource).ok().flatten()},
        "connector_id": connector,
        // The registration of the unit's current run only; an earlier run's lines are not read. Null
        // when that run's journal could not be read.
        "connector_registered": match &registered { Ok(id) => json!(id.is_some()), Err(_) => Value::Null },
        "connector_registration_error": registered.as_ref().err(),
        "lease": lease.as_ref().map(|l| json!({"holder_host_uuid": l.holder_host_uuid, "expires_at": l.expires_at, "epoch": l.epoch,
                                               "generation": l.generation, "acquired_at": l.acquired_at})),
        "epoch": epoch,
        "service": service.as_ref().map(|(ip, carrier)| json!({"ip": ip, "carrier_universe_uuid": carrier})),
        "carrier_replica_id": carrier_identity,
        "origin_readiness": readiness,
        "origin_ready_at_lease_epoch": origin_at_epoch,
        "active_manager_mark": mark,
        "active_manager_mark_epoch": mark_epoch,
        // present (with its epoch), absent (read, no file at either path) or unknown (not read).
        "active_manager_mark_read": mark_state,
        "active_manager_mark_error": mark_now.as_ref().and_then(|m| m.as_ref().err()),
        // Deprecated: the field's previous name, the same value, kept for one release.
        "governor_mark": mark,
        "transition": transition_of(db, resource)?.map(|(s, e)| json!({"state": s, "epoch": e})),
        "publisher_eligible": eligible,
        "gates": gates,
        "reasons": reasons,
        "takeover_resume": resume,
        "last": {"start": last_event(db, resource, "start")?, "stop": last_event(db, resource, "stop")?, "fence": last_event(db, resource, "fence")?,
                 "observed": last_event(db, resource, "observed")?, "takeover_resumed": last_event(db, resource, "takeover_resumed")?,
                 "startup_withdrawal": last_event(db, resource, "startup_withdrawal")?},
        "scope": "this host's declaration, unit, journal and tables, and the origin asked now; the connector's identity is what cloudflared logged in its current run; Cloudflare's side is observed only through publisher_observed",
    }))
}

fn perform(db: &Connection, request: &Value, resource: &str) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    let id = lc::text(request, "operation_id")?;
    let reference = lc::text(request, "authorization_ref")?;
    match operation {
        "publisher_declare" => {
            let hostname = lc::text(request, "hostname")?;
            if hostname.is_empty() || hostname.len() > 253 || !hostname.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.')) {
                return Err("hostname must be a DNS name".into());
            }
            let tunnel = lc::text(request, "tunnel_uuid")?;
            lc::token(tunnel)?;
            let credential = lc::text(request, "credential")?;
            lc::token(credential)?;
            let port = request.get("origin_port").and_then(Value::as_u64).unwrap_or(8080);
            if !(1..=65535).contains(&port) {
                return Err("origin_port must be 1 to 65535".into());
            }
            if crate::activation::policy(db, resource)?.is_none() {
                return Err(format!("{resource} is under no activation policy on this host; a publisher follows a governed resource").into());
            }
            crate::secrets::declared(db, credential)?;
            db.execute(
                "INSERT INTO publishers VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(resource) DO UPDATE SET hostname=excluded.hostname, tunnel_uuid=excluded.tunnel_uuid, credential=excluded.credential,
                   origin_port=excluded.origin_port, declared_at=excluded.declared_at, operation_id=excluded.operation_id, authorization_ref=excluded.authorization_ref",
                params![resource, hostname, tunnel, credential, port as i64, crate::now() as i64, id, reference],
            )?;
            event(db, resource, "declare", id, None)?;
            view(db, resource)
        }
        "publisher_start" => {
            let Some(p) = declared(db, resource)? else { return Err("no publisher declared for this resource on this host".into()) };
            if transition_of(db, resource)?.is_some() || connector_present(resource) == Some(true) {
                return Err("a publisher is already recorded or active on this host; stop it first".into());
            }
            // 1. the epoch gate, 2. the service address effective here, 3. the credential
            let (eligible, reasons, service, epoch, _) = eligibility(db, &p)?;
            if !eligible {
                return Err(format!("publisher_start refused: {}", reasons.join("; ")).into());
            }
            let (ip, carrier) = service.ok_or("no service address")?;
            let epoch = epoch.ok_or("no epoch on the lease")?;
            // 4. the takeover proof from the authority (the gate), or the same-epoch resume under the one
            //    this host already verified; the agent's `previous` is provenance only
            let this_host = crate::activation::host_uuid_public(db)?;
            let proof_verified = takeover(db, resource, request, epoch, &this_host, &crate::activation::boot_id()?, id)?;
            let previous = request.get("previous").cloned().unwrap_or(Value::Null);
            // 5. the transition recorded before any effect; 6. the mark, then readiness at the
            //    service address; 7. the connector, then its registration; 8. effective last.
            //    Any failure undoes what was made, last first, and leaves no transition.
            transition(db, resource, "starting", epoch, id)?;
            let mut done = vec![];
            let mut failure: Option<String> = None;
            let mut connector: Option<String> = None;
            for (kind, key, intent) in [
                (KIND_MARK, format!("{resource}@{carrier}"), json!({"carrier": carrier, "resource": resource, "epoch": epoch})),
                (KIND_PUBLISHER, unit_name(resource), json!({"resource": resource, "ip": ip, "port": p.origin_port})),
            ] {
                let e = crate::network::effect_begin_public(db, kind, &key, resource, intent, id)?;
                done.push(e.clone());
                if let Err(err) = crate::network::effect_do_public(db, &e) {
                    failure = Some(err.to_string());
                    break;
                }
                if kind == KIND_MARK {
                    if let Err(err) = lc::fault("publisher-after-mark") { failure = Some(err.to_string()); break; }
                    match origin_ready(&ip, p.origin_port) {
                        Ok((200, body)) if body["logical_manager_id"] == json!(resource) && body["epoch"] == json!(epoch) && body["ready"] == json!(true) => {
                            let replica = crate::manager::execute(db, &json!({"operation": "manager_status", "universe_uuid": carrier}))?["resident_status"]["replica_id"].clone();
                            if body["replica_id"] != replica {
                                failure = Some(format!("the origin names replica {} but the carrier's resident is {}", body["replica_id"], replica));
                                break;
                            }
                        }
                        Ok((status, body)) => {
                            failure = Some(format!("the origin at {ip}:{} is not ready for this active manager at epoch {epoch}: HTTP {status} {body}", p.origin_port));
                            break;
                        }
                        Err(e) => {
                            failure = Some(format!("the origin at {ip}:{} could not be asked: {e}", p.origin_port));
                            break;
                        }
                    }
                } else {
                    if let Err(err) = lc::fault("publisher-after-connector") { failure = Some(err.to_string()); break; }
                    match wait_registered(resource, 60) {
                        Ok(idc) => connector = Some(idc),
                        Err(err) => { failure = Some(err.to_string()); break; }
                    }
                }
            }
            if failure.is_none() {
                if let Err(err) = lc::fault("publisher-before-effective") {
                    failure = Some(err.to_string());
                }
            }
            if let Some(err) = failure {
                let _ = lc::fault("publisher-during-compensation");
                let report = crate::network::compensate_public(db, &done);
                let all_gone = report.iter().all(|r| r["gone"] == json!(true));
                if all_gone {
                    db.execute("DELETE FROM publisher_transitions WHERE resource=?1", [resource])?;
                }
                event(db, resource, "start_failed", id, Some(json!({"error": err, "compensation": report})))?;
                return Err(format!("{err}; compensation: {}{}", json!(report), if all_gone { "" } else { "; the transition is kept for reconciliation" }).into());
            }
            transition(db, resource, "effective", epoch, id)?;
            event(db, resource, "start", id, Some(json!({"epoch": epoch, "ip": ip, "carrier": carrier, "connector_id": connector, "takeover_proof": proof_verified, "previous": previous})))?;
            let mut v = view(db, resource)?;
            v["published"] = json!(true);
            v["takeover_proof"] = proof_verified;
            Ok(v)
        }
        "publisher_stop" => {
            let report = withdraw(db, resource, "stop", id)?;
            if report["withdrawn"] != json!(true) {
                return Err(format!("the publisher's withdrawal did not complete: {report}").into());
            }
            view(db, resource)
        }
        "publisher_observed" => {
            let observation = request.get("observation").cloned().ok_or("publisher_observed requires `observation`")?;
            event(db, resource, "observed", id, Some(observation))?;
            view(db, resource)
        }
        _ => Err("Unsupported publisher operation".into()),
    }
}

/// The ledger rows a withdrawal of the resource undoes, in the ledger's order: the connector's and
/// the mark's, under either of the mark's kinds, whatever their state.
fn publisher_effects(db: &Connection, resource: &str) -> Result<Vec<crate::network::Effect>, Error> {
    Ok(crate::network::effect_rows_public(db, Some(resource))?.into_iter().filter(|e| e.kind == KIND_PUBLISHER || is_mark(&e.kind)).collect())
}

/// The connector stopped and the mark removed, each recorded `removing` first and verified gone:
/// what `publisher_stop` and the fence do. The effects of the resource whose kind is the
/// publisher's or the mark's, last first (the connector before the mark).
fn withdraw(db: &Connection, resource: &str, why: &str, id: &str) -> Result<Value, Error> {
    if let Some((_, epoch)) = transition_of(db, resource)? {
        transition(db, resource, "stopping", epoch, id)?;
    }
    let effects = publisher_effects(db, resource)?;
    let mut steps = vec![];
    let mut failed = false;
    for e in effects.iter().rev() {
        match crate::network::effect_remove_public(db, e) {
            Ok(()) => steps.push(json!({"kind": e.kind, "key": e.key, "gone": true})),
            Err(err) => { failed = true; steps.push(json!({"kind": e.kind, "key": e.key, "gone": false, "error": err.to_string()})) }
        }
    }
    // A connector or a mark from before this ledger, or made by hand: stopped and removed all the
    // same, verified, so that nothing publishes without a record.
    if connector_present(resource) == Some(true) {
        match connector_stop(resource) {
            Ok(()) => steps.push(json!({"kind": KIND_PUBLISHER, "unrecorded": true, "gone": connector_present(resource) == Some(false)})),
            Err(err) => { failed = true; steps.push(json!({"kind": KIND_PUBLISHER, "unrecorded": true, "gone": false, "error": err.to_string()})) }
        }
    }
    if !failed {
        db.execute("DELETE FROM publisher_transitions WHERE resource=?1", [resource])?;
    }
    event(db, resource, why, id, Some(json!({"steps": steps})))?;
    Ok(json!({"resource": resource, "withdrawn": !failed, "steps": steps, "unit": unit_state(resource)}))
}

/// For the fence: every declared publisher whose resource this host no longer holds is withdrawn,
/// connector first, mark second -- before the fence takes the alias and the route.
pub(crate) fn withdraw_unentitled(db: &Connection, entitled: &dyn Fn(&str) -> bool, id: &str) -> Result<Vec<Value>, Error> {
    ensure_schema(db)?;
    let mut s = db.prepare("SELECT resource FROM publishers")?;
    let resources: Vec<String> = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut report = vec![];
    for resource in resources {
        if entitled(&resource) {
            continue;
        }
        // Nothing to withdraw is not a withdrawal: reported only when something was there.
        let had = !publisher_effects(db, &resource)?.is_empty() || connector_present(&resource) == Some(true);
        if had {
            report.push(withdraw(db, &resource, "fence", id)?);
        }
    }
    Ok(report)
}

/// Why this host is not entitled to publish the resource now, or None when it is: a policy, and a
/// lease held here, live and unsuperseded -- the fence's predicate -- and, when a transition is
/// recorded, at the transition's epoch.
fn unentitled_why(db: &Connection, resource: &str, this_host: &str, now: i64) -> Result<Option<String>, Error> {
    if crate::activation::policy(db, resource)?.is_none() {
        return Ok(Some("the resource is under no activation policy here".into()));
    }
    let Some(l) = crate::activation::lease(db, resource)? else { return Ok(Some("no activation lease is held".into())) };
    if l.holder_host_uuid != this_host {
        return Ok(Some("the activation lease is held by another host".into()));
    }
    if l.expires_at <= now {
        return Ok(Some(format!("the activation lease expired {} seconds ago", now - l.expires_at)));
    }
    if let Some(seen) = crate::activation::superseded_by(db, resource, &l)? {
        return Ok(Some(format!("the activation was superseded by epoch {seen}")));
    }
    if let Some((_, epoch)) = transition_of(db, resource)?.filter(|(_, e)| *e != l.epoch) {
        return Ok(Some(format!("the publisher was started at epoch {epoch}, the lease is under epoch {}", l.epoch)));
    }
    Ok(None)
}

/// The resources a startup withdrawal acts on: declared here, with something of a publisher present
/// -- a transition, a connector's or a mark's ledger row in any state, or an active connector unit,
/// recorded or not -- and no entitlement now. `active` says whether a resource's connector unit runs.
fn startup_unentitled(db: &Connection, active: &dyn Fn(&str) -> bool) -> Result<Vec<Value>, Error> {
    let this_host = crate::activation::host_uuid_public(db)?;
    let now = crate::now() as i64;
    let mut s = db.prepare("SELECT resource FROM publishers ORDER BY resource")?;
    let resources: Vec<String> = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut found = vec![];
    for resource in resources {
        let present = transition_of(db, &resource)?.is_some() || !publisher_effects(db, &resource)?.is_empty() || active(&resource);
        if !present {
            continue;
        }
        if let Some(why) = unentitled_why(db, &resource, &this_host, now)? {
            found.push(json!({"resource": resource, "why": why}));
        }
    }
    Ok(found)
}

/// At the daemon's start, before anything is served: every connector whose lease is no longer live,
/// held here and unsuperseded is withdrawn -- the connector stopped, the mark removed, each verified --
/// in one journaled operation (`publisher_startup_withdrawal`, its ID derived from this boot and the
/// time). A lease that lapsed while the daemon was down left its connector publishing: the unit is
/// systemd's, not the daemon's, and nothing else withdrew it (the fence timer is disarmed on the
/// laboratory, and runs through the daemon anyway). Journaled only when something is to be withdrawn:
/// an empty start is not evidence. The route and the alias stay: withdrawing them is the fence's.
pub fn withdraw_at_startup(db: &Connection) -> Result<Value, Error> {
    lc::ensure_schema(db)?;
    crate::activation::ensure_schema(db)?;
    crate::network::ensure_schema(db)?;
    ensure_schema(db)?;
    let candidates = startup_unentitled(db, &|resource: &str| connector_present(resource) == Some(true))?;
    if candidates.is_empty() {
        return Ok(json!({"withdrawn": [], "journaled": false, "note": "no declared publisher present without entitlement"}));
    }
    let boot = crate::activation::boot_id().map(|b| b.replace('-', "")).unwrap_or_else(|_| "unknown".into());
    let id = format!("startup-withdrawal-{boot}-{}", crate::now());
    let request = json!({"operation": "publisher_startup_withdrawal", "operation_id": id, "authorization_ref": "podmeshd-startup", "resources": candidates});
    lc::journaled(db, &request, |db| {
        let mut report = vec![];
        for c in &candidates {
            let resource = c["resource"].as_str().unwrap_or_default();
            let mut r = withdraw(db, resource, "startup_withdrawal", &id)?;
            r["why"] = c["why"].clone();
            report.push(r);
        }
        if report.iter().any(|r| r["withdrawn"] != json!(true)) {
            return Err(format!("the startup withdrawal did not complete: {}", json!(report)).into());
        }
        Ok(json!({"withdrawn": report, "journaled": true, "operation_id": id}))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: &str = "91eeb6bf-5489-405b-b77a-53105b0aff7a";
    const CARRIER: &str = "c7a1f732-80b5-4421-9241-e08066f04cb8";

    fn journal() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        crate::network::ensure_schema(&db).unwrap();
        ensure_schema(&db).unwrap();
        db
    }

    fn row(db: &Connection, kind: &str, key: &str, owner: &str, state: &str) {
        db.execute(
            "INSERT INTO network_effects(kind,key,owner,intent,state,operation_id,changed_at) VALUES(?1,?2,?3,?4,?5,'x',0)",
            params![kind, key, owner, json!({"carrier": CARRIER, "resource": owner, "epoch": 1}).to_string(), state],
        )
        .unwrap();
    }

    #[test]
    fn the_mark_is_matched_under_its_previous_kind() {
        assert!(is_mark("active_manager_mark") && is_mark("governor_mark"));
        for other in [KIND_PUBLISHER, "route", "alias", "bridge", "peer_route", "nat_table", ""] {
            assert!(!is_mark(other), "{other}");
        }
    }

    #[test]
    fn a_withdrawal_finds_marks_of_the_previous_kind_in_every_state() {
        let db = journal();
        let key = format!("{R}@{CARRIER}");
        // As an earlier build left them in a host's journal: a mark effective beside its connector,
        // and marks left `applying` and `removing` by crashes.
        row(&db, "governor_mark", &key, R, "effective");
        row(&db, KIND_PUBLISHER, &unit_name(R), R, "effective");
        row(&db, "governor_mark", &key, R, "applying");
        row(&db, "governor_mark", &key, R, "removing");
        // This build's mark, a kind that is not the publisher's under the same owner, and another
        // resource's mark.
        row(&db, "active_manager_mark", &key, R, "applying");
        row(&db, "route", "10.86.0.100/32", R, "effective");
        row(&db, "active_manager_mark", "elsewhere", "00000000-0000-4000-8000-000000000002", "effective");
        let found: Vec<(String, String)> = publisher_effects(&db, R).unwrap().into_iter().map(|e| (e.kind, e.state)).collect();
        let expected = [
            ("governor_mark", "effective"),
            ("publisher", "effective"),
            ("governor_mark", "applying"),
            ("governor_mark", "removing"),
            ("active_manager_mark", "applying"),
        ];
        assert_eq!(found, expected.map(|(k, s)| (k.to_string(), s.to_string())));
    }

    // ---------------------------------------------------------------- the same-epoch resume (V3-1)

    const HOST: &str = "5d1c0b8e-3f59-4d0e-9d7a-2a1e7c4b9f10";
    const BOOT: &str = "0b7f6f1e-6a55-4c1d-8f53-1c2d3e4f5a6b";
    const EPOCH: i64 = 157;
    const ACQUIRED: i64 = 1_700_000_000;

    /// A journal as a laboratory host holds it for the logical manager: this host's identity, the
    /// resource under a policy that names an authority (no key: laboratory proofs), and a live lease
    /// held here at epoch 157, generation 3, the screen at the same epoch.
    fn gated() -> Connection {
        let db = journal();
        db.execute_batch("CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        db.execute("INSERT INTO metadata VALUES('host_uuid',?1)", [HOST]).unwrap();
        crate::activation::ensure_schema(&db).unwrap();
        let now = crate::now() as i64;
        db.execute(
            "INSERT INTO activation_policy(universe_uuid,lease_seconds,takeover_margin_seconds,declared_at,operation_id,authority_id,authority_key)
             VALUES(?1,3600,30,0,'p','lab-gate','')",
            [R],
        )
        .unwrap();
        db.execute(
            "INSERT INTO activation_leases(universe_uuid,holder_host_uuid,generation,acquired_at,expires_at,operation_id,epoch,grant_id)
             VALUES(?1,?2,3,?3,?4,'a',?5,'g157')",
            params![R, HOST, ACQUIRED, now + 3000, EPOCH],
        )
        .unwrap();
        db.execute("INSERT INTO activation_epochs VALUES(?1,'lab-gate',?2,'g157',?3,0,'a')", params![R, EPOCH, HOST]).unwrap();
        db
    }

    /// The gate's laboratory document for the transition into `EPOCH`, same holder, issued `age`
    /// seconds ago with the gate's one hour of life.
    fn proof(age: i64) -> Value {
        let issued = crate::now() as i64 - age;
        json!({"kind": crate::signing::UNSIGNED_PROOF_KIND, "authority_id": "lab-gate", "resource": R, "new_holder": HOST,
               "previous_holder": HOST, "new_epoch": EPOCH, "previous_epoch": EPOCH - 1, "issued_at": issued, "expires_at": issued + 3600,
               "method": "same_holder"})
    }

    fn start(db: &Connection, proof: Option<Value>, boot: &str, id: &str) -> Result<Value, Error> {
        let request = match proof { Some(p) => json!({"takeover_proof": p}), None => json!({}) };
        takeover(db, R, &request, EPOCH, HOST, boot, id)
    }

    fn refusal(db: &Connection, boot: &str) -> &'static str {
        match resume_refusal(&resume_facts(db, R, HOST, boot).unwrap()) {
            Ok(_) => "resumes",
            Err((code, _)) => code,
        }
    }

    #[test]
    fn a_verified_proof_is_recorded_with_the_lease_incarnation_and_the_boot() {
        let db = gated();
        let v = start(&db, Some(proof(10)), BOOT, "op-verify").unwrap();
        assert_eq!(v["method"], json!("same_holder"));
        let r = last_verified(&db, R).unwrap().unwrap();
        assert_eq!((r.epoch, r.generation, r.acquired_at, r.boot_id.as_str(), r.operation_id.as_str()), (EPOCH, 3, ACQUIRED, BOOT, "op-verify"));
        assert_eq!((r.authority_id.as_str(), r.authority_key.as_str()), ("lab-gate", ""));
        assert_eq!(r.proof["issued_at"], proof(10)["issued_at"]);
    }

    #[test]
    fn with_every_condition_true_a_start_resumes_and_the_journal_says_so() {
        let db = gated();
        start(&db, Some(proof(10)), BOOT, "op-verify").unwrap();
        assert_eq!(refusal(&db, BOOT), "resumes");
        // No proof at all, then the same document an hour and more later: both resume, neither is
        // verified again, and each resume is journaled with the original proof's identity.
        for (presented, id) in [(None, "op-resume-1"), (Some(proof(4000)), "op-resume-2")] {
            let refused_expected = presented.is_some();
            let v = start(&db, presented, BOOT, id).unwrap();
            assert_eq!(v["method"], json!("resume_same_epoch"), "{v}");
            assert_eq!(v["new_epoch"], json!(EPOCH));
            assert_eq!(v["resumed_from"]["operation_id"], json!("op-verify"));
            assert_eq!(v["resumed_from"]["method"], json!("same_holder"));
            assert_eq!(v["presented_proof_refused"].as_str().is_some_and(|e| e.contains("expired")), refused_expected, "{v}");
            let e = last_event(&db, R, "takeover_resumed").unwrap().unwrap();
            assert_eq!(e["operation_id"], json!(id));
            assert_eq!(e["detail"]["resumed_from"]["operation_id"], json!("op-verify"));
        }
        // The resume records nothing new: the verification it rests on is still the first one.
        assert_eq!(last_verified(&db, R).unwrap().unwrap().operation_id, "op-verify");
    }

    #[test]
    fn each_condition_false_is_a_named_refusal() {
        type Change = fn(&Connection);
        let cases: [(&str, Change); 9] = [
            ("no_lease", |db| { db.execute("DELETE FROM activation_leases", []).unwrap(); }),
            ("lease_held_elsewhere", |db| { db.execute("UPDATE activation_leases SET holder_host_uuid='00000000-0000-4000-8000-00000000000b'", []).unwrap(); }),
            ("lease_expired", |db| { db.execute("UPDATE activation_leases SET expires_at=?1", [crate::now() as i64 - 1]).unwrap(); }),
            ("lease_superseded", |db| { db.execute("UPDATE activation_epochs SET epoch=158", []).unwrap(); }),
            ("no_verified_proof", |db| { db.execute("DELETE FROM publisher_takeover_verified", []).unwrap(); }),
            // The gate granted this host a newer epoch: live, unsuperseded, and not the transition
            // the recorded proof accounted for.
            ("epoch_changed", |db| {
                db.execute("UPDATE activation_leases SET epoch=158", []).unwrap();
                db.execute("UPDATE activation_epochs SET epoch=158", []).unwrap();
            }),
            ("generation_changed", |db| { db.execute("UPDATE activation_leases SET generation=4", []).unwrap(); }),
            // A lapse of this host's own lease, retaken: the same generation and epoch, a new acquisition.
            ("lease_reacquired", |db| { db.execute("UPDATE activation_leases SET acquired_at=acquired_at+4000", []).unwrap(); }),
            ("authority_changed", |db| { db.execute("UPDATE activation_policy SET authority_id='another-gate'", []).unwrap(); }),
        ];
        for (expected, change) in cases {
            let db = gated();
            start(&db, Some(proof(10)), BOOT, "op-verify").unwrap();
            change(&db);
            assert_eq!(refusal(&db, BOOT), expected);
            // Through the start: without a proof, refused, naming the condition, and nothing journaled
            // as a resume.
            let err = start(&db, None, BOOT, "op-later").unwrap_err().to_string();
            assert!(err.contains(&format!("({expected})")) && err.contains("requires `takeover_proof`"), "{expected}: {err}");
            assert!(last_event(&db, R, "takeover_resumed").unwrap().is_none(), "{expected}");
        }
        let db = gated();
        start(&db, Some(proof(10)), BOOT, "op-verify").unwrap();
        assert_eq!(refusal(&db, "another-boot"), "boot_changed");
        let err = start(&db, Some(proof(4000)), "another-boot", "op-later").unwrap_err().to_string();
        assert!(err.contains("takeover_proof refused") && err.contains("expired") && err.contains("(boot_changed)"), "{err}");
    }

    #[test]
    fn a_different_acquisition_needs_a_new_proof_and_a_valid_one_is_recorded_for_it() {
        let db = gated();
        start(&db, Some(proof(10)), BOOT, "op-verify").unwrap();
        db.execute("UPDATE activation_leases SET acquired_at=acquired_at+4000", []).unwrap();
        let err = start(&db, None, BOOT, "op-refused").unwrap_err().to_string();
        assert!(err.contains("(lease_reacquired)"), "{err}");
        // The gate's new document for the same epoch is verified and replaces the record: the new
        // incarnation may then be resumed, the old one never again.
        start(&db, Some(proof(5)), BOOT, "op-verify-2").unwrap();
        let r = last_verified(&db, R).unwrap().unwrap();
        assert_eq!((r.acquired_at, r.operation_id.as_str()), (ACQUIRED + 4000, "op-verify-2"));
        assert_eq!(start(&db, None, BOOT, "op-resume").unwrap()["resumed_from"]["operation_id"], json!("op-verify-2"));
    }

    #[test]
    fn a_registration_is_read_from_the_lines_given_the_last_one_first() {
        let text = "2026-09-18T06:00:00Z INF Starting tunnel\n\
                    2026-09-18T06:00:01Z INF Registered tunnel connection connIndex=0 connection=aaaa-1 event=0 ip=198.41.200.13 location=cdg\n\
                    2026-09-18T06:00:02Z INF Registered tunnel connection connIndex=1 connection=bbbb-2 event=0 ip=198.41.192.7 location=mrs\n\
                    2026-09-18T06:10:00Z WRN something else connection=not-a-registration\n";
        assert_eq!(registration(text).as_deref(), Some("bbbb-2"));
        assert_eq!(registration("INF Starting tunnel\n"), None);
        assert_eq!(registration(""), None);
    }

    /// A mark read is present with its epoch, absent, or unknown; the last is never a wrong epoch.
    #[test]
    fn a_mark_is_present_absent_or_unknown() {
        assert_eq!(mark_answer(&json!({"resource": R, "epoch": 157, "marked_at": 1}).to_string()), Ok(Some(157)));
        assert_eq!(mark_answer("absent\n"), Ok(None));
        for garbage in ["", "{", "{\"resource\": \"x\"}", "not a mark"] {
            assert!(mark_answer(garbage).is_err(), "{garbage:?}");
        }
    }

    /// The origin's verdict: true, a positive mismatch, or unknown -- a failed request or an unknown
    /// replica is never reported as a wrong origin.
    #[test]
    fn the_origin_is_ready_wrong_or_unknown() {
        let replica = json!("replica-a");
        let good = json!({"status": 200, "body": {"ready": true, "epoch": 157, "logical_manager_id": R, "replica_id": "replica-a"}});
        assert_eq!(origin_verdict(&good, 157, R, Some(&replica)), Some(true));
        assert_eq!(origin_verdict(&good, 158, R, Some(&replica)), Some(false), "another epoch");
        assert_eq!(origin_verdict(&good, 157, R, Some(&json!("replica-b"))), Some(false), "another replica");
        let unmarked = json!({"status": 503, "body": {"ready": false}});
        assert_eq!(origin_verdict(&unmarked, 157, R, Some(&replica)), Some(false), "503: no mark");
        let other = json!({"status": 200, "body": {"ready": true, "epoch": 157, "logical_manager_id": "another", "replica_id": "replica-a"}});
        assert_eq!(origin_verdict(&other, 157, R, Some(&replica)), Some(false), "another logical manager");
        assert_eq!(origin_verdict(&json!({"error": "connect 10.86.0.100:8080: timed out"}), 157, R, Some(&replica)), None, "not asked");
        assert_eq!(origin_verdict(&good, 157, R, None), None, "replica unknown");
        assert_eq!(origin_verdict(&good, 157, R, Some(&Value::Null)), None, "replica unknown");
    }

    #[test]
    fn the_startup_withdrawal_selects_what_is_present_without_entitlement() {
        let db = gated();
        db.execute("INSERT INTO publishers VALUES(?1,'h.example','t','c',8080,0,'d','r')", [R]).unwrap();
        let none: &dyn Fn(&str) -> bool = &|_| false;
        let running: &dyn Fn(&str) -> bool = &|_| true;
        // Nothing present: nothing selected, whatever the lease.
        db.execute("UPDATE activation_leases SET expires_at=1", []).unwrap();
        assert!(startup_unentitled(&db, none).unwrap().is_empty());
        // A connector unit running with no record, its lease lapsed while the daemon was down.
        let found = startup_unentitled(&db, running).unwrap();
        assert!(found.len() == 1 && found[0]["why"].as_str().unwrap().contains("expired"), "{found:?}");
        // A recorded publisher: selected while unentitled for each reason, left alone while entitled.
        transition(&db, R, "effective", EPOCH, "s").unwrap();
        db.execute("UPDATE activation_leases SET expires_at=?1", [crate::now() as i64 + 600]).unwrap();
        assert!(startup_unentitled(&db, none).unwrap().is_empty());
        type Change = fn(&Connection);
        let cases: [(&str, Change); 4] = [
            ("expired", |db| { db.execute("UPDATE activation_leases SET expires_at=1", []).unwrap(); }),
            ("another host", |db| { db.execute("UPDATE activation_leases SET holder_host_uuid='00000000-0000-4000-8000-00000000000b'", []).unwrap(); }),
            ("superseded by epoch 158", |db| { db.execute("UPDATE activation_epochs SET epoch=158", []).unwrap(); }),
            ("started at epoch 156", |db| { db.execute("UPDATE publisher_transitions SET epoch=156", []).unwrap(); }),
        ];
        for (why, change) in cases {
            let db2 = gated();
            db2.execute("INSERT INTO publishers VALUES(?1,'h.example','t','c',8080,0,'d','r')", [R]).unwrap();
            transition(&db2, R, "effective", EPOCH, "s").unwrap();
            change(&db2);
            let found = startup_unentitled(&db2, none).unwrap();
            assert!(found.len() == 1 && found[0]["why"].as_str().unwrap().contains(why), "{why}: {found:?}");
        }
        // A mark's ledger row alone counts as present.
        let db3 = gated();
        db3.execute("INSERT INTO publishers VALUES(?1,'h.example','t','c',8080,0,'d','r')", [R]).unwrap();
        db3.execute("UPDATE activation_leases SET expires_at=1", []).unwrap();
        row(&db3, KIND_MARK, &format!("{R}@{CARRIER}"), R, "effective");
        assert_eq!(startup_unentitled(&db3, none).unwrap().len(), 1);
    }
}

#[cfg(test)]
mod review_regressions {
    //! The adversarial probes of the V3-1 review, kept as regression tests: the same-epoch resume driven
    //! through the real activation operations, not through direct UPDATEs of the lease table.
    use super::*;
    const R: &str = "91eeb6bf-5489-405b-b77a-53105b0aff7a";
    const HOST: &str = "5d1c0b8e-3f59-4d0e-9d7a-2a1e7c4b9f10";
    const OTHER: &str = "00000000-0000-4000-8000-00000000000b";

    fn next() -> u64 { use std::sync::atomic::{AtomicU64, Ordering}; static N: AtomicU64 = AtomicU64::new(1); N.fetch_add(1, Ordering::SeqCst) }
    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        db.execute("INSERT INTO metadata VALUES('host_uuid',?1)", [HOST]).unwrap();
        lc::ensure_schema(&db).unwrap();
        crate::network::ensure_schema(&db).unwrap();
        crate::activation::ensure_schema(&db).unwrap();
        ensure_schema(&db).unwrap();
        db
    }
    fn act(db: &Connection, op: &str, extra: Value) -> Result<Value, String> {
        let mut r = json!({"operation": op, "universe_uuid": R, "operation_id": format!("op-{}", next()), "authorization_ref": "probe"});
        for (k, v) in extra.as_object().unwrap() { r[k] = v.clone(); }
        crate::activation::execute(db, &r).map_err(|e| e.to_string())
    }
    fn permit(epoch: i64, host: &str) -> Value {
        json!({"authority_id": "lab-gate", "resource": R, "epoch": epoch, "replica_id": host,
               "instance_id": crate::activation::boot_id().unwrap(), "grant_id": format!("g{epoch}")})
    }
    fn proof(epoch: i64, method: &str) -> Value {
        let now = crate::now() as i64;
        json!({"kind": crate::signing::UNSIGNED_PROOF_KIND, "authority_id": "lab-gate", "resource": R, "new_holder": HOST,
               "previous_holder": HOST, "new_epoch": epoch, "previous_epoch": epoch - 1, "issued_at": now, "expires_at": now + 3600, "method": method})
    }
    fn expired(epoch: i64) -> Value {
        let mut p = proof(epoch, "same_holder");
        p["issued_at"] = json!(crate::now() as i64 - 4000); p["expires_at"] = json!(crate::now() as i64 - 400); p
    }
    fn code(db: &Connection) -> &'static str {
        let boot = crate::activation::boot_id().unwrap();
        match resume_refusal(&resume_facts(db, R, HOST, &boot).unwrap()) { Ok(_) => "resumes", Err((c, _)) => c }
    }
    fn setup(lease: u64) -> Connection {
        let db = db();
        act(&db, "activation_require", json!({"lease_seconds": lease, "takeover_margin_seconds": 5, "authority_id": "lab-gate"})).unwrap();
        act(&db, "activation_acquire", json!({"permit": permit(157, HOST)})).unwrap();
        let boot = crate::activation::boot_id().unwrap();
        takeover(&db, R, &json!({"takeover_proof": proof(157, "same_holder")}), 157, HOST, &boot, "op-verify").unwrap();
        assert_eq!(code(&db), "resumes");
        db
    }
    fn start(db: &Connection, p: Option<Value>, epoch: i64) -> Result<Value, String> {
        let boot = crate::activation::boot_id().unwrap();
        let req = match p { Some(p) => json!({"takeover_proof": p}), None => json!({}) };
        takeover(db, R, &req, epoch, HOST, &boot, &format!("op-{}", next())).map_err(|e| e.to_string())
    }

    #[test]
    fn probe_renewal_keeps_the_resume() {
        let db = setup(3600);
        act(&db, "activation_renew", json!({})).unwrap();
        assert_eq!(code(&db), "resumes");
    }

    #[test]
    fn probe_same_holder_rotation_does_not_resume_next_epoch() {
        let db = setup(3600);
        act(&db, "activation_acquire", json!({"permit": permit(158, HOST)})).unwrap();
        assert_eq!(code(&db), "epoch_changed");
        // An expired document for 158, or 157's own, never resumes 158.
        let e = start(&db, Some(expired(158)), 158).unwrap_err();
        assert!(e.contains("(epoch_changed)"), "{e}");
        let e = start(&db, Some(proof(157, "same_holder")), 158).unwrap_err();
        assert!(e.contains("(epoch_changed)"), "{e}");
    }

    #[test]
    fn probe_idempotent_reacquire_of_a_live_lease_kills_the_resume() {
        let db = setup(3600);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        // The same permit presented again while the lease is live (an agent's retry, a tool re-run).
        act(&db, "activation_acquire", json!({"permit": permit(157, HOST)})).unwrap();
        assert_eq!(code(&db), "lease_reacquired");
    }

    #[test]
    fn probe_lapse_and_retake_with_the_same_permit() {
        let db = setup(5);
        std::thread::sleep(std::time::Duration::from_millis(6100));
        assert_eq!(code(&db), "lease_expired");
        act(&db, "activation_acquire", json!({"permit": permit(157, HOST)})).unwrap();
        assert_eq!(code(&db), "lease_reacquired");
        // The gate's hour-old document for 157 is still valid: it verifies again and re-records the
        // new incarnation, which then resumes. The lapse is only as strong as the proof's expiry.
        start(&db, Some(proof(157, "same_holder")), 157).unwrap();
        assert_eq!(code(&db), "resumes");
    }

    #[test]
    fn probe_supersession_and_policy_changes() {
        let db = setup(3600);
        act(&db, "activation_supersede", json!({"permit": permit(158, OTHER)})).unwrap();
        assert_eq!(code(&db), "lease_superseded");
        let db = setup(3600);
        // Re-required with the same authority and a key: the recorded key was "".
        let key: String = {
            let vk = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]).verifying_key();
            vk.to_bytes().iter().map(|b| format!("{b:02x}")).collect()
        };
        act(&db, "activation_require", json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "lab-gate", "authority_key": key})).unwrap();
        assert_eq!(code(&db), "authority_changed");
        // Re-required identically (what --refresh does on the other hosts): the resume survives.
        let db = setup(3600);
        act(&db, "activation_require", json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "lab-gate"})).unwrap();
        assert_eq!(code(&db), "resumes");
    }

    #[test]
    fn probe_forged_or_foreign_proof_falls_back_to_exactly_the_resume() {
        let db = setup(3600);
        let mut foreign = proof(157, "same_holder");
        foreign["new_holder"] = json!(OTHER);
        let v = start(&db, Some(foreign), 157).unwrap();
        assert_eq!(v["method"], json!("resume_same_epoch"));
        // And the record is untouched by a refused document.
        assert_eq!(last_verified(&db, R).unwrap().unwrap().operation_id, "op-verify");
        // Without a record, a forged document gives nothing.
        let db2 = db_without_record();
        let mut forged = proof(157, "same_holder"); forged["authority_id"] = json!("evil");
        assert!(start(&db2, Some(forged), 157).unwrap_err().contains("no_verified_proof"));
    }
    fn db_without_record() -> Connection {
        let db = db();
        act(&db, "activation_require", json!({"lease_seconds": 3600, "takeover_margin_seconds": 5, "authority_id": "lab-gate"})).unwrap();
        act(&db, "activation_acquire", json!({"permit": permit(157, HOST)})).unwrap();
        db
    }

    #[test]
    fn probe_a_start_that_fails_after_verification_still_leaves_the_record() {
        // takeover() records before the start's effects; a start that then fails (readiness, say)
        // leaves a record that later resumes without the proof ever having produced a publication.
        let db = db_without_record();
        start(&db, Some(proof(157, "same_holder")), 157).unwrap();
        assert!(last_verified(&db, R).unwrap().is_some());
        assert_eq!(code(&db), "resumes");
    }
}
