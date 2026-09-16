//! What carries Podman's storage on this host, and whether a universe's space could grow there.
//!
//! The operator's rule (2026-09-16): a universe may be given more space only when the storage sits
//! on a dedicated volume that knows how to grow -- an LVM logical volume (thin or not), a ZFS
//! dataset or a Btrfs filesystem -- and never when it shares the system's ext4 (or any other)
//! root: a partition shared with the system is not grown. This operation reads the facts and
//! applies that rule; it changes nothing.
use serde_json::{json, Value};
use std::process::Command;

type Error = Box<dyn std::error::Error>;

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() { return None; }
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
    if !(source.starts_with("/dev/mapper/") || source.starts_with("/dev/dm-")) { return None; }
    let text = run("lvs", &["--noheadings", "--separator", "|", "-o", "vg_name,lv_name,lv_layout,pool_lv,lv_size,data_percent", source])?;
    let cols: Vec<&str> = text.split('|').map(str::trim).collect();
    if cols.len() < 4 { return None; }
    Some(json!({"volume_group": cols[0], "logical_volume": cols[1], "layout": cols[2], "thin_pool": if cols[3].is_empty() { Value::Null } else { json!(cols[3]) },
                "size": cols.get(4), "data_percent": cols.get(5)}))
}

pub fn status() -> Result<Value, Error> {
    let info = run("podman", &["info", "--format", "json"]).ok_or("podman info failed")?;
    let info: Value = serde_json::from_str(&info)?;
    let graph_root = info["store"]["graphRoot"].as_str().unwrap_or("/var/lib/containers/storage").to_string();
    let driver = info["store"]["graphDriverName"].clone();
    let volume_path = info["store"]["volumePath"].clone();
    let mount = mount_of(&graph_root);
    let (source, fstype, target) = match &mount {
        Some(m) => (m["source"].as_str().unwrap_or("").to_string(), m["fstype"].as_str().unwrap_or("").to_string(), m["target"].as_str().unwrap_or("").to_string()),
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
    // Dedicated: the mount that carries the graph root is not the system's root filesystem.
    let dedicated = !target.is_empty() && target != "/";
    let grows = matches!(backend, "zfs" | "btrfs" | "lvm-thin" | "lvm");
    let (growth, reason) = if !dedicated {
        ("refused", format!("Podman's storage shares the system's {} root filesystem ({}); a partition shared with the system is not grown", if fstype.is_empty() { "unknown".into() } else { fstype.clone() }, if source.is_empty() { "unknown source".into() } else { source.clone() }))
    } else if grows {
        ("possible", format!("a dedicated {backend} volume carries Podman's storage; a universe's space can grow there"))
    } else {
        ("refused", format!("a dedicated {fstype} filesystem carries Podman's storage, but it is not on a volume that knows how to grow (LVM, ZFS or Btrfs)"))
    };
    let sizes = df(&graph_root).map(|(size, used, avail)| json!({"size_bytes": size, "used_bytes": used, "available_bytes": avail}));
    Ok(json!({
        "graph_root": graph_root,
        "graph_driver": driver,
        "volume_path": volume_path,
        "mount": mount,
        "backend": backend,
        "dedicated": dedicated,
        "lvm": lvm,
        "growth": growth,
        "reason": reason,
        "filesystem": sizes,
        "universe_volumes": "not declared in this version: a universe keeps its data in its container's layer; volume_declare and volume_grow come with the dedicated storage",
        "scope": "read from Podman, findmnt, df and lvs now; nothing is changed by this operation",
    }))
}
