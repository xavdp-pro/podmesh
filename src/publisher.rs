//! The publishing connector that follows the governor role (the operator's decision of
//! 2026-09-15, `CLOUDFLARE-TUNNEL-MANAGER-HA-DECISION`).
//!
//! The logical manager has one public hostname, one logical Cloudflare tunnel and, in this first
//! candidate, exactly one publishing `cloudflared`, co-located with the governor replica and
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
//!   agent's word, recorded as provenance and refused when absent), the governor mark is written
//!   inside the carrier universe, and the origin answers ready at the service address with the
//!   expected logical manager, replica and epoch. Then the connector runs as a transient unit
//!   from a root-only runtime copy of the credential, and the unit's activity is verified.
//! - `publisher_stop`: the connector stopped and the mark removed, verified.
//! - the fence (`withdraw_unentitled`): for a resource this host no longer holds, the connector
//!   is stopped and the mark removed BEFORE the alias and the route go -- one transition, each
//!   step recorded, each verified.
//! - `publisher_status`: what is declared, what the unit does, the connector's identity from its
//!   journal, the lease and epoch, the origin's readiness now, `publisher_eligible` with the
//!   refusal reasons, the last start, stop, fence and externally observed request.
//! - `publisher_observed`: the agent records an external request's result, as provenance.
//!
//! What it does not decide: which replica is governor (the gate does), whether the hostname
//! resolves (Cloudflare's), and the manager's web interface (the origin here is the universe's
//! epoch-qualified readiness responder).
use crate::lifecycle as lc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::io::{Read, Write};

type Error = Box<dyn std::error::Error>;

pub const KIND_PUBLISHER: &str = "publisher";
pub const KIND_MARK: &str = "governor_mark";
const MARK_PATH: &str = "/run/podmesh-manager/governor.json";

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
            detail TEXT);",
    )?;
    Ok(())
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

