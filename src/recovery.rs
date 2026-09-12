//! Recovery of a reservation that never left this host: release, abandonment and local restore.
//!
//! Protocol: docs/MIGRATION-PROTOCOL.md. These three operations are the way out of a reservation whose
//! migration is not going to happen. None of them starts an application by accident, and none of them
//! can run once a transfer authorization has been issued: from that moment only a verified destination
//! outcome bound to it ends the reservation (invariant 3).
//!
//! * `migration_release` returns a checkpointed or failed reservation to `released` after observing that
//!   the reserved container is still the same one and is not running. It lifts the generic-operation
//!   gate and starts nothing.
//! * `migration_abandon` records that a reservation whose container is gone will never be completed.
//!   Artifacts are preserved and the universe UUID stays refused on this host.
//! * `migration_restore_local` resumes the checkpointed memory here, preferring the checkpoint files
//!   Podman kept for the container (in place, same container ID) and falling back to the preserved
//!   archive only when those files are gone. It is verified exactly like a destination restore, and it
//!   archives the reservation so that the universe is fully operable again.
use crate::cleanup::{self, Bound};
use crate::lifecycle::{self as lc, failure, Error};
use crate::migration::{self as mg, Reservation, ABANDONED, COLLECTED, RELEASED, RESTORED_LOCALLY};
use crate::restore as ds;
use crate::transfer as tr;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Command,
};

const RESTORE_SECONDS: u64 = 300;
const UNIVERSE_LABEL: &str = "io.podmesh.universe";
const SCOPE: &str = "experimental local restore: default rootful Podman store, the reserved universe resumed on this host from the checkpoint files Podman kept for it, or from the preserved archive, with the packaged podmesh-vzcriu 3.15.5.3 through its private-path shim";
const NO_RESTART: &str = "nothing was started: the application is resumed only by migration_restore_local, and an ordinary start would begin it afresh without its checkpointed memory";
/// States a reservation can be released from, abandoned from, and locally restored from.
const RELEASABLE: [&str; 2] = ["checkpointed", "checkpoint_failed"];
const ABANDONABLE: [&str; 4] = [
    "reserved",
    "checkpointing",
    "checkpoint_failed",
    "checkpointed",
];

