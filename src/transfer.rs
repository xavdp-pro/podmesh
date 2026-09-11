//! Experimental transfer of authority on the migration source: authorization, completion and retirement.
//!
//! Protocol: docs/MIGRATION-PROTOCOL.md. Documents travel through fixed directories under the service state
//! directory: `outbox/<authorization_id>/` is written only by this service and `inbox/<authorization_id>/` by
//! the transport controller as root. Requests never carry paths. A document is never a credential: every value
//! it carries is checked against this host's journal and against bytes this service hashes itself.
use crate::lifecycle::{self as lc, failure, Error};
use crate::migration::{self as mg, Reservation};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub(crate) const HANDOFF: &str = "handoff.json";
pub(crate) const OUTCOME: &str = "outcome.json";
pub(crate) const HANDOFF_FORMAT: &str = "podmesh-transfer-handoff/1";
pub(crate) const OUTCOME_FORMAT: &str = "podmesh-transfer-outcome/1";
const MAX_DOCUMENT_BYTES: u64 = 64 * 1024;
const AUTHORITY: &str = "The handoff permits a restore only on the named destination host, which checks it against its own journal and the bytes it hashes. The source stays reserved until a verified destination outcome bound to this handoff is completed; an unreachable destination leaves it held.";

static INBOX: OnceLock<PathBuf> = OnceLock::new();
static OUTBOX: OnceLock<PathBuf> = OnceLock::new();

/// Creates the private delivery directories under the service state directory.
pub(crate) fn prepare(state: &Path) -> Result<(), Error> {
    for (lock, name) in [(&INBOX, "inbox"), (&OUTBOX, "outbox")] {
        let dir = state.join(name);
        fs::create_dir_all(&dir)?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        lock.get_or_init(|| dir);
    }
    Ok(())
}
pub(crate) fn inbox(authorization: &str) -> Result<PathBuf, Error> {
    Ok(INBOX.get().ok_or("Inbox directory not prepared")?.join(authorization))
}
pub(crate) fn outbox_base() -> Result<&'static PathBuf, Error> {
    OUTBOX.get().ok_or_else(|| Error::from("Outbox directory not prepared"))
}
pub(crate) fn outbox(authorization: &str) -> Result<PathBuf, Error> {
    Ok(outbox_base()?.join(authorization))
}
pub(crate) fn is_sha256(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub(crate) fn is_token(v: &str) -> bool {
    !v.is_empty() && v.len() <= 80 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
/// Creates a private directory for this service, or accepts an existing real directory.
pub(crate) fn private_dir(dir: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(dir)?,
        Err(e) => return Err(e.into()),
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err(format!("{} is not a directory", dir.display()).into()),
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
/// A delivered regular file (never a symbolic link) and its size; `None` when it is absent.
pub(crate) fn regular_file(path: &Path) -> Result<Option<u64>, Error> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(m) if m.is_file() => Ok(Some(m.len())),
        Ok(_) => Err(format!("{} is not a regular file", path.display()).into()),
    }
}
/// Reads a JSON object from a delivery directory: a real directory and a bounded regular file.
/// `Ok(None)` when the directory or the file is absent.
pub(crate) fn read_document(dir: &Path, file: &str) -> Result<Option<(Vec<u8>, Value)>, Error> {
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(m) if !m.is_dir() => return Err(format!("{} is not a directory", dir.display()).into()),
        Ok(_) => {}
    }
    let path = dir.join(file);
    let Some(size) = regular_file(&path)? else {
        return Ok(None);
    };
    if size > MAX_DOCUMENT_BYTES {
        return Err(format!("{} exceeds {MAX_DOCUMENT_BYTES} bytes", path.display()).into());
    }
    let bytes = fs::read(&path)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
    if !value.is_object() {
        return Err(format!("{} is not a JSON object", path.display()).into());
    }
    Ok(Some((bytes, value)))
}
/// Every container known to Podman, with labels, from one bounded inventory call.
pub(crate) fn all_containers() -> Result<Vec<Value>, Error> {
    let all: Value = serde_json::from_str(&lc::podman(lc::QUICK, &["ps", "--all", "--format", "json"])?)?;
    Ok(all.as_array().ok_or("Invalid inventory")?.clone())
}

