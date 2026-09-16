//! Recovery points: the capture side of a warm standby.
//!
//! `recovery_point_prepare` turns a STOPPED universe into an immutable, digested recovery
//! point in this host's outbox, with a manifest that binds every field the Backup Server
//! design requires of one. It is B1's step 1 and the first half of step 2.
//!
//! WHAT IT PRODUCES, AND WHAT IT DOES NOT. The design's manifest is *sealed*: authenticated by
//! an Ed25519 signature over its canonical serialization. This code base carries no signing
//! crate -- serde_json and rusqlite are its entire dependency list, and the crate registry is
//! not reachable from this build -- so a signature cannot be produced here, and this module
//! does not pretend to produce one. The point it writes is in the `prepared` state of the
//! design's own typed list, one step short of `sealed`, and the manifest says so in three
//! fields: `signed: false`, `signature: null`, `state: "prepared"`. A verifier that treats an
//! unsigned manifest as sealed is wrong; one that refuses it is doing its job. Signing and
//! chunk encryption are two well-delimited additions once the dependency decision is taken,
//! and that decision is the operator's, not something to slip in beside a capture.
//!
//! The class is `quiescent`, and only that: the universe must already be stopped, and this
//! operation refuses a running one rather than stopping it itself. Stopping is the typed
//! `stop` operation, which already reports whether Podman escalated to SIGKILL. What the
//! manifest records about the stop is OBSERVED from the container -- its status, exit code
//! and finish time -- because the observations journal keys by operation name and cannot say
//! which universe a past stop belonged to. An exit code of 137 is the escalation signature
//! the API document names, and a point whose stop escalated has no class: it is recorded as
//! a failed capture, never as a weaker success.
use crate::lifecycle::{self as lc, failure};
use crate::migration as mg;
use crate::transfer as tr;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::Instant;

type Error = Box<dyn std::error::Error>;

pub const FORMAT: &str = "podmesh-recovery-point/0-unsigned-unencrypted";
pub const MANIFEST: &str = "recovery-point-manifest.json";
pub const ROOTFS: &str = "rootfs.tar";
/// The archive of a live point: Podman's checkpoint export (CRIU images and the writable layer), zstd.
pub const ARCHIVE: &str = "checkpoint.tar.zst";
pub const CAPTURE_STOPPED: &str = "stopped";
pub const CAPTURE_LIVE: &str = "live";
pub const PIECE_ROOTFS: &str = "rootfs-export";
pub const PIECE_CHECKPOINT: &str = "podman-checkpoint-export";
/// The class a live point may claim (docs/BACKUP-SERVER.md): a memory checkpoint bound to the exact disk state
/// it was taken against -- which the dump produces by construction, its processes being stopped when Podman
/// exports the writable layer into the same archive.
const LIVE_CLASS: &str = "memory-coherent";
/// Where a restore's imported image is tagged; only images named nowhere else are ever removed.
pub const RESTORE_REPOSITORY: &str = "localhost/podmesh-restore:";
const EXPORT_TIMEOUT_SECONDS: u64 = 900;
const KILLED_EXIT_CODE: i64 = 137;

pub fn ensure_schema(db: &Connection) -> Result<(), Error> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS recovery_points(
            recovery_point_uuid TEXT PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            generation INTEGER NOT NULL,
            parent_recovery_point_uuid TEXT,
            operation_id TEXT NOT NULL UNIQUE,
            state TEXT NOT NULL,
            manifest_sha256 TEXT NOT NULL,
            rootfs_sha256 TEXT NOT NULL,
            rootfs_bytes INTEGER NOT NULL,
            prepared_at INTEGER NOT NULL,
            outbox TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS recovery_point_restores(
            operation_id TEXT PRIMARY KEY,
            restored_universe_uuid TEXT NOT NULL UNIQUE,
            recovery_point_uuid TEXT NOT NULL,
            source_universe_uuid TEXT NOT NULL,
            imported_image_id TEXT NOT NULL,
            container_id TEXT NOT NULL,
            manifest_sha256 TEXT NOT NULL,
            rootfs_sha256 TEXT NOT NULL,
            manifest_signed INTEGER NOT NULL,
            restored_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS recovery_point_promotions(
            operation_id TEXT PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            restored_universe_uuid TEXT NOT NULL,
            recovery_point_uuid TEXT NOT NULL,
            container_id TEXT NOT NULL,
            lease_generation INTEGER NOT NULL,
            promoted_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS recovery_point_staged(
            recovery_point_uuid TEXT PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            operation_id TEXT NOT NULL UNIQUE,
            generation INTEGER NOT NULL,
            archive_sha256 TEXT NOT NULL,
            archive_bytes INTEGER NOT NULL,
            manifest_sha256 TEXT NOT NULL,
            image_id TEXT NOT NULL,
            inbox TEXT NOT NULL,
            staged_at INTEGER NOT NULL,
            discarded_at INTEGER,
            discard_operation_id TEXT,
            promoted_operation_id TEXT);
         CREATE TABLE IF NOT EXISTS recovery_point_live_captures(
            operation_id TEXT PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            container_id TEXT NOT NULL,
            recovery_point_uuid TEXT NOT NULL,
            began_at INTEGER NOT NULL,
            state TEXT NOT NULL,
            detail TEXT);
         CREATE TABLE IF NOT EXISTS recovery_point_final_captures(
            recovery_point_uuid TEXT PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            container_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            captured_at INTEGER NOT NULL,
            resumed_operation_id TEXT,
            resumed_at INTEGER);
         CREATE TABLE IF NOT EXISTS recovery_point_live_promote_attempts(
            operation_id TEXT PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            recovery_point_uuid TEXT NOT NULL,
            lease_generation INTEGER NOT NULL,
            launched_at INTEGER NOT NULL,
            state TEXT NOT NULL,
            detail TEXT);
         CREATE TABLE IF NOT EXISTS recovery_point_live_promotions(
            operation_id TEXT PRIMARY KEY,
            universe_uuid TEXT NOT NULL,
            recovery_point_uuid TEXT NOT NULL,
            container_id TEXT NOT NULL,
            lease_generation INTEGER NOT NULL,
            restore_log_sha256 TEXT NOT NULL,
            promoted_at INTEGER NOT NULL);",
    )?;
    // The capture mode of a point, added beside the original columns: a journal from before live points gains
    // it with the mode every earlier point was made with.
    let has_capture = {
        let mut s = db.prepare("PRAGMA table_info(recovery_points)")?;
        let names: Vec<String> = s.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<_, _>>()?;
        names.iter().any(|n| n == "capture")
    };
    if !has_capture {
        db.execute("ALTER TABLE recovery_points ADD COLUMN capture TEXT NOT NULL DEFAULT 'stopped'", [])?;
    }
    // The uncompressed size a staging measured, reused by the promotion of the same verified bytes.
    let staged_has_size: bool = db.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('recovery_point_staged') WHERE name='uncompressed_bytes'",
        [],
        |r| Ok(r.get::<_, i64>(0)? > 0),
    )?;
    if !staged_has_size {
        db.execute("ALTER TABLE recovery_point_staged ADD COLUMN uncompressed_bytes INTEGER", [])?;
    }
    Ok(())
}

const PROMOTION_SCOPE: &str = "The lease this promotion required lives in this host's journal. It proves this host's own restraint, not mutual exclusion: a host that never asks is not restrained by it. Nothing here proves the previous holder is stopped.";

fn promotion_view(db: &Connection, id: &str, replayed: bool) -> Result<Option<Value>, Error> {
    Ok(db
        .query_row(
            "SELECT universe_uuid,restored_universe_uuid,recovery_point_uuid,container_id,lease_generation
             FROM recovery_point_promotions WHERE operation_id=?1",
            [id],
            |r| Ok(json!({
                "universe_uuid": r.get::<_, String>(0)?, "restored_universe_uuid": r.get::<_, String>(1)?,
                "recovery_point_uuid": r.get::<_, String>(2)?, "container_id": r.get::<_, String>(3)?,
                "lease_generation": r.get::<_, i64>(4)?, "started": false,
                "replayed": replayed, "scope": PROMOTION_SCOPE,
            })),
        )
        .optional()?
        .map(|mut v| {
            // The network the promoted universe was created with, read from its own create request:
            // the profile the caller named, and the address if the managed profile was asked for.
            let created: Option<String> = db
                .query_row("SELECT request FROM operations WHERE id=?1", [format!("{id}-create")], |r| r.get(0))
                .optional()
                .ok()
                .flatten();
            let request: Value = created.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
            v["network"] = json!({"profile": request["network_profile"], "requested_address": request["network_address"]});
            v["secrets"] = request.get("secrets").cloned().unwrap_or(json!([]));
            v
        }))
}

fn restore_view(db: &Connection, id: &str, replayed: bool) -> Result<Option<Value>, Error> {
    Ok(db
        .query_row(
            "SELECT restored_universe_uuid,recovery_point_uuid,source_universe_uuid,imported_image_id,container_id,manifest_sha256,rootfs_sha256,manifest_signed
             FROM recovery_point_restores WHERE operation_id=?1",
            [id],
            |r| Ok(json!({
                "restored_universe_uuid": r.get::<_, String>(0)?, "recovery_point_uuid": r.get::<_, String>(1)?,
                "source_universe_uuid": r.get::<_, String>(2)?, "imported_image_id": r.get::<_, String>(3)?,
                "container_id": r.get::<_, String>(4)?, "manifest_sha256": r.get::<_, String>(5)?,
                "rootfs_sha256": r.get::<_, String>(6)?, "manifest_signed": r.get::<_, i64>(7)? != 0,
                "quarantined": true, "network": "none", "started": false, "replayed": replayed,
                "manifest_verification": "unsigned: the archive is bound to the manifest by digest; the manifest's origin is not authenticated, and this build could not check a signature if one were present",
            })),
        )
        .optional()?)
}

fn host_uuid(db: &Connection) -> Result<String, Error> {
    Ok(db.query_row("SELECT value FROM metadata WHERE key='host_uuid'", [], |r| r.get(0))?)
}

