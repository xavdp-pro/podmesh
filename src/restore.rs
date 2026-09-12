//! Experimental destination side of serial migration: preflight, restore and abort.
//!
//! Protocol: docs/MIGRATION-PROTOCOL.md. A restore requires the inbox handoff of an authorization naming this
//! host. Every precondition is checked before a durable claim; the archive is then copied into the operation's
//! private directory, re-hashed and restored from that copy with the packaged runtime in its own transient
//! scope. An outcome document is written to the outbox only from a durable claim state: `restored` after
//! verification, or `not_restored` once this host has recorded that it will never restore the authorization.
use crate::cleanup::{self, Bound};
use crate::lifecycle::{self as lc, failure, Error};
use crate::migration as mg;
use crate::transfer as tr;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    process::{Command, Stdio},
};

const RESTORE_SECONDS: u64 = 300;
const UNIVERSE_LABEL: &str = "io.podmesh.universe";
const SCOPE: &str = "experimental destination restore: default rootful Podman store, handoff-bound archive restored with the packaged podmesh-vzcriu 3.15.5.3 through its private-path shim, network-disabled and mount-free universe";

pub(crate) struct Claim {
    pub authorization_id: String,
    pub operation_id: String,
    pub universe_uuid: String,
    pub handoff: String,
    pub handoff_sha256: String,
    pub source_host: String,
    pub source_container_id: String,
    pub image_id: String,
    pub state: String,
    pub container_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub outcome: Option<String>,
    pub outcome_sha256: Option<String>,
    pub detail: Option<String>,
}
const CLAIM_COLUMNS: &str = "authorization_id,operation_id,universe_uuid,handoff,handoff_sha256,source_host_uuid,source_container_id,image_id,state,container_id,created_at,updated_at,outcome,outcome_sha256,detail";
fn claim_row(r: &rusqlite::Row) -> rusqlite::Result<Claim> {
    Ok(Claim {
        authorization_id: r.get(0)?,
        operation_id: r.get(1)?,
        universe_uuid: r.get(2)?,
        handoff: r.get(3)?,
        handoff_sha256: r.get(4)?,
        source_host: r.get(5)?,
        source_container_id: r.get(6)?,
        image_id: r.get(7)?,
        state: r.get(8)?,
        container_id: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        outcome: r.get(12)?,
        outcome_sha256: r.get(13)?,
        detail: r.get(14)?,
    })
}
impl Claim {
    fn detail_value(&self) -> Value {
        self.detail
            .as_deref()
            .and_then(|d| serde_json::from_str::<Value>(d).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}))
    }
    fn view(&self) -> Value {
        json!({"authorization_id": self.authorization_id, "operation_id": self.operation_id, "universe_uuid": self.universe_uuid,
            "handoff_sha256": self.handoff_sha256, "source_host_uuid": self.source_host, "source_container_id": self.source_container_id,
            "image_id": self.image_id, "state": self.state, "container_id": self.container_id, "created_at": self.created_at,
            "updated_at": self.updated_at, "outcome_sha256": self.outcome_sha256, "detail": self.detail_value()})
    }
}
fn claim(db: &Connection, authorization: &str) -> Result<Option<Claim>, Error> {
    Ok(db
        .query_row(
            &format!("SELECT {CLAIM_COLUMNS} FROM migration_restore_claims WHERE authorization_id=?1"),
            [authorization],
            claim_row,
        )
        .optional()?)
}
pub(crate) fn claims_view(db: &Connection, uuid: &str) -> Result<Vec<Value>, Error> {
    let mut stmt = db.prepare(&format!(
        "SELECT {CLAIM_COLUMNS} FROM migration_restore_claims WHERE universe_uuid=?1 ORDER BY created_at, authorization_id"
    ))?;
    let rows = stmt.query_map([uuid], claim_row)?;
    Ok(rows.map(|k| k.map(|k| k.view())).collect::<Result<Vec<_>, _>>()?)
}
/// A claim that is neither verified nor closed: it may hold a container created by its restore.
pub(crate) fn unresolved_claim(db: &Connection, uuid: &str) -> Result<Option<Value>, Error> {
    Ok(db
        .query_row(
            &format!(
                "SELECT {CLAIM_COLUMNS} FROM migration_restore_claims WHERE universe_uuid=?1 AND state IN ('restoring','restore_failed') ORDER BY created_at LIMIT 1"
            ),
            [uuid],
            claim_row,
        )
        .optional()?
        .map(|k| k.view()))
}
fn merge_claim(db: &Connection, authorization: &str, state: &str, container: Option<&str>, patch: &Value) -> Result<(), Error> {
    let k = claim(db, authorization)?.ok_or("Restore claim not found")?;
    let mut detail = k.detail_value();
    if let (Some(d), Some(p)) = (detail.as_object_mut(), patch.as_object()) {
        for (key, v) in p {
            d.insert(key.clone(), v.clone());
        }
    }
    db.execute(
        "UPDATE migration_restore_claims SET state=?2, container_id=COALESCE(?3, container_id), updated_at=?4, detail=?5 WHERE authorization_id=?1",
        params![authorization, state, container, crate::now() as i64, detail.to_string()],
    )?;
    Ok(())
}
fn set_outcome(db: &Connection, authorization: &str, text: &str) -> Result<(), Error> {
    db.execute(
        "UPDATE migration_restore_claims SET outcome=?2, outcome_sha256=?3 WHERE authorization_id=?1",
        params![authorization, text, mg::sha256_bytes(text.as_bytes())?],
    )?;
    Ok(())
}
fn restore_unit(id: &str) -> String {
    format!("podmesh-restore-{id}.scope")
}

