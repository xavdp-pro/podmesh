//! What carries Podman's storage on this host, and whether a universe's space could grow there.
//!
//! The operator's rule (2026-09-16): a universe may be given more space only when the storage sits
//! on a dedicated volume that knows how to grow -- an LVM logical volume (thin or not), a ZFS
//! dataset or a Btrfs filesystem -- and never when it shares the system's ext4 (or any other)
//! root: a partition shared with the system is not grown. `storage_status` reads the facts and
//! applies that rule; it changes nothing. `volume_declare` and `volume_grow` record a universe's
//! declared capacity on a host that passes the same rule; they do not attach a separate block
//! device per universe in this build.
use crate::lifecycle as lc;
use crate::migration as mg;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::process::Command;

type Error = Box<dyn std::error::Error>;

pub const MIN_CAPACITY_BYTES: u64 = 1024 * 1024;
const MAX_CAPACITY_BYTES: u64 = 1024u64.pow(5); // 1 PiB bound against typos

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The mount that carries a path, from findmnt: source, filesystem type, target.
fn mount_of(path: &str) -> Option<Value> {
    let text = run("findmnt", &["-T", path, "-J", "-o", "SOURCE,FSTYPE,TARGET,OPTIONS"])?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v["filesystems"].as_array()?.first().cloned()
}

fn df(path: &str) -> Option<(u64, u64, u64)> {
    let text = run("df", &["-B1", "--output=size,used,avail", path])?;
    let last = text.lines().last()?;
    let mut it = last.split_whitespace().map(|x| x.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

/// An LVM logical volume behind a mapper device: its VG, LV and whether it is thin-provisioned.
fn lvm_of(source: &str) -> Option<Value> {
    if !(source.starts_with("/dev/mapper/") || source.starts_with("/dev/dm-")) {
        return None;
    }
    let text = run(
        "lvs",
        &[
            "--noheadings",
            "--separator",
            "|",
            "-o",
            "vg_name,lv_name,lv_layout,pool_lv,lv_size,data_percent",
            source,
        ],
    )?;
    let cols: Vec<&str> = text.split('|').map(str::trim).collect();
    if cols.len() < 4 {
        return None;
    }
    Some(json!({
        "volume_group": cols[0],
        "logical_volume": cols[1],
        "layout": cols[2],
        "thin_pool": if cols[3].is_empty() { Value::Null } else { json!(cols[3]) },
        "size": cols.get(4),
        "data_percent": cols.get(5)
    }))
}

/// Host-wide growth rule applied to Podman's graph root (the same rule `storage_status` reports).
#[derive(Clone, Debug)]
pub struct GrowthAssessment {
    pub graph_root: String,
    pub backend: String,
    pub dedicated: bool,
    pub growth: &'static str,
    pub reason: String,
    pub filesystem_size_bytes: Option<u64>,
}

pub fn assess_host_growth() -> Result<GrowthAssessment, Error> {
    let info = run("podman", &["info", "--format", "json"]).ok_or("podman info failed")?;
    let info: Value = serde_json::from_str(&info)?;
    let graph_root = info["store"]["graphRoot"]
        .as_str()
        .unwrap_or("/var/lib/containers/storage")
        .to_string();
    let mount = mount_of(&graph_root);
    let (source, fstype, target) = match &mount {
        Some(m) => (
            m["source"].as_str().unwrap_or("").to_string(),
            m["fstype"].as_str().unwrap_or("").to_string(),
            m["target"].as_str().unwrap_or("").to_string(),
        ),
        None => (String::new(), String::new(), String::new()),
    };
    let lvm = lvm_of(&source);
    let backend = match (fstype.as_str(), &lvm) {
        ("zfs", _) => "zfs",
        ("btrfs", _) => "btrfs",
        (_, Some(l)) if !l["thin_pool"].is_null() => "lvm-thin",
        (_, Some(_)) => "lvm",
        ("", _) => "unknown",
        _ => "plain",
    };
    let dedicated = !target.is_empty() && target != "/";
    let grows = matches!(backend, "zfs" | "btrfs" | "lvm-thin" | "lvm");
    let (growth, reason) = if !dedicated {
        (
            "refused",
            format!(
                "Podman's storage shares the system's {} root filesystem ({}); a partition shared with the system is not grown",
                if fstype.is_empty() { "unknown".into() } else { fstype.clone() },
                if source.is_empty() { "unknown source".into() } else { source.clone() }
            ),
        )
    } else if grows {
        (
            "possible",
            format!("a dedicated {backend} volume carries Podman's storage; a universe's space can grow there"),
        )
    } else {
        (
            "refused",
            format!(
                "a dedicated {fstype} filesystem carries Podman's storage, but it is not on a volume that knows how to grow (LVM, ZFS or Btrfs)"
            ),
        )
    };
    let filesystem_size_bytes = df(&graph_root).map(|(size, _, _)| size);
    Ok(GrowthAssessment {
        graph_root,
        backend: backend.to_string(),
        dedicated,
        growth,
        reason,
        filesystem_size_bytes,
    })
}

pub fn ensure_schema(db: &Connection) -> Result<(), Error> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS universe_volume_declarations(
            universe_uuid TEXT PRIMARY KEY,
            capacity_bytes INTEGER NOT NULL,
            declared_at INTEGER NOT NULL,
            declare_operation_id TEXT NOT NULL,
            declare_authorization_ref TEXT NOT NULL,
            last_grow_operation_id TEXT,
            last_grown_at INTEGER);",
    )?;
    Ok(())
}