fn latest(db: &Connection, uuid: &str) -> Result<Option<(String, i64)>, Error> {
    Ok(db
        .query_row(
            "SELECT recovery_point_uuid,generation FROM recovery_points WHERE universe_uuid=?1 ORDER BY generation DESC LIMIT 1",
            [uuid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

fn by_operation(db: &Connection, id: &str) -> Result<Option<Value>, Error> {
    let row: Option<(String, String)> = db
        .query_row(
            "SELECT recovery_point_uuid,outbox FROM recovery_points WHERE operation_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((point, outbox)) = row else { return Ok(None) };
    // A collected point's archive is gone; its manifest survives in the retained record, and the replay
    // serves that rather than failing on a file the collector removed.
    let retained: Option<String> = db
        .query_row("SELECT manifest FROM recovery_point_retained WHERE recovery_point_uuid=?1", [&point], |r| r.get(0))
        .optional()?;
    if let Some(text) = retained {
        let manifest: Value = serde_json::from_str(&text)?;
        return Ok(Some(json!({"recovery_point_uuid": point, "outbox": outbox, "manifest": manifest, "replayed": true,
            "collected": true, "note": "the archive was collected after its declared retention; this manifest is the retained one"})));
    }
    let manifest_path = Path::new(&outbox).join(MANIFEST);
    let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
    Ok(Some(json!({"recovery_point_uuid": point, "outbox": outbox, "manifest": manifest, "replayed": true, "collected": false})))
}

/// What the manifest says about the stop, read from the container and not from a journal.
fn stop_evidence(c: &Value) -> Value {
    let state = &c["State"];
    let exit_code = state["ExitCode"].as_i64();
    json!({
        "observed_status": state["Status"],
        "running": state["Running"],
        "exit_code": exit_code,
        "finished_at": state["FinishedAt"],
        "started_at": state["StartedAt"],
        "escalated_to_kill_suspected": exit_code == Some(KILLED_EXIT_CODE),
        "source": "podman inspect at capture time; the observations journal keys by operation name and cannot attribute a past stop to a universe",
    })
}

pub fn execute(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    let uuid = lc::text(request, "universe_uuid")?;
    lc::token(uuid)?;
    lc::ensure_schema(db)?;
    ensure_schema(db)?;
    // The retained-manifest table belongs to the collector's retention module; a replay of a
    // collected point reads it, so it is prepared here too rather than assumed.
    crate::retention::ensure_schema(db)?;
    if operation == "recovery_point_status" {
        return perform(db, request);
    }
    // The journal contract of every other operation. The per-table replay lookups inside
    // `perform` (by operation ID in the point, restore and promotion tables) are reached only
    // when the journal has no verified row for the ID -- an attempt interrupted after its
    // record and before the journal's own update -- and are defence in depth for that window.
    let mut result = lc::journaled(db, request, |db| perform(db, request))?;
    // A replayed prepare is history; beside it, as every replay in this service does, the
    // present: the point's state now, and its manifest -- from the outbox while the archive
    // is there, from the retained record once the collector has taken the archive.
    if result["replayed"] == json!(true) && operation == "recovery_point_prepare" {
        let id = lc::text(request, "operation_id")?;
        if let Some(now) = by_operation(db, id)? {
            result["collected"] = now["collected"].clone();
            result["current_manifest"] = now["manifest"].clone();
        }
    }
    Ok(result)
}

fn perform(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    let uuid = lc::text(request, "universe_uuid")?;
    if operation == "recovery_point_status" {
        let points: Vec<Value> = {
            let mut s = db.prepare(
                "SELECT recovery_point_uuid,generation,parent_recovery_point_uuid,state,manifest_sha256,rootfs_sha256,rootfs_bytes,prepared_at,outbox,capture
                 FROM recovery_points WHERE universe_uuid=?1 ORDER BY generation",
            )?;
            let rows: Vec<Value> = s
                .query_map([uuid], |r| {
                    let capture: String = r.get(9)?;
                    Ok(json!({
                        "recovery_point_uuid": r.get::<_, String>(0)?, "generation": r.get::<_, i64>(1)?,
                        "parent_recovery_point_uuid": r.get::<_, Option<String>>(2)?, "state": r.get::<_, String>(3)?,
                        "manifest_sha256": r.get::<_, String>(4)?, "rootfs_sha256": r.get::<_, String>(5)?,
                        "rootfs_bytes": r.get::<_, i64>(6)?, "prepared_at": r.get::<_, i64>(7)?, "outbox": r.get::<_, String>(8)?,
                        "archive": piece_name(&capture), "capture": capture,
                    }))
                })?
                .collect::<Result<_, _>>()?;
            rows
        };
        let staged = staged_for(db, uuid)?;
        return Ok(json!({"universe_uuid": uuid, "recovery_points": points, "staged": staged,
            "note": "every point here is prepared and unsigned; none is sealed, because this build can produce no signature. rootfs_sha256 and rootfs_bytes name a live point's checkpoint archive; `archive` says which file"}));
    }
    if operation == "recovery_point_restore" {
        return restore(db, request, uuid);
    }
    if operation == "recovery_point_promote" {
        return promote(db, request, uuid);
    }
    if operation == "recovery_point_stage" {
        return stage(db, request, uuid);
    }
    if operation == "recovery_point_discard" {
        return discard(db, request, uuid);
    }
    if operation == "recovery_point_resume" {
        return resume_final(db, request, uuid);
    }
    if operation != "recovery_point_prepare" {
        return Err("Unsupported recovery point operation".into());
    }
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let reference = lc::text(request, "authorization_ref")?;

    // Idempotent by operation ID: a repeat returns the point that operation already made.
    if let Some(previous) = by_operation(db, id)? {
        return Ok(previous);
    }

    // A universe mid-handoff is not captured: its reservation is the authority on its state.
    mg::refuse_if_reserved(db, uuid, "recovery_point_prepare")?;
    let capture = match request.get("capture") {
        None => CAPTURE_STOPPED,
        Some(v) => v.as_str().ok_or("capture must be \"stopped\" or \"live\"")?,
    };
    let resume_after = match request.get("resume") {
        None => true,
        Some(v) => v.as_bool().ok_or("resume must be true or false")?,
    };
    if capture == CAPTURE_LIVE {
        return prepare_live(db, uuid, id, reference, resume_after);
    }
    if !resume_after {
        return Err("resume applies to a live capture only".into());
    }
    if capture != CAPTURE_STOPPED {
        return Err("capture must be \"stopped\" or \"live\"".into());
    }

    let name = format!("podmesh-{uuid}");
    let Some(c) = lc::inspect(&name)? else {
        return Err("No such universe on this host; nothing was captured".into());
    };
    if c["Config"]["Labels"]["io.podmesh.universe"].as_str() != Some(uuid) {
        return Err("The container is not this universe; nothing was captured".into());
    }
    let stop = stop_evidence(&c);
    if stop["running"] == json!(true) {
        return Err("The universe is running; a recovery point of this class needs it stopped first, through the typed stop operation".into());
    }
    let exit_code = stop["exit_code"].as_i64();
    if stop["observed_status"].as_str() != Some("exited") && stop["observed_status"].as_str() != Some("created") {
        return Err(format!(
            "The universe is in state {:?}, neither exited nor created; nothing was captured",
            stop["observed_status"]
        )
        .into());
    }
    // A capture whose stop escalated has no class. The design forbids the label the old
    // draft reached for -- crash-consistent -- and `incoherent` is defined for a running
    // universe, so this is a failed capture rather than a weaker success.
    if exit_code == Some(KILLED_EXIT_CODE) {
        return Err(format!(
            "The universe's last stop escalated to SIGKILL (exit code {KILLED_EXIT_CODE}); a quiescent point cannot be claimed from it and no weaker class exists. Restart it, stop it unforced, and capture again"
        )
        .into());
    }

    // The point's own identifier is minted here. The outbox is keyed by it, never by a
    // migration authorization_id, which only migration_authorize_transfer may issue.
    let point = std::fs::read_to_string("/proc/sys/kernel/random/uuid")?.trim().to_string();
    let outbox = tr::outbox(&point)?;
    tr::private_dir(&outbox)?;
    let rootfs_tmp = outbox.join(format!("{ROOTFS}.partial"));
    let rootfs = outbox.join(ROOTFS);
    // Written by Podman to a partial name and renamed only once it has a size and a digest:
    // Rule 12's own discipline, applied to the archive this lot does produce.
    let out = lc::run_podman(EXPORT_TIMEOUT_SECONDS, &["export", "--output", &rootfs_tmp.to_string_lossy(), &name])?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&rootfs_tmp);
        return Err(format!("podman export failed: {}", String::from_utf8_lossy(&out.stderr).trim()).into());
    }
    let rootfs_bytes = std::fs::metadata(&rootfs_tmp)?.len();
    if rootfs_bytes == 0 {
        let _ = std::fs::remove_file(&rootfs_tmp);
        return Err("podman export produced an empty archive; refusing to record a recovery point of nothing".into());
    }
    let rootfs_sha256 = mg::sha256(&rootfs_tmp)?;
    std::fs::rename(&rootfs_tmp, &rootfs)?;

    let (parent, generation) = match latest(db, uuid)? {
        Some((previous, g)) => (Some(previous), g + 1),
        None => (None, 1),
    };
    let now = crate::now() as i64;
    let host = host_uuid(db)?;

    // The manifest, every field the design binds and, for each one this build cannot fill,
    // a typed absence rather than an omission. Keys sort and whitespace is absent under
    // serde_json's default map, which is RFC 8785 for the integer-and-string manifests this
    // produces; it is not a general canonicalizer and a signer must use one.
    let manifest = json!({
        "format_version": FORMAT,
        "state": "prepared",
        "signed": false,
        "signature": null,
        "signing_algorithm": null,
        "producer_identity": host,
        "authorization_ref": reference,
        "universe_uuid": uuid,
        "recovery_point_uuid": point,
        "parent_recovery_point_uuid": parent,
        "generation": generation,
        "capture_operation_id": id,
        "image_id": c["Image"],
        "image_name_at_capture": c["ImageName"],
        "podman_config": {
            "cmd": c["Config"]["Cmd"], "entrypoint": c["Config"]["Entrypoint"], "env": c["Config"]["Env"],
            "labels": c["Config"]["Labels"], "working_dir": c["Config"]["WorkingDir"], "stop_signal": c["Config"]["StopSignal"],
        },
        "data_lifecycle": null,
        "data_lifecycle_declared": false,
        "data_lifecycle_note": "a PodMesh universe carries no ShaperOS manifest.json; the field the design requires is absent and says so",
        "stop": stop,
        "consistency": {
            "class": "quiescent",
            "quiesce_mechanism": "stopped before capture; export taken while stopped",
            "boundary_opened_at": stop["finished_at"],
            "boundary_closed_at": now,
            "claim_basis": "the universe was observed exited or created for the whole export, and its last exit was not an escalation",
        },
        "pieces": [{
            "name": ROOTFS,
            "kind": PIECE_ROOTFS,
            "order": 0,
            "bytes": rootfs_bytes,
            "plaintext_sha256": rootfs_sha256,
            "ciphertext_sha256": null,
            "chunks": null,
        }],
        "chunking": null,
        "encryption": null,
        "compression": null,
        "declared_exclusions": ["volumes: none exist for this universe class", "image bytes: never captured, per Rule 11"],
        "level_completeness": {"level_2_volumes": "not applicable", "level_3_databases": "not applicable"},
        "producer_software": {"podmesh": env!("CARGO_PKG_VERSION"), "minimum_restore_tool": FORMAT},
        "prepared_at": now,
    });
    let canonical = serde_json::to_string(&manifest)?;
    let manifest_sha256 = mg::sha256_bytes(canonical.as_bytes())?;
    mg::write_private(&outbox.join(MANIFEST), canonical.as_bytes())?;
    if mg::sha256(&outbox.join(MANIFEST))? != manifest_sha256 {
        return Err("The manifest written to the outbox does not hash to the value computed in memory".into());
    }
    db.execute(
        "INSERT INTO recovery_points(recovery_point_uuid,universe_uuid,generation,parent_recovery_point_uuid,operation_id,state,manifest_sha256,rootfs_sha256,rootfs_bytes,prepared_at,outbox,capture)
         VALUES(?1,?2,?3,?4,?5,'prepared',?6,?7,?8,?9,?10,?11)",
        params![point, uuid, generation, parent, id, manifest_sha256, rootfs_sha256, rootfs_bytes as i64, now,
                outbox.to_string_lossy().to_string(), CAPTURE_STOPPED],
    )?;
    Ok(json!({
        "recovery_point_uuid": point,
        "generation": generation,
        "parent_recovery_point_uuid": manifest["parent_recovery_point_uuid"],
        "state": "prepared",
        "signed": false,
        "outbox": outbox,
        "manifest_sha256": manifest_sha256,
        "rootfs_sha256": rootfs_sha256,
        "rootfs_bytes": rootfs_bytes,
        "consistency_class": "quiescent",
        "replayed": false,
        "note": "prepared and unsigned: one step short of the design's sealed state, and the manifest says so",
    }))
}

/// Restore a prepared recovery point from this host's inbox as a QUARANTINED, NEW-IDENTITY
/// universe: created with no network and not started, under a universe UUID the caller
/// chooses and that must differ from the source's. This is B1's step 6.
///
/// What is verified, and what cannot be. The archive is bound to the manifest by digest and
/// size, the manifest is required to be in canonical form, and its format is pinned. Its
/// ORIGIN is not verified: the manifest is unsigned, and this build could not check a
/// signature if one were present. A manifest that claims to be signed is therefore refused
/// outright -- accepting a signature nobody can verify would be trusting the manifest on its
/// own say-so, which is the anchoring-in-nothing the design exists to forbid.
///
/// The container is created through the ordinary `create` operation under a derived
/// operation ID, so the restored universe is owned the way every created universe is owned:
/// by a verified creation in this journal binding it to its container. No new ownership rule
/// was added for it, which is the point.
fn restore(db: &Connection, request: &Value, uuid: &str) -> Result<Value, Error> {
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let reference = lc::text(request, "authorization_ref")?;
    let point = lc::text(request, "recovery_point_uuid")?;
    lc::token(point)?;
    if let Some(previous) = restore_view(db, id, true)? {
        return Ok(previous);
    }
    let create_id = format!("{id}-create");
    lc::token(&create_id)?;
    let inbox = tr::inbox(point)?;
    if !inbox.is_dir() {
        return Err("No recovery point with this identifier in the inbox; nothing was restored".into());
    }
    let (bytes, manifest) = tr::read_document(&inbox, MANIFEST)?
        .ok_or("The inbox holds no manifest for this recovery point; nothing was restored")?;
    let manifest_sha256 = mg::sha256_bytes(&bytes)?;
    if manifest["format_version"].as_str() != Some(FORMAT) {
        return Err(format!("The manifest format {:?} is not {FORMAT}; nothing was restored", manifest["format_version"]).into());
    }
    if serde_json::to_string(&manifest)?.as_bytes() != bytes.as_slice() {
        return Err("The manifest is not in canonical form; its digest cannot be trusted to name it, and nothing was restored".into());
    }
    if manifest["signed"] != json!(false) {
        return Err("The manifest claims a signature this build cannot verify; a signature nobody can check is refused rather than trusted, and nothing was restored".into());
    }
    let source = manifest["universe_uuid"].as_str().ok_or("The manifest names no source universe")?;
    if source == uuid {
        return Err("A restore creates a new identity: the restored universe must not reuse the source universe's UUID".into());
    }
    let piece = &manifest["pieces"][0];
    if piece["kind"].as_str() == Some(PIECE_CHECKPOINT) {
        return Err("This point is a live memory checkpoint: it restores as running processes and gets no quarantined copy; hold it with recovery_point_stage and bring it back with recovery_point_promote under the lease".into());
    }
    let expected_sha = piece["plaintext_sha256"].as_str().ok_or("The manifest's piece has no digest")?;
    let expected_bytes = piece["bytes"].as_u64().ok_or("The manifest's piece has no size")?;
    let rootfs = inbox.join(ROOTFS);
    let Some(size) = tr::regular_file(&rootfs)? else {
        return Err("The inbox holds no rootfs archive for this recovery point; nothing was restored".into());
    };
    if size != expected_bytes {
        return Err(format!("The rootfs archive is {size} bytes, not the {expected_bytes} the manifest binds; nothing was restored").into());
    }
    let rootfs_sha256 = mg::sha256(&rootfs)?;
    if rootfs_sha256 != expected_sha {
        return Err("The rootfs archive does not hash to the digest the manifest binds; nothing was restored".into());
    }
    let cmd: Vec<String> = serde_json::from_value(manifest["podman_config"]["cmd"].clone())
        .map_err(|_| "The manifest carries no usable command; nothing was restored")?;
    if cmd.is_empty() {
        return Err("The manifest's command is empty and PodMesh creates nothing without an explicit command".into());
    }

    // If this operation already imported and created on an earlier attempt that died before
    // recording, the derived create is verified in the journal and names the image it used.
    // Reuse that image rather than importing a second time and recording a different one.
    //
    // Measured 2026-09-14: `podman import` of the same archive returns the existing image
    // (same ID, same creation time) while that image is still in storage, so this branch
    // changes the outcome only when the image was pruned between the create and the resume.
    // That cannot be manufactured in the lab without removing the container too, so the
    // check `tests/check-recovery-point-restore.py` proves the resume, not this branch.
    let earlier: Option<String> = db
        .query_row("SELECT request FROM operations WHERE id=?1 AND status='verified'", [&create_id], |r| r.get(0))
        .optional()?;
    let image = match earlier {
        Some(req) => {
            let v: Value = serde_json::from_str(&req)?;
            v["image"].as_str().ok_or("The earlier create names no image")?.to_string()
        }
        None => {
            let mut args: Vec<String> = vec!["import".into(), "--change".into(), format!("CMD {}", serde_json::to_string(&cmd)?)];
            if let Some(entry) = manifest["podman_config"]["entrypoint"].as_array().filter(|a| !a.is_empty()) {
                args.push("--change".into());
                args.push(format!("ENTRYPOINT {}", serde_json::to_string(entry)?));
            }
            args.push(rootfs.to_string_lossy().to_string());
            args.push(format!("{RESTORE_REPOSITORY}{point}"));
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let out = lc::run_podman(EXPORT_TIMEOUT_SECONDS, &refs)?;
            if !out.status.success() {
                return Err(format!("podman import failed: {}", String::from_utf8_lossy(&out.stderr).trim()).into());
            }
            let imported = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let hex = imported.strip_prefix("sha256:").unwrap_or(&imported);
            if !tr::is_sha256(hex) {
                return Err(format!("podman import did not report an image digest: {imported:?}").into());
            }
            format!("sha256:{hex}")
        }
    };
    let create_request = json!({
        "operation": "create", "operation_id": create_id, "universe_uuid": uuid,
        "authorization_ref": reference, "image": image, "command": cmd,
        // A restore does not carry the managed profile yet (docs/UNIVERSE-NETWORK-CONTRACT.md).
        "network_profile": crate::network::PROFILE_ISOLATED,
    });
    let created = lc::execute(db, &create_request)?;
    let container_id = created_container(&created)?;
    db.execute(
        "INSERT INTO recovery_point_restores VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0,?9)",
        params![id, uuid, point, source, image, container_id, manifest_sha256, rootfs_sha256, crate::now() as i64],
    )?;
    let mut view = restore_view(db, id, false)?.ok_or("The restore was recorded but cannot be read back")?;
    // What the source carried on its network, from the labels the manifest recorded: the profile it
    // ran under and the address allocated to its UUID. A promotion that wants the universe back at
    // that address names it; nothing here allocates on the caller's behalf.
    let labels = &manifest["podman_config"]["labels"];
    view["source_network"] = json!({
        "profile": labels[crate::network::LABEL_PROFILE],
        "ip": labels[crate::network::LABEL_IP],
        "network_uuid": labels[crate::network::LABEL_NETWORK],
    });
    // The secrets the source carried, by name and target only: a promotion that wants them back
    // names them (`secrets`), after declaring them on this host; the quarantined copy carries none.
    view["source_secrets"] = crate::secrets::from_label(labels);
    Ok(view)
}

/// The container a verified create left behind, whether the create just ran or replayed.
fn created_container(created: &Value) -> Result<String, Error> {
    // On resume the create replays: its persisted result is under `original_result`, and the
    // container it names must still be the one Podman has, or the resume is not a resume.
    let id = if created["replayed"] == json!(true) {
        if created["current_matches_recorded_container"] != json!(true) {
            return Err("The earlier create's container is no longer the one Podman holds; the operation cannot be resumed and nothing was recorded".into());
        }
        created["original_result"]["container_id"].as_str()
    } else {
        created["container_id"].as_str()
    };
    Ok(id.ok_or("The create reported no container")?.to_string())
}

/// Promote a quarantined restore into the universe's OWN identity on this host: level 2's
/// takeover step. The universe must be under an activation policy here and this host must
/// hold its live lease -- which `activation_acquire` grants only after the previous holder's
/// lease has lapsed by the takeover margin. The promotion creates the universe from exactly
/// the image and command the quarantined copy was created from, under the network profile the
/// caller names (and, managed, at the address it names), not started; starting it is the
/// caller's, and goes through the same gate.
///
/// What the lease proves is written into every answer: this host's own restraint, in this
/// host's journal. It does not prove the previous holder is stopped. The quarantined copy is
/// left where it is; removing it is the collector's or the operator's, never a side effect.
fn promote(db: &Connection, request: &Value, uuid: &str) -> Result<Value, Error> {
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let reference = lc::text(request, "authorization_ref")?;
    // A staged live point is promoted by its own identifier; a quarantined copy by the copy's.
    if request.get("recovery_point_uuid").is_some() && request.get("restored_universe_uuid").is_none() {
        return promote_live(db, request, uuid);
    }
    let restored = lc::text(request, "restored_universe_uuid")?;
    lc::token(restored)?;
    if let Some(previous) = promotion_view(db, id, true)? {
        return Ok(previous);
    }
    let create_id = format!("{id}-create");
    lc::token(&create_id)?;
    if restored == uuid {
        return Err("A promotion names the quarantined copy and the identity it is promoted into, and they cannot be the same universe".into());
    }
    let row: Option<(String, String, String, String)> = db
        .query_row(
            "SELECT operation_id,recovery_point_uuid,source_universe_uuid,imported_image_id FROM recovery_point_restores WHERE restored_universe_uuid=?1",
            [restored],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((restore_id, point, source, image)) = row else {
        return Err("No quarantined restore under this identifier on this host; nothing was promoted".into());
    };
    if source != uuid {
        return Err(format!("The quarantined copy was restored from universe {source}, not from this one; nothing was promoted").into());
    }
    // The takeover contract, in the order it is refused: a policy must exist here, because a
    // promotion without lease semantics would be a start on nobody's authority; then the lease
    // gate itself, whose three refusals say why.
    //
    // The activation schema is prepared here and not assumed: a standby that has never run
    // an activation operation has no such tables, and the first two-host run of this sequence
    // (2026-09-14) answered "no such table" where it should have said "no activation policy".
    // The single-host check cannot reach that state, since it starts the source first.
    crate::activation::ensure_schema(db)?;
    if crate::activation::policy(db, uuid)?.is_none() {
        return Err("recovery_point_promote refused: this universe is under no activation policy on this host; declare one with activation_require and acquire its lease first".into());
    }
    crate::activation::refuse_if_not_activated(db, uuid, "recovery_point_promote")?;
    // Defence in depth, not the rule: the gate above has already refused every case in which
    // this lookup could find nothing, so this refusal is unreachable by the check and says so.
    let generation = crate::activation::lease(db, uuid)?.map(|l| l.generation).ok_or("The lease vanished between the gate and the record")?;
    // The command is the quarantined copy's own, read from its verified create rather than
    // from the manifest again, so the promoted universe is the copy the operator inspected.
    let quarantined_request: String = db
        .query_row("SELECT request FROM operations WHERE id=?1 AND status='verified'", [format!("{restore_id}-create")], |r| r.get(0))
        .optional()?
        .ok_or("The quarantined copy's creation is not verified in this journal; nothing was promoted")?;
    let quarantined: Value = serde_json::from_str(&quarantined_request)?;
    if quarantined["image"].as_str() != Some(image.as_str()) {
        return Err("The quarantined copy's creation names a different image than its restore record; nothing was promoted".into());
    }
    // The network is the caller's decision, as for `create`: the profile is required, and under the
    // managed profile the address may be named -- the one the restore reported as the source's, so
    // that a universe put back keeps the address allocated to its UUID -- or left to the pool.
    let profile = lc::text(request, "network_profile")
        .map_err(|_| "recovery_point_promote requires network_profile (isolated or managed), as create does; a promoted universe's network is a decision, not a default")?;
    let mut create_request = json!({
        "operation": "create", "operation_id": create_id, "universe_uuid": uuid,
        "authorization_ref": reference, "image": image, "command": quarantined["command"],
        "network_profile": profile,
    });
    if let Some(address) = request.get("network_address") {
        create_request["network_address"] = address.clone();
    }
    // Secrets are the caller's decision too, by name: the restore reported what the source carried,
    // and each must be declared on this host (secret_declare) before the promotion asks for it.
    if let Some(secrets) = request.get("secrets") {
        create_request["secrets"] = secrets.clone();
    }
    let created = lc::execute(db, &create_request)?;
    let container_id = created_container(&created)?;
    db.execute(
        "INSERT INTO recovery_point_promotions VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![id, uuid, restored, point, container_id, generation, crate::now() as i64],
    )?;
    promotion_view(db, id, false)?.ok_or_else(|| "The promotion was recorded but cannot be read back".into())
}

// ------------------------------------------------------------------------------------------------
// Live points: a running universe captured and resumed in place; the copy staged, then promoted.
// ------------------------------------------------------------------------------------------------

/// The archive a point holds, by its capture mode.
pub fn piece_name(capture: &str) -> &'static str {
    if capture == CAPTURE_LIVE { ARCHIVE } else { ROOTFS }
}

/// An exit code that may be unknown (an attempt resumed after an interruption did not see its command end).
struct ExitCode(Option<i32>);
impl ExitCode {
    fn code(&self) -> Option<i32> {
        self.0
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Podman's `--print-stats` answer from a command's preserved stdout, or null when there is none to read.
fn print_stats(stdout: &Path) -> Value {
    let bytes = mg::read_bounded(stdout).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    match text.find('{') {
        Some(i) => serde_json::from_str(text[i..].trim()).unwrap_or(Value::Null),
        None => Value::Null,
    }
}

/// The dump is verified before anything resumes: the same container, checkpointed and stopped; an archive that
/// lists the CRIU entries; a dump log by the qualified runtime that reports success. The command's exit code
/// alone proves nothing.
fn verify_dump(container_id: &str, observed: Option<&Value>, archive: &Path, log: &Path) -> Result<(), String> {
    let c = observed.ok_or("the universe container disappeared during the dump")?;
    if c["Id"].as_str() != Some(container_id) {
        return Err("the container under the universe name is no longer the one that was dumped".into());
    }
    if c["State"]["Checkpointed"] != json!(true) || lc::process_active(c) {
        return Err(format!("the container is not in a checkpointed, stopped state (state {})", lc::status(c)));
    }
    let bytes = fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
    if bytes == 0 {
        return Err("the archive is missing or empty".into());
    }
    let listing = std::process::Command::new("/usr/bin/tar").arg("-tf").arg(archive).output().map_err(|e| format!("the archive could not be listed: {e}"))?;
    let entries = String::from_utf8_lossy(&listing.stdout);
    if !listing.status.success() || !["config.dump", "spec.dump", "checkpoint/inventory.img"].iter().all(|e| entries.lines().any(|l| l == *e)) {
        return Err("the archive is unreadable or lacks the expected checkpoint entries".into());
    }
    if !mg::copy_podman_log(c, "CheckpointLog", "dump.log", log) {
        return Err("the CRIU dump log is unavailable".into());
    }
    let text = fs::read_to_string(log).unwrap_or_default();
    if !text.contains(&format!("(gitid {})", mg::RUNTIME_GIT_ID)) || !text.contains("Dumping finished successfully") {
        return Err("the dump log does not show a successful dump by the qualified private runtime".into());
    }
    Ok(())
}

/// A resumed or promoted universe is verified from outside: the expected container, running and not frozen,
/// reported restored after this operation began, and a restore log by the qualified runtime that reports success.
fn verify_restored(container_id: Option<&str>, observed: Option<&Value>, log: &Path, preserved: bool, since: i64) -> Result<(), String> {
    let c = observed.ok_or("the universe container is absent")?;
    if let Some(expected) = container_id {
        if c["Id"].as_str() != Some(expected) {
            return Err("the container under the universe name is not the one that was checkpointed".into());
        }
    }
    if !lc::process_active(c) {
        return Err(format!("the universe is not running (state {})", lc::status(c)));
    }
    if crate::cleanup::frozen(c["Id"].as_str().unwrap_or("")) {
        return Err("the container's cgroup is frozen: the universe is suspended, not running".into());
    }
    if c["State"]["Restored"] != json!(true) {
        return Err("Podman does not report the container as restored".into());
    }
    if !c["State"]["RestoredAt"].as_str().and_then(lc::epoch).is_some_and(|t| t >= since) {
        return Err("the container was not restored after this operation began".into());
    }
    if !preserved {
        return Err("the CRIU restore log is unavailable".into());
    }
    let text = fs::read_to_string(log).unwrap_or_default();
    if !text.contains(&format!("(gitid {})", mg::RUNTIME_GIT_ID)) || !text.contains("Restore finished successfully") {
        return Err("the restore log does not show a successful restore by the qualified private runtime".into());
    }
    Ok(())
}

/// A recovery point of a RUNNING universe that the universe survives. The qualified private runtime dumps its
/// processes -- memory and descriptors, then the processes end -- Podman exports the writable layer of the
/// stopped container into the same archive, and the universe resumes in place from the checkpoint files Podman
/// kept. Memory and disk are of one instant, the design's `memory-coherent` class, and the universe is
/// interrupted for the dump and the resume rather than stopped and started: its memory continues. Measured
/// about half a second on a small universe (2026-09-16). Refused outside the migration checkpoint's qualified
/// scope (network-disabled, mount-free, unprivileged, no TTY, musl or rseq-free glibc processes, at most 1 GiB,
/// the private runtime intact), and, where a policy names the universe, without this host's live lease:
/// resuming is a start.
///
/// When the resume fails the point is still recorded -- its archive is whole and verified -- and the answer
/// says `resumed: false` with the reason: the universe is then stopped with its checkpoint files kept, and an
/// ordinary `start` begins it afresh without its memory. Nothing here starts it on its own.
fn prepare_live(db: &Connection, uuid: &str, id: &str, reference: &str, resume_after: bool) -> Result<Value, Error> {
    let name = format!("podmesh-{uuid}");
    let Some(c) = lc::inspect(&name)? else {
        return Err("No such universe on this host; nothing was captured".into());
    };
    if c["Config"]["Labels"]["io.podmesh.universe"].as_str() != Some(uuid) {
        return Err("The container is not this universe; nothing was captured".into());
    }
    // An earlier attempt of this operation that the service did not see to the end: the dump may have ended the
    // processes and nobody resumed them. It is settled first -- the universe brought back in place from its kept
    // images if it is stopped and checkpointed -- and reported as an interrupted capture: no point is recorded
    // from bytes whose resume this service did not observe.
    if let Some((began_at, recorded_container, recorded_point, state)) = db
        .query_row(
            "SELECT began_at,container_id,recovery_point_uuid,state FROM recovery_point_live_captures WHERE operation_id=?1",
            [id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)),
        )
        .optional()?
    {
        return settle_interrupted_capture(db, id, uuid, &c, began_at, &recorded_container, &recorded_point, &state);
    }
    crate::activation::refuse_if_not_activated(db, uuid, "recovery_point_prepare (live: the resume is a start)")?;
    let (blockers, facts) = mg::capture_assess(db, uuid, &c)?;
    if !blockers.is_empty() {
        return Err(failure("Live capture preconditions not met; nothing was suspended or written", json!({"blockers": blockers, "facts": facts})));
    }
    let container_id = c["Id"].as_str().ok_or("Universe container has no ID")?.to_string();
    let dump_unit = format!("podmesh-live-capture-{id}.scope");
    let resume_unit = format!("podmesh-live-resume-{id}.scope");
    if mg::unit_busy(&dump_unit) || mg::unit_busy(&resume_unit) {
        return Err("A scope of this operation has not finished, or its state cannot be queried; retry after it finishes".into());
    }
    let work = mg::base()?.join(id);
    fs::create_dir_all(&work)?;
    fs::set_permissions(&work, fs::Permissions::from_mode(0o700))?;
    let open = |path: &Path| fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path);
    let point = fs::read_to_string("/proc/sys/kernel/random/uuid")?.trim().to_string();
    let outbox = tr::outbox(&point)?;
    tr::private_dir(&outbox)?;
    let archive_tmp = outbox.join(format!("{ARCHIVE}.partial"));
    let archive = outbox.join(ARCHIVE);

    // --- Durable before anything is suspended: an interruption from here on is settled by the next attempt.
    let opened_at = crate::now() as i64;
    db.execute(
        "INSERT INTO recovery_point_live_captures VALUES(?1,?2,?3,?4,?5,'dumping',NULL)",
        params![id, uuid, container_id, point, opened_at],
    )?;
    // --- The dump. From here until the resume below, the universe's processes are stopped.
    let began = Instant::now();
    let dump = mg::checkpoint_export(&dump_unit, id, &container_id, &archive_tmp, open(&work.join("dump.stdout"))?, open(&work.join("dump.stderr"))?)?;
    let dump_seconds = began.elapsed().as_secs_f64();
    let closed_at = crate::now() as i64;
    let after_dump = lc::inspect(&name)?;
    let dump_log = work.join("dump.log");
    let dump_stats = print_stats(&work.join("dump.stdout"));
    if let Err(reason) = verify_dump(&container_id, after_dump.as_ref(), &archive_tmp, &dump_log) {
        let _ = fs::remove_file(&archive_tmp);
        // A universe the dump left stopped is brought back in place, once; what happened is in the refusal.
        let brought_back = match after_dump.as_ref() {
            Some(c2) if !lc::process_active(c2) && c2["State"]["Checkpointed"] == json!(true) => {
                let r = mg::restore_kept(&resume_unit, id, &container_id, open(&work.join("resume.stdout"))?, open(&work.join("resume.stderr"))?);
                let now_c = lc::inspect(&name)?;
                json!({"attempted": true, "exit_code": r.ok().and_then(|s| s.code()), "running_now": now_c.as_ref().is_some_and(lc::process_active)})
            }
            _ => json!({"attempted": false}),
        };
        let now_c = lc::inspect(&name)?;
        let stderr = mg::read_bounded(&work.join("dump.stderr")).unwrap_or_default();
        let detail = json!({"reason": reason, "exit_code": dump.code(), "stderr_tail": mg::tail(&stderr), "observed": now_c.as_ref().map(lc::state_view),
            "brought_back": brought_back, "artifact_directory": work, "dump_statistics": dump_stats});
        mg::write_private(&work.join("failure.json"), serde_json::to_string_pretty(&detail)?.as_bytes())?;
        let _ = fs::remove_dir(&outbox);
        db.execute("UPDATE recovery_point_live_captures SET state='dump_failed', detail=?2 WHERE operation_id=?1", params![id, detail.to_string()])?;
        return Err(failure(
            format!("Live capture failed: {reason}; no recovery point was recorded. If the universe is not running now, start begins it afresh without its memory"),
            detail,
        ));
    }
    let dump_log_sha256 = mg::sha256(&dump_log)?;

    // --- The resume: the same container ID, from the files Podman kept. The archive is complete on disk and
    // untouched by it; its digest and the manifest wait until the universe is back, adding nothing to the
    // interruption.
    // A final capture is not resumed: the universe stays stopped with its images kept, so that nothing it does
    // after the dump is lost -- it runs next where this point is promoted, or here again by recovery_point_resume.
    let (resume_seconds, interruption_seconds, now_c, restore_preserved, restore_log, resume_stats, resume_failure, kept_images_removed_bytes) = if resume_after {
        let resume_began = Instant::now();
        let resume = mg::restore_kept(&resume_unit, id, &container_id, open(&work.join("resume.stdout"))?, open(&work.join("resume.stderr"))?)?;
        let resume_seconds = resume_began.elapsed().as_secs_f64();
        let interruption_seconds = began.elapsed().as_secs_f64();
        let now_c = lc::inspect(&name)?;
        let restore_log = work.join("restore.log");
        let restore_preserved = now_c.as_ref().is_some_and(|c| mg::copy_podman_log(c, "RestoreLog", "restore.log", &restore_log));
        let resume_stats = print_stats(&work.join("resume.stdout"));
        let resume_failure = verify_restored(Some(&container_id), now_c.as_ref(), &restore_log, restore_preserved, opened_at).err();
        // The memory image Podman kept for the resume is a second copy of the universe's memory beside the archive;
        // once the resume is verified it serves nothing, and it is removed from the container's own storage.
        let kept_images_removed_bytes = if resume_failure.is_none() { now_c.as_ref().map(remove_kept_images) } else { None };
        if let Some(reason) = &resume_failure {
            let stderr = mg::read_bounded(&work.join("resume.stderr")).unwrap_or_default();
            mg::write_private(
                &work.join("resume-failure.json"),
                serde_json::to_string_pretty(&json!({"reason": reason, "exit_code": resume.code(), "stderr_tail": mg::tail(&stderr),
                    "observed": now_c.as_ref().map(lc::state_view)}))?
                .as_bytes(),
            )?;
        }
        (resume_seconds, Some(interruption_seconds), now_c, restore_preserved, restore_log, resume_stats, resume_failure, kept_images_removed_bytes)
    } else {
        (0.0, None, after_dump.clone(), false, work.join("restore.log"), Value::Null, None, None)
    };

    // --- The point: digest, rename, manifest, record.
    let archive_bytes = fs::metadata(&archive_tmp)?.len();
    let archive_sha256 = mg::sha256(&archive_tmp)?;
    fs::set_permissions(&archive_tmp, fs::Permissions::from_mode(0o600))?;
    fs::rename(&archive_tmp, &archive)?;
    let (parent, generation) = match latest(db, uuid)? {
        Some((previous, g)) => (Some(previous), g + 1),
        None => (None, 1),
    };
    let now = crate::now() as i64;
    let host = host_uuid(db)?;
    let capture = json!({
        "mode": CAPTURE_LIVE, "container_id": container_id, "final": !resume_after,
        "dump_seconds": round3(dump_seconds), "resume_seconds": round3(resume_seconds), "interruption_seconds": interruption_seconds.map(round3),
        "dump_statistics": dump_stats, "resume_statistics": resume_stats,
        "resumed": resume_after && resume_failure.is_none(), "resume_failure": resume_failure,
        "kept_images_removed_bytes": kept_images_removed_bytes,
        "dump_log_sha256": dump_log_sha256,
        "source_runtime": {"binary_sha256": facts["runtime"]["sha256"][mg::RUNTIME_REAL], "git_id": mg::RUNTIME_GIT_ID, "kernel_release": facts["runtime"]["kernel"]},
        "restore_log_sha256": if restore_preserved { mg::sha256(&restore_log).ok() } else { None },
        "runtime_git_id": mg::RUNTIME_GIT_ID, "artifact_directory": work,
    });
    let manifest = json!({
        "format_version": FORMAT,
        "state": "prepared",
        "signed": false,
        "signature": null,
        "signing_algorithm": null,
        "producer_identity": host,
        "authorization_ref": reference,
        "universe_uuid": uuid,
        "recovery_point_uuid": point,
        "parent_recovery_point_uuid": parent,
        "generation": generation,
        "capture_operation_id": id,
        "image_id": c["Image"],
        "image_name_at_capture": c["ImageName"],
        "podman_config": {
            "cmd": c["Config"]["Cmd"], "entrypoint": c["Config"]["Entrypoint"], "env": c["Config"]["Env"],
            "labels": c["Config"]["Labels"], "working_dir": c["Config"]["WorkingDir"], "stop_signal": c["Config"]["StopSignal"],
        },
        "data_lifecycle": null,
        "data_lifecycle_declared": false,
        "data_lifecycle_note": "a PodMesh universe carries no ShaperOS manifest.json; the field the design requires is absent and says so",
        "stop": null,
        "stop_note": if resume_after {
            "no stop operation: the universe was checkpointed by the qualified runtime and resumed in place from the kept images"
        } else {
            "no stop operation: the universe was checkpointed by the qualified runtime and left stopped with its images kept; it runs next where this point is promoted, or here again by recovery_point_resume"
        },
        "consistency": {
            "class": LIVE_CLASS,
            "quiesce_mechanism": "CRIU dump by the qualified private runtime: processes frozen, memory and descriptors dumped, processes ended; Podman exported the writable layer while the universe was stopped, into the same archive; the universe then resumed in place from the kept checkpoint images",
            "boundary_opened_at": opened_at,
            "boundary_closed_at": closed_at,
            "maximum_freeze_seconds": 300,
            "claim_basis": "the memory image and the writable layer were taken while the universe's processes were stopped by the same dump, so the memory is bound to the exact disk state it was taken against; the dump log names the qualified runtime and reports success",
        },
        "capture": capture,
        "pieces": [{
            "name": ARCHIVE,
            "kind": PIECE_CHECKPOINT,
            "order": 0,
            "bytes": archive_bytes,
            "plaintext_sha256": archive_sha256,
            "ciphertext_sha256": null,
            "chunks": null,
            "compression": "zstd",
            "contents": "Podman checkpoint export: CRIU images under checkpoint/, config.dump, spec.dump, network.status, the writable layer as rootfs-diff.tar",
        }],
        "chunking": null,
        "encryption": null,
        "compression": null,
        "declared_exclusions": ["volumes: none exist for this universe class", "image bytes: never captured, per Rule 11", "network: none; a restore keeps the universe network-disabled"],
        "level_completeness": {"level_2_volumes": "not applicable", "level_3_databases": "not applicable"},
        "producer_software": {"podmesh": env!("CARGO_PKG_VERSION"), "minimum_restore_tool": FORMAT},
        "prepared_at": now,
    });
    let canonical = serde_json::to_string(&manifest)?;
    let manifest_sha256 = mg::sha256_bytes(canonical.as_bytes())?;
    mg::write_private(&outbox.join(MANIFEST), canonical.as_bytes())?;
    if mg::sha256(&outbox.join(MANIFEST))? != manifest_sha256 {
        return Err("The manifest written to the outbox does not hash to the value computed in memory".into());
    }
    let tx = db.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO recovery_points(recovery_point_uuid,universe_uuid,generation,parent_recovery_point_uuid,operation_id,state,manifest_sha256,rootfs_sha256,rootfs_bytes,prepared_at,outbox,capture)
         VALUES(?1,?2,?3,?4,?5,'prepared',?6,?7,?8,?9,?10,?11)",
        params![point, uuid, generation, parent, id, manifest_sha256, archive_sha256, archive_bytes as i64, now, outbox.to_string_lossy().to_string(), CAPTURE_LIVE],
    )?;
    tx.execute("UPDATE recovery_point_live_captures SET state='recorded' WHERE operation_id=?1", [id])?;
    if !resume_after {
        tx.execute(
            "INSERT INTO recovery_point_final_captures(recovery_point_uuid,universe_uuid,container_id,operation_id,captured_at) VALUES(?1,?2,?3,?4,?5)",
            params![point, uuid, container_id, id, now],
        )?;
    }
    tx.commit()?;
    Ok(json!({
        "recovery_point_uuid": point,
        "generation": generation,
        "parent_recovery_point_uuid": manifest["parent_recovery_point_uuid"],
        "state": "prepared",
        "signed": false,
        "outbox": outbox,
        "manifest_sha256": manifest_sha256,
        "archive": {"name": ARCHIVE, "sha256": archive_sha256, "bytes": archive_bytes},
        "consistency_class": LIVE_CLASS,
        "capture": manifest["capture"],
        "resumed": resume_after && resume_failure.is_none(),
        "final": !resume_after,
        "universe": now_c.as_ref().map(lc::state_view),
        "replayed": false,
        "note": if !resume_after {
            "prepared and unsigned; a FINAL capture: the universe is stopped with its images kept and loses nothing after the dump; promote this point elsewhere, or bring it back here with recovery_point_resume"
        } else if resume_failure.is_none() {
            "prepared and unsigned; the universe resumed in place with its memory, interrupted for the dump and the resume"
        } else {
            "prepared and unsigned; THE RESUME FAILED: the universe is stopped with its checkpoint files kept, and start begins it afresh without its memory"
        },
    }))
}

/// The memory image files Podman kept under the container's own storage (`<StaticDir>/checkpoint`), removed once a
/// resume from them is verified. Bounded to that one directory, which must hold a CRIU inventory; the bytes freed,
/// or zero when there was nothing this service recognizes.
fn remove_kept_images(c: &Value) -> u64 {
    let Some(dir) = c["StaticDir"].as_str().filter(|d| d.starts_with(mg::CONTAINER_STORAGE) && !d.contains("..")) else { return 0 };
    let kept = Path::new(dir).join("checkpoint");
    if !kept.join("inventory.img").is_file() {
        return 0;
    }
    let bytes: u64 = fs::read_dir(&kept)
        .map(|entries| entries.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
        .unwrap_or(0);
    match fs::remove_dir_all(&kept) {
        Ok(()) => bytes,
        Err(_) => 0,
    }
}

/// Settles an earlier attempt of the same live capture that the service did not see to the end. Never records a
/// point: whatever the archive holds, its resume was not observed by this service. The universe is brought back in
/// place if the dump left it stopped and checkpointed; otherwise its state is reported as found.
#[allow(clippy::too_many_arguments)]
fn settle_interrupted_capture(db: &Connection, id: &str, uuid: &str, c: &Value, began_at: i64, container_id: &str, point: &str, state: &str) -> Result<Value, Error> {
    if state == "recorded" {
        return Err("This capture was recorded but its record cannot be read back; nothing was repeated".into());
    }
    let dump_unit = format!("podmesh-live-capture-{id}.scope");
    let resume_unit = format!("podmesh-live-resume-{id}.scope");
    if mg::unit_busy(&dump_unit) || mg::unit_busy(&resume_unit) {
        return Err("An earlier attempt of this capture is still running in its scope; retry after it finishes".into());
    }
    let work = mg::base()?.join(id);
    let mut brought_back = json!({"attempted": false});
    if c["Id"].as_str() == Some(container_id) && !lc::process_active(c) && c["State"]["Checkpointed"] == json!(true) && state == "dumping" {
        let since = crate::now() as i64;
        let open = |path: &Path| fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path);
        let exit = mg::restore_kept(&format!("podmesh-live-settle-{id}.scope"), id, container_id, open(&work.join("settle.stdout"))?, open(&work.join("settle.stderr"))?);
        let now_c = lc::inspect(&format!("podmesh-{uuid}"))?;
        let log = work.join("settle-restore.log");
        let preserved = now_c.as_ref().is_some_and(|c| mg::copy_podman_log(c, "RestoreLog", "restore.log", &log));
        let verified = verify_restored(Some(container_id), now_c.as_ref(), &log, preserved, since);
        if verified.is_ok() {
            if let Some(c2) = now_c.as_ref() {
                remove_kept_images(c2);
            }
        }
        brought_back = json!({"attempted": true, "exit_code": exit.ok().and_then(|s| s.code()), "verified": verified.is_ok(),
            "reason": verified.err(), "observed": now_c.as_ref().map(lc::state_view)});
    }
    let outbox = tr::outbox(point)?;
    for name in [format!("{ARCHIVE}.partial"), ARCHIVE.to_string(), MANIFEST.to_string()] {
        let _ = fs::remove_file(outbox.join(name));
    }
    let _ = fs::remove_dir(&outbox);
    let detail = json!({"interrupted_attempt_began_at": began_at, "state_found": state, "brought_back": brought_back,
        "observed": lc::observe(uuid)?});
    db.execute("UPDATE recovery_point_live_captures SET state='interrupted', detail=?2 WHERE operation_id=?1", params![id, detail.to_string()])?;
    Err(failure(
        "An earlier attempt of this live capture was interrupted before its point was recorded; the universe was settled as reported and no point was recorded. Capture again under a new operation ID",
        detail,
    ))
}

/// Brings a universe left stopped by a final live capture back in place, with its memory, from the images the
/// capture kept: the way back when its promotion elsewhere did not happen. It is a start, so it passes the lease
/// gate and the reservation gate; and it refuses once the container no longer holds that capture's images.
fn resume_final(db: &Connection, request: &Value, uuid: &str) -> Result<Value, Error> {
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let point = lc::text(request, "recovery_point_uuid")?;
    lc::token(point)?;
    let row: Option<(String, String, Option<String>, Option<i64>)> = db
        .query_row(
            "SELECT universe_uuid,container_id,resumed_operation_id,resumed_at FROM recovery_point_final_captures WHERE recovery_point_uuid=?1",
            [point],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((owner, container_id, resumed_by, resumed_at)) = row else {
        return Err("No final capture with this identifier on this host; nothing was resumed".into());
    };
    if owner != uuid {
        return Err(format!("This final capture is of universe {owner}, not of the one this request names; nothing was resumed").into());
    }
    let name = format!("podmesh-{uuid}");
    if let Some(by) = resumed_by {
        if by == id {
            let c = lc::inspect(&name)?;
            return Ok(json!({"universe_uuid": uuid, "recovery_point_uuid": point, "resumed": true, "resumed_at": resumed_at,
                "universe": c.as_ref().map(lc::state_view), "replayed": true}));
        }
        return Err(format!("This final capture was already resumed here by operation {by}; nothing was resumed again").into());
    }
    let unit = format!("podmesh-final-resume-{id}.scope");
    if mg::unit_busy(&unit) {
        return Err("The resume scope of this operation has not finished; retry after it finishes".into());
    }
    mg::refuse_if_reserved(db, uuid, "recovery_point_resume")?;
    crate::activation::refuse_if_not_activated(db, uuid, "recovery_point_resume (the resume is a start)")?;
    let Some(c) = lc::inspect(&name)? else {
        return Err("The universe's container is no longer on this host (deleted after its promotion elsewhere?); nothing was resumed".into());
    };
    let dir = mg::base()?.join(id);
    fs::create_dir_all(&dir)?;
    let since = crate::now() as i64;
    // An earlier attempt of this operation may have resumed it already: observed, not repeated.
    let already = c["Id"].as_str() == Some(container_id.as_str()) && lc::process_active(&c) && c["State"]["Restored"] == json!(true);
    let (exit_code, seconds, observed) = if already {
        (None, 0.0, Some(c))
    } else {
        if c["Id"].as_str() != Some(container_id.as_str()) {
            return Err("The container under this universe's name is not the one the final capture left; nothing was resumed".into());
        }
        if lc::process_active(&c) || c["State"]["Checkpointed"] != json!(true) {
            return Err(failure("The container is not stopped with a checkpoint kept; nothing was resumed", json!({"observed": lc::state_view(&c)})));
        }
        let open = |path: &Path| fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path);
        let began = Instant::now();
        let exit = mg::restore_kept(&unit, id, &container_id, open(&dir.join("resume.stdout"))?, open(&dir.join("resume.stderr"))?)?;
        (exit.code(), began.elapsed().as_secs_f64(), lc::inspect(&name)?)
    };
    let log = dir.join("restore.log");
    let preserved = observed.as_ref().is_some_and(|c| mg::copy_podman_log(c, "RestoreLog", "restore.log", &log));
    let bound = if already { 0 } else { since };
    if let Err(reason) = verify_restored(Some(&container_id), observed.as_ref(), &log, preserved, bound) {
        let stderr = mg::read_bounded(&dir.join("resume.stderr")).unwrap_or_default();
        return Err(failure(
            format!("The resume could not be verified: {reason}; if the universe is not running, start begins it afresh without its memory"),
            json!({"exit_code": exit_code, "stderr_tail": mg::tail(&stderr), "observed": observed.as_ref().map(lc::state_view)}),
        ));
    }
    if let Some(c2) = observed.as_ref() {
        remove_kept_images(c2);
    }
    let now = crate::now() as i64;
    db.execute(
        "UPDATE recovery_point_final_captures SET resumed_operation_id=?2, resumed_at=?3 WHERE recovery_point_uuid=?1",
        params![point, id, now],
    )?;
    Ok(json!({"universe_uuid": uuid, "recovery_point_uuid": point, "resumed": true, "resumed_at": now, "seconds": round3(seconds),
        "universe": observed.as_ref().map(lc::state_view), "replayed": false,
        "note": "the universe runs again here with the memory of its final capture; the point stays recorded, and a copy staged elsewhere must not be promoted now"}))
}

/// Whether a verified live promotion of this host bound this container ID, whatever the universe: the collector's
/// and the abort paths' question.
pub(crate) fn promoted_live_container(db: &Connection, container_id: &str) -> Result<bool, Error> {
    ensure_schema(db)?;
    Ok(db
        .query_row("SELECT 1 FROM recovery_point_live_promotions WHERE container_id=?1", [container_id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn staged_row(db: &Connection, point: &str) -> Result<Option<(String, String, Option<i64>, Option<String>, Option<String>, String, String, i64)>, Error> {
    Ok(db
        .query_row(
            "SELECT universe_uuid,operation_id,discarded_at,discard_operation_id,promoted_operation_id,inbox,archive_sha256,archive_bytes
             FROM recovery_point_staged WHERE recovery_point_uuid=?1",
            [point],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
        )
        .optional()?)
}

const STAGED_NOTE: &str = "a memory checkpoint restores as running processes, so no copy is created here: the archive is held, verified against its manifest, and restored only by recovery_point_promote under the lease";

fn staged_view(db: &Connection, id: &str, replayed: bool) -> Result<Option<Value>, Error> {
    let row: Option<(String, String, i64, String, i64, String, String, String, i64, Option<i64>, Option<String>)> = db
        .query_row(
            "SELECT recovery_point_uuid,universe_uuid,generation,archive_sha256,archive_bytes,manifest_sha256,image_id,inbox,staged_at,discarded_at,promoted_operation_id
             FROM recovery_point_staged WHERE operation_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?)),
        )
        .optional()?;
    let Some((point, uuid, generation, sha, bytes, manifest_sha256, image, inbox, staged_at, discarded_at, promoted)) = row else { return Ok(None) };
    let present = discarded_at.is_none() && tr::regular_file(&Path::new(&inbox).join(ARCHIVE)).ok().flatten() == Some(bytes as u64);
    let image_present = lc::images()?.iter().any(|i| lc::image_id(i) == image);
    Ok(Some(json!({
        "recovery_point_uuid": point, "universe_uuid": uuid, "generation": generation,
        "archive": {"name": ARCHIVE, "sha256": sha, "bytes": bytes, "present": present},
        "manifest_sha256": manifest_sha256, "image_id": image, "image_present_on_this_host": image_present,
        "staged": true, "quarantined": false, "container": null, "started": false,
        "staged_at": staged_at, "discarded_at": discarded_at, "promoted_operation_id": promoted, "replayed": replayed,
        "consistency_class": LIVE_CLASS, "note": STAGED_NOTE,
        "manifest_verification": "unsigned: the archive is bound to the manifest by digest; the manifest's origin is not authenticated, and this build could not check a signature if one were present",
    })))
}

/// Every live point staged on this host for a universe, with whether its archive is still there.
fn staged_for(db: &Connection, uuid: &str) -> Result<Vec<Value>, Error> {
    let mut s = db.prepare(
        "SELECT recovery_point_uuid,operation_id,generation,archive_sha256,archive_bytes,image_id,inbox,staged_at,discarded_at,promoted_operation_id
         FROM recovery_point_staged WHERE universe_uuid=?1 ORDER BY generation",
    )?;
    let rows: Vec<(String, String, i64, String, i64, String, String, i64, Option<i64>, Option<String>)> = s
        .query_map([uuid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?)))?
        .collect::<Result<_, _>>()?;
    let now = crate::now() as i64;
    Ok(rows
        .into_iter()
        .map(|(point, op, generation, sha, bytes, image, inbox, staged_at, discarded_at, promoted)| {
            let present = discarded_at.is_none() && tr::regular_file(&Path::new(&inbox).join(ARCHIVE)).ok().flatten() == Some(bytes as u64);
            json!({"recovery_point_uuid": point, "operation_id": op, "generation": generation,
                "archive": {"name": ARCHIVE, "sha256": sha, "bytes": bytes, "present": present}, "image_id": image,
                "staged_at": staged_at, "age_seconds": now - staged_at, "discarded_at": discarded_at, "promoted_operation_id": promoted})
        })
        .collect())
}

/// Hold a live point on this host for a later promotion. A memory checkpoint restores as running processes, and
/// a second running instance of a universe is exactly what a standby must never be; so, unlike a stopped point,
/// a live one gets no quarantined container: its archive is verified against its manifest -- canonical form,
/// pinned format, unsigned and saying so, size and digest -- and recorded under the universe's OWN identity, the
/// one it is promoted into. Nothing runs, no image is imported.
fn stage(db: &Connection, request: &Value, uuid: &str) -> Result<Value, Error> {
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let point = lc::text(request, "recovery_point_uuid")?;
    lc::token(point)?;
    if let Some(previous) = staged_view(db, id, true)? {
        return Ok(previous);
    }
    let inbox = tr::inbox(point)?;
    if !inbox.is_dir() {
        return Err("No recovery point with this identifier in the inbox; nothing was staged".into());
    }
    let (bytes, manifest) = tr::read_document(&inbox, MANIFEST)?
        .ok_or("The inbox holds no manifest for this recovery point; nothing was staged")?;
    let manifest_sha256 = mg::sha256_bytes(&bytes)?;
    if manifest["format_version"].as_str() != Some(FORMAT) {
        return Err(format!("The manifest format {:?} is not {FORMAT}; nothing was staged", manifest["format_version"]).into());
    }
    if serde_json::to_string(&manifest)?.as_bytes() != bytes.as_slice() {
        return Err("The manifest is not in canonical form; its digest cannot be trusted to name it, and nothing was staged".into());
    }
    if manifest["signed"] != json!(false) {
        return Err("The manifest claims a signature this build cannot verify; a signature nobody can check is refused rather than trusted, and nothing was staged".into());
    }
    let source = manifest["universe_uuid"].as_str().ok_or("The manifest names no source universe")?;
    if source != uuid {
        return Err(format!("A live point is staged under its own universe's identity ({source}); it has no quarantine identity, and this request names {uuid}").into());
    }
    let piece = &manifest["pieces"][0];
    if piece["kind"].as_str() != Some(PIECE_CHECKPOINT) {
        return Err("This point is the rootfs export of a stopped universe; it is restored into quarantine with recovery_point_restore, not staged".into());
    }
    let expected_sha = piece["plaintext_sha256"].as_str().ok_or("The manifest's piece has no digest")?;
    let expected_bytes = piece["bytes"].as_u64().ok_or("The manifest's piece has no size")?;
    let archive = inbox.join(ARCHIVE);
    let Some(size) = tr::regular_file(&archive)? else {
        return Err("The inbox holds no checkpoint archive for this recovery point; nothing was staged".into());
    };
    if size != expected_bytes {
        return Err(format!("The archive is {size} bytes, not the {expected_bytes} the manifest binds; nothing was staged").into());
    }
    if mg::sha256(&archive)? != expected_sha {
        return Err("The archive does not hash to the digest the manifest binds; nothing was staged".into());
    }
    if let Some((_, op, ..)) = staged_row(db, point)? {
        return Err(format!("This point is already staged on this host by operation {op}").into());
    }
    let image = manifest["image_id"].as_str().ok_or("The manifest names no image")?.trim_start_matches("sha256:").to_string();
    // A staged copy is one this host could promote: what a takeover would discover too late is checked now, and
    // any blocker refuses the staging with the whole list.
    let mut blockers: Vec<String> = vec![];
    if !lc::images()?.iter().any(|i| lc::image_id(i) == image) {
        blockers.push(format!("image sha256:{image} is not present in the local store; PodMesh never pulls"));
    }
    let runtime = mg::runtime_facts(&mut blockers);
    let source = &manifest["capture"]["source_runtime"];
    let local_binary = runtime["sha256"][mg::RUNTIME_REAL].as_str().unwrap_or("");
    if source["binary_sha256"].as_str() != Some(local_binary) {
        blockers.push(format!("the local runtime binary {local_binary} differs from the source runtime {}", source["binary_sha256"].as_str().unwrap_or("unrecorded")));
    }
    if source["git_id"].as_str() != Some(mg::RUNTIME_GIT_ID) {
        blockers.push(format!("the source runtime git ID {} is not the qualified {}", source["git_id"].as_str().unwrap_or("unrecorded"), mg::RUNTIME_GIT_ID));
    }
    let kernel = runtime["kernel"].as_str().unwrap_or("");
    if source["kernel_release"].as_str() != Some(kernel) {
        blockers.push(format!("the local kernel release {kernel} differs from the source kernel {}", source["kernel_release"].as_str().unwrap_or("unrecorded")));
    }
    let mut archive_facts = json!({});
    let mut measured_uncompressed: Option<u64> = None;
    // One decompression reads the configuration, the entry list and the uncompressed size together.
    match crate::restore::archive_scan(&archive) {
        Ok((config, entries, uncompressed)) => {
            measured_uncompressed = Some(uncompressed);
            if config["rootfsImageID"].as_str() != Some(image.as_str()) || config["labels"]["io.podmesh.universe"].as_str() != Some(uuid) {
                blockers.push("the archive configuration does not name the manifest's image and this universe's label".into());
            }
            // Podman's import pulls the image by its recorded name when that name does not resolve locally; the
            // name must resolve here to the very image the manifest binds, or the promotion could pull.
            match config["rootfsImageName"].as_str().filter(|n| !n.is_empty()) {
                None => blockers.push("the archive records no image name; Podman's import would try to pull an empty name".into()),
                Some(name) => {
                    let resolved = lc::run_podman(lc::QUICK, &["image", "inspect", "--format", "{{.Id}}", name]).ok()
                        .filter(|o| o.status.success())
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_start_matches("sha256:").to_string());
                    if resolved.as_deref() != Some(image.as_str()) {
                        blockers.push(format!("the image name {name} the archive records does not resolve here to sha256:{image}; Podman's import would pull it"));
                    }
                }
            }
            let missing: Vec<&str> = ["config.dump", "spec.dump", "checkpoint/inventory.img"].into_iter().filter(|e| !entries.iter().any(|l| l == e)).collect();
            if !missing.is_empty() {
                blockers.push(format!("the archive lacks expected entries: {}", missing.join(", ")));
            }
            archive_facts = json!({"image_id": config["rootfsImageID"], "image_name": config["rootfsImageName"], "universe_label": config["labels"]["io.podmesh.universe"],
                "entries": entries.len(), "uncompressed_bytes": uncompressed});
            let required = uncompressed.saturating_mul(2).saturating_add(mg::SPACE_MARGIN_BYTES);
            let available = mg::available_bytes(Path::new(mg::CONTAINER_STORAGE));
            if available < required {
                blockers.push(format!("{available} bytes available under {}, {required} required to promote this point", mg::CONTAINER_STORAGE));
            }
        }
        Err(e) => blockers.push(format!("the archive cannot be read: {e}")),
    }
    if !blockers.is_empty() {
        return Err(failure("This point could not be promoted on this host; nothing was staged", json!({"blockers": blockers, "archive": archive_facts, "runtime": runtime})));
    }
    let generation = manifest["generation"].as_i64().ok_or("The manifest carries no generation")?;
    db.execute(
        "INSERT INTO recovery_point_staged(recovery_point_uuid,universe_uuid,operation_id,generation,archive_sha256,archive_bytes,manifest_sha256,image_id,inbox,staged_at,uncompressed_bytes)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![point, uuid, id, generation, expected_sha, size as i64, manifest_sha256, image, inbox.to_string_lossy().to_string(), crate::now() as i64,
            measured_uncompressed.map(|b| b as i64)],
    )?;
    staged_view(db, id, false)?.ok_or_else(|| "The staging was recorded but cannot be read back".into())
}

/// Remove a staged point's archive and manifest from this host's inbox. Bounded to the point's own directory,
/// recomputed from its identifier; a point promoted here is refused, its archive being the record of what was
/// restored. The workstation tool uses this to keep the newest copies on each standby.
fn discard(db: &Connection, request: &Value, uuid: &str) -> Result<Value, Error> {
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let point = lc::text(request, "recovery_point_uuid")?;
    lc::token(point)?;
    let Some((owner, staged_by, discarded_at, discard_op, promoted, inbox, _, _)) = staged_row(db, point)? else {
        return Err("No staged point with this identifier on this host; nothing was discarded".into());
    };
    if owner != uuid {
        return Err(format!("This point is staged for universe {owner}, not for the one this request names; nothing was discarded").into());
    }
    let view = |replayed: bool, removed: u64, dir_removed: bool| {
        json!({"recovery_point_uuid": point, "universe_uuid": uuid, "staged_by": staged_by, "discarded": true,
            "bytes_removed": removed, "directory_removed": dir_removed, "replayed": replayed})
    };
    if discard_op.as_deref() == Some(id) {
        return Ok(view(true, 0, !Path::new(&inbox).exists()));
    }
    if let Some(at) = discarded_at {
        return Err(format!("This point was already discarded at {at} by operation {}", discard_op.unwrap_or_default()).into());
    }
    if let Some(p) = promoted {
        return Err(format!("This point was promoted here by operation {p}; its archive stays as the record of what was restored, and nothing was discarded").into());
    }
    let dir = tr::inbox(point)?;
    if dir.to_string_lossy() != inbox {
        return Err("The recorded inbox path is not the path this service derives for the point; nothing was touched".into());
    }
    let mut removed = 0u64;
    for name in [MANIFEST, ARCHIVE] {
        let path = dir.join(name);
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
            Ok(m) if m.is_file() => {
                removed += m.len();
                fs::remove_file(&path)?;
            }
            Ok(_) => return Err(format!("{} is not a regular file; nothing under the directory is removed", path.display()).into()),
        }
    }
    let dir_removed = match fs::remove_dir(&dir) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        // Anything else in the directory was not written by this service; it stays, and the answer says so.
        Err(_) => false,
    };
    db.execute(
        "UPDATE recovery_point_staged SET discarded_at=?2, discard_operation_id=?3 WHERE recovery_point_uuid=?1",
        params![point, crate::now() as i64, id],
    )?;
    Ok(view(false, removed, dir_removed))
}