fn scope_unit(id: &str) -> String {
    format!("podmesh-restore-local-{id}.scope")
}
/// Every transfer authorization ever recorded for this reservation's checkpoint operation, whatever its
/// state. One is enough to refuse a release or an abandonment for good.
fn authorizations_issued(
    db: &Connection,
    uuid: &str,
    checkpoint: &str,
) -> Result<Vec<Value>, Error> {
    let mut stmt = db.prepare(
        "SELECT authorization_id,state,destination_host_uuid FROM migration_authorizations
         WHERE universe_uuid=?1 AND checkpoint_operation_id=?2 ORDER BY created_at",
    )?;
    let rows = stmt.query_map(rusqlite::params![uuid, checkpoint], |r| {
        Ok(
            json!({"authorization_id": r.get::<_, String>(0)?, "state": r.get::<_, String>(1)?,
            "destination_host_uuid": r.get::<_, String>(2)?}),
        )
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
/// The checkpoint files Podman kept for a container (`--keep`), inside the container's own storage.
fn kept_checkpoint(c: &Value) -> Option<PathBuf> {
    let dir = c["StaticDir"].as_str()?;
    if !dir.starts_with("/var/lib/containers/storage/") || dir.contains("..") {
        return None;
    }
    let checkpoint = Path::new(dir).join("checkpoint");
    checkpoint
        .join("inventory.img")
        .is_file()
        .then_some(checkpoint)
}
fn directory_bytes(dir: &Path) -> u64 {
    Command::new("/usr/bin/du")
        .args(["-sb", "--"])
        .arg(dir)
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok())
        })
        .unwrap_or(0)
}
/// Whether a verified `migration_restore_local` of this host binds this universe to this container ID.
/// A container restored from the preserved archive carries the original creation label but a new ID, so
/// its ownership is the verified local restore, exactly as an imported restore's is on a destination.
pub(crate) fn restored_locally(
    db: &Connection,
    uuid: &str,
    container_id: &str,
) -> Result<bool, Error> {
    // The ID is hexadecimal, so LIKE only narrows the scan; each candidate is then checked exactly.
    let mut stmt = db.prepare(
        "SELECT request,result FROM operations WHERE status='verified' AND result LIKE ?1",
    )?;
    let rows = stmt.query_map([format!("%{container_id}%")], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    for row in rows {
        let (request, result) = row?;
        let request: Value = serde_json::from_str(&request)?;
        let result: Value = serde_json::from_str(result.as_deref().unwrap_or("null"))?;
        if request["operation"] == "migration_restore_local"
            && request["universe_uuid"].as_str() == Some(uuid)
            && result["container_id"].as_str() == Some(container_id)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The reserved container as a fresh observation, and what it allows. `named` is the container under the
/// universe name, if any.
struct Observed<'a> {
    named: Option<&'a Value>,
    same: bool,
    running: bool,
    checkpointed: bool,
    kept: Option<PathBuf>,
}
fn observe<'a>(r: &Reservation, named: Option<&'a Value>) -> Observed<'a> {
    let same = named.is_some_and(|c| c["Id"].as_str() == Some(r.container_id.as_str()));
    Observed {
        named,
        same,
        running: named.is_some_and(lc::process_active),
        checkpointed: named.is_some_and(|c| c["State"]["Checkpointed"] == true),
        kept: named.filter(|_| same).and_then(kept_checkpoint),
    }
}
fn release_blockers(r: &Reservation, o: &Observed, issued: &[Value]) -> Vec<String> {
    let mut blockers = vec![];
    if !RELEASABLE.contains(&r.state.as_str()) {
        blockers.push(format!(
            "the reservation is in state {}; only a checkpointed or checkpoint_failed reservation can be released",
            r.state
        ));
    }
    if !issued.is_empty() {
        blockers.push(format!(
            "{} transfer authorization(s) were issued for this reservation ({}); once an authorization exists only a verified destination outcome bound to it can end the reservation",
            issued.len(),
            issued
                .iter()
                .map(|a| format!("{} {}", a["authorization_id"].as_str().unwrap_or(""), a["state"].as_str().unwrap_or("")))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    match o.named {
        None => blockers.push(
            "the reserved source container is absent; release requires a fresh observation of the same container, and migration_abandon is the path for a reservation whose container is gone".into(),
        ),
        Some(_) if !o.same => blockers.push("the container under the universe name is not the reserved source container".into()),
        Some(_) if o.running => blockers.push("the reserved source container is running; stop it explicitly first".into()),
        Some(_) => {}
    }
    blockers
}
fn abandon_blockers(
    r: &Reservation,
    o: &Observed,
    issued: &[Value],
    elsewhere: bool,
) -> Vec<String> {
    let mut blockers = vec![];
    if !ABANDONABLE.contains(&r.state.as_str()) {
        blockers.push(format!(
            "the reservation is in state {}; only a reserved, checkpointing, checkpoint_failed or checkpointed reservation can be abandoned",
            r.state
        ));
    }
    if !issued.is_empty() {
        blockers.push(format!(
            "{} transfer authorization(s) were issued for this reservation; only a verified destination outcome bound to one of them can end it",
            issued.len()
        ));
    }
    if o.named.is_some() {
        blockers.push(
            "a container occupies the universe name; abandonment is only for a reservation whose container is gone, and migration_release is the path for one that is still there".into(),
        );
    }
    if elsewhere {
        blockers.push("the reserved source container exists under another name".into());
    }
    blockers
}
fn restore_local_blockers(
    r: &Reservation,
    o: &Observed,
    artifacts: &Value,
    elsewhere: bool,
) -> Vec<String> {
    let mut blockers = vec![];
    // A collected reservation is a released one the garbage collector settled on proof: the contract's
    // class 1 says a later local memory restore stays a separate explicit operation, so it stays available.
    if r.state != RELEASED && r.state != COLLECTED {
        blockers.push(format!(
            "the reservation is in state {}; a local restore resumes a released reservation, so migration_release comes first, or a garbage collection that ends a terminal one",
            r.state
        ));
    }
    match o.named {
        Some(_) if !o.same => blockers.push("the container under the universe name is not the reserved source container".into()),
        Some(_) if o.running => blockers.push("the reserved source container is running; there is nothing to resume".into()),
        Some(_) if !o.checkpointed => blockers.push(
            "the reserved source container is no longer in its checkpointed state, so its memory is gone from Podman; delete the universe to restore the preserved archive under this name, or start it afresh".into(),
        ),
        Some(_) if o.kept.is_none() => blockers.push(
            "the checkpoint files Podman kept for the reserved container are gone; delete the universe to restore the preserved archive under this name".into(),
        ),
        Some(_) => {}
        None => {
            if elsewhere {
                blockers.push("the reserved source container exists under another name".into());
            }
            if artifacts["archive_sha256_matches"] != true {
                blockers.push(
                    "the reserved container is absent and the preserved archive is missing or no longer hashes to the value recorded at checkpoint; there is nothing to resume".into(),
                );
            }
        }
    }
    blockers
}
/// What `migration_status` reports about the three recovery paths, from the same checks the operations use.
pub(crate) fn availability(
    db: &Connection,
    uuid: &str,
    r: &Reservation,
    named: Option<&Value>,
    issued_count: usize,
    artifacts: &Value,
) -> Result<Value, Error> {
    let issued = authorizations_issued(db, uuid, &r.operation_id)?;
    let o = observe(r, named);
    let elsewhere = named.is_none()
        && tr::all_containers()?
            .iter()
            .any(|c| c["Id"].as_str() == Some(r.container_id.as_str()));
    let path = if o.kept.is_some() {
        "kept_checkpoint_files"
    } else if named.is_none() && artifacts["archive_sha256_matches"] == true {
        "preserved_archive"
    } else {
        "none"
    };
    let entry = |blockers: Vec<String>| {
        json!({"permitted": blockers.is_empty(), "blockers": blockers.clone(),
            "reason": if blockers.is_empty() { Value::Null } else { json!(blockers.join("; ")) }})
    };
    let mut restore_local = entry(restore_local_blockers(r, &o, artifacts, elsewhere));
    restore_local["memory_source"] = json!(path);
    Ok(json!({
        "release": entry(release_blockers(r, &o, &issued)),
        "abandon": entry(abandon_blockers(r, &o, &issued, elsewhere)),
        "restore_local": restore_local,
        "authorizations_ever_issued": issued.len().max(issued_count),
        "observed": {"reserved_container_present": o.same, "reserved_container_running": o.running,
            "reserved_container_checkpointed": o.checkpointed, "kept_checkpoint_files": o.kept,
            "reserved_container_under_another_name": elsewhere},
    }))
}

/// Common entry: the reservation this request names, or a refusal that changed nothing.
fn reserved(db: &Connection, uuid: &str, checkpoint: &str) -> Result<Reservation, Error> {
    let r = mg::reservation(db, uuid)?.ok_or_else(|| {
        failure(
            "The universe has no migration reservation on this host; there is nothing to recover",
            json!({"universe_uuid": uuid}),
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
    Ok(r)
}
fn artifacts_of(r: &Reservation) -> Result<Value, Error> {
    let detail = r.detail_value();
    mg::verify_artifacts(
        &r.operation_id,
        detail["archive_sha256"].as_str(),
        detail["manifest_sha256"].as_str(),
    )
}

pub(crate) fn release(
    db: &Connection,
    id: &str,
    uuid: &str,
    checkpoint: &str,
    existing: Option<Value>,
) -> Result<Value, Error> {
    let r = reserved(db, uuid, checkpoint)?;
    let issued = authorizations_issued(db, uuid, checkpoint)?;
    let o = observe(&r, existing.as_ref());
    let blockers = release_blockers(&r, &o, &issued);
    if !blockers.is_empty() {
        return Err(failure(
            "Release preconditions not met; the reservation is unchanged and nothing was started",
            json!({"blockers": blockers, "reservation": r.view(), "observed": existing.as_ref().map(lc::state_view),
                "transfer_authorizations": issued}),
        ));
    }
    let artifacts = artifacts_of(&r)?;
    let now = crate::now() as i64;
    let observed = existing.as_ref().map(lc::state_view);
    mg::merge_state(
        db,
        uuid,
        RELEASED,
        &json!({"released_by_operation": id, "released_at": now, "released_from_state": r.state,
            "released_container_id": r.container_id, "observed_at_release": observed}),
    )?;
    let r = mg::reservation(db, uuid)?.ok_or("Reservation not found after release")?;
    Ok(json!({
        "status": "verified", "operation": "migration_release", "universe_uuid": uuid,
        "checkpoint_operation_id": checkpoint, "container_id": r.container_id,
        "released_from_state": r.detail_value()["released_from_state"], "reservation": {"state": RELEASED},
        "source_observed": observed, "artifacts": artifacts,
        "generic_operations": "create, start, delete and clone are available again for this universe",
        "memory": if artifacts["archive_sha256_matches"] == true || o.kept.is_some() {
            "the checkpointed memory is preserved: migration_restore_local resumes it, an ordinary start begins the application afresh without it"
        } else {
            "no preserved memory remains for this reservation: only an ordinary start is possible, and it begins the application afresh"
        },
        "effects": NO_RESTART, "scope": SCOPE,
    }))
}

pub(crate) fn abandon(
    db: &Connection,
    id: &str,
    uuid: &str,
    checkpoint: &str,
    existing: Option<Value>,
) -> Result<Value, Error> {
    let r = reserved(db, uuid, checkpoint)?;
    let issued = authorizations_issued(db, uuid, checkpoint)?;
    let o = observe(&r, existing.as_ref());
    let elsewhere = existing.is_none()
        && tr::all_containers()?
            .iter()
            .any(|c| c["Id"].as_str() == Some(r.container_id.as_str()));
    let blockers = abandon_blockers(&r, &o, &issued, elsewhere);
    if !blockers.is_empty() {
        return Err(failure(
            "Abandonment preconditions not met; the reservation is unchanged and nothing was removed",
            json!({"blockers": blockers, "reservation": r.view(), "observed": existing.as_ref().map(lc::state_view),
                "transfer_authorizations": issued}),
        ));
    }
    let artifacts = artifacts_of(&r)?;
    let from = r.state.clone();
    mg::merge_state(
        db,
        uuid,
        ABANDONED,
        &json!({"abandoned_by_operation": id, "abandoned_at": crate::now(), "abandoned_from_state": from,
            "abandoned_container_id": r.container_id}),
    )?;
    Ok(json!({
        "status": "verified", "operation": "migration_abandon", "universe_uuid": uuid,
        "checkpoint_operation_id": checkpoint, "container_id": r.container_id, "abandoned_from_state": from,
        "reservation": {"state": ABANDONED}, "artifacts": artifacts,
        "artifact_directory": mg::base()?.join(&r.operation_id),
        "universe_uuid_still_refused": "create, start, delete and clone stay refused for this universe on this host; only a verified restore of a handoff from another host brings it back",
        "memory": "the preserved artifacts are kept as evidence; abandonment gives up the local restore path for them",
        "effects": NO_RESTART, "scope": SCOPE,
    }))
}

/// Verified exactly like a destination restore: Podman must show the universe container running and
/// `Restored`, restored after this attempt began, from the recorded image, network-disabled and
/// mount-free, and the preserved CRIU restore log must show a successful restore by the qualified runtime.
fn verify(
    r: &Reservation,
    uuid: &str,
    observed: Option<&Value>,
    log: &Path,
    preserved: bool,
    since: i64,
    in_place: bool,
) -> Result<(), String> {
    let c = observed.ok_or("the universe container is absent")?;
    if c["Config"]["Labels"][UNIVERSE_LABEL].as_str() != Some(uuid) {
        return Err("the container does not carry the universe label".into());
    }
    if in_place && c["Id"].as_str() != Some(r.container_id.as_str()) {
        return Err(
            "the container is not the reserved one, which an in-place restore never replaces"
                .into(),
        );
    }
    if lc::status(c) != "running" || !lc::process_active(c) {
        return Err(format!(
            "the container is not running (state {})",
            lc::status(c)
        ));
    }
    // Podman keeps reporting a running container whose cgroup this service froze, because the freeze
    // goes to the kernel and not through Podman's own bookkeeping. A suspended universe is not restored.
    if cleanup::frozen(c["Id"].as_str().unwrap_or("")) {
        return Err(
            "the container's cgroup is frozen: the universe is suspended, not running".into(),
        );
    }
    if c["State"]["Restored"] != true {
        return Err("Podman does not report the container as restored".into());
    }
    if !c["State"]["RestoredAt"]
        .as_str()
        .and_then(lc::epoch)
        .is_some_and(|t| t >= since)
    {
        return Err("the container was not restored after this operation began".into());
    }
    if !in_place
        && !c["Created"]
            .as_str()
            .and_then(lc::epoch)
            .is_some_and(|t| t >= since)
    {
        return Err("the container was not created after this operation began".into());
    }
    if c["Image"].as_str().map(|i| i.trim_start_matches("sha256:")) != Some(r.image_id.as_str()) {
        return Err("the container image is not the image recorded by the reservation".into());
    }
    if c["HostConfig"]["NetworkMode"].as_str() != Some("none")
        || c["Mounts"]
            .as_array()
            .map(|m| !m.is_empty())
            .unwrap_or(true)
    {
        return Err("the container is not network-disabled and mount-free".into());
    }
    if !preserved {
        return Err("the CRIU restore log is unavailable".into());
    }
    let text = fs::read_to_string(log).unwrap_or_default();
    if !text.contains(&format!("(gitid {})", mg::RUNTIME_GIT_ID))
        || !text.contains("Restore finished successfully")
    {
        return Err(
            "the restore log does not show a successful restore by the qualified private runtime"
                .into(),
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn restore_local(
    db: &Connection,
    attempt: i64,
    id: &str,
    uuid: &str,
    name: &str,
    checkpoint: &str,
    existing: Option<Value>,
) -> Result<Value, Error> {
    let r = reserved(db, uuid, checkpoint)?;
    let unit = scope_unit(id);
    if mg::unit_busy(&unit) {
        return Err(failure(
            "The local restore scope of this operation has not finished, or its state cannot be queried; retry after it finishes",
            json!({"reservation": r.view(), "scope": unit}),
        ));
    }
    let o = observe(&r, existing.as_ref());
    let artifacts = artifacts_of(&r)?;
    let elsewhere = existing.is_none()
        && tr::all_containers()?
            .iter()
            .any(|c| c["Id"].as_str() == Some(r.container_id.as_str()));
    let blockers = restore_local_blockers(&r, &o, &artifacts, elsewhere);
    if !blockers.is_empty() {
        return Err(failure(
            "Local restore preconditions not met; nothing was restored and the reservation is unchanged",
            json!({"blockers": blockers, "reservation": r.view(), "observed": existing.as_ref().map(lc::state_view),
                "artifacts": artifacts}),
        ));
    }
    let dir = mg::base()?.join(&r.operation_id);
    let in_place = o.kept.is_some();
    let graph_root = lc::podman(lc::QUICK, &["info", "--format", "{{.Store.GraphRoot}}"])?
        .trim()
        .to_string();
    if graph_root != mg::CONTAINER_STORAGE {
        return Err(failure(
            format!(
                "Podman graph root {graph_root} is not the qualified default store {}",
                mg::CONTAINER_STORAGE
            ),
            json!({"reservation": r.view()}),
        ));
    }
    // What this attempt is allowed to consume on the graph root: the restore writes the checkpoint
    // images it reads, plus its log. An archive is measured uncompressed, kept files as they are.
    let required = match o.kept {
        Some(ref kept) => directory_bytes(kept).saturating_add(mg::SPACE_MARGIN_BYTES),
        None => ds::uncompressed_bytes(&dir.join(mg::ARCHIVE))?
            .saturating_mul(2)
            .saturating_add(mg::SPACE_MARGIN_BYTES),
    };
    let available = mg::available_bytes(Path::new(&graph_root)).map_err(|error| {
        failure(
            format!("Available space under {graph_root} could not be observed: {error}; nothing was restored"),
            json!({"reservation": r.view(), "graph_root": {"path": graph_root,
                "known": false, "available_bytes": Value::Null, "error": error.to_string()}}),
        )
    })?;
    if available < required {
        return Err(failure(
            format!("{available} bytes available under {graph_root}, {required} required; nothing was restored"),
            json!({"reservation": r.view()}),
        ));
    }
    let since = crate::now() as i64;
    // The reservation keeps the state the restore started from: a collected reservation stays collected
    // whatever this attempt does, and only a verified restore archives it.
    let from = r.state.clone();
    mg::merge_state(
        db,
        uuid,
        &from,
        &json!({"local_restore_attempt": attempt, "local_restore_operation": id, "local_restore_started_at": since,
            "local_restore_source": if in_place { "kept_checkpoint_files" } else { "preserved_archive" }}),
    )?;
    let open = |path: &Path| {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    };
    let stdout = open(&dir.join(format!("restore-local-attempt-{attempt}.stdout")))?;
    let stderr = open(&dir.join(format!("restore-local-attempt-{attempt}.stderr")))?;
    let import = format!("--import={}", dir.join(mg::ARCHIVE).display());
    let arguments: Vec<&str> = if in_place {
        // In place: the same container ID resumes from the files Podman kept for it.
        vec![
            "container",
            "restore",
            "--keep",
            "--file-locks",
            "--print-stats",
            &r.container_id,
        ]
    } else {
        vec![
            "container",
            "restore",
            &import,
            "--name",
            name,
            "--keep",
            "--file-locks",
            "--print-stats",
        ]
    };
    let bound = Bound::new(
        Path::new(&graph_root),
        required,
        &unit,
        name,
        uuid,
        id,
        attempt,
        &r.operation_id,
        &r.image_id,
        since,
        in_place.then_some(r.container_id.as_str()),
    )?;
    let (exit, mut prevention) = mg::scoped_podman(
        &unit,
        id,
        RESTORE_SECONDS,
        &arguments,
        stdout,
        stderr,
        Some(&bound),
    )?;
    let observed = lc::inspect(name)?;
    cleanup::confirm_or_thaw(
        &mut prevention,
        observed.as_ref().and_then(|c| c["Id"].as_str()),
    );
    let log = dir.join(format!("restore-local-attempt-{attempt}.log"));
    let preserved = observed
        .as_ref()
        .is_some_and(|c| mg::copy_podman_log(c, "RestoreLog", "restore.log", &log));
    if let Err(reason) = verify(
        &r,
        uuid,
        observed.as_ref(),
        &log,
        preserved,
        since,
        in_place,
    ) {
        let stderr_tail =
            mg::read_bounded(&dir.join(format!("restore-local-attempt-{attempt}.stderr")))
                .unwrap_or_default();
        let leftovers = observed
            .as_ref()
            .and_then(|c| c["Id"].as_str())
            .map(|cid| cleanup::runtime_processes(cid, since));
        // A failed attempt that created a container here removes it, but only when it is provably this
        // attempt's own and nothing of it survives; otherwise everything is preserved and reported.
        let mut removed = Value::Null;
        if !in_place {
            removed = remove_own_failed_container(
                db,
                uuid,
                name,
                observed.as_ref(),
                since,
                leftovers.as_ref(),
            )?;
        }
        // A freeze the bound applied stays applied: it is what stopped a runaway from writing, and only
        // an explicit reclaim ends those processes. It is reported so that no reader has to guess.
        let frozen_now = observed
            .as_ref()
            .and_then(|c| c["Id"].as_str())
            .map(cleanup::frozen);
        let detail = json!({"container_cgroup_frozen_now": frozen_now,
            "reason": reason, "attempt": attempt, "exit_code": exit.code(), "in_place": in_place,
            "observed": observed.as_ref().map(lc::state_view), "restored": observed.as_ref().map(|c| c["State"]["Restored"].clone()),
            "restore_log_preserved": preserved.then(|| log.display().to_string()), "stderr_tail": mg::tail(&stderr_tail),
            "scope_finished": !mg::unit_busy(&unit), "prevention": prevention, "runtime_processes": leftovers,
            "failed_container_removed": removed});
        mg::write_private(
            &dir.join(format!("local-restore-failure-attempt-{attempt}.json")),
            serde_json::to_string_pretty(&detail)?.as_bytes(),
        )?;
        mg::merge_state(
            db,
            uuid,
            &from,
            &json!({"local_restore_failed": detail.clone()}),
        )?;
        return Err(failure(
            format!("The local restore could not be verified: {reason}; the reservation stays {from} and the diagnostics are preserved"),
            detail,
        ));
    }
    let c = observed.ok_or("Restored container not observable")?;
    let container_id = c["Id"].as_str().unwrap_or("").to_string();
    mg::write_private(&dir.join("restore-local.log"), &mg::read_bounded(&log)?)?;
    let now = crate::now() as i64;
    let detail = json!({"restored_locally_by_operation": id, "restored_at": now, "restored_container_id": container_id,
        "memory_source": if in_place { "kept_checkpoint_files" } else { "preserved_archive" },
        "restore_log_sha256": mg::sha256(&log)?, "prevention": prevention.clone()});
    // The reservation is archived in the same transaction that records the restore: the universe is
    // operable again — it can be checkpointed, stopped and deleted — and its history is kept.
    let tx = db.unchecked_transaction()?;
    mg::merge_state(&tx, uuid, RESTORED_LOCALLY, &detail)?;
    let archived = mg::archive_reservation(&tx, uuid, id, RESTORED_LOCALLY)?;
    tx.commit()?;
    Ok(json!({
        "status": "verified", "operation": "migration_restore_local", "universe_uuid": uuid,
        "checkpoint_operation_id": checkpoint, "container_id": container_id,
        "reserved_container_id": r.container_id, "in_place": in_place,
        "memory_source": if in_place { "kept_checkpoint_files" } else { "preserved_archive" },
        "memory_restored": true,
        "restored_observed": lc::state_view(&c), "restored_at": c["State"]["RestoredAt"],
        "restore_log": {"file": dir.join("restore-local.log"), "sha256": detail["restore_log_sha256"], "runtime_git_id": mg::RUNTIME_GIT_ID},
        "restore_scope": {"unit": unit, "finished": !mg::unit_busy(&unit)}, "prevention": prevention,
        "artifact_directory": dir, "reservation": {"state": RESTORED_LOCALLY, "archived": archived},
        "generic_operations": "the reservation is archived: this universe can be checkpointed, stopped, started and deleted again",
        "ownership": if in_place {
            "the reserved container resumed under its own ID and keeps the ownership of its recorded creation"
        } else {
            "this verified migration_restore_local binds the universe UUID to the restored container ID in this host's journal"
        },
        "memory_proof": "the restore log and Podman's Restored state are this operation's evidence; memory continuity itself is established by an observer outside the universe",
        "scope": SCOPE,
    }))
}
/// Removes a container this operation's own failed restore created, and only that: not running, created
/// after the attempt began, carrying the universe label, owned by no verified operation, and with nothing
/// of the attempt still alive in its cgroups. Anything else is preserved and reported.
fn remove_own_failed_container(
    db: &Connection,
    uuid: &str,
    name: &str,
    observed: Option<&Value>,
    since: i64,
    leftovers: Option<&Value>,
) -> Result<Value, Error> {
    let Some(c) = observed else {
        return Ok(json!({"action": "none_absent"}));
    };
    let container_id = c["Id"].as_str().unwrap_or("").to_string();
    let refuse = |reason: &str| {
        Ok(json!({"action": "kept", "reason": reason, "container_id": container_id}))
    };
    if c["Config"]["Labels"][UNIVERSE_LABEL].as_str() != Some(uuid) {
        return refuse("the container under the universe name is not labelled for this universe, so this attempt did not create it");
    }
    if lc::process_active(c) {
        return refuse("the container is running; it is not removed");
    }
    if !c["Created"]
        .as_str()
        .and_then(lc::epoch)
        .is_some_and(|t| t >= since)
    {
        return refuse("the container predates this attempt");
    }
    if ds::verified_owner(db, &container_id)? {
        return refuse("the container is owned by a verified operation of this host");
    }
    if leftovers.is_some_and(|l| l["count"].as_u64().unwrap_or(0) > 0) {
        return refuse(
            "processes of the attempt are still in the container's cgroups; removing it now would unlink their files without freeing the space, so everything is preserved for inspection",
        );
    }
    lc::podman(lc::QUICK, &["rm", &container_id])?;
    if lc::inspect(name)?.is_some() {
        return Err(
            "The container of the failed local restore is still present after removal".into(),
        );
    }
    Ok(json!({"action": "removed", "container_id": container_id}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collected_reservation_keeps_the_explicit_local_restore_path() {
        let reservation = Reservation {
            operation_id: "checkpoint-1".into(),
            container_id: "original-container".into(),
            image_id: "image".into(),
            source_host: "source".into(),
            destination: "destination".into(),
            started_at: "started".into(),
            state: COLLECTED.into(),
            created_at: 1,
            updated_at: 2,
            detail: None,
        };
        let observed = Observed {
            named: None,
            same: false,
            running: false,
            checkpointed: false,
            kept: None,
        };
        let blockers = restore_local_blockers(
            &reservation,
            &observed,
            &json!({"archive_sha256_matches": true}),
            false,
        );
        assert!(blockers.is_empty());
    }
}