pub(crate) struct Authorization {
    pub authorization_id: String,
    pub operation_id: String,
    pub universe_uuid: String,
    pub checkpoint_operation_id: String,
    pub destination: String,
    pub handoff: String,
    pub handoff_sha256: String,
    pub state: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub outcome_sha256: Option<String>,
    pub completed_by: Option<String>,
}
const AUTHORIZATION_COLUMNS: &str = "authorization_id,operation_id,universe_uuid,checkpoint_operation_id,destination_host_uuid,handoff,handoff_sha256,state,created_at,updated_at,outcome_sha256,completed_by_operation";
fn authorization_row(r: &rusqlite::Row) -> rusqlite::Result<Authorization> {
    Ok(Authorization {
        authorization_id: r.get(0)?,
        operation_id: r.get(1)?,
        universe_uuid: r.get(2)?,
        checkpoint_operation_id: r.get(3)?,
        destination: r.get(4)?,
        handoff: r.get(5)?,
        handoff_sha256: r.get(6)?,
        state: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        outcome_sha256: r.get(10)?,
        completed_by: r.get(11)?,
    })
}
impl Authorization {
    fn view(&self) -> Value {
        json!({"authorization_id": self.authorization_id, "operation_id": self.operation_id, "universe_uuid": self.universe_uuid,
            "checkpoint_operation_id": self.checkpoint_operation_id, "destination_host_uuid": self.destination,
            "handoff_sha256": self.handoff_sha256, "state": self.state, "created_at": self.created_at, "updated_at": self.updated_at,
            "outcome_sha256": self.outcome_sha256, "completed_by_operation": self.completed_by})
    }
}
fn authorization_by(db: &Connection, column: &str, value: &str) -> Result<Option<Authorization>, Error> {
    Ok(db
        .query_row(
            &format!("SELECT {AUTHORIZATION_COLUMNS} FROM migration_authorizations WHERE {column}=?1"),
            [value],
            authorization_row,
        )
        .optional()?)
}
pub(crate) fn authorizations_view(db: &Connection, uuid: &str) -> Result<Vec<Value>, Error> {
    let mut stmt = db.prepare(&format!(
        "SELECT {AUTHORIZATION_COLUMNS} FROM migration_authorizations WHERE universe_uuid=?1 ORDER BY created_at, authorization_id"
    ))?;
    let rows = stmt.query_map([uuid], authorization_row)?;
    Ok(rows.map(|a| a.map(|a| a.view())).collect::<Result<Vec<_>, _>>()?)
}
/// The verified checkpoint result recorded for a reservation.
fn checkpoint_record(db: &Connection, checkpoint: &str, uuid: &str) -> Result<Value, Error> {
    let row: Option<(String, String, Option<String>)> = db
        .query_row("SELECT request,status,result FROM operations WHERE id=?1", [checkpoint], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    let (request, status, result) = row.ok_or("The checkpoint operation is not recorded on this host")?;
    let request: Value = serde_json::from_str(&request)?;
    if request["operation"] != "migration_checkpoint" || request["universe_uuid"].as_str() != Some(uuid) || status != "verified" {
        return Err("The checkpoint operation is not a verified checkpoint of this universe".into());
    }
    Ok(serde_json::from_str(&result.ok_or("Missing persisted checkpoint result")?)?)
}

/// `checkpointed` -> `transfer_authorized`. The authorization row is durable before any artifact is placed in
/// the outbox; a retry of the same operation publishes the same handoff again and never issues another.
#[allow(clippy::too_many_arguments)]
pub(crate) fn authorize(
    db: &Connection,
    id: &str,
    uuid: &str,
    checkpoint: &str,
    destination: &str,
    authorization_ref: &str,
    existing: Option<Value>,
) -> Result<Value, Error> {
    let host = mg::host_uuid(db)?;
    let r = mg::reservation(db, uuid)?.ok_or_else(|| {
        failure(
            "The universe has no migration reservation on this host; nothing to authorize",
            json!({}),
        )
    })?;
    if r.operation_id != checkpoint {
        return Err(failure(
            format!(
                "The reservation belongs to checkpoint operation {}, not {checkpoint}",
                r.operation_id
            ),
            json!({"reservation": r.view()}),
        ));
    }
    if r.destination != destination || r.source_host != host {
        return Err(failure(
            "destination_host_uuid differs from the destination recorded by the checkpoint, or the reservation names another source host",
            json!({"reservation": r.view()}),
        ));
    }
    if let Some(a) = authorization_by(db, "operation_id", id)? {
        return publish(db, uuid, &r, &a, true);
    }
    if r.state != "checkpointed" {
        return Err(failure(
            format!(
                "The reservation is in state {}; only a checkpointed reservation can be authorized",
                r.state
            ),
            json!({"reservation": r.view()}),
        ));
    }
    let recorded = checkpoint_record(db, checkpoint, uuid)?;
    let archive_sha256 = recorded["archive"]["sha256"].as_str().ok_or("Missing recorded archive hash")?;
    let manifest_sha256 = recorded["manifest"]["sha256"].as_str().ok_or("Missing recorded manifest hash")?;
    let bytes = recorded["archive"]["bytes"].as_u64().ok_or("Missing recorded archive size")?;
    let fresh = mg::verify_artifacts(checkpoint, Some(archive_sha256), Some(manifest_sha256))?;
    if fresh["archive_sha256_matches"] != true || fresh["manifest_sha256_matches"] != true {
        return Err(failure(
            "The preserved archive or manifest no longer hashes to the recorded values; nothing was authorized",
            json!({"artifacts": fresh}),
        ));
    }
    let c = existing.ok_or_else(|| {
        failure(
            "The reserved source container is absent; its checkpointed state cannot be observed and nothing was authorized",
            json!({"reservation": r.view()}),
        )
    })?;
    if c["Id"].as_str() != Some(r.container_id.as_str()) || c["State"]["Checkpointed"] != true || lc::process_active(&c) {
        return Err(failure(
            "The source is not the reserved container in a checkpointed, stopped state; nothing was authorized",
            json!({"observed": lc::state_view(&c), "checkpointed": c["State"]["Checkpointed"], "reservation": r.view()}),
        ));
    }
    let manifest: Value = serde_json::from_slice(&fs::read(mg::base()?.join(checkpoint).join(mg::MANIFEST))?)?;
    for (field, expected) in [
        ("operation_id", checkpoint),
        ("universe_uuid", uuid),
        ("container_id", r.container_id.as_str()),
        ("image_id", r.image_id.as_str()),
        ("source_host_uuid", host.as_str()),
        ("destination_host_uuid", destination),
    ] {
        if manifest[field].as_str() != Some(expected) {
            return Err(failure(
                format!("The manifest {field} does not match the reservation; nothing was authorized"),
                json!({"reservation": r.view()}),
            ));
        }
    }
    // The handoff names the runtime and kernel that produced the dump, as recorded when it was finalized.
    let binary = manifest["runtime"]["sha256"][mg::RUNTIME_REAL]
        .as_str()
        .filter(|h| is_sha256(h))
        .ok_or("The manifest does not record the runtime binary SHA-256")?;
    let kernel = manifest["runtime"]["kernel"]
        .as_str()
        .filter(|k| !k.is_empty())
        .ok_or("The manifest does not record the kernel release")?;
    let available = mg::available_bytes(outbox_base()?);
    let required = bytes.saturating_add(mg::SPACE_MARGIN_BYTES);
    if available < required {
        return Err(failure(
            format!("{available} bytes available for the outbox, {required} required; nothing was authorized"),
            json!({}),
        ));
    }
    let authorization = fs::read_to_string("/proc/sys/kernel/random/uuid")?.trim().to_string();
    let now = crate::now() as i64;
    let handoff = json!({
        "format": HANDOFF_FORMAT, "authorization_id": authorization, "universe_uuid": uuid,
        "source_container_id": r.container_id, "image_id": r.image_id,
        "source_host_uuid": host, "destination_host_uuid": destination, "checkpoint_operation_id": checkpoint,
        "archive": {"file": mg::ARCHIVE, "bytes": bytes, "sha256": archive_sha256},
        "manifest": {"file": mg::MANIFEST, "sha256": manifest_sha256},
        "runtime": {"git_id": mg::RUNTIME_GIT_ID, "binary_sha256": binary},
        "kernel_release": kernel, "issued_at": now,
        "authorization_ref": authorization_ref, "authorize_operation_id": id,
    });
    let text = format!("{}\n", serde_json::to_string_pretty(&handoff)?);
    let handoff_sha256 = mg::sha256_bytes(text.as_bytes())?;
    let tx = db.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO migration_authorizations VALUES(?1,?2,?3,?4,?5,?6,?7,'issued',?8,?8,NULL,NULL,NULL)",
        params![authorization, id, uuid, checkpoint, destination, text, handoff_sha256, now],
    )?;
    mg::merge_state(
        &tx,
        uuid,
        "transfer_authorized",
        &json!({"authorization_id": authorization, "handoff_sha256": handoff_sha256}),
    )?;
    tx.commit()?;
    let a = authorization_by(db, "authorization_id", &authorization)?.ok_or("Authorization not persisted")?;
    publish(db, uuid, &r, &a, false)
}