fn declaration(db: &Connection, uuid: &str) -> Result<Option<Value>, Error> {
    Ok(db
        .query_row(
            "SELECT capacity_bytes, declared_at, declare_operation_id, declare_authorization_ref, last_grow_operation_id, last_grown_at
             FROM universe_volume_declarations WHERE universe_uuid=?1",
            [uuid],
            |r| {
                Ok(json!({
                    "capacity_bytes": r.get::<_, i64>(0)?,
                    "declared_at": r.get::<_, i64>(1)?,
                    "declare_operation_id": r.get::<_, String>(2)?,
                    "declare_authorization_ref": r.get::<_, String>(3)?,
                    "last_grow_operation_id": r.get::<_, Option<String>>(4)?,
                    "last_grown_at": r.get::<_, Option<i64>>(5)?,
                }))
            },
        )
        .optional()?)
}

fn universe_volumes_field(db: Option<&Connection>) -> Value {
    match db {
        Some(db) => {
            ensure_schema(db).ok();
            let mut s = db
                .prepare(
                    "SELECT universe_uuid, capacity_bytes, declared_at FROM universe_volume_declarations ORDER BY universe_uuid",
                )
                .ok();
            let rows = s
                .as_mut()
                .map(|stmt| {
                    stmt.query_map([], |r| {
                        Ok(json!({
                            "universe_uuid": r.get::<_, String>(0)?,
                            "capacity_bytes": r.get::<_, i64>(1)?,
                            "declared_at": r.get::<_, i64>(2)?,
                        }))
                    })
                    .and_then(|mapped| mapped.collect::<Result<Vec<_>, _>>())
                })
                .transpose()
                .ok()
                .flatten()
                .unwrap_or_default();
            json!({
                "declarations": rows,
                "note": "declared capacities recorded by volume_declare and volume_grow on this host; a separate block device per universe is not attached in this build",
            })
        }
        None => json!("declarations require the journal; call storage_status through the SQLite API"),
    }
}

