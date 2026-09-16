//! Health, read-only: what a host carries and what each universe on it uses, measured where the
//! kernel keeps it. `host_status` reads /proc and the storage; `universe_stats` reads every
//! PodMesh universe's cgroup (memory.current, memory.max, cpu.max, cpu.stat sampled twice over a
//! short window for a CPU percentage) and Podman's size accounting. Nothing is changed.
use serde_json::{json, Value};
use std::process::Command;
use std::time::{Duration, Instant};

type Error = Box<dyn std::error::Error>;

const SAMPLE: Duration = Duration::from_millis(500);

fn read(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

pub fn host_status() -> Result<Value, Error> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb = |key: &str| -> Option<u64> {
        meminfo.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()).map(|v| v * 1024)
    };
    let load: Vec<f64> = std::fs::read_to_string("/proc/loadavg").unwrap_or_default().split_whitespace().take(3).filter_map(|v| v.parse().ok()).collect();
    let uptime = std::fs::read_to_string("/proc/uptime").ok().and_then(|u| u.split_whitespace().next().and_then(|v| v.parse::<f64>().ok())).map(|s| s as u64);
    let hostname = read(std::path::Path::new("/proc/sys/kernel/hostname"));
    let storage = crate::storage::status().unwrap_or_else(|e| json!({"error": e.to_string()}));
    Ok(json!({
        "hostname": hostname,
        "cpu_count": std::thread::available_parallelism().map(|n| n.get()).ok(),
        "load_average": {"1m": load.first(), "5m": load.get(1), "15m": load.get(2)},
        "memory_total_bytes": kb("MemTotal:"),
        "memory_available_bytes": kb("MemAvailable:"),
        "swap_total_bytes": kb("SwapTotal:"),
        "swap_free_bytes": kb("SwapFree:"),
        "uptime_seconds": uptime,
        "storage": {
            "backend": storage["backend"], "dedicated": storage["dedicated"], "growth": storage["growth"], "reason": storage["reason"],
            "size_bytes": storage["filesystem"]["size_bytes"], "used_bytes": storage["filesystem"]["used_bytes"], "available_bytes": storage["filesystem"]["available_bytes"],
        },
        "scope": "read from /proc and from what carries Podman's storage, now",
    }))
}

fn cgroup_dir(path: &str) -> std::path::PathBuf {
    std::path::Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/'))
}

fn usage_usec(dir: &std::path::Path) -> Option<u64> {
    read(&dir.join("cpu.stat"))?.lines().find_map(|l| l.strip_prefix("usage_usec ").and_then(|v| v.trim().parse().ok()))
}

pub fn universe_stats() -> Result<Value, Error> {
    let out = Command::new("podman").args(["ps", "-a", "--size", "--filter", "label=io.podmesh.universe", "--format", "json"]).output()?;
    if !out.status.success() {
        return Err(format!("podman ps failed: {}", String::from_utf8_lossy(&out.stderr).trim()).into());
    }
    let listed: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    // One inspect per universe for the cgroup path and the recorded limits.
    let mut rows = Vec::new();
    for c in &listed {
        let name = c["Names"][0].as_str().unwrap_or_default().to_string();
        let Some(ins) = crate::lifecycle::inspect(&name)? else { continue };
        let running = ins["State"]["Running"] == json!(true);
        let cg = ins["State"]["CgroupPath"].as_str().filter(|p| running && !p.is_empty()).map(cgroup_dir);
        rows.push((c.clone(), ins, cg));
    }
    // CPU sampled over one window for every running universe at once.
    let first: Vec<Option<u64>> = rows.iter().map(|(_, _, cg)| cg.as_ref().and_then(|d| usage_usec(d))).collect();
    let t0 = Instant::now();
    if first.iter().any(Option::is_some) {
        std::thread::sleep(SAMPLE);
    }
    let elapsed = t0.elapsed().as_micros().max(1) as f64;
    let mut universes = Vec::new();
    for ((c, ins, cg), before) in rows.iter().zip(first) {
        let labels = &ins["Config"]["Labels"];
        let (cpu_percent, memory_current, memory_max, cpus_allowed, pids) = match cg {
            Some(dir) => {
                let after = usage_usec(dir);
                let pct = match (before, after) { (Some(b), Some(a)) if a >= b => Some(((a - b) as f64 / elapsed * 1000.0).round() / 10.0), _ => None };
                let mem_max = read(&dir.join("memory.max"));
                let cpu_max = read(&dir.join("cpu.max")).and_then(|m| {
                    let mut it = m.split_whitespace();
                    match (it.next(), it.next().and_then(|p| p.parse::<f64>().ok())) {
                        (Some(q), Some(p)) if q != "max" => q.parse::<f64>().ok().map(|q| (q / p * 100.0).round() / 100.0),
                        _ => None,
                    }
                });
                (pct, read(&dir.join("memory.current")).and_then(|v| v.parse::<u64>().ok()),
                 mem_max.as_deref().and_then(|v| v.parse::<u64>().ok()), cpu_max, read(&dir.join("pids.current")).and_then(|v| v.parse::<u64>().ok()))
            }
            None => (None, None, None, None, None),
        };
        universes.push(json!({
            "universe_uuid": labels["io.podmesh.universe"],
            "name": c["Names"][0],
            "state": ins["State"]["Status"],
            "image": ins["ImageName"],
            "network_profile": labels[crate::network::LABEL_PROFILE],
            "address": labels["io.podmesh.universe-ip"],
            "cpu_percent_of_one_core": cpu_percent,
            "cpus_allowed": cpus_allowed,
            "memory_current_bytes": memory_current,
            "memory_max_bytes": memory_max,
            "memory_unlimited": cg.is_some() && memory_max.is_none(),
            "pids": pids,
            "disk_written_bytes": c["Size"]["rwSize"],
            "disk_rootfs_bytes": c["Size"]["rootFsSize"],
        }));
    }
    Ok(json!({
        "universes": universes,
        "sample_ms": if universes.iter().any(|u| !u["cpu_percent_of_one_core"].is_null()) { json!(SAMPLE.as_millis()) } else { Value::Null },
        "scope": "every PodMesh universe on this host; CPU as a percentage of one core over the sample window, memory and limits from its cgroup, disk from Podman's size accounting (written layer and root filesystem); a stopped universe has no cgroup and reports only its disk",
    }))
}