const LIVE_PROMOTION_NOTE: &str = "a memory checkpoint restore resumes the processes: the universe is running now, with the memory the point holds, under the same lease gate a start goes through; there is no separate start";

fn live_promotion_view(db: &Connection, id: &str, replayed: bool) -> Result<Option<Value>, Error> {
    Ok(db
        .query_row(
            "SELECT universe_uuid,recovery_point_uuid,container_id,lease_generation,restore_log_sha256,promoted_at
             FROM recovery_point_live_promotions WHERE operation_id=?1",
            [id],
            |r| Ok(json!({
                "universe_uuid": r.get::<_, String>(0)?, "recovery_point_uuid": r.get::<_, String>(1)?,
                "container_id": r.get::<_, String>(2)?, "lease_generation": r.get::<_, i64>(3)?,
                "restore_log_sha256": r.get::<_, String>(4)?, "promoted_at": r.get::<_, i64>(5)?,
                "started": true, "resumed_from_checkpoint": true, "consistency_class": LIVE_CLASS,
                "network": {"profile": crate::network::PROFILE_ISOLATED, "requested_address": null}, "secrets": [],
                "replayed": replayed, "scope": PROMOTION_SCOPE, "note": LIVE_PROMOTION_NOTE,
            })),
        )
        .optional()?)
}