/// Places the archive, manifest and handoff of a recorded authorization in its outbox, each verified against
/// the recorded hashes. Repeating it keeps intact files and rewrites others from the preserved artifacts.
fn publish(db: &Connection, uuid: &str, r: &Reservation, a: &Authorization, resumed: bool) -> Result<Value, Error> {
    let handoff: Value = serde_json::from_str(&a.handoff)?;
    let source = mg::base()?.join(&a.checkpoint_operation_id);
    let out = outbox(&a.authorization_id)?;
    private_dir(&out)?;
    let mut files = json!({});
    for (file, expected) in [
        (mg::ARCHIVE, handoff["archive"]["sha256"].as_str()),
        (mg::MANIFEST, handoff["manifest"]["sha256"].as_str()),
    ] {
        let expected = expected.ok_or("The recorded handoff lacks an artifact hash")?;
        let target = out.join(file);
        if !(regular_file(&target)?.is_some() && mg::sha256(&target)? == expected) {
            mg::copy_private(&source.join(file), &target)?;
            let copied = mg::sha256(&target)?;
            if copied != expected {
                return Err(failure(
                    format!("The {file} copied to the outbox hashes to {copied}, not the recorded {expected}; the authorization stays recorded and the copy can be retried"),
                    json!({"authorization_id": a.authorization_id}),
                ));
            }
        }
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))?;
        files[file] = json!({"bytes": fs::metadata(&target)?.len(), "sha256": expected});
    }
    let target = out.join(HANDOFF);
    if !(regular_file(&target)?.is_some() && mg::sha256(&target)? == a.handoff_sha256) {
        mg::write_private(&target, a.handoff.as_bytes())?;
        if mg::sha256(&target)? != a.handoff_sha256 {
            return Err("The handoff written to the outbox does not hash to the recorded value".into());
        }
    }
    files[HANDOFF] = json!({"bytes": a.handoff.len(), "sha256": a.handoff_sha256});
    let state = mg::reservation(db, uuid)?.map(|r| r.state);
    Ok(json!({
        "status": "verified", "operation": "migration_authorize_transfer", "universe_uuid": uuid,
        "authorization_id": a.authorization_id, "checkpoint_operation_id": a.checkpoint_operation_id,
        "source_container_id": r.container_id, "destination_host_uuid": a.destination,
        "handoff_sha256": a.handoff_sha256, "handoff": handoff, "outbox_directory": out, "files": files,
        "reservation": {"state": state}, "resumed_after_interruption": resumed, "authority": AUTHORITY,
    }))
}