/// Whether a verified migration_restore on this host binds this universe to this container ID.
pub(crate) fn restored_here(db: &Connection, uuid: &str, container_id: &str) -> Result<bool, Error> {
    mg::ensure_schema(db)?;
    let row: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT o.request, o.result FROM migration_restore_claims k JOIN operations o ON o.id = k.operation_id
             WHERE k.universe_uuid=?1 AND k.container_id=?2 AND k.state='restored' AND o.status='verified'",
            params![uuid, container_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((request, result)) = row else {
        return Ok(false);
    };
    let request: Value = serde_json::from_str(&request)?;
    let result: Value = serde_json::from_str(result.as_deref().unwrap_or("null"))?;
    Ok(request["operation"] == "migration_restore"
        && request["universe_uuid"].as_str() == Some(uuid)
        && result["container_id"].as_str() == Some(container_id))
}
/// Whether a verified create, clone or migration_restore of this host's journal owns this container ID.
pub(crate) fn verified_owner(db: &Connection, container_id: &str) -> Result<bool, Error> {
    let claimed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM migration_restore_claims WHERE container_id=?1 AND state='restored')",
        [container_id],
        |r| r.get(0),
    )?;
    if claimed {
        return Ok(true);
    }
    // The ID is hexadecimal, so LIKE only narrows the scan; each candidate is checked exactly.
    let mut stmt = db.prepare("SELECT request,result FROM operations WHERE status='verified' AND result LIKE ?1")?;
    let rows = stmt.query_map([format!("%{container_id}%")], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    for row in rows {
        let (request, result) = row?;
        let request: Value = serde_json::from_str(&request)?;
        let result: Value = serde_json::from_str(result.as_deref().unwrap_or("null"))?;
        if ["create", "clone"].contains(&request["operation"].as_str().unwrap_or(""))
            && result["container_id"].as_str() == Some(container_id)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

struct Handoff {
    value: Value,
    bytes: Vec<u8>,
    sha256: String,
}
/// Loads and structurally validates the inbox handoff. A missing or malformed handoff is a refusal: no binding
/// can be evaluated from it.
fn load_handoff(authorization: &str) -> Result<Handoff, Error> {
    let (bytes, v) = tr::read_document(&tr::inbox(authorization)?, tr::HANDOFF)
        .map_err(|e| {
            failure(
                format!("The inbox handoff is unusable: {e}; nothing was claimed or restored"),
                json!({}),
            )
        })?
        .ok_or_else(|| {
            failure(
                "No handoff for this authorization in the inbox; nothing was claimed or restored",
                json!({"authorization_id": authorization}),
            )
        })?;
    let s = |k: &str| v[k].as_str().unwrap_or("");
    let mut malformed = vec![];
    if s("format") != tr::HANDOFF_FORMAT {
        malformed.push("format");
    }
    if s("authorization_id") != authorization {
        malformed.push("authorization_id");
    }
    for k in ["universe_uuid", "source_host_uuid", "destination_host_uuid"] {
        if !lc::is_uuid(s(k)) {
            malformed.push(k);
        }
    }
    for k in ["source_container_id", "image_id"] {
        if !tr::is_sha256(s(k)) {
            malformed.push(k);
        }
    }
    if !tr::is_token(s("checkpoint_operation_id")) {
        malformed.push("checkpoint_operation_id");
    }
    if v["archive"]["file"] != mg::ARCHIVE
        || v["archive"]["bytes"].as_u64().is_none()
        || !tr::is_sha256(v["archive"]["sha256"].as_str().unwrap_or(""))
    {
        malformed.push("archive");
    }
    if v["manifest"]["file"] != mg::MANIFEST || !tr::is_sha256(v["manifest"]["sha256"].as_str().unwrap_or("")) {
        malformed.push("manifest");
    }
    if v["runtime"]["git_id"].as_str().is_none() || !tr::is_sha256(v["runtime"]["binary_sha256"].as_str().unwrap_or("")) {
        malformed.push("runtime");
    }
    if s("kernel_release").is_empty() {
        malformed.push("kernel_release");
    }
    if !malformed.is_empty() {
        return Err(failure(
            "The inbox handoff is malformed; nothing was claimed or restored",
            json!({"malformed_fields": malformed}),
        ));
    }
    let sha256 = mg::sha256_bytes(&bytes)?;
    Ok(Handoff { value: v, bytes, sha256 })
}
fn labelled(uuid: &str) -> Result<Vec<Value>, Error> {
    Ok(tr::all_containers()?
        .into_iter()
        .filter(|c| c["Labels"][UNIVERSE_LABEL].as_str() == Some(uuid))
        .map(|c| json!({"id": c["Id"], "names": c["Names"], "state": c["State"]}))
        .collect())
}
/// The destination's fresh observation recorded in an outcome.
fn observation(uuid: &str) -> Result<Value, Error> {
    Ok(json!({"observed_at": crate::now(), "universe_container": lc::observe(uuid)?, "labelled_containers": labelled(uuid)?}))
}
/// Uncompressed size of the zstd archive, streamed through the distribution zstd under a bound.
pub(crate) fn uncompressed_bytes(archive: &Path) -> Result<u64, Error> {
    let mut child = Command::new("/usr/bin/timeout")
        .args(["--signal=KILL", "300", "/usr/bin/zstd", "-dcq", "--"])
        .arg(archive)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut output = child.stdout.take().ok_or("zstd output unavailable")?;
    let bytes = std::io::copy(&mut output, &mut std::io::sink())?;
    if !child.wait()?.success() {
        return Err("zstd could not decompress the archive".into());
    }
    Ok(bytes)
}
/// Output of one bounded command. A damaged archive must never be able to exhaust the service's memory, so a
/// command that produces more than the limit is killed and reported instead of being read to the end.
fn bounded_output(command: &mut Command, limit: u64) -> Result<(bool, Vec<u8>), Error> {
    use std::io::Read;
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let mut bytes = Vec::new();
    let mut stdout = child.stdout.take().ok_or("command output unavailable")?;
    stdout.by_ref().take(limit).read_to_end(&mut bytes)?;
    if bytes.len() as u64 >= limit {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("the command produced more than {limit} bytes of output").into());
    }
    Ok((child.wait()?.success(), bytes))
}
/// Podman's container configuration and the entry list of the archive, read without extracting to disk.
fn archive_contents(archive: &Path) -> Result<(Value, Vec<String>), Error> {
    const LIMIT: u64 = 4 * 1024 * 1024;
    let (readable, config) = bounded_output(
        Command::new("/usr/bin/timeout")
            .args(["--signal=KILL", "120", "/usr/bin/tar", "-xOf"])
            .arg(archive)
            .arg("config.dump"),
        LIMIT,
    )?;
    if !readable {
        return Err("the archive has no readable config.dump".into());
    }
    let (listed, listing) = bounded_output(
        Command::new("/usr/bin/timeout")
            .args(["--signal=KILL", "120", "/usr/bin/tar", "-tf"])
            .arg(archive),
        LIMIT,
    )?;
    if !listed {
        return Err("the archive entries cannot be listed".into());
    }
    Ok((
        serde_json::from_slice(&config)?,
        String::from_utf8_lossy(&listing).lines().map(str::to_string).collect(),
    ))
}

struct Assessment {
    handoff: Handoff,
    blockers: Vec<String>,
    facts: Value,
}
/// Every destination precondition, freshly observed and without effect. `own` names the operation whose claim
/// is being re-assessed, so that its own claim is not reported as a conflict.
fn assess(db: &Connection, uuid: &str, authorization: &str, named: Option<&Value>, own: Option<&str>) -> Result<Assessment, Error> {
    let host = mg::host_uuid(db)?;
    let h = load_handoff(authorization)?;
    let v = &h.value;
    let s = |k: &str| v[k].as_str().unwrap_or("");
    let mut blockers: Vec<String> = vec![];
    if s("universe_uuid") != uuid {
        blockers.push(format!("the handoff is for universe {}, not {uuid}", s("universe_uuid")));
    }
    if s("destination_host_uuid") != host {
        blockers.push(format!("the handoff destination {} is not this host", s("destination_host_uuid")));
    }
    if s("source_host_uuid") == host {
        blockers.push("the handoff source is this host".into());
    }
    if let Some(c) = named {
        blockers.push(format!(
            "the container name podmesh-{uuid} is occupied by container {} (state {})",
            c["Id"].as_str().unwrap_or(""),
            lc::status(c)
        ));
    }
    let labelled = labelled(uuid)?;
    let named_id = named.and_then(|c| c["Id"].as_str());
    for c in labelled.iter().filter(|c| c["id"].as_str() != named_id) {
        blockers.push(format!("container {} carries this universe label", c["id"].as_str().unwrap_or("")));
    }
    let reservation = mg::reservation(db, uuid)?;
    if let Some(ref r) = reservation {
        // A transferred history does not block a return trip; every other reservation state holds the universe here.
        if r.state != "transferred" {
            blockers.push(format!(
                "the universe has an active migration reservation on this host (operation {}, state {})",
                r.operation_id, r.state
            ));
        }
    }
    if let Some(k) = claim(db, authorization)? {
        if Some(k.operation_id.as_str()) != own {
            blockers.push(format!(
                "authorization {authorization} is already claimed on this host by operation {} (state {})",
                k.operation_id, k.state
            ));
        }
    }
    let mut stmt = db.prepare(
        "SELECT authorization_id,state FROM migration_restore_claims WHERE universe_uuid=?1 AND authorization_id<>?2 AND state IN ('restoring','restore_failed')",
    )?;
    let others = stmt
        .query_map(params![uuid, authorization], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (other, state) in others {
        blockers.push(format!(
            "the universe has an unresolved restore claim for authorization {other} (state {state})"
        ));
    }
    let image = s("image_id");
    let image_present = lc::images()?.iter().any(|i| lc::image_id(i) == image);
    if !image_present {
        blockers.push(format!(
            "image sha256:{image} is not present in the local store; PodMesh never pulls"
        ));
    }
    let runtime = mg::runtime_facts(&mut blockers);
    let local_binary = runtime["sha256"][mg::RUNTIME_REAL].as_str().unwrap_or("").to_string();
    if v["runtime"]["binary_sha256"].as_str() != Some(local_binary.as_str()) {
        blockers.push(format!(
            "the local runtime binary {local_binary} differs from the source runtime {}",
            v["runtime"]["binary_sha256"].as_str().unwrap_or("")
        ));
    }
    if v["runtime"]["git_id"].as_str() != Some(mg::RUNTIME_GIT_ID) {
        blockers.push(format!(
            "the source runtime git ID {} is not the qualified {}",
            v["runtime"]["git_id"].as_str().unwrap_or(""),
            mg::RUNTIME_GIT_ID
        ));
    }
    let kernel = runtime["kernel"].as_str().unwrap_or("").to_string();
    if s("kernel_release") != kernel {
        blockers.push(format!(
            "the local kernel release {kernel} differs from the source kernel {}",
            s("kernel_release")
        ));
    }
    let inbox = tr::inbox(authorization)?;
    let archive = inbox.join(mg::ARCHIVE);
    let mut archive_facts = json!({"present": false});
    let mut uncompressed = None;
    match tr::regular_file(&archive) {
        Ok(Some(size)) => {
            let sha = mg::sha256(&archive)?;
            let intact = Some(sha.as_str()) == v["archive"]["sha256"].as_str() && Some(size) == v["archive"]["bytes"].as_u64();
            archive_facts = json!({"present": true, "bytes": size, "sha256": sha, "matches_handoff": intact});
            if !intact {
                blockers.push(format!(
                    "the archive in the inbox ({size} bytes, sha256 {sha}) does not match the handoff"
                ));
            } else {
                // Only intact bytes are opened: the configuration Podman will import must name this universe and source.
                match archive_contents(&archive) {
                    Ok((config, entries)) => {
                        let label = config["labels"][UNIVERSE_LABEL].as_str();
                        if config["id"].as_str() != Some(s("source_container_id"))
                            || config["rootfsImageID"].as_str() != Some(image)
                            || label != Some(uuid)
                        {
                            blockers.push(
                                "the archive configuration does not name the handoff's source container, image and universe label".into(),
                            );
                        }
                        let missing: Vec<&str> = ["config.dump", "spec.dump", "checkpoint/inventory.img"]
                            .into_iter()
                            .filter(|e| !entries.iter().any(|l| l == e))
                            .collect();
                        if !missing.is_empty() {
                            blockers.push(format!("the archive lacks expected entries: {}", missing.join(", ")));
                        }
                        archive_facts["config"] = json!({"container_id": config["id"], "image_id": config["rootfsImageID"],
                            "universe_label": label, "create_network_namespace": config["createNetNS"], "entries": entries.len()});
                    }
                    Err(e) => blockers.push(format!("the archive cannot be read: {e}")),
                }
                match uncompressed_bytes(&archive) {
                    Ok(b) => {
                        uncompressed = Some(b);
                        archive_facts["uncompressed_bytes"] = json!(b);
                    }
                    Err(e) => blockers.push(format!("the archive cannot be decompressed: {e}")),
                }
            }
        }
        Ok(None) => blockers.push("the archive is missing from the inbox".into()),
        Err(e) => blockers.push(format!("the archive in the inbox is unusable: {e}")),
    }
    let manifest = inbox.join(mg::MANIFEST);
    let mut manifest_facts = json!({"present": false});
    match tr::regular_file(&manifest) {
        Ok(Some(_)) => {
            let sha = mg::sha256(&manifest)?;
            let intact = Some(sha.as_str()) == v["manifest"]["sha256"].as_str();
            manifest_facts = json!({"present": true, "sha256": sha, "matches_handoff": intact});
            if !intact {
                blockers.push(format!("the manifest in the inbox (sha256 {sha}) does not match the handoff"));
            } else {
                let m: Value = serde_json::from_slice(&fs::read(&manifest)?).unwrap_or(Value::Null);
                for (field, agrees) in [
                    ("operation_id", m["operation_id"].as_str() == Some(s("checkpoint_operation_id"))),
                    ("universe_uuid", m["universe_uuid"].as_str() == Some(s("universe_uuid"))),
                    ("container_id", m["container_id"].as_str() == Some(s("source_container_id"))),
                    ("image_id", m["image_id"].as_str() == Some(image)),
                    ("source_host_uuid", m["source_host_uuid"].as_str() == Some(s("source_host_uuid"))),
                    (
                        "destination_host_uuid",
                        m["destination_host_uuid"].as_str() == Some(s("destination_host_uuid")),
                    ),
                    (
                        "archive",
                        m["archive"]["sha256"] == v["archive"]["sha256"] && m["archive"]["bytes"] == v["archive"]["bytes"],
                    ),
                    ("runtime", m["runtime"]["sha256"][mg::RUNTIME_REAL] == v["runtime"]["binary_sha256"]),
                    ("kernel", m["runtime"]["kernel"] == v["kernel_release"]),
                ] {
                    if !agrees {
                        blockers.push(format!("the manifest disagrees with the handoff on {field}"));
                    }
                }
            }
        }
        Ok(None) => blockers.push("the manifest is missing from the inbox".into()),
        Err(e) => blockers.push(format!("the manifest in the inbox is unusable: {e}")),
    }
    let state_required = v["archive"]["bytes"].as_u64().unwrap_or(0).saturating_add(mg::SPACE_MARGIN_BYTES);
    let state_available = mg::available_bytes(mg::base()?);
    if state_available < state_required {
        blockers.push(format!(
            "{state_available} bytes available under the state directory, {state_required} required"
        ));
    }
    let graph_root = lc::podman(lc::QUICK, &["info", "--format", "{{.Store.GraphRoot}}"])?
        .trim()
        .to_string();
    if graph_root != mg::CONTAINER_STORAGE {
        blockers.push(format!(
            "Podman graph root {graph_root} is not the qualified default store {}",
            mg::CONTAINER_STORAGE
        ));
    }
    // Podman extracts the archive into the new container's storage and keeps the restore files (--keep).
    let graph_required = uncompressed.unwrap_or(0).saturating_mul(2).saturating_add(mg::SPACE_MARGIN_BYTES);
    let graph_available = mg::available_bytes(Path::new(&graph_root));
    if graph_available < graph_required {
        blockers.push(format!(
            "{graph_available} bytes available under {graph_root}, {graph_required} required"
        ));
    }
    let facts = json!({
        "observed_at": crate::now(), "host_uuid": host, "authorization_id": authorization, "handoff_sha256": h.sha256,
        "handoff": v, "name_occupied_by": named.map(lc::state_view), "labelled_containers": labelled,
        "reservation": reservation.as_ref().map(|r| r.view()), "image_present": image_present, "runtime": runtime,
        "archive": archive_facts, "manifest": manifest_facts,
        "space": {"state_directory": {"available_bytes": state_available, "required_bytes": state_required},
                  "graph_root": {"path": graph_root, "available_bytes": graph_available, "required_bytes": graph_required}},
    });
    Ok(Assessment {
        handoff: h,
        blockers,
        facts,
    })
}

pub(crate) fn preflight(db: &Connection, uuid: &str, authorization: &str, existing: Option<Value>) -> Result<Value, Error> {
    let a = assess(db, uuid, authorization, existing.as_ref(), None)?;
    Ok(json!({
        "status": "verified", "operation": "migration_destination_preflight", "universe_uuid": uuid, "authorization_id": authorization,
        "compatible": a.blockers.is_empty(), "blockers": a.blockers, "facts": a.facts,
        "effects": "none: preflight does not claim, copy, create or restore anything", "scope": SCOPE,
    }))
}

/// The operation directory of a restore without a claim may only hold the copies made before claiming (a crash
/// can leave them); they are replaced. Anything else is refused.
fn prepare_operation_dir(dir: &Path) -> Result<(), Error> {
    const PRE_CLAIM: [&str; 6] = [
        mg::ARCHIVE,
        mg::MANIFEST,
        tr::HANDOFF,
        "checkpoint.tar.partial-write",
        "manifest.partial-write",
        "handoff.partial-write",
    ];
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(dir)?,
        Err(e) => return Err(e.into()),
        Ok(m) if m.is_dir() => {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let file = entry.file_name().to_string_lossy().to_string();
                if !PRE_CLAIM.contains(&file.as_str()) || !entry.file_type()?.is_file() {
                    return Err(format!("The operation directory holds {file} without a restore claim; refusing to reuse it").into());
                }
            }
        }
        Ok(_) => return Err("The operation directory path is not a directory".into()),
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub(crate) fn restore(
    db: &Connection,
    attempt: i64,
    id: &str,
    uuid: &str,
    name: &str,
    authorization: &str,
    existing: Option<Value>,
) -> Result<Value, Error> {
    if let Some(k) = claim(db, authorization)? {
        if k.operation_id != id {
            return Err(failure(
                format!(
                    "Authorization {authorization} is already claimed on this host by operation {} (state {}); nothing was restored",
                    k.operation_id, k.state
                ),
                json!({"restore_claim": k.view()}),
            ));
        }
        return resume(db, attempt, id, uuid, name, k, existing);
    }
    let a = assess(db, uuid, authorization, existing.as_ref(), None)?;
    if !a.blockers.is_empty() {
        return Err(failure(
            "Restore preconditions not met; nothing was claimed, copied or restored",
            json!({"blockers": a.blockers, "facts": a.facts}),
        ));
    }
    let v = &a.handoff.value;
    let dir = mg::base()?.join(id);
    prepare_operation_dir(&dir)?;
    // Podman reads a private copy hashed after copying: a later change in the inbox cannot reach the restore.
    let expected = v["archive"]["sha256"].as_str().unwrap_or("");
    let inbox = tr::inbox(authorization)?;
    mg::copy_private(&inbox.join(mg::ARCHIVE), &dir.join(mg::ARCHIVE))?;
    let copied = mg::sha256(&dir.join(mg::ARCHIVE))?;
    if copied != expected {
        let _ = fs::remove_file(dir.join(mg::ARCHIVE));
        return Err(failure(
            format!("The archive changed in the inbox while it was copied (sha256 {copied}); nothing was claimed or restored"),
            json!({"expected_sha256": expected}),
        ));
    }
    mg::copy_private(&inbox.join(mg::MANIFEST), &dir.join(mg::MANIFEST))?;
    mg::write_private(&dir.join(tr::HANDOFF), &a.handoff.bytes)?;
    let now = crate::now() as i64;
    // The claim is durable before any Podman effect.
    db.execute(
        "INSERT INTO migration_restore_claims VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'restoring',NULL,?9,?9,NULL,NULL,'{}')",
        params![
            authorization,
            id,
            uuid,
            String::from_utf8(a.handoff.bytes.clone())?,
            a.handoff.sha256,
            v["source_host_uuid"].as_str(),
            v["source_container_id"].as_str(),
            v["image_id"].as_str(),
            now
        ],
    )?;
    mg::write_private(&dir.join("preflight.json"), serde_json::to_string_pretty(&a.facts)?.as_bytes())?;
    let k = claim(db, authorization)?.ok_or("Restore claim not persisted")?;
    let graph = a.facts["space"]["graph_root"].clone();
    launch(db, attempt, id, uuid, name, &k, &dir, false, &graph)
}

#[allow(clippy::too_many_arguments)]
fn launch(
    db: &Connection,
    attempt: i64,
    id: &str,
    uuid: &str,
    name: &str,
    k: &Claim,
    dir: &Path,
    resumed: bool,
    graph: &Value,
) -> Result<Value, Error> {
    // Durable before the command can start: a claim without this mark never reached Podman.
    merge_claim(db, &k.authorization_id, "restoring", None, &json!({"launched_attempt": attempt}))?;
    let open = |path: &Path| fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path);
    let stdout = open(&dir.join(format!("restore-attempt-{attempt}.stdout")))?;
    let stderr = open(&dir.join(format!("restore-attempt-{attempt}.stderr")))?;
    let import = format!("--import={}", dir.join(mg::ARCHIVE).display());
    let unit = restore_unit(id);
    // The attempt may consume what its own preflight required of the graph root, and no more: a restore
    // of a damaged archive was measured writing about 20 MB/s without the command ever returning.
    let bound = graph["path"]
        .as_str()
        .map(|p| Bound::new(Path::new(p), graph["required_bytes"].as_u64().unwrap_or(0), &unit));
    let exit = mg::scoped_podman(
        &unit,
        id,
        RESTORE_SECONDS,
        &[
            "container",
            "restore",
            &import,
            "--name",
            name,
            "--keep",
            "--file-locks",
            "--print-stats",
        ],
        stdout,
        stderr,
        bound.as_ref(),
    );
    match exit {
        Err(e) => {
            // The command was never started: Podman did not begin restoring.
            let detail = json!({"reason": format!("the restore command could not be started: {e}"), "attempt": attempt});
            close(db, &k.authorization_id, id, None, &detail, observation(uuid)?)?;
            Err(failure(
                "The restore command could not be started; this host recorded not_restored and wrote the outcome",
                detail,
            ))
        }
        Ok((status, prevention)) => finalize(db, attempt, attempt, id, uuid, name, k, dir, status.code(), resumed, prevention),
    }
}

/// Verified only when Podman shows the universe container created after this claim, running, restored,
/// network-disabled and mount-free, from the handoff image, and its preserved CRIU restore log shows a successful
/// restore by the qualified private runtime. The command's exit code alone proves nothing.
fn verify(k: &Claim, observed: Option<&Value>, log: &Path, log_preserved: bool) -> Result<(), String> {
    let c = observed.ok_or("the universe container is absent")?;
    if c["Config"]["Labels"][UNIVERSE_LABEL].as_str() != Some(k.universe_uuid.as_str()) {
        return Err("the container does not carry the handoff's universe label".into());
    }
    if lc::status(c) != "running" || !lc::process_active(c) {
        return Err(format!("the container is not running (state {})", lc::status(c)));
    }
    // Podman keeps reporting a running container whose cgroup the bound froze, because the freeze goes
    // to the kernel and not through Podman's own bookkeeping. A suspended universe is not restored.
    if cleanup::frozen(c["Id"].as_str().unwrap_or("")) {
        return Err("the container's cgroup is frozen: the universe is suspended, not running".into());
    }
    if c["State"]["Restored"] != true {
        return Err("Podman does not report the container as restored".into());
    }
    let after_claim = |v: &Value| v.as_str().and_then(lc::epoch).is_some_and(|t| t >= k.created_at);
    if !after_claim(&c["Created"]) || !after_claim(&c["State"]["RestoredAt"]) {
        return Err("the container was not created and restored after this claim".into());
    }
    if c["Image"].as_str().map(|i| i.trim_start_matches("sha256:")) != Some(k.image_id.as_str()) {
        return Err("the container image is not the handoff image".into());
    }
    if c["HostConfig"]["NetworkMode"].as_str() != Some("none") || c["Mounts"].as_array().map(|m| !m.is_empty()).unwrap_or(true) {
        return Err("the container is not network-disabled and mount-free".into());
    }
    if !log_preserved {
        return Err("the CRIU restore log is unavailable".into());
    }
    let text = fs::read_to_string(log).unwrap_or_default();
    if !text.contains(&format!("(gitid {})", mg::RUNTIME_GIT_ID)) || !text.contains("Restore finished successfully") {
        return Err("the restore log does not show a successful restore by the qualified private runtime".into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finalize(
    db: &Connection,
    attempt: i64,
    launched: i64,
    id: &str,
    uuid: &str,
    name: &str,
    k: &Claim,
    dir: &Path,
    exit: Option<i32>,
    resumed: bool,
    mut prevention: Value,
) -> Result<Value, Error> {
    let observed = lc::inspect(name)?;
    // A freeze the bound applied is confirmed against the container this claim actually created, and
    // undone if it caught anything else.
    cleanup::confirm_or_thaw(&mut prevention, observed.as_ref().and_then(|c| c["Id"].as_str()));
    let log = dir.join(format!("restore-attempt-{launched}.log"));
    let log_preserved = observed
        .as_ref()
        .is_some_and(|c| mg::copy_podman_log(c, "RestoreLog", "restore.log", &log));
    if let Err(reason) = verify(k, observed.as_ref(), &log, log_preserved) {
        let stderr = mg::read_bounded(&dir.join(format!("restore-attempt-{launched}.stderr"))).unwrap_or_default();
        let detail = json!({"reason": reason, "attempt": attempt, "launched_attempt": launched, "exit_code": exit,
            "observed": observed.as_ref().map(lc::state_view), "restored": observed.as_ref().map(|c| c["State"]["Restored"].clone()),
            "restore_log_preserved": log_preserved.then(|| log.display().to_string()), "stderr_tail": mg::tail(&stderr),
            "restore_scope_finished": !mg::unit_busy(&restore_unit(id)), "prevention": prevention,
            // A freeze the bound applied stays applied: it is what stopped a runaway from writing, and
            // only an explicit reclaim ends those processes. It is reported, never left to be guessed.
            "container_cgroup_frozen_now": observed.as_ref().and_then(|c| c["Id"].as_str()).map(cleanup::frozen),
            "runtime_processes": observed.as_ref().and_then(|c| c["Id"].as_str())
                .map(|cid| cleanup::runtime_processes(cid, k.created_at))});
        mg::write_private(
            &dir.join(format!("failure-attempt-{attempt}.json")),
            serde_json::to_string_pretty(&detail)?.as_bytes(),
        )?;
        // A failed claim records the container its own attempt created — labelled for this universe and
        // created after the claim — so that an abort, and any observer reading the journal, can name what
        // has to be cleaned up. Nothing else is ever recorded here.
        let created_here = observed.as_ref().filter(|c| {
            c["Config"]["Labels"][UNIVERSE_LABEL].as_str() == Some(k.universe_uuid.as_str())
                && c["Created"].as_str().and_then(lc::epoch).is_some_and(|t| t >= k.created_at)
        });
        merge_claim(
            db,
            &k.authorization_id,
            "restore_failed",
            created_here.and_then(|c| c["Id"].as_str()),
            &detail,
        )?;
        return Err(failure(
            format!("The restore could not be verified: {reason}; the claim is held and no outcome was written. migration_restore_abort removes only a non-running container created by this claim."),
            detail,
        ));
    }
    let c = observed.ok_or("Restored container not observable")?;
    let container_id = c["Id"].as_str().unwrap_or("").to_string();
    mg::write_private(&dir.join("restore.log"), &mg::read_bounded(&log)?)?;
    let now = crate::now() as i64;
    let tx = db.unchecked_transaction()?;
    let archived = mg::archive_transferred(&tx, uuid, id)?;
    merge_claim(
        &tx,
        &k.authorization_id,
        "restored",
        Some(&container_id),
        &json!({"verified_at": now, "verified_attempt": attempt, "launched_attempt": launched, "exit_code": exit,
            "reservation_history_archived": archived, "restore_log_sha256": mg::sha256(&log)?, "prevention": prevention}),
    )?;
    let k = claim(&tx, &k.authorization_id)?.ok_or("Restore claim disappeared")?;
    let outcome = outcome_text(
        &k,
        id,
        "restored",
        Some(&container_id),
        json!({"observed_at": now, "universe_container": lc::state_view(&c), "restored": c["State"]["Restored"],
            "restored_at": c["State"]["RestoredAt"], "network_mode": c["HostConfig"]["NetworkMode"], "image_id": c["Image"],
            "universe_label": c["Config"]["Labels"][UNIVERSE_LABEL]}),
        now,
    )?;
    set_outcome(&tx, &k.authorization_id, &outcome)?;
    tx.commit()?;
    let k = claim(db, &k.authorization_id)?.ok_or("Restore claim disappeared")?;
    restored_result(uuid, &k, dir, resumed)
}

fn outcome_text(k: &Claim, operation: &str, result: &str, container: Option<&str>, observation: Value, now: i64) -> Result<String, Error> {
    let handoff: Value = serde_json::from_str(&k.handoff)?;
    let outcome = json!({
        "format": tr::OUTCOME_FORMAT, "authorization_id": k.authorization_id, "handoff_sha256": k.handoff_sha256,
        "universe_uuid": k.universe_uuid, "source_host_uuid": k.source_host, "destination_host_uuid": handoff["destination_host_uuid"],
        "destination_operation_id": operation, "claim_operation_id": k.operation_id, "result": result,
        "restored_container_id": container, "destination_observation": observation, "claimed_at": k.created_at, "decided_at": now,
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&outcome)?))
}
/// Writes the claim's recorded outcome to the outbox unless an identical file is already there.
fn publish_outcome(k: &Claim) -> Result<Value, Error> {
    let text = k.outcome.as_deref().ok_or("No recorded outcome")?;
    let sha = k.outcome_sha256.as_deref().ok_or("No recorded outcome hash")?;
    let out = tr::outbox(&k.authorization_id)?;
    tr::private_dir(&out)?;
    let target = out.join(tr::OUTCOME);
    if !(tr::regular_file(&target)?.is_some() && mg::sha256(&target)? == sha) {
        mg::write_private(&target, text.as_bytes())?;
        if mg::sha256(&target)? != sha {
            return Err("The outcome written to the outbox does not hash to the recorded value".into());
        }
    }
    Ok(json!({"file": target, "sha256": sha, "bytes": text.len()}))
}
fn restored_result(uuid: &str, k: &Claim, dir: &Path, resumed: bool) -> Result<Value, Error> {
    let outcome_file = publish_outcome(k)?;
    let outcome: Value = serde_json::from_str(k.outcome.as_deref().unwrap_or("null"))?;
    let handoff: Value = serde_json::from_str(&k.handoff)?;
    let detail = k.detail_value();
    let current = lc::inspect(&format!("podmesh-{uuid}"))?;
    let conmon_cgroup = current
        .as_ref()
        .filter(|c| c["Id"].as_str() == k.container_id.as_deref())
        .and_then(|c| c["State"]["ConmonPid"].as_i64())
        .filter(|p| *p > 0)
        .and_then(|p| fs::read_to_string(format!("/proc/{p}/cgroup")).ok())
        .map(|s| s.trim().to_string());
    let unit = restore_unit(&k.operation_id);
    Ok(json!({
        "status": "verified", "operation": "migration_restore", "universe_uuid": uuid, "authorization_id": k.authorization_id,
        "handoff_sha256": k.handoff_sha256, "source_host_uuid": k.source_host, "source_container_id": k.source_container_id,
        "container_id": k.container_id, "image_id": k.image_id, "restored_observed": outcome["destination_observation"],
        "outcome": {"result": "restored", "file": outcome_file["file"], "sha256": k.outcome_sha256},
        "restore_log": {"file": dir.join("restore.log"), "sha256": detail["restore_log_sha256"], "runtime_git_id": mg::RUNTIME_GIT_ID},
        "restore_scope": {"unit": unit, "finished": !mg::unit_busy(&unit)}, "conmon_cgroup": conmon_cgroup,
        "prevention": detail["prevention"],
        "artifact_directory": dir, "archive": handoff["archive"],
        "reservation_history_archived": detail["reservation_history_archived"], "finalized_after_interruption": resumed,
        "ownership": "this verified migration_restore binds the universe UUID to the restored container ID in this host's journal",
        "scope": SCOPE,
    }))
}
/// Closes a claim as `not_restored` and records its outcome in one transaction, before the outcome file is
/// written: this host never restores that authorization afterwards.
fn close(
    db: &Connection,
    authorization: &str,
    operation: &str,
    removed: Option<&str>,
    detail: &Value,
    observation: Value,
) -> Result<Claim, Error> {
    let now = crate::now() as i64;
    let mut patch = detail.clone();
    patch["closed_by_operation"] = json!(operation);
    patch["closed_at"] = json!(now);
    patch["removed_container_id"] = json!(removed);
    let tx = db.unchecked_transaction()?;
    merge_claim(&tx, authorization, "not_restored", None, &patch)?;
    let k = claim(&tx, authorization)?.ok_or("Restore claim not found")?;
    set_outcome(
        &tx,
        authorization,
        &outcome_text(&k, operation, "not_restored", None, observation, now)?,
    )?;
    tx.commit()?;
    let k = claim(db, authorization)?.ok_or("Restore claim disappeared")?;
    publish_outcome(&k)?;
    Ok(k)
}

fn resume(db: &Connection, attempt: i64, id: &str, uuid: &str, name: &str, k: Claim, existing: Option<Value>) -> Result<Value, Error> {
    let unit = restore_unit(id);
    if mg::unit_busy(&unit) {
        return Err(failure(
            "The restore scope of this operation has not finished, or its state cannot be queried; retry after it finishes",
            json!({"restore_claim": k.view(), "scope": unit}),
        ));
    }
    let dir = mg::base()?.join(id);
    match k.state.as_str() {
        "restored" => restored_result(uuid, &k, &dir, true),
        "not_restored" => Err(failure(
            "This restore claim is closed as not_restored: this host will not restore the authorization; a new authorization is required",
            json!({"restore_claim": k.view()}),
        )),
        _ => match k.detail_value()["launched_attempt"].as_i64() {
            // No attempt reached the restore command, so nothing Podman created belongs to this claim.
            None => {
                let a = assess(db, uuid, &k.authorization_id, existing.as_ref(), Some(id))?;
                if !a.blockers.is_empty() {
                    return Err(failure(
                        "Restore preconditions are no longer met; nothing was restored and the claim is kept",
                        json!({"blockers": a.blockers, "facts": a.facts}),
                    ));
                }
                let expected = a.handoff.value["archive"]["sha256"].as_str().unwrap_or("");
                if a.handoff.sha256 != k.handoff_sha256 || mg::sha256(&dir.join(mg::ARCHIVE))? != expected {
                    return Err(failure(
                        "The inbox handoff or the private archive copy no longer matches this claim; nothing was restored",
                        json!({"restore_claim": k.view()}),
                    ));
                }
                let graph = a.facts["space"]["graph_root"].clone();
                launch(db, attempt, id, uuid, name, &k, &dir, true, &graph)
            }
            // The command may have run: only observation decides, and nothing is restored twice.
            Some(launched) => finalize(
                db,
                attempt,
                launched,
                id,
                uuid,
                name,
                &k,
                &dir,
                None,
                true,
                json!({"watched": false, "reason": "this attempt did not run the restore command"}),
            ),
        },
    }
}

/// Ends a restore claim without a restore. A verified restore is never aborted. A held claim is closed only
/// after its scope finished and a non-running container created after the claim, owned by nothing verified,
/// has been removed and its absence verified. An authorization never claimed here is declined: the closed
/// claim is recorded first, so that this host never restores it afterwards.
///
/// `reclaim` is the caller's explicit, default-false `reclaim_processes`. Without it nothing is signalled:
/// if processes of the attempt survive in the container's cgroups, the abort refuses and reports them
/// rather than removing a container whose writers are still holding its files. With it, only processes
/// proven to be members of that container's own `libpod-<id>.scope` or `libpod-conmon-<id>.scope`, with a
/// start time at or after the claim, are ended; the field is the requester's provenance, never the proof.
#[allow(clippy::too_many_arguments)]
pub(crate) fn abort(
    db: &Connection,
    id: &str,
    uuid: &str,
    name: &str,
    authorization: &str,
    reference: &str,
    reclaim: bool,
    existing: Option<Value>,
) -> Result<Value, Error> {
    let host = mg::host_uuid(db)?;
    let Some(k) = claim(db, authorization)? else {
        let h = load_handoff(authorization)?;
        let v = &h.value;
        if v["universe_uuid"].as_str() != Some(uuid) || v["destination_host_uuid"].as_str() != Some(host.as_str()) {
            return Err(failure(
                "The inbox handoff does not name this universe and this destination host; nothing was recorded",
                json!({"handoff_sha256": h.sha256}),
            ));
        }
        let now = crate::now() as i64;
        let detail = json!({"declined_without_restore": true, "closed_by_operation": id, "closed_at": now, "removed_container_id": null});
        let tx = db.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO migration_restore_claims VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'not_restored',NULL,?9,?9,NULL,NULL,?10)",
            params![
                authorization,
                id,
                uuid,
                String::from_utf8(h.bytes.clone())?,
                h.sha256,
                v["source_host_uuid"].as_str(),
                v["source_container_id"].as_str(),
                v["image_id"].as_str(),
                now,
                detail.to_string()
            ],
        )?;
        let k = claim(&tx, authorization)?.ok_or("Restore claim not persisted")?;
        set_outcome(
            &tx,
            authorization,
            &outcome_text(&k, id, "not_restored", None, observation(uuid)?, now)?,
        )?;
        tx.commit()?;
        let k = claim(db, authorization)?.ok_or("Restore claim disappeared")?;
        return abort_result(uuid, &k, "declined_without_restore");
    };
    match k.state.as_str() {
        "restored" => Err(failure(
            format!(
                "The restore of authorization {authorization} was verified (container {}); aborting would remove the active universe. Nothing was changed.",
                k.container_id.as_deref().unwrap_or("")
            ),
            json!({"restore_claim": k.view()}),
        )),
        "not_restored" => {
            let action = if k.detail_value()["closed_by_operation"].as_str() == Some(id) {
                "closed_by_this_operation"
            } else {
                "none_already_not_restored"
            };
            abort_result(uuid, &k, action)
        }
        _ => {
            let unit = restore_unit(&k.operation_id);
            if mg::unit_busy(&unit) {
                return Err(failure(
                    "The restore scope of the claim has not finished, or its state cannot be queried; nothing was removed",
                    json!({"scope": unit, "restore_claim": k.view()}),
                ));
            }
            let graph_root = lc::podman(lc::QUICK, &["info", "--format", "{{.Store.GraphRoot}}"])?.trim().to_string();
            let space_before = mg::available_bytes(Path::new(&graph_root));
            let mut removed = None;
            let mut reclaimed = Value::Null;
            let mut before_removal = Value::Null;
            if let Some(c) = existing {
                let refuse = |reason: &str, extra: Value| {
                    failure(
                        format!("{reason}; nothing was removed"),
                        json!({"observed": lc::state_view(&c), "restore_claim": k.view(), "detail": extra}),
                    )
                };
                if c["Config"]["Labels"][UNIVERSE_LABEL].as_str() != Some(uuid) {
                    return Err(refuse(
                        "The container under the universe name is not labelled for this universe, so this claim did not create it",
                        Value::Null,
                    ));
                }
                if lc::process_active(&c) {
                    return Err(refuse(
                        "The container is running: abort never removes a running universe; retry migration_restore to verify it, or investigate it",
                        Value::Null,
                    ));
                }
                if !c["Created"].as_str().and_then(lc::epoch).is_some_and(|t| t >= k.created_at) {
                    return Err(refuse("The container predates this claim", Value::Null));
                }
                let container_id = c["Id"].as_str().unwrap_or("").to_string();
                if verified_owner(db, &container_id)? {
                    return Err(refuse("The container is owned by a verified operation of this host", Value::Null));
                }
                // Every fact a reclaim is judged on, read before anything is removed or signalled.
                let leftovers = cleanup::runtime_processes(&container_id, k.created_at);
                before_removal = leftovers.clone();
                let surviving = leftovers["count"].as_u64().unwrap_or(0);
                if surviving > 0 && !reclaim {
                    return Err(refuse(
                        "Processes of the failed restore are still in this container's cgroups: removing it now would unlink the files they are writing without freeing the space. They are reported, not ended; retry with reclaim_processes: true to end the ones this host can prove belong to the attempt",
                        json!({"runtime_processes": leftovers, "graph_root": graph_root, "available_bytes": space_before}),
                    ));
                }
                if reclaim {
                    // Provenance of the request, recorded verbatim beside the facts; the proof is what gates
                    // the act, and it is re-read for every PID immediately before its signal.
                    let mut outcome = cleanup::reclaim(&container_id, k.created_at);
                    outcome["requested_by"] = json!({"operation_id": id, "authorization_ref": reference, "reclaim_processes": true});
                    outcome["graph_root"] = json!(graph_root);
                    outcome["available_bytes_before"] = json!(space_before);
                    mg::write_private(
                        &mg::base()?.join(&k.operation_id).join(format!("reclaim-{id}.json")),
                        serde_json::to_string_pretty(&outcome)?.as_bytes(),
                    )?;
                    if outcome["complete"] != true && surviving > 0 {
                        let reason = outcome["incomplete_reason"].as_str().unwrap_or("").to_string();
                        reclaimed = outcome;
                        return Err(refuse(
                            &format!("The reclaim did not end every process of the failed restore ({reason})"),
                            json!({"reclaim": reclaimed}),
                        ));
                    }
                    reclaimed = outcome;
                }
                // Keep the CRIU restore log of the failed restore before its container storage is removed.
                let _ = mg::copy_podman_log(
                    &c,
                    "RestoreLog",
                    "restore.log",
                    &mg::base()?.join(&k.operation_id).join(format!("abort-{id}-restore.log")),
                );
                lc::podman(lc::QUICK, &["rm", &container_id])?;
                if lc::inspect(name)?.is_some() || tr::all_containers()?.iter().any(|x| x["Id"].as_str() == Some(container_id.as_str())) {
                    return Err("The container is still present after removal".into());
                }
                removed = Some(container_id);
            }
            let mut observed = observation(uuid)?;
            if observed["universe_container"]["present"] == true {
                return Err("A universe container is present after the abort; nothing was recorded".into());
            }
            // A failed attempt can leave its conmon and CRIU processes running after its container is removed.
            let leftover = removed
                .as_deref()
                .map(|cid| cleanup::runtime_processes(cid, k.created_at))
                .unwrap_or(Value::Null);
            let remaining = leftover["count"].as_u64().unwrap_or(0);
            observed["runtime_processes_of_removed_container"] = leftover.clone();
            let cgroups_absent = removed.as_ref().map(|cid| {
                !cleanup::container_scope(cid).exists() && !cleanup::conmon_scope(cid).exists()
            });
            let space_after = mg::available_bytes(Path::new(&graph_root));
            let k = close(
                db,
                authorization,
                id,
                removed.as_deref(),
                &json!({"reason": "migration_restore_abort", "restore_scope_finished": true,
                    "conmon_scope_absent": removed.as_ref().map(|cid| !cleanup::conmon_scope(cid).exists()),
                    "container_cgroups_absent": cgroups_absent, "runtime_processes_remaining": remaining,
                    "runtime_processes": leftover, "runtime_processes_before_removal": before_removal,
                    "reclaim_processes_requested": reclaim, "reclaim": reclaimed, "authorization_ref": reference,
                    "graph_root": {"path": graph_root, "available_bytes_before": space_before, "available_bytes_after": space_after,
                        "recovered_bytes": space_after as i64 - space_before as i64}}),
                observed,
            )?;
            abort_result(
                uuid,
                &k,
                if removed.is_some() {
                    "removed_restore_leftover"
                } else {
                    "none_absent"
                },
            )
        }
    }
}
fn abort_result(uuid: &str, k: &Claim, action: &str) -> Result<Value, Error> {
    let outcome_file = publish_outcome(k)?;
    let detail = k.detail_value();
    Ok(json!({
        "status": "verified", "operation": "migration_restore_abort", "universe_uuid": uuid, "authorization_id": k.authorization_id,
        "action": action, "handoff_sha256": k.handoff_sha256, "claim_operation_id": k.operation_id,
        "removed_container_id": detail["removed_container_id"], "conmon_scope_absent": detail["conmon_scope_absent"],
        "container_cgroups_absent": detail["container_cgroups_absent"],
        "runtime_processes_remaining": detail["runtime_processes_remaining"], "runtime_processes": detail["runtime_processes"],
        "runtime_processes_before_removal": detail["runtime_processes_before_removal"],
        "reclaim_processes_requested": detail["reclaim_processes_requested"], "reclaim": detail["reclaim"],
        "authorization_ref": detail["authorization_ref"], "graph_root": detail["graph_root"],
        "outcome": {"result": "not_restored", "file": outcome_file["file"], "sha256": k.outcome_sha256},
        "restore_claim": k.view(),
        "note": "this host recorded that it will not restore this authorization; completing the transfer on the source with this outcome ends the authorization",
    }))
}
/// Fresh re-hash of a restore or abort outcome in the outbox, for historical replays.
pub(crate) fn verify_outcome(original: &Value) -> Result<Value, Error> {
    let authorization = original["authorization_id"].as_str().ok_or("Missing authorization_id")?;
    let expected = original["outcome"]["sha256"].as_str();
    let path = tr::outbox(authorization)?.join(tr::OUTCOME);
    let now = if tr::regular_file(&path).ok().flatten().is_some() {
        Some(mg::sha256(&path)?)
    } else {
        None
    };
    Ok(
        json!({"observed_at": crate::now(), "file": path, "present": now.is_some(), "sha256": now,
        "matches": now.is_some() && now.as_deref() == expected}),
    )
}