/// Whether a verified live promotion of this host binds this universe to this container ID: the ownership of a
/// universe that came back from a staged checkpoint, as a migration restore's is on a destination.
pub(crate) fn promoted_live_here(db: &Connection, uuid: &str, container_id: &str) -> Result<bool, Error> {
    ensure_schema(db)?;
    Ok(db
        .query_row(
            "SELECT 1 FROM recovery_point_live_promotions WHERE universe_uuid=?1 AND container_id=?2",
            params![uuid, container_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Promote a staged live point into the universe's OWN identity on this host: the takeover step for a live
/// copy. The gate is the one every promotion passes -- a policy here, and this host's live lease -- and the
/// effect is a CRIU restore of the archive under the universe's name: the processes resume with the memory
/// the point holds, so the universe is RUNNING when this returns, and the answer says so; there is no separate
/// start, and the lease gate passed here is the one a start would pass. Only where the universe is absent:
/// a container already carrying its name or label refuses the promotion. Verified from outside as a migration
/// restore is; a failed attempt's non-running container is removed so that the operator can try again, a
/// running one is never touched and is reported.
fn promote_live(db: &Connection, request: &Value, uuid: &str) -> Result<Value, Error> {
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let point = lc::text(request, "recovery_point_uuid")?;
    lc::token(point)?;
    if let Some(previous) = live_promotion_view(db, id, true)? {
        return Ok(previous);
    }
    let Some((owner, _, discarded_at, _, promoted, inbox, archive_sha256, archive_bytes)) = staged_row(db, point)? else {
        return Err("No staged point with this identifier on this host; nothing was promoted".into());
    };
    if owner != uuid {
        return Err(format!("This point is staged for universe {owner}, not for the one this request names; nothing was promoted").into());
    }
    if discarded_at.is_some() {
        return Err("This point was discarded from this host; nothing was promoted".into());
    }
    if let Some(p) = promoted {
        return Err(format!("This point was already promoted here by operation {p}; nothing was promoted again").into());
    }
    if let Some(profile) = request.get("network_profile") {
        if profile.as_str() != Some(crate::network::PROFILE_ISOLATED) {
            return Err("A live point restores the universe network-disabled, as it was captured: only the isolated profile is possible, and it may be omitted".into());
        }
    }
    if request.get("secrets").is_some() || request.get("network_address").is_some() {
        return Err("A live point carries no secrets and no address: the universe comes back exactly as it was captured".into());
    }
    let name = format!("podmesh-{uuid}");
    let unit = format!("podmesh-live-promote-{id}.scope");
    let dir = mg::base()?.join(id);
    // An earlier attempt of this operation that reached the restore command: decided by observation only, never
    // by launching a second restore. Its container, if it is there, is verified against the attempt's own launch
    // instant and recorded; if nothing was created, the attempt is forgotten and the gates below run again.
    let attempt: Option<(i64, i64, String)> = db
        .query_row(
            "SELECT launched_at,lease_generation,state FROM recovery_point_live_promote_attempts WHERE operation_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((launched_at, generation, state)) = attempt {
        if mg::unit_busy(&unit) {
            return Err("An earlier attempt of this promotion is still running in its scope; retry after it finishes".into());
        }
        let observed = lc::inspect(&name)?;
        let created_by_attempt = observed.as_ref().is_some_and(|c| {
            // An imported container keeps the source's creation time; the restore instant is this attempt's own.
            c["State"]["RestoredAt"].as_str().and_then(lc::epoch).is_some_and(|t| t >= launched_at) && c["Config"]["Labels"]["io.podmesh.universe"].as_str() == Some(uuid)
        });
        if created_by_attempt {
            return finish_live_promotion(db, id, uuid, point, &dir, launched_at, generation, None, 0.0, json!({"resumed_after_interruption": true, "attempt_state": state}), observed);
        }
        db.execute("DELETE FROM recovery_point_live_promote_attempts WHERE operation_id=?1", [id])?;
    }
    // The gates a create passes, which this path does not go through: a migration reservation or an unresolved
    // restore claim holds the universe; a collected universe's tombstone refuses its identity.
    mg::refuse_if_reserved(db, uuid, "recovery_point_promote")?;
    mg::refuse_identity_reuse(db, uuid, "recovery_point_promote")?;
    crate::activation::ensure_schema(db)?;
    if crate::activation::policy(db, uuid)?.is_none() {
        return Err("recovery_point_promote refused: this universe is under no activation policy on this host; declare one with activation_require and acquire its lease first".into());
    }
    crate::activation::refuse_if_not_activated(db, uuid, "recovery_point_promote")?;
    let generation = crate::activation::lease(db, uuid)?.map(|l| l.generation).ok_or("The lease vanished between the gate and the record")?;
    if let Some(c) = lc::inspect(&name)? {
        return Err(failure(
            "A container already carries this universe's name on this host; a live point is restored only where the universe is absent, and nothing was promoted",
            json!({"container": lc::state_view(&c)}),
        ));
    }
    let labelled = crate::restore::labelled(uuid)?;
    if !labelled.is_empty() {
        return Err(failure(
            "A container already carries this universe's label on this host; a live point is restored only where the universe is absent, and nothing was promoted",
            json!({"labelled_containers": labelled}),
        ));
    }
    if mg::unit_busy(&unit) {
        return Err("The promotion scope of this operation has not finished, or its state cannot be queried; retry after it finishes".into());
    }
    // A private copy, hashed after copying: a later change in the inbox cannot reach the restore.
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let archive = dir.join(ARCHIVE);
    if archive.exists() {
        fs::remove_file(&archive)?;
    }
    mg::copy_private(&Path::new(&inbox).join(ARCHIVE), &archive)?;
    let copied = mg::sha256(&archive)?;
    if copied != archive_sha256 || fs::metadata(&archive)?.len() != archive_bytes as u64 {
        let _ = fs::remove_file(&archive);
        return Err(format!("The staged archive no longer matches its record (sha256 {copied}); nothing was promoted").into());
    }
    let graph_root = lc::podman(lc::QUICK, &["info", "--format", "{{.Store.GraphRoot}}"])?.trim().to_string();
    if graph_root != mg::CONTAINER_STORAGE {
        return Err(format!("Podman graph root {graph_root} is not the qualified default store {}", mg::CONTAINER_STORAGE).into());
    }
    // The copy hashes to the staged bytes, so the size the staging measured holds; a point staged by an older build
    // has none recorded and is measured now.
    let staged_size: Option<i64> = db.query_row("SELECT uncompressed_bytes FROM recovery_point_staged WHERE recovery_point_uuid=?1", [point], |r| r.get(0))?;
    let uncompressed = match staged_size {
        Some(b) if b >= 0 => b as u64,
        _ => crate::restore::uncompressed_bytes(&archive)?,
    };
    let required = uncompressed.saturating_mul(2).saturating_add(mg::SPACE_MARGIN_BYTES);
    let available = mg::available_bytes(Path::new(&graph_root));
    if available < required {
        return Err(format!("{available} bytes available under {graph_root}, {required} required; nothing was promoted").into());
    }
    let open = |path: &Path| fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path);
    let since = crate::now() as i64;
    // Durable before the command can start: a replay decides by observation from here on.
    db.execute(
        "INSERT INTO recovery_point_live_promote_attempts VALUES(?1,?2,?3,?4,?5,'launched',NULL)",
        params![id, uuid, point, generation, since],
    )?;
    let began = Instant::now();
    let bound = crate::cleanup::Bound::new(Path::new(&graph_root), required, &unit);
    let (exit, mut prevention) = mg::restore_import(&unit, id, &archive, &name, open(&dir.join("promote.stdout"))?, open(&dir.join("promote.stderr"))?, Some(&bound))?;
    let seconds = began.elapsed().as_secs_f64();
    let observed = lc::inspect(&name)?;
    crate::cleanup::confirm_or_thaw(&mut prevention, observed.as_ref().and_then(|c| c["Id"].as_str()));
    finish_live_promotion(db, id, uuid, point, &dir, since, generation, exit.code(), seconds, prevention, observed)
}

/// Verifies the container a live promotion's restore left under the universe's name -- bound to the attempt's own
/// launch instant -- and records the promotion, or removes a non-running container the attempt created and refuses.
#[allow(clippy::too_many_arguments)]
fn finish_live_promotion(
    db: &Connection,
    id: &str,
    uuid: &str,
    point: &str,
    dir: &Path,
    since: i64,
    generation: i64,
    exit_code: Option<i32>,
    seconds: f64,
    prevention: Value,
    observed: Option<Value>,
) -> Result<Value, Error> {
    let exit = ExitCode(exit_code);
    let log = dir.join("restore.log");
    let preserved = observed.as_ref().is_some_and(|c| mg::copy_podman_log(c, "RestoreLog", "restore.log", &log));
    let verified = verify_restored(None, observed.as_ref(), &log, preserved, since).and_then(|()| {
        let c = observed.as_ref().ok_or("absent")?;
        if c["Config"]["Labels"]["io.podmesh.universe"].as_str() != Some(uuid) {
            return Err("the restored container does not carry this universe's label".to_string());
        }
        let image: String = db.query_row("SELECT image_id FROM recovery_point_staged WHERE recovery_point_uuid=?1", [point], |r| r.get(0)).map_err(|e| e.to_string())?;
        if c["Image"].as_str().map(|i| i.trim_start_matches("sha256:")) != Some(image.as_str()) {
            return Err("the restored container's image is not the point's image".to_string());
        }
        if c["HostConfig"]["NetworkMode"].as_str() != Some("none") || c["Mounts"].as_array().map(|m| !m.is_empty()).unwrap_or(true) {
            return Err("the restored container is not network-disabled and mount-free".to_string());
        }
        Ok(())
    });
    if let Err(reason) = verified {
        let stderr = mg::read_bounded(&dir.join("promote.stderr")).unwrap_or_default();
        // A container this attempt created and that is not running is removed, so that the operator can try
        // again from a clean host; one that runs is never touched here and is named.
        let created_here = observed.as_ref().filter(|c| c["Created"].as_str().and_then(lc::epoch).is_some_and(|t| t >= since));
        let removed = match created_here {
            Some(c) if !lc::process_active(c) => {
                let cid = c["Id"].as_str().unwrap_or("").to_string();
                let out = lc::run_podman(lc::QUICK, &["rm", "--force", "--time", "0", &cid])?;
                json!({"container_id": cid, "removed": out.status.success()})
            }
            Some(c) => json!({"container_id": c["Id"], "removed": false, "reason": "the container is running; nothing running is removed by a failed verification"}),
            None => Value::Null,
        };
        let detail = json!({"reason": reason, "exit_code": exit.code(), "seconds": round3(seconds), "stderr_tail": mg::tail(&stderr),
            "observed": observed.as_ref().map(lc::state_view), "restore_log_preserved": preserved.then(|| log.display().to_string()),
            "prevention": prevention, "failed_container": removed, "artifact_directory": dir});
        mg::write_private(&dir.join("failure.json"), serde_json::to_string_pretty(&detail)?.as_bytes())?;
        // The attempt is closed: a retry of this operation runs every gate again, and a container left running
        // refuses it there by name.
        db.execute("DELETE FROM recovery_point_live_promote_attempts WHERE operation_id=?1", [id])?;
        return Err(failure(format!("The live promotion could not be verified: {reason}; nothing was promoted"), detail));
    }
    let c = observed.ok_or("Restored container not observable")?;
    let container_id = c["Id"].as_str().unwrap_or("").to_string();
    let log_sha256 = mg::sha256(&log)?;
    let now = crate::now() as i64;
    let tx = db.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO recovery_point_live_promotions VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![id, uuid, point, container_id, generation, log_sha256, now],
    )?;
    tx.execute("UPDATE recovery_point_staged SET promoted_operation_id=?2 WHERE recovery_point_uuid=?1", params![point, id])?;
    tx.execute("UPDATE recovery_point_live_promote_attempts SET state='promoted' WHERE operation_id=?1", [id])?;
    tx.commit()?;
    let mut view = live_promotion_view(db, id, false)?.ok_or("The promotion was recorded but cannot be read back")?;
    view["restore"] = json!({"seconds": round3(seconds), "statistics": print_stats(&dir.join("promote.stdout")), "prevention": prevention,
        "log": log, "runtime_git_id": mg::RUNTIME_GIT_ID});
    view["universe"] = lc::state_view(&c);
    Ok(view)
}

/// The images a restore imported for a universe -- the quarantined copy's own, or, for a promoted
/// universe, the copy it was promoted from -- removed when the universe is deleted and nothing else
/// uses them. Called from `delete`, beside the clone snapshots' own cleanup, for the same reason:
/// an image nobody can start any more is a disk cost with no owner, and on a standby it is one per
/// capture cycle. An image with any name outside the restore repository, or one a container still
/// uses, is retained and the reason is reported; nothing here forces a removal.
pub(crate) fn remove_restore_images(db: &Connection, uuid: &str) -> Result<(Vec<String>, Vec<Value>), Error> {
    ensure_schema(db)?;
    let mut ids: Vec<String> = {
        let mut s = db.prepare(
            "SELECT imported_image_id FROM recovery_point_restores WHERE restored_universe_uuid=?1
             UNION SELECT r.imported_image_id FROM recovery_point_promotions p
               JOIN recovery_point_restores r ON r.restored_universe_uuid=p.restored_universe_uuid
               WHERE p.universe_uuid=?1",
        )?;
        let rows = s.query_map([uuid], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        rows
    };
    ids.sort();
    ids.dedup();
    let (mut removed, mut retained) = (vec![], vec![]);
    if ids.is_empty() {
        return Ok((removed, retained));
    }
    let present = lc::images()?;
    for id in ids {
        let bare = id.trim_start_matches("sha256:").to_string();
        let Some(image) = present.iter().find(|i| lc::image_id(i) == bare) else { continue };
        let names: Vec<&str> = image["Names"].as_array().map(|n| n.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        if names.iter().any(|n| !n.starts_with(RESTORE_REPOSITORY)) {
            retained.push(json!({"image": bare, "reason": "image has names outside the PodMesh restore repository"}));
            continue;
        }
        match lc::run_podman(lc::QUICK, &["image", "rm", &bare]) {
            Ok(out) if out.status.success() => removed.push(bare),
            Ok(out) => retained.push(json!({"image": bare, "reason": String::from_utf8_lossy(&out.stderr).trim().to_string()})),
            Err(e) => retained.push(json!({"image": bare, "reason": e.to_string()})),
        }
    }
    Ok((removed, retained))
}