/// Fresh re-hash of an authorization's outbox files, for historical replays.
pub(crate) fn verify_outbox(original: &Value) -> Result<Value, Error> {
    let authorization = original["authorization_id"].as_str().ok_or("Missing authorization_id")?;
    let out = outbox(authorization)?;
    let mut files = json!({});
    for (file, expected) in [
        (mg::ARCHIVE, original["handoff"]["archive"]["sha256"].as_str()),
        (mg::MANIFEST, original["handoff"]["manifest"]["sha256"].as_str()),
        (HANDOFF, original["handoff_sha256"].as_str()),
    ] {
        let path = out.join(file);
        let now = if regular_file(&path).ok().flatten().is_some() {
            Some(mg::sha256(&path)?)
        } else {
            None
        };
        files[file] = json!({"present": now.is_some(), "sha256": now, "matches": now.is_some() && now.as_deref() == expected});
    }
    Ok(json!({"observed_at": crate::now(), "outbox_directory": out, "files": files}))
}

/// `transfer_authorized` -> `transferred` (outcome `restored`) or `checkpointed` (outcome `not_restored`,
/// authorization ended). Any mismatch between the inbox outcome and this host's authorization is refused
/// without a state change.
pub(crate) fn complete(db: &Connection, id: &str, uuid: &str, authorization: &str, existing: Option<Value>) -> Result<Value, Error> {
    let host = mg::host_uuid(db)?;
    let a = authorization_by(db, "authorization_id", authorization)?
        .filter(|a| a.universe_uuid == uuid)
        .ok_or_else(|| {
            failure(
                "No transfer authorization with this ID is recorded for this universe on this host",
                json!({"authorization_id": authorization}),
            )
        })?;
    if a.state != "issued" {
        if a.completed_by.as_deref() == Some(id) {
            // An earlier attempt of this operation recorded the completion but could not report it.
            return completion_result(db, uuid, &a, existing, true);
        }
        return Err(failure(
            format!(
                "Authorization {authorization} was already completed by operation {} (state {}); nothing was changed",
                a.completed_by.as_deref().unwrap_or("unknown"),
                a.state
            ),
            json!({"authorization": a.view()}),
        ));
    }
    let r = mg::reservation(db, uuid)?.ok_or("The universe has no migration reservation on this host")?;
    if r.state != "transfer_authorized" || r.detail_value()["authorization_id"].as_str() != Some(authorization) {
        return Err(failure(
            format!("The reservation is in state {} and does not hold this authorization", r.state),
            json!({"reservation": r.view()}),
        ));
    }
    let (bytes, outcome) = read_document(&inbox(authorization)?, OUTCOME)
        .map_err(|e| failure(format!("The outcome document is unusable: {e}; nothing was changed"), json!({})))?
        .ok_or_else(|| {
            failure(
                "No outcome document for this authorization in the inbox; the source stays reserved",
                json!({"authorization_id": authorization}),
            )
        })?;
    let outcome_sha256 = mg::sha256_bytes(&bytes)?;
    let text = |k: &str| outcome[k].as_str().unwrap_or("").to_string();
    let mut mismatches: Vec<String> = vec![];
    if text("format") != OUTCOME_FORMAT {
        mismatches.push(format!("format {:?} is not {OUTCOME_FORMAT}", text("format")));
    }
    if text("authorization_id") != authorization {
        mismatches.push("the outcome names another authorization".into());
    }
    if text("handoff_sha256") != a.handoff_sha256 {
        mismatches.push(format!(
            "the outcome is bound to handoff {:?}, not to this authorization's handoff {}",
            text("handoff_sha256"),
            a.handoff_sha256
        ));
    }
    if text("universe_uuid") != uuid {
        mismatches.push("the outcome names another universe".into());
    }
    if text("source_host_uuid") != host {
        mismatches.push("the outcome names another source host".into());
    }
    if text("destination_host_uuid") != a.destination {
        mismatches.push("the outcome comes from another destination host".into());
    }
    if !is_token(&text("destination_operation_id")) {
        mismatches.push("the outcome lacks a valid destination operation ID".into());
    }
    let result = text("result");
    match result.as_str() {
        "restored" if is_sha256(&text("restored_container_id")) => {}
        "not_restored" if outcome["restored_container_id"].is_null() => {}
        _ => mismatches.push("result must be restored with a restored container ID, or not_restored without one".into()),
    }
    if !mismatches.is_empty() {
        return Err(failure(
            "The outcome does not bind to this authorization; nothing was changed",
            json!({"mismatches": mismatches, "outcome_sha256": outcome_sha256, "authorization": a.view()}),
        ));
    }
    if let Some(ref c) = existing {
        if c["Id"].as_str() != Some(r.container_id.as_str()) || lc::process_active(c) {
            return Err(failure(
                "The source container is running or has been replaced: an activity this protocol did not authorize; completion is refused and the source stays reserved",
                json!({"observed": lc::state_view(c), "reservation": r.view()}),
            ));
        }
    }
    let now = crate::now() as i64;
    let restored = result == "restored";
    let tx = db.unchecked_transaction()?;
    tx.execute(
        "UPDATE migration_authorizations SET state=?2, updated_at=?3, outcome=?4, outcome_sha256=?5, completed_by_operation=?6 WHERE authorization_id=?1",
        params![
            authorization,
            if restored { "completed_restored" } else { "ended_not_restored" },
            now,
            String::from_utf8(bytes.clone())?,
            outcome_sha256,
            id
        ],
    )?;
    if restored {
        mg::merge_state(
            &tx,
            uuid,
            "transferred",
            &json!({"authorization_id": authorization, "outcome_sha256": outcome_sha256, "transferred_at": now,
                "restored_container_id": text("restored_container_id"), "destination_operation_id": text("destination_operation_id")}),
        )?;
    } else {
        mg::merge_state(
            &tx,
            uuid,
            "checkpointed",
            &json!({"authorization_id": null, "ended_authorization_id": authorization, "ended_outcome_sha256": outcome_sha256}),
        )?;
    }
    tx.commit()?;
    mg::write_private(
        &mg::base()?
            .join(&a.checkpoint_operation_id)
            .join(format!("outcome-{authorization}.json")),
        &bytes,
    )?;
    let a = authorization_by(db, "authorization_id", authorization)?.ok_or("Authorization disappeared")?;
    completion_result(db, uuid, &a, existing, false)
}
fn completion_result(db: &Connection, uuid: &str, a: &Authorization, existing: Option<Value>, resumed: bool) -> Result<Value, Error> {
    let outcome: Value = db
        .query_row(
            "SELECT outcome FROM migration_authorizations WHERE authorization_id=?1",
            [&a.authorization_id],
            |r| r.get::<_, Option<String>>(0),
        )?
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    let r = mg::reservation(db, uuid)?;
    let restored = outcome["result"] == "restored";
    Ok(json!({
        "status": "verified", "operation": "migration_complete_transfer", "universe_uuid": uuid,
        "authorization_id": a.authorization_id, "handoff_sha256": a.handoff_sha256, "outcome_sha256": a.outcome_sha256,
        "destination_host_uuid": a.destination, "destination_result": outcome["result"],
        "restored_container_id": outcome["restored_container_id"], "destination_operation_id": outcome["destination_operation_id"],
        "container_id": r.as_ref().map(|r| r.container_id.clone()),
        "source_observed": existing.as_ref().map(lc::state_view),
        "reservation": {"state": r.as_ref().map(|r| r.state.clone())},
        "authorization": {"state": a.state},
        "resumed_after_interruption": resumed,
        "note": if restored {
            "the source is transferred: it stays reserved and refuses generic operations; migration_retire_source removes its stopped container"
        } else {
            "the authorization ended without a restore: the reservation is checkpointed again and may be authorized again to its recorded destination; no release exists in this version"
        },
    }))
}