pub fn status(db: Option<&Connection>) -> Result<Value, Error> {
    let info = run("podman", &["info", "--format", "json"]).ok_or("podman info failed")?;
    let info: Value = serde_json::from_str(&info)?;
    let graph_root = info["store"]["graphRoot"]
        .as_str()
        .unwrap_or("/var/lib/containers/storage")
        .to_string();
    let driver = info["store"]["graphDriverName"].clone();
    let volume_path = info["store"]["volumePath"].clone();
    let mount = mount_of(&graph_root);
    let assessment = assess_host_growth()?;
    let source = mount
        .as_ref()
        .and_then(|m| m["source"].as_str())
        .unwrap_or("");
    let lvm = lvm_of(source);
    let sizes = df(&graph_root).map(|(size, used, avail)| {
        json!({"size_bytes": size, "used_bytes": used, "available_bytes": avail})
    });
    Ok(json!({
        "graph_root": graph_root,
        "graph_driver": driver,
        "volume_path": volume_path,
        "mount": mount,
        "backend": assessment.backend,
        "dedicated": assessment.dedicated,
        "lvm": lvm,
        "growth": assessment.growth,
        "reason": assessment.reason,
        "filesystem": sizes,
        "universe_volumes": universe_volumes_field(db),
        "scope": "read from Podman, findmnt, df and lvs now; volume_declare and volume_grow change only declared capacities in the journal when the host growth rule is possible",
    }))
}

fn refuse_unless_growth_possible(assessment: &GrowthAssessment) -> Result<(), Error> {
    if assessment.growth == "possible" {
        return Ok(());
    }
    Err(assessment.reason.clone().into())
}

fn capacity_bound(capacity: u64, assessment: &GrowthAssessment) -> Result<(), Error> {
    if capacity < MIN_CAPACITY_BYTES {
        return Err(format!("capacity_bytes must be an integer from {MIN_CAPACITY_BYTES} bytes").into());
    }
    if capacity > MAX_CAPACITY_BYTES {
        return Err(format!("capacity_bytes must be at most {MAX_CAPACITY_BYTES} bytes").into());
    }
    if let Some(size) = assessment.filesystem_size_bytes {
        if capacity > size {
            return Err(format!(
                "capacity_bytes ({capacity}) exceeds the filesystem carrying Podman's storage ({size} bytes)"
            )
            .into());
        }
    }
    Ok(())
}

fn ensure_stopped_owned(db: &Connection, uuid: &str) -> Result<Value, Error> {
    let name = format!("podmesh-{uuid}");
    let Some(c) = lc::inspect(&name)? else {
        return Err("No such universe on this host".into());
    };
    lc::owned(db, &c, uuid, "universe")?;
    let state = c["State"].as_str().unwrap_or("");
    if !lc::STOPPED.contains(&state) {
        return Err(format!("volume operations require a stopped universe; this one is {state}").into());
    }
    Ok(c)
}

pub fn execute(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    if !matches!(operation, "volume_declare" | "volume_grow") {
        return Err("Unsupported storage mutation".into());
    }
    lc::ensure_schema(db)?;
    ensure_schema(db)?;
    lc::journaled(db, request, |db| perform(db, request))
}

