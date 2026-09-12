//! On-demand observations of a container and its default rootful nested Podman store.
use crate::lifecycle::Error;
use serde_json::{json, Value};
use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
};

fn path(request: &Value) -> Result<Vec<&str>, Error> {
    let values = request["container_path"]
        .as_array()
        .ok_or("container_path must be an array")?;
    if values.is_empty() || values.len() > 4 {
        return Err("Container path depth must be 1..4".into());
    }
    values
        .iter()
        .map(|v| {
            let id = v.as_str().ok_or("Invalid container ID")?;
            if id.len() != 64
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err("Full hexadecimal container IDs required".into());
            }
            Ok(id)
        })
        .collect()
}
fn query(parents: &[&str], tail: &[&str]) -> Result<Value, Error> {
    let mut args = Vec::new();
    for id in parents {
        args.extend(["exec", "--user", "0", "--", *id, "podman"]);
    }
    args.extend_from_slice(tail);
    let mut child = Command::new("/usr/bin/timeout")
        .args(["--signal=TERM", "--kill-after=2", "10", "/usr/bin/podman"])
        .args(&args)
        .env_remove("INVOCATION_ID")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("Observation pipe unavailable")?;
    let reader = thread::spawn(move || {
        let mut data = Vec::new();
        stdout
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut data)
            .map(|_| data)
    });
    let status = child.wait()?;
    let bytes = reader.join().map_err(|_| "Observation reader failed")??;
    if !status.success() {
        return Err(
            "Podman observation unavailable or timed out; no workload start requested".into(),
        );
    }
    let text = String::from_utf8(bytes)?;
    if text.len() > 4 * 1024 * 1024 {
        return Err("Observation exceeds size limit".into());
    }
    Ok(serde_json::from_str(&text)?)
}
fn observed(result: Result<Value, Error>) -> Value {
    match result {
        Ok(data) => json!({"status":"observed","data":data}),
        Err(e) => json!({"status":"unavailable","reason":e.to_string()}),
    }
}
fn normalize_metrics(raw: Value) -> Result<Value, Error> {
    let row = raw
        .as_array()
        .and_then(|r| r.first())
        .ok_or("No metric sample returned")?;
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| row.get(*k))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let data = json!({"cpu_percent":pick(&["cpu_percent","CPU","CPUPerc"]),"mem_usage":pick(&["mem_usage","MemUsage"]),"net_io":pick(&["net_io","NetIO"]),"block_io":pick(&["block_io","BlockIO"])});
    if data.as_object().unwrap().values().all(Value::is_null) {
        return Err("Podman metric schema is unsupported".into());
    }
    Ok(json!([data]))
}
pub fn inspect(request: &Value) -> Result<Value, Error> {
    let ids = path(request)?;
    let id = ids[ids.len() - 1];
    let parents = &ids[..ids.len() - 1];
    let raw = query(parents, &["container", "inspect", id])?;
    let c = raw.get(0).ok_or("Container not found")?;
    if c["Id"].as_str() != Some(id) {
        return Err("Observed container identity mismatch".into());
    }
    // Deliberately do not expose Env, command arguments, labels or secret mounts.
    let h = &c["HostConfig"];
    let configuration = json!({"hostname":c["Config"]["Hostname"],"image":c["ImageName"],"user":c["Config"]["User"],
        "memory_limit_bytes":h["Memory"],"memory_swap_limit_bytes":h["MemorySwap"],"cpu_quota":h["CpuQuota"],"cpu_period":h["CpuPeriod"],
        "cpu_shares":h["CpuShares"],"cpuset_cpus":h["CpusetCpus"],"network_mode":h["NetworkMode"],"privileged":h["Privileged"],
        "restart_policy":h["RestartPolicy"],"mount_count":c["Mounts"].as_array().map(|a|a.len())});
    let sizes = query(parents, &["container", "inspect", "--size", id]).ok();
    let size = sizes.as_ref().and_then(|v| v.get(0));
    let running = c["State"]["Running"].as_bool() == Some(true);
    let metrics = if running {
        observed(
            query(parents, &["stats", "--no-stream", "--format", "json", id])
                .and_then(normalize_metrics),
        )
    } else {
        json!({"status":"unavailable","reason":"Container is not running; live metrics are not sampled"})
    };
    let children = if running {
        observed(query(&ids,&["ps","--all","--format","json"]).and_then(|v|{let rows=v.as_array().ok_or("Invalid nested inventory")?;Ok(Value::Array(rows.iter().map(|r|json!({"Id":r["Id"],"Names":r["Names"],"Image":r["Image"],"State":r["State"],"shaper_brick":r["Labels"]["org.shaper.brick"]})).collect()))}))
    } else {
        json!({"status":"unavailable","reason":"Container is stopped. Nested Podman cannot be queried without starting it; no start was requested."})
    };
    let shaper = children["data"]
        .as_array()
        .map(|rows| rows.iter().any(|r| r["shaper_brick"].as_str().is_some()))
        .unwrap_or(false);
    let kind = if shaper {
        "shaper_universe"
    } else if children["status"] == "observed" {
        "nested_universe"
    } else {
        "container"
    };
    Ok(
        json!({"presentation":kind,"observation_source":if parents.is_empty(){"Host Podman"}else{"Podman inside the parent container; not host-attested"},"classification_basis":if shaper {"Observed org.shaper.brick labels in nested inventory; not a certification"} else {"Observed default rootful nested Podman availability"},"container_path":ids,"id":id,"name":c["Name"],"state":c["State"]["Status"],"configuration":configuration,
        "storage":{"writable_layer_bytes":size.map(|v|&v["SizeRw"]),"rootfs_bytes":size.map(|v|&v["SizeRootFs"]),"available_bytes":null,"note":"Layer sizes exclude external volumes. Available filesystem space is not reported."},
        "metrics":metrics,"children":children,"observed_at":crate::now(),"scope":"Default rootful Podman store inside each running parent; rootless stores are not enumerated"}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unbounded_or_ambiguous_paths() {
        for p in [json!([]), json!(["--latest"]), json!(["abc"])] {
            assert!(path(&json!({"container_path":p})).is_err());
        }
    }
    #[test]
    fn metrics_preserve_zero_and_refuse_unknown_schema() {
        assert_eq!(
            normalize_metrics(json!([{"cpu_percent":0,"mem_usage":"1B / 2B"}])).unwrap()[0]
                ["cpu_percent"],
            0
        );
        assert!(normalize_metrics(json!([{"future":"schema"}])).is_err());
    }
    #[test]
    fn full_ids_and_depth() {
        let id = "a".repeat(64);
        assert!(path(&json!({"container_path":[id]})).is_ok());
        assert!(path(&json!({"container_path":vec!["a".repeat(64);5]})).is_err());
    }
}
