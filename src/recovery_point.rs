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
use crate::lifecycle as lc;
use crate::migration as mg;
use crate::transfer as tr;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::Path;

type Error = Box<dyn std::error::Error>;

pub const FORMAT: &str = "podmesh-recovery-point/0-unsigned-unencrypted";
pub const MANIFEST: &str = "recovery-point-manifest.json";
pub const ROOTFS: &str = "rootfs.tar";
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
            promoted_at INTEGER NOT NULL);",
    )?;
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
                "lease_generation": r.get::<_, i64>(4)?, "started": false, "network": "none",
                "replayed": replayed, "scope": PROMOTION_SCOPE,
            })),
        )
        .optional()?)
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
        let points: Vec<Value> = {
            let mut s = db.prepare(
                "SELECT recovery_point_uuid,generation,parent_recovery_point_uuid,state,manifest_sha256,rootfs_sha256,rootfs_bytes,prepared_at,outbox
                 FROM recovery_points WHERE universe_uuid=?1 ORDER BY generation",
            )?;
            let rows: Vec<Value> = s
                .query_map([uuid], |r| {
                    Ok(json!({
                        "recovery_point_uuid": r.get::<_, String>(0)?, "generation": r.get::<_, i64>(1)?,
                        "parent_recovery_point_uuid": r.get::<_, Option<String>>(2)?, "state": r.get::<_, String>(3)?,
                        "manifest_sha256": r.get::<_, String>(4)?, "rootfs_sha256": r.get::<_, String>(5)?,
                        "rootfs_bytes": r.get::<_, i64>(6)?, "prepared_at": r.get::<_, i64>(7)?, "outbox": r.get::<_, String>(8)?,
                    }))
                })?
                .collect::<Result<_, _>>()?;
            rows
        };
        return Ok(json!({"universe_uuid": uuid, "recovery_points": points,
            "note": "every point here is prepared and unsigned; none is sealed, because this build can produce no signature"}));
    }
    if operation == "recovery_point_restore" {
        return restore(db, request, uuid);
    }
    if operation == "recovery_point_promote" {
        return promote(db, request, uuid);
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
            "kind": "rootfs-export",
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
        "INSERT INTO recovery_points VALUES(?1,?2,?3,?4,?5,'prepared',?6,?7,?8,?9,?10)",
        params![point, uuid, generation, parent, id, manifest_sha256, rootfs_sha256, rootfs_bytes as i64, now,
                outbox.to_string_lossy().to_string()],
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
    });
    let created = lc::execute(db, &create_request)?;
    let container_id = created_container(&created)?;
    db.execute(
        "INSERT INTO recovery_point_restores VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0,?9)",
        params![id, uuid, point, source, image, container_id, manifest_sha256, rootfs_sha256, crate::now() as i64],
    )?;
    restore_view(db, id, false)?.ok_or_else(|| "The restore was recorded but cannot be read back".into())
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
/// the image and command the quarantined copy was created from, with no network and not
/// started; starting it is the caller's, and goes through the same gate.
///
/// What the lease proves is written into every answer: this host's own restraint, in this
/// host's journal. It does not prove the previous holder is stopped. The quarantined copy is
/// left where it is; removing it is the collector's or the operator's, never a side effect.
fn promote(db: &Connection, request: &Value, uuid: &str) -> Result<Value, Error> {
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let reference = lc::text(request, "authorization_ref")?;
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
    let created = lc::execute(
        db,
        &json!({
            "operation": "create", "operation_id": create_id, "universe_uuid": uuid,
            "authorization_ref": reference, "image": image, "command": quarantined["command"],
        }),
    )?;
    let container_id = created_container(&created)?;
    db.execute(
        "INSERT INTO recovery_point_promotions VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![id, uuid, restored, point, container_id, generation, crate::now() as i64],
    )?;
    promotion_view(db, id, false)?.ok_or_else(|| "The promotion was recorded but cannot be read back".into())
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