/// The connector identity cloudflared logged when it registered with Cloudflare, from the
/// unit's journal: `connection=<id>` on the registration line, the last one.
fn connector_id(resource: &str) -> Option<String> {
    let out = std::process::Command::new("journalctl").args(["-u", &unit_name(resource), "--no-pager", "-o", "cat", "-n", "200"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .rev()
        .filter(|l| l.contains("Registered tunnel connection"))
        .find_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix("connection=").map(str::to_string)))
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

/// The governor mark inside the carrier universe: written and removed with `podman exec`, root-only.
pub(crate) fn mark_write(carrier: &str, resource: &str, epoch: i64) -> Result<(), Error> {
    let content = json!({"resource": resource, "epoch": epoch, "marked_at": crate::now()}).to_string();
    lc::podman(lc::QUICK, &["exec", &format!("podmesh-{carrier}"), "sh", "-c", &format!("umask 077; printf '%s' '{content}' > {MARK_PATH}.tmp && mv -f {MARK_PATH}.tmp {MARK_PATH}")])?;
    Ok(())
}

pub(crate) fn mark_present(carrier: &str) -> Option<bool> {
    let out = std::process::Command::new("podman").args(["exec", &format!("podmesh-{carrier}"), "test", "-f", MARK_PATH]).output().ok()?;
    if out.status.success() {
        return Some(true);
    }
    // A universe that is not running carries no mark; exec on it fails for that reason.
    match lc::inspect(&format!("podmesh-{carrier}")) {
        Ok(Some(c)) if c["State"]["Running"] == json!(true) => Some(false),
        Ok(_) => Some(false),
        Err(_) => None,
    }
}

pub(crate) fn mark_remove(carrier: &str) -> Result<(), Error> {
    if mark_present(carrier) == Some(true) {
        lc::podman(lc::QUICK, &["exec", &format!("podmesh-{carrier}"), "rm", "-f", MARK_PATH])?;
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
/// eligible, the refusal reasons, the service (ip, carrier) if effective, the epoch if entitled.
type Eligibility = (bool, Vec<String>, Option<(String, String)>, Option<i64>);

fn eligibility(db: &Connection, p: &Publisher) -> Result<Eligibility, Error> {
    let mut reasons = vec![];
    let mut epoch = None;
    match crate::activation::refuse_if_not_activated(db, &p.resource, "publisher_start") {
        Ok(()) => epoch = crate::activation::lease(db, &p.resource)?.map(|l| l.epoch),
        Err(e) => reasons.push(e.to_string()),
    }
    if crate::activation::policy(db, &p.resource)?.is_none() {
        reasons.push(format!("{} is under no activation policy on this host", p.resource));
    }
    let service = service_here(db, &p.resource)?;
    if service.is_none() {
        reasons.push("no effective exclusive route and alias for the resource on this host: publish the service address first".into());
    }
    match crate::secrets::declared(db, &p.credential) {
        Ok(()) => {}
        Err(e) => reasons.push(e.to_string()),
    }
    Ok((reasons.is_empty(), reasons, service, epoch))
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
    let (eligible, reasons, service, epoch) = eligibility(db, &p)?;
    let lease = crate::activation::lease(db, resource)?;
    let readiness = service.as_ref().map(|(ip, _)| match origin_ready(ip, p.origin_port) {
        Ok((status, body)) => json!({"status": status, "body": body}),
        Err(e) => json!({"error": e}),
    });
    let carrier_identity = service.as_ref().and_then(|(_, carrier)| {
        crate::manager::execute(db, &json!({"operation": "manager_status", "universe_uuid": carrier})).ok().map(|s| s["resident_status"]["replica_id"].clone())
    });
    Ok(json!({
        "resource": resource,
        "declared": {"hostname": p.hostname, "tunnel_uuid": p.tunnel_uuid, "credential": p.credential, "origin_port": p.origin_port},
        "unit": {"name": unit_name(resource), "state": unit_state(resource)},
        "connector_id": connector_id(resource),
        "lease": lease.as_ref().map(|l| json!({"holder_host_uuid": l.holder_host_uuid, "expires_at": l.expires_at, "epoch": l.epoch})),
        "epoch": epoch,
        "service": service.as_ref().map(|(ip, carrier)| json!({"ip": ip, "carrier_universe_uuid": carrier})),
        "carrier_replica_id": carrier_identity,
        "origin_readiness": readiness,
        "governor_mark": service.as_ref().and_then(|(_, carrier)| mark_present(carrier)),
        "publisher_eligible": eligible,
        "reasons": reasons,
        "last": {"start": last_event(db, resource, "start")?, "stop": last_event(db, resource, "stop")?, "fence": last_event(db, resource, "fence")?,
                 "observed": last_event(db, resource, "observed")?},
        "scope": "this host's declaration, unit, journal and tables, and the origin asked now; the connector's identity is what cloudflared logged; Cloudflare's side is observed only through publisher_observed",
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
            if connector_present(resource) == Some(true) {
                return Err("the connector is already active on this host".into());
            }
            // 1. the epoch gate, 2. the service address effective here, 3. the credential
            let (eligible, reasons, service, epoch) = eligibility(db, &p)?;
            if !eligible {
                return Err(format!("publisher_start refused: {}", reasons.join("; ")).into());
            }
            let (ip, carrier) = service.ok_or("no service address")?;
            let epoch = epoch.ok_or("no epoch on the lease")?;
            // 4. the previous publisher accounted for: the agent's word, recorded, refused when absent
            let previous = request.get("previous").ok_or("publisher_start requires `previous`: {fenced: true, operation_id} for a previous publisher fenced, or {waited_seconds: n} after an unreachable one, or {none: true} for the first")?;
            let accounted = previous.get("fenced") == Some(&json!(true)) || previous.get("waited_seconds").and_then(Value::as_u64).is_some_and(|w| w > 0) || previous.get("none") == Some(&json!(true));
            if !accounted {
                return Err("publisher_start refused: the previous publisher is not accounted for (fenced, waited, or none)".into());
            }
            // 5. the governor mark inside the carrier, recorded first, verified; 6. readiness at the
            //    service address with the expected identities and epoch; 7. the connector, recorded
            //    first, verified active. Any failure undoes what was made, last first.
            let mut done = vec![];
            let mut failure: Option<String> = None;
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
                    match origin_ready(&ip, p.origin_port) {
                        Ok((200, body)) if body["logical_manager_id"] == json!(resource) && body["epoch"] == json!(epoch) && body["ready"] == json!(true) => {
                            let replica = crate::manager::execute(db, &json!({"operation": "manager_status", "universe_uuid": carrier}))?["resident_status"]["replica_id"].clone();
                            if body["replica_id"] != replica {
                                failure = Some(format!("the origin names replica {} but the carrier's resident is {}", body["replica_id"], replica));
                                break;
                            }
                        }
                        Ok((status, body)) => {
                            failure = Some(format!("the origin at {ip}:{} is not ready for this governor at epoch {epoch}: HTTP {status} {body}", p.origin_port));
                            break;
                        }
                        Err(e) => {
                            failure = Some(format!("the origin at {ip}:{} could not be asked: {e}", p.origin_port));
                            break;
                        }
                    }
                    lc::fault("publisher-after-readiness")?;
                }
            }
            if let Some(err) = failure {
                let report = crate::network::compensate_public(db, &done);
                event(db, resource, "start_failed", id, Some(json!({"error": err, "compensation": report})))?;
                return Err(format!("{err}; compensation: {}", json!(report)).into());
            }
            event(db, resource, "start", id, Some(json!({"epoch": epoch, "ip": ip, "carrier": carrier, "previous": previous})))?;
            view(db, resource)
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

/// The connector stopped and the mark removed, each recorded `removing` first and verified gone:
/// what `publisher_stop` and the fence do. The effects of the resource whose kind is the
/// publisher's or the mark's, last first (the connector before the mark).
fn withdraw(db: &Connection, resource: &str, why: &str, id: &str) -> Result<Value, Error> {
    let effects: Vec<_> = crate::network::effect_rows_public(db, Some(resource))?
        .into_iter()
        .filter(|e| e.kind == KIND_PUBLISHER || e.kind == KIND_MARK)
        .collect();
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
        let had = crate::network::effect_rows_public(db, Some(&resource))?.iter().any(|e| e.kind == KIND_PUBLISHER || e.kind == KIND_MARK)
            || connector_present(&resource) == Some(true);
        if had {
            report.push(withdraw(db, &resource, "fence", id)?);
        }
    }
    Ok(report)
}
