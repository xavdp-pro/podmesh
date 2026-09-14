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
            outbox TEXT NOT NULL);",
    )?;
    Ok(())
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
    let manifest_path = Path::new(&outbox).join(MANIFEST);
    let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
    Ok(Some(json!({"recovery_point_uuid": point, "outbox": outbox, "manifest": manifest, "replayed": true})))
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