/// From `transferred`: removes only the stopped, still checkpointed reserved container, with the checkpoint
/// files Podman kept for it. The reservation stays `transferred` and the evidence directory is kept.
pub(crate) fn retire(
    db: &Connection,
    id: &str,
    uuid: &str,
    name: &str,
    authorization: &str,
    existing: Option<Value>,
) -> Result<Value, Error> {
    let r = mg::reservation(db, uuid)?.ok_or_else(|| failure("The universe has no migration reservation on this host", json!({})))?;
    let detail = r.detail_value();
    if r.state != "transferred" || detail["authorization_id"].as_str() != Some(authorization) {
        return Err(failure(
            format!(
                "Retirement requires a transferred reservation completed by authorization {authorization} (state {}); nothing was removed",
                r.state
            ),
            json!({"reservation": r.view()}),
        ));
    }
    authorization_by(db, "authorization_id", authorization)?
        .filter(|a| a.state == "completed_restored" && a.universe_uuid == uuid)
        .ok_or_else(|| {
            failure(
                "The authorization is not recorded as completed by a verified restore; nothing was removed",
                json!({"reservation": r.view()}),
            )
        })?;
    let (action, kept_checkpoint_files_removed) = match existing {
        None => {
            if all_containers()?.iter().any(|c| c["Id"].as_str() == Some(r.container_id.as_str())) {
                return Err(failure(
                    "The reserved source container exists under another name; nothing was removed",
                    json!({"reservation": r.view()}),
                ));
            }
            ("none_already_absent", Value::Null)
        }
        Some(c) => {
            if c["Id"].as_str() != Some(r.container_id.as_str()) {
                return Err(failure(
                    "The container under the universe name is not the reserved source; nothing was removed",
                    json!({"observed": lc::state_view(&c), "reservation": r.view()}),
                ));
            }
            if lc::process_active(&c) || c["State"]["Checkpointed"] != true {
                return Err(failure(
                    "The source is running or no longer in its checkpointed state (it may have run after the checkpoint); nothing was removed",
                    json!({"observed": lc::state_view(&c), "checkpointed": c["State"]["Checkpointed"]}),
                ));
            }
            let static_dir = c["StaticDir"].as_str().map(PathBuf::from);
            // Never forced: the container is stopped.
            lc::podman(lc::QUICK, &["rm", &r.container_id])?;
            if lc::inspect(name)?.is_some() || all_containers()?.iter().any(|c| c["Id"].as_str() == Some(r.container_id.as_str())) {
                return Err("The source container is still present after removal".into());
            }
            ("removed", json!(static_dir.map(|d| !d.exists())))
        }
    };
    if detail["retired_by_operation"].is_null() {
        mg::merge_state(
            db,
            uuid,
            "transferred",
            &json!({"retired_by_operation": id, "retired_at": crate::now(), "retired_container_id": r.container_id}),
        )?;
    }
    let evidence = mg::base()?.join(&r.operation_id);
    Ok(json!({
        "status": "verified", "operation": "migration_retire_source", "universe_uuid": uuid, "authorization_id": authorization,
        "action": action, "container_id": r.container_id, "kept_checkpoint_files_removed": kept_checkpoint_files_removed,
        "evidence_directory": evidence, "evidence_directory_kept": evidence.is_dir(), "reservation": {"state": "transferred"},
        "note": "the transferred reservation stays: it refuses generic operations, and create with this universe UUID, on this host; only a verified restore of a later handoff brings the universe back",
    }))
}