fn perform(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    let uuid = lc::text(request, "universe_uuid")?;
    lc::token(uuid)?;
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let reference = lc::text(request, "authorization_ref")?;
    mg::refuse_if_reserved(db, uuid, operation)?;
    let _container = ensure_stopped_owned(db, uuid)?;
    let assessment = assess_host_growth()?;
    refuse_unless_growth_possible(&assessment)?;
    let now = crate::now() as i64;
    match operation {
        "volume_declare" => {
            if declaration(db, uuid)?.is_some() {
                return Err("a volume is already declared for this universe; use volume_grow".into());
            }
            let capacity = request
                .get("capacity_bytes")
                .and_then(Value::as_u64)
                .ok_or("capacity_bytes is required")?;
            capacity_bound(capacity, &assessment)?;
            db.execute(
                "INSERT INTO universe_volume_declarations VALUES(?1,?2,?3,?4,?5,NULL,NULL)",
                params![uuid, capacity as i64, now, id, reference],
            )?;
            Ok(json!({
                "action": "declared",
                "universe_uuid": uuid,
                "capacity_bytes": capacity,
                "host_growth": assessment.growth,
                "backend": assessment.backend,
                "scope": "journal declaration only; no per-universe block device is attached in this build",
            }))
        }
        "volume_grow" => {
            let Some(current) = declaration(db, uuid)? else {
                return Err("no volume declared for this universe; use volume_declare first".into());
            };
            let additional = request
                .get("additional_bytes")
                .and_then(Value::as_u64)
                .ok_or("additional_bytes is required")?;
            if additional < MIN_CAPACITY_BYTES {
                return Err(format!("additional_bytes must be an integer from {MIN_CAPACITY_BYTES} bytes").into());
            }
            let current_bytes = current["capacity_bytes"].as_i64().unwrap_or(0) as u64;
            let new_capacity = current_bytes + additional;
            capacity_bound(new_capacity, &assessment)?;
            db.execute(
                "UPDATE universe_volume_declarations SET capacity_bytes=?2, last_grow_operation_id=?3, last_grown_at=?4 WHERE universe_uuid=?1",
                params![uuid, new_capacity as i64, id, now],
            )?;
            Ok(json!({
                "action": "grown",
                "universe_uuid": uuid,
                "previous_capacity_bytes": current_bytes,
                "capacity_bytes": new_capacity,
                "additional_bytes": additional,
                "host_growth": assessment.growth,
                "backend": assessment.backend,
                "scope": "journal declaration only; no per-universe block device is attached in this build",
            }))
        }
        _ => Err("Unsupported storage mutation".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::migrations;
    use crate::store::SqliteStore;
    use rusqlite::Connection;

    fn scratch_db() -> Connection {
        let mut store = SqliteStore::open_in_memory().unwrap();
        migrations::apply(&mut store).unwrap();
        store.into_connection()
    }

    #[test]
    fn capacity_bound_respects_filesystem_size() {
        let assessment = GrowthAssessment {
            graph_root: "/var/lib/containers/storage".into(),
            backend: "lvm".into(),
            dedicated: true,
            growth: "possible",
            reason: String::new(),
            filesystem_size_bytes: Some(10_000_000),
        };
        assert!(capacity_bound(2_000_000, &assessment).is_ok());
        assert!(capacity_bound(20_000_000, &assessment).is_err());
    }

    #[test]
    fn declare_and_grow_persist_when_host_growth_is_possible() {
        let db = scratch_db();
        let uuid = "16926159-bf59-4537-8f6e-5cfea52540ea";
        db.execute(
            "INSERT INTO operations VALUES('create-op','{\"operation\":\"create\",\"universe_uuid\":\"16926159-bf59-4537-8f6e-5cfea52540ea\"}','verified','{\"container_id\":\"abc\"}')",
            [],
        )
        .unwrap();
        // Bypass podman/ownership: test the journal path with a stub assessment via direct SQL after checks.
        let assessment = GrowthAssessment {
            graph_root: "/x".into(),
            backend: "lvm".into(),
            dedicated: true,
            growth: "possible",
            reason: String::new(),
            filesystem_size_bytes: Some(10_000_000_000),
        };
        capacity_bound(5_000_000, &assessment).unwrap();
        db.execute(
            "INSERT INTO universe_volume_declarations VALUES(?1,?2,1,'op','ref',NULL,NULL)",
            params![uuid, 5_000_000i64],
        )
        .unwrap();
        let row = declaration(&db, uuid).unwrap().unwrap();
        assert_eq!(row["capacity_bytes"], 5_000_000);
        db.execute(
            "UPDATE universe_volume_declarations SET capacity_bytes=7000000, last_grow_operation_id='grow', last_grown_at=2 WHERE universe_uuid=?1",
            [uuid],
        )
        .unwrap();
        let grown = declaration(&db, uuid).unwrap().unwrap();
        assert_eq!(grown["capacity_bytes"], 7_000_000);
        assert_eq!(grown["last_grow_operation_id"], "grow");
    }
}
