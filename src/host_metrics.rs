//! Bounded, read-only observations of resources available to the local PodMesh host.

use crate::{lifecycle, now};
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{Read, Take},
    path::Path,
    process::Command,
};

type Error = Box<dyn std::error::Error>;
const PROC_LIMIT: u64 = 1024 * 1024;

fn read_bounded(path: &Path) -> Result<String, Error> {
    let mut bytes = Vec::new();
    let mut input: Take<File> = File::open(path)?.take(PROC_LIMIT + 1);
    input.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > PROC_LIMIT {
        return Err("Kernel observation exceeded its size bound".into());
    }
    Ok(String::from_utf8(bytes)?)
}

fn parse_memory(input: &str) -> Result<(u64, u64), Error> {
    fn kib(input: &str, key: &str) -> Result<u64, Error> {
        let line = input
            .lines()
            .find(|line| line.starts_with(key))
            .ok_or_else(|| format!("{key} is absent"))?;
        let mut fields = line[key.len()..].split_whitespace();
        let value = fields
            .next()
            .ok_or("Memory value is absent")?
            .parse::<u64>()?;
        if fields.next() != Some("kB") || fields.next().is_some() {
            return Err("Unexpected memory unit or shape".into());
        }
        value
            .checked_mul(1024)
            .ok_or_else(|| "Memory value overflow".into())
    }
    let total = kib(input, "MemTotal:")?;
    let available = kib(input, "MemAvailable:")?;
    if available > total {
        return Err("Available memory exceeds total memory".into());
    }
    Ok((total, available))
}

fn parse_load(input: &str) -> Result<(f64, f64, f64), Error> {
    let values = input
        .split_whitespace()
        .take(3)
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()?;
    if values.len() != 3
        || values
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err("Invalid load average observation".into());
    }
    Ok((values[0], values[1], values[2]))
}

fn parse_space(input: &str) -> Result<(u64, u64), Error> {
    let mut rows = input.lines().map(str::trim).filter(|line| !line.is_empty());
    let _header = rows.next().ok_or("Filesystem observation has no header")?;
    let values = rows.next().ok_or("Filesystem observation has no value")?;
    if rows.next().is_some() {
        return Err("Filesystem observation returned multiple values".into());
    }
    let mut fields = values.split_whitespace();
    let total = fields
        .next()
        .ok_or("Filesystem total is absent")?
        .parse::<u64>()?;
    let available = fields
        .next()
        .ok_or("Filesystem availability is absent")?
        .parse::<u64>()?;
    if fields.next().is_some() || available > total {
        return Err("Invalid filesystem capacity observation".into());
    }
    Ok((total, available))
}

fn storage() -> Result<(u64, u64), Error> {
    // The graph root comes from one fixed, bounded, read-only Podman command. It is passed as one
    // argument to df and is never returned to API clients.
    let graph_root = lifecycle::podman(10, &["info", "--format", "{{.Store.GraphRoot}}"])?;
    let graph_root = graph_root.trim();
    if graph_root.is_empty() || !Path::new(graph_root).is_absolute() {
        return Err("Podman did not report an absolute graph root".into());
    }
    let output = Command::new("/usr/bin/timeout")
        .args([
            "--signal=TERM",
            "--kill-after=1",
            "5",
            "/usr/bin/df",
            "-B1",
            "--output=size,avail",
            "--",
            graph_root,
        ])
        .output()?;
    if !output.status.success() {
        return Err("Podman storage capacity observation failed".into());
    }
    parse_space(&String::from_utf8(output.stdout)?)
}

fn unknown(source: &str) -> Value {
    json!({"known":false,"source":source,"reason":"Observation unavailable"})
}

/// Returns independent observations. Failure of one source never turns another source into zero.
pub(crate) fn observe() -> Value {
    let observed_at = now();
    let memory = match read_bounded(Path::new("/proc/meminfo")).and_then(|v| parse_memory(&v)) {
        Ok((total, available)) => {
            json!({"known":true,"source":"procfs meminfo","total_bytes":total,"available_bytes":available})
        }
        Err(_) => unknown("procfs meminfo"),
    };
    let load = match read_bounded(Path::new("/proc/loadavg")).and_then(|v| parse_load(&v)) {
        Ok((one, five, fifteen)) => {
            json!({"known":true,"source":"procfs loadavg","load_1m":one,"load_5m":five,"load_15m":fifteen})
        }
        Err(_) => unknown("procfs loadavg"),
    };
    let cpu_count = match std::thread::available_parallelism() {
        Ok(value) => {
            json!({"known":true,"source":"process scheduler availability","logical_count":value.get()})
        }
        Err(_) => unknown("process scheduler availability"),
    };
    let storage = match storage() {
        Ok((total, available)) => {
            json!({"known":true,"source":"Podman info and filesystem statistics","name":"Podman graph-root filesystem","total_bytes":total,"available_bytes":available})
        }
        Err(_) => unknown("Podman info and filesystem statistics"),
    };
    json!({
        "observed_at": observed_at,
        "memory": memory,
        "cpu": {"count":cpu_count,"load":load},
        "storage": storage,
        "note": "Host-level observations. Unknown values are never represented as zero."
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_memory_as_bytes() {
        assert_eq!(
            parse_memory("MemTotal: 1024 kB\nMemAvailable: 256 kB\n").unwrap(),
            (1_048_576, 262_144)
        );
    }

    #[test]
    fn rejects_missing_or_unexpected_memory_units() {
        assert!(parse_memory("MemTotal: 1 MB\nMemAvailable: 1 kB\n").is_err());
        assert!(parse_memory("MemTotal: 1 kB\n").is_err());
        assert!(parse_memory("MemTotal: 1 kB\nMemAvailable: 2 kB\n").is_err());
    }

    #[test]
    fn parses_load_and_filesystem_bytes() {
        assert_eq!(
            parse_load("0.25 1.50 2.75 1/100 42\n").unwrap(),
            (0.25, 1.5, 2.75)
        );
        assert_eq!(
            parse_space("1B-blocks Avail\n1000 400\n").unwrap(),
            (1000, 400)
        );
        assert!(parse_space("header\n10 11\n").is_err());
    }

    #[test]
    fn unknown_observations_are_explicit_and_never_numeric_zero() {
        assert_eq!(
            unknown("fixture"),
            json!({"known":false,"source":"fixture","reason":"Observation unavailable"})
        );
    }
}
