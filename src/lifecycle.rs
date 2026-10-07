//! Local managed-container operations. No implicit image pulls or network access.
use crate::migration::{self, Binding};
use crate::store::{DurableStore, Row as StoreRow, Value as Stored};
use crate::{recovery, restore, transfer};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fmt, fs,
    net::Ipv4Addr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};
pub(crate) type Error = Box<dyn std::error::Error>;

const UNIVERSE: &str = "io.podmesh.universe";
const CREATION: &str = "io.podmesh.creation-operation";
const SNAPSHOT_FOR: &str = "io.podmesh.snapshot-for";
const SNAPSHOT_OPERATION: &str = "io.podmesh.snapshot-operation";
const SNAPSHOT_SOURCE: &str = "io.podmesh.snapshot-source-container";
const SNAPSHOT_REPOSITORY: &str = "localhost/podmesh-clone:";
const OPERATIONS: [&str; 19] = [
    "create",
    "delete",
    "clone",
    "start",
    "stop",
    "pause",
    "resume",
    "resources",
    "migration_preflight",
    "migration_checkpoint",
    "migration_authorize_transfer",
    "migration_complete_transfer",
    "migration_retire_source",
    "migration_release",
    "migration_abandon",
    "migration_restore_local",
    "migration_destination_preflight",
    "migration_restore",
    "migration_restore_abort",
];
// Podman states in which no container process can write the root filesystem.
pub(crate) const STOPPED: [&str; 3] = ["created", "exited", "stopped"];
// Seconds. The service handles one request at a time, so these also bound queueing.
pub(crate) const QUICK: u64 = 30;
const COMMIT: u64 = 300;
const DEFAULT_OBSERVE_SECONDS: u64 = 2;
const MAX_OBSERVE_SECONDS: u64 = 30;
const MAX_STOP_TIMEOUT_SECONDS: u64 = 300;
/// The smallest memory limit a universe may be given: below it, a container with any runtime in it
/// is killed at once, which is not a limit but a stop by other means.
const MIN_MEMORY_BYTES: u64 = 32 * 1024 * 1024;
/// The smallest CPU allowance, a tenth of a core; the largest is what the host has.
const MIN_CPUS: f64 = 0.1;
// Time allowed to Podman itself beyond the declared graceful stop period.
const STOP_MARGIN_SECONDS: u64 = 30;
const POLL: Duration = Duration::from_millis(200);

/// An operation failure carrying a structured, freshly observed state.
#[derive(Debug)]
pub struct Failure {
    message: String,
    pub details: Value,
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}
pub(crate) fn failure(message: impl Into<String>, details: Value) -> Error {
    Box::new(Failure {
        message: message.into(),
        details,
    })
}

/// Validated request parameters. Validation happens before an operation ID is reserved.
enum Params<'a> {
    Create {
        image: &'a str,
        command: Vec<&'a str>,
        universe_profile: &'a str,
        profile: &'a str,
        address: Option<&'a str>,
        /// Podman `--secret` arguments and the label naming them, from `secrets::mounts`.
        secrets: (Vec<String>, Option<String>),
        /// A manager replica's host state (V3-5): its name, for `manager::host_state_mounts`.
        host_state: Option<&'a str>,
    },
    Clone {
        source: &'a str,
    },
    Delete,
    Start {
        observe_seconds: u64,
    },
    Stop {
        timeout: u64,
        on_timeout: &'a str,
    },
    /// Freeze every process of a running universe (`podman pause`): its memory stays, nothing runs.
    Pause,
    /// Thaw a paused universe (`podman unpause`). It leaves a running writer behind, so it is
    /// gated exactly as `start` is.
    Resume,
    /// Set the memory limit and the CPU allowance of a universe, applied to its live cgroup when
    /// it runs and kept for its next start (`podman update`). At least one of the two.
    Resources {
        memory_bytes: Option<u64>,
        cpus: Option<f64>,
    },
    MigrationPreflight(Binding<'a>),
    MigrationCheckpoint(Binding<'a>),
    MigrationAuthorize {
        checkpoint: &'a str,
        destination: &'a str,
        authorization_ref: &'a str,
    },
    MigrationComplete {
        authorization: &'a str,
    },
    MigrationRetire {
        authorization: &'a str,
    },
    MigrationRelease {
        checkpoint: &'a str,
    },
    MigrationAbandon {
        checkpoint: &'a str,
    },
    MigrationRestoreLocal {
        checkpoint: &'a str,
    },
    DestinationPreflight {
        authorization: &'a str,
    },
    MigrationRestore {
        authorization: &'a str,
    },
    MigrationRestoreAbort {
        authorization: &'a str,
        reference: &'a str,
        reclaim_processes: bool,
    },
}
impl Params<'_> {
    fn name(&self) -> &'static str {
        match self {
            Params::Create { .. } => "create",
            Params::Clone { .. } => "clone",
            Params::Delete => "delete",
            Params::Start { .. } => "start",
            Params::Stop { .. } => "stop",
            Params::Pause => "pause",
            Params::Resume => "resume",
            Params::Resources { .. } => "resources",
            Params::MigrationPreflight(_) => "migration_preflight",
            Params::MigrationCheckpoint(_) => "migration_checkpoint",
            Params::MigrationAuthorize { .. } => "migration_authorize_transfer",
            Params::MigrationComplete { .. } => "migration_complete_transfer",
            Params::MigrationRetire { .. } => "migration_retire_source",
            Params::MigrationRelease { .. } => "migration_release",
            Params::MigrationAbandon { .. } => "migration_abandon",
            Params::MigrationRestoreLocal { .. } => "migration_restore_local",
            Params::DestinationPreflight { .. } => "migration_destination_preflight",
            Params::MigrationRestore { .. } => "migration_restore",
            Params::MigrationRestoreAbort { .. } => "migration_restore_abort",
        }
    }
}

pub(crate) fn text<'a>(r: &'a Value, key: &str) -> Result<&'a str, Error> {
    r.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("Missing {key}").into())
}
pub(crate) fn token(value: &str) -> Result<(), Error> {
    if value.len() > 80
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Identifier must contain 1-80 ASCII letters, digits or hyphens".into());
    }
    Ok(())
}

/// `universe_profile` on `create`: flat (default) or nested Rule 11 outer (privileged, isolated network only).
pub(crate) fn parse_universe_profile(
    request: &Value,
    network_profile: &str,
) -> Result<&'static str, Error> {
    let profile = match request.get("universe_profile") {
        None | Some(Value::Null) => crate::network::UNIVERSE_PROFILE_FLAT,
        Some(value) => value
            .as_str()
            .ok_or("universe_profile must be a string")?,
    };
    match profile {
        crate::network::UNIVERSE_PROFILE_FLAT => Ok(crate::network::UNIVERSE_PROFILE_FLAT),
        crate::network::UNIVERSE_PROFILE_NESTED => {
            if network_profile != crate::network::PROFILE_ISOLATED {
                return Err(
                    "universe_profile nested requires network_profile isolated (Rule 11 outer universe)"
                        .into(),
                );
            }
            Ok(crate::network::UNIVERSE_PROFILE_NESTED)
        }
        other => Err(format!("universe_profile must be flat or nested, not {other}").into()),
    }
}
pub(crate) fn is_uuid(v: &str) -> bool {
    v.len() == 36
        && v.chars().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

// Podman temporary files for this service. A killed commit leaves its layer copy behind,
// so PodMesh owns the directory and empties it at startup and before each commit.
static SCRATCH: OnceLock<PathBuf> = OnceLock::new();
pub fn prepare_scratch(dir: &Path) -> Result<(), Error> {
    SCRATCH.get_or_init(|| dir.to_path_buf());
    empty_scratch()
}
fn empty_scratch() -> Result<(), Error> {
    let dir = SCRATCH
        .get()
        .ok_or("Podman scratch directory not prepared")?;
    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
pub(crate) fn run_podman(timeout: u64, args: &[&str]) -> Result<Output, Error> {
    // GNU timeout bounds this process group; no shell evaluates caller input.
    let limit = timeout.to_string();
    let scratch = SCRATCH
        .get()
        .ok_or("Podman scratch directory not prepared")?;
    Ok(Command::new("/usr/bin/timeout")
        .env("TMPDIR", scratch)
        // systemd sets INVOCATION_ID for this service. Podman then leaves conmon in the
        // service cgroup, where stopping or restarting PodMesh kills it and orphans every
        // started universe. Without it, Podman places conmon in its own libpod-conmon scope.
        .env_remove("INVOCATION_ID")
        .args(["--signal=TERM", "--kill-after=5", &limit, "/usr/bin/podman"])
        .args(args)
        .output()?)
}
fn podman_error(timeout: u64, args: &[&str], out: &Output) -> String {
    match out.status.code() {
        // GNU timeout exits 124 after TERM and 137 after its KILL follow-up.
        Some(124) | Some(137) => format!(
            "Podman {} exceeded its {timeout} s bound and was terminated; the container state is re-observed, not assumed",
            args.first().unwrap_or(&"")
        ),
        _ => format!("Podman operation failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
    }
}
pub(crate) fn podman(timeout: u64, args: &[&str]) -> Result<String, Error> {
    let out = run_podman(timeout, args)?;
    if !out.status.success() {
        return Err(podman_error(timeout, args, &out).into());
    }
    Ok(String::from_utf8(out.stdout)?)
}
fn label<'a>(c: &'a Value, key: &str) -> Option<&'a str> {
    c["Config"]["Labels"][key].as_str()
}
pub(crate) fn inspect(name: &str) -> Result<Option<Value>, Error> {
    let all: Value = serde_json::from_str(&podman(QUICK, &["ps", "--all", "--format", "json"])?)?;
    let exists = all.as_array().ok_or("Invalid inventory")?.iter().any(|c| {
        c["Names"]
            .as_array()
            .map(|n| n.iter().any(|v| v.as_str() == Some(name)))
            .unwrap_or(false)
    });
    if !exists {
        return Ok(None);
    }
    let data: Value = serde_json::from_str(&podman(QUICK, &["container", "inspect", name])?)?;
    Ok(Some(data[0].clone()))
}
pub(crate) fn images() -> Result<Vec<Value>, Error> {
    let all: Value =
        serde_json::from_str(&podman(QUICK, &["images", "--all", "--format", "json"])?)?;
    Ok(all.as_array().ok_or("Invalid image inventory")?.clone())
}
pub(crate) fn image_id(i: &Value) -> &str {
    i["Id"].as_str().unwrap_or("").trim_start_matches("sha256:")
}
fn names(i: &Value) -> Vec<&str> {
    i["Names"]
        .as_array()
        .map(|n| n.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}
pub(crate) fn status(c: &Value) -> &str {
    c["State"]["Status"].as_str().unwrap_or("unknown")
}
fn stopped(c: &Value) -> Result<(), Error> {
    let status = status(c);
    if !STOPPED.contains(&status) {
        return Err(format!("Container state {status} is not stopped").into());
    }
    Ok(())
}
/// Seconds since the Unix epoch for Podman container timestamps
/// (`YYYY-MM-DDTHH:MM:SS[.frac]Z` or the same with a numeric zone suffix `±HH:MM`).
pub(crate) fn epoch(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |from: usize, to: usize| ts.get(from..to)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    // Days from the civil date (H. Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let mut secs = (era * 146097 + doe - 719468) * 86400 + hh * 3600 + mm * 60 + ss;
    if !ts.ends_with('Z') {
        let zone = ts.get(19..)?;
        let pos = zone.rfind('+').or_else(|| zone.rfind('-'))?;
        let sign = if zone.as_bytes()[pos] == b'+' { 1i64 } else { -1i64 };
        let off = zone.get(pos + 1..)?;
        if off.len() != 5 || off.as_bytes().get(2) != Some(&b':') {
            return None;
        }
        let (oh, om) = (off.get(0..2)?.parse::<i64>().ok()?, off.get(3..5)?.parse::<i64>().ok()?);
        secs -= sign * (oh * 3600 + om * 60);
    }
    Some(secs)
}
/// The observed container state, without claims about anything not observed.
pub(crate) fn state_view(c: &Value) -> Value {
    let s = &c["State"];
    let running = process_active(c);
    let mut view = json!({
        "observed_at": crate::now(),
        "container_id": c["Id"],
        "state": s["Status"],
        "running": running,
        "started_at": s["StartedAt"],
        "finished_at": if running { Value::Null } else { s["FinishedAt"].clone() },
        "exit_code": if running { Value::Null } else { s["ExitCode"].clone() },
    });
    if status(c) == "stopping" {
        view["note"] =
            json!("Podman reports stopping: an earlier stop did not complete; a recorded application process means it is still running");
    }
    view
}
/// Podman leaves a container in `stopping`, with `Running` false, when the process running
/// `podman stop` dies before the application exits. The recorded PID is still the application.
pub(crate) fn process_active(c: &Value) -> bool {
    match status(c) {
        "running" => true,
        "stopping" => c["State"]["Pid"].as_i64().unwrap_or(0) > 0,
        _ => false,
    }
}
pub(crate) fn observe(uuid: &str) -> Result<Value, Error> {
    Ok(match inspect(&format!("podmesh-{uuid}"))? {
        None => json!({"observed_at": crate::now(), "present": false}),
        Some(c) => {
            let mut view = state_view(&c);
            view["present"] = json!(true);
            view["managed_label"] = json!(label(&c, UNIVERSE) == Some(uuid));
            view["network"] = crate::network::of_container(&c);
            view
        }
    })
}
/// A target must be a universe this host's journal recorded as created, cloned or restored from a verified
/// migration handoff, still carried by the same container. A matching label alone is not enough.
pub(crate) fn owned(db: &Connection, c: &Value, uuid: &str, role: &str) -> Result<(), Error> {
    // A restored container keeps the source journal's creation label, which this journal does not know: its
    // ownership is the verified migration_restore that binds this universe to this container ID. A local
    // restore from a preserved archive produces a new container ID the same way.
    if let Some(id) = c["Id"].as_str() {
        if restore::restored_here(db, uuid, id)?
            || recovery::restored_locally(db, uuid, id)?
            || crate::recovery_point::promoted_live_here(db, uuid, id)?
        {
            return Ok(());
        }
    }
    let operation =
        label(c, CREATION).ok_or_else(|| format!("{role} has no creation operation"))?;
    let row: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT request,result FROM operations WHERE id=?1 AND status='verified'",
            [operation],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (request, result) = row.ok_or_else(|| {
        format!("{role} is not recorded as a verified PodMesh universe on this host")
    })?;
    let request: Value = serde_json::from_str(&request)?;
    let result: Value = serde_json::from_str(&result.ok_or("Missing persisted result")?)?;
    let kind = request["operation"].as_str().unwrap_or("");
    if !["create", "clone"].contains(&kind)
        || request["universe_uuid"].as_str() != Some(uuid)
        || result["container_id"] != c["Id"]
    {
        return Err(format!("{role} container does not match its recorded creation").into());
    }
    // A container this host transferred away never regains ownership through its original creation.
    if migration::transferred_away(db, uuid, c["Id"].as_str().unwrap_or(""))? {
        return Err(format!(
            "{role} container was transferred to another host by a completed migration; its original creation no longer grants ownership"
        )
        .into());
    }
    // Neither does a container a garbage collection proved absent when it collected the universe: it can
    // only have come back out of band.
    if migration::collected_absent(db, uuid, c["Id"].as_str().unwrap_or(""))? {
        return Err(format!(
            "{role} container was proven absent when this universe was collected; its original creation no longer grants ownership"
        )
        .into());
    }
    Ok(())
}
fn parse<'a>(
    db: &Connection,
    operation: &str,
    uuid: &str,
    request: &'a Value,
) -> Result<Params<'a>, Error> {
    Ok(match operation {
        "create" => {
            let image = text(request, "image")?;
            // An immutable local image ID is mandatory for this first version.
            if image.len() != 71
                || !image.starts_with("sha256:")
                || !image[7..].bytes().all(|c| c.is_ascii_hexdigit())
            {
                return Err("Use a full local sha256 image ID".into());
            }
            let command = request["command"]
                .as_array()
                .ok_or("command must be an array")?;
            let command: Vec<&str> = command
                .iter()
                .map(|v| v.as_str().ok_or("command items must be strings"))
                .collect::<Result<_, _>>()?;
            if command.is_empty() {
                return Err("Explicit command required".into());
            }
            // A universe's network is a decision, not a default that depends on host state.
            let profile = text(request, "network_profile")
                .map_err(|_| "network_profile is required: isolated or managed (docs/UNIVERSE-NETWORK-CONTRACT.md)")?;
            if profile != crate::network::PROFILE_ISOLATED
                && profile != crate::network::PROFILE_MANAGED
            {
                return Err("network_profile must be isolated or managed".into());
            }
            let address = request
                .get("network_address")
                .map(|v| v.as_str().ok_or("network_address must be a string"))
                .transpose()?;
            if address.is_some() && profile != crate::network::PROFILE_MANAGED {
                return Err("network_address is only meaningful with the managed profile".into());
            }
            let universe_profile = parse_universe_profile(request, profile)?;
            // Secrets are mounted from Podman's store, never from an image: names and targets only.
            let secrets = crate::secrets::mounts(db, request)?;
            // A manager replica's host state: its vote directory, the operator's evidence directory and
            // the host's machine-id, mounted at fixed paths derived from this name alone (V3-5).
            let host_state = request
                .get("manager_host_state")
                .map(|v| {
                    v.as_str()
                        .filter(|s| !s.is_empty())
                        .ok_or("manager_host_state must be a non-empty name")
                })
                .transpose()?;
            if let Some(name) = host_state {
                token(name)?;
            }
            Params::Create {
                image,
                command,
                universe_profile,
                profile,
                address,
                secrets,
                host_state,
            }
        }
        "clone" => {
            let source = text(request, "source_uuid")?;
            if !is_uuid(source) {
                return Err("Invalid source UUID".into());
            }
            if source == uuid {
                return Err("A clone requires a new universe UUID".into());
            }
            Params::Clone { source }
        }
        "start" => {
            let observe_seconds = match request.get("observe_seconds") {
                None => DEFAULT_OBSERVE_SECONDS,
                Some(v) => v
                    .as_u64()
                    .filter(|s| *s <= MAX_OBSERVE_SECONDS)
                    .ok_or("observe_seconds must be an integer from 0 to 30")?,
            };
            Params::Start { observe_seconds }
        }
        "stop" => {
            // No default: the caller declares how long to wait and whether PodMesh may force.
            let timeout = request
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .filter(|t| *t <= MAX_STOP_TIMEOUT_SECONDS)
                .ok_or("timeout_seconds must be an integer from 0 to 300")?;
            let on_timeout = request
                .get("on_timeout")
                .and_then(Value::as_str)
                .filter(|v| ["kill", "leave_running"].contains(v))
                .ok_or("on_timeout must be \"kill\" or \"leave_running\"")?;
            Params::Stop {
                timeout,
                on_timeout,
            }
        }
        "pause" => Params::Pause,
        "resume" => Params::Resume,
        "resources" => {
            let memory_bytes = match request.get("memory_bytes") {
                None | Some(Value::Null) => None,
                Some(v) => {
                    let m = v
                        .as_u64()
                        .ok_or("memory_bytes must be a positive integer of bytes")?;
                    let total = host_memory_bytes()?;
                    if m < MIN_MEMORY_BYTES || m > total {
                        return Err(format!("memory_bytes must be from {MIN_MEMORY_BYTES} to this host's {total} bytes").into());
                    }
                    Some(m)
                }
            };
            let cpus = match request.get("cpus") {
                None | Some(Value::Null) => None,
                Some(v) => {
                    let c = v
                        .as_f64()
                        .ok_or("cpus must be a number of cores, fractions allowed")?;
                    let cores = host_cpus() as f64;
                    if !(MIN_CPUS..=cores).contains(&c) {
                        return Err(format!(
                            "cpus must be from {MIN_CPUS} to this host's {cores} cores"
                        )
                        .into());
                    }
                    Some((c * 100.0).round() / 100.0)
                }
            };
            if memory_bytes.is_none() && cpus.is_none() {
                return Err("resources requires memory_bytes, cpus or both".into());
            }
            Params::Resources { memory_bytes, cpus }
        }
        "migration_preflight" | "migration_checkpoint" => {
            // Every identity the checkpoint is bound to is explicit and immutable.
            let container_id = text(request, "container_id")?;
            if container_id.len() != 64
                || !container_id
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err(
                    "container_id must be a full 64-character lowercase hexadecimal container ID"
                        .into(),
                );
            }
            let image = text(request, "image")?;
            if image.len() != 71
                || !image.starts_with("sha256:")
                || !image[7..].bytes().all(|c| c.is_ascii_hexdigit())
            {
                return Err("Use a full local sha256 image ID".into());
            }
            let source_host = text(request, "source_host_uuid")?;
            let destination = text(request, "destination_host_uuid")?;
            if !is_uuid(source_host) || !is_uuid(destination) {
                return Err("source_host_uuid and destination_host_uuid must be UUIDs".into());
            }
            if source_host == destination {
                return Err("destination_host_uuid must differ from source_host_uuid".into());
            }
            let profile = match request.get("migration_profile") {
                None | Some(Value::Null) => migration::MigrationProfile::Flat,
                Some(Value::String(raw)) => migration::MigrationProfile::parse(raw)?,
                _ => return Err("migration_profile must be a string (flat or nested)".into()),
            };
            let binding = Binding {
                container_id,
                image,
                source_host,
                destination,
                profile,
            };
            if operation == "migration_preflight" {
                Params::MigrationPreflight(binding)
            } else {
                Params::MigrationCheckpoint(binding)
            }
        }
        "migration_authorize_transfer" => {
            let checkpoint = text(request, "checkpoint_operation_id")?;
            token(checkpoint).map_err(|_| {
                "checkpoint_operation_id must contain 1-80 ASCII letters, digits or hyphens"
            })?;
            let destination = text(request, "destination_host_uuid")?;
            if !is_uuid(destination) {
                return Err("destination_host_uuid must be a UUID".into());
            }
            Params::MigrationAuthorize {
                checkpoint,
                destination,
                authorization_ref: text(request, "authorization_ref")?,
            }
        }
        // The recovery operations name the checkpoint whose reservation they act on, so that a request
        // can never resolve to a reservation the caller did not mean.
        "migration_release" | "migration_abandon" | "migration_restore_local" => {
            let checkpoint = text(request, "checkpoint_operation_id")?;
            token(checkpoint).map_err(|_| {
                "checkpoint_operation_id must contain 1-80 ASCII letters, digits or hyphens"
            })?;
            match operation {
                "migration_release" => Params::MigrationRelease { checkpoint },
                "migration_abandon" => Params::MigrationAbandon { checkpoint },
                _ => Params::MigrationRestoreLocal { checkpoint },
            }
        }
        "migration_complete_transfer"
        | "migration_retire_source"
        | "migration_destination_preflight"
        | "migration_restore"
        | "migration_restore_abort" => {
            // Documents are found by this service-issued identifier; requests never carry paths.
            let authorization = text(request, "authorization_id")?;
            if !is_uuid(authorization) {
                return Err("authorization_id must be a UUID".into());
            }
            match operation {
                "migration_complete_transfer" => Params::MigrationComplete { authorization },
                "migration_retire_source" => Params::MigrationRetire { authorization },
                "migration_destination_preflight" => Params::DestinationPreflight { authorization },
                "migration_restore" => Params::MigrationRestore { authorization },
                _ => {
                    // Explicit, typed and default false: ending processes is never implied by an abort.
                    let reclaim_processes = match request.get("reclaim_processes") {
                        None | Some(Value::Null) => false,
                        Some(v) => v
                            .as_bool()
                            .ok_or("reclaim_processes must be true or false")?,
                    };
                    Params::MigrationRestoreAbort {
                        authorization,
                        reference: text(request, "authorization_ref")?,
                        reclaim_processes,
                    }
                }
            }
        }
        _ => Params::Delete,
    })
}
pub(crate) fn ensure_schema(db: &Connection) -> Result<(), Error> {
    // operation_attempts is a separate table so that an experimental3 rollback, which inserts
    // four values into operations, keeps working on a journal written by this version.
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS operations(id TEXT PRIMARY KEY, request TEXT NOT NULL, status TEXT NOT NULL, result TEXT);
         CREATE TABLE IF NOT EXISTS operation_attempts(id INTEGER PRIMARY KEY, operation_id TEXT NOT NULL, started_at INTEGER NOT NULL, finished_at INTEGER, outcome TEXT, detail TEXT);",
    )?;
    migration::ensure_schema(db)?;
    Ok(())
}

trait LifecycleDb {
    fn secret_mounts(&mut self, request: &Value) -> Result<(Vec<String>, Option<String>), Error>;
    fn owned(&mut self, c: &Value, uuid: &str, role: &str) -> Result<(), Error>;
    fn refuse_reserved(&mut self, uuid: &str, operation: &str) -> Result<(), Error>;
    fn refuse_activation(&mut self, uuid: &str, operation: &str) -> Result<(), Error>;
    fn refuse_identity_reuse(&mut self, uuid: &str, operation: &str) -> Result<(), Error>;
    fn allocate_network(
        &mut self,
        uuid: &str,
        id: &str,
        requested: Option<&str>,
    ) -> Result<(String, String, String), Error>;
    fn release_network(&mut self, uuid: &str, id: &str) -> Result<Option<String>, Error>;
    fn earlier_attempt(&mut self, id: &str, attempt: i64) -> Result<Option<i64>, Error>;
    fn reservation_state(&mut self, uuid: &str) -> Result<Option<String>, Error>;
    fn remove_snapshots(&mut self, uuid: &str) -> Result<(Vec<String>, Vec<Value>), Error>;
    fn remove_restore_images(&mut self, uuid: &str) -> Result<(Vec<String>, Vec<Value>), Error>;
}

struct SqliteLifecycleDb<'a> {
    db: &'a Connection,
}

impl LifecycleDb for SqliteLifecycleDb<'_> {
    fn secret_mounts(&mut self, request: &Value) -> Result<(Vec<String>, Option<String>), Error> {
        crate::secrets::mounts(self.db, request)
    }

    fn owned(&mut self, c: &Value, uuid: &str, role: &str) -> Result<(), Error> {
        owned(self.db, c, uuid, role)
    }

    fn refuse_reserved(&mut self, uuid: &str, operation: &str) -> Result<(), Error> {
        migration::refuse_if_reserved(self.db, uuid, operation)
    }

    fn refuse_activation(&mut self, uuid: &str, operation: &str) -> Result<(), Error> {
        crate::activation::refuse_if_not_activated(self.db, uuid, operation)
    }

    fn refuse_identity_reuse(&mut self, uuid: &str, operation: &str) -> Result<(), Error> {
        migration::refuse_identity_reuse(self.db, uuid, operation)
    }

    fn allocate_network(
        &mut self,
        uuid: &str,
        id: &str,
        requested: Option<&str>,
    ) -> Result<(String, String, String), Error> {
        crate::network::allocate(self.db, uuid, id, requested)
    }

    fn release_network(&mut self, uuid: &str, id: &str) -> Result<Option<String>, Error> {
        crate::network::release(self.db, uuid, id)
    }

    fn earlier_attempt(&mut self, id: &str, attempt: i64) -> Result<Option<i64>, Error> {
        earlier_attempt(self.db, id, attempt)
    }

    fn reservation_state(&mut self, uuid: &str) -> Result<Option<String>, Error> {
        Ok(migration::reservation(self.db, uuid)?.map(|reservation| reservation.state))
    }

    fn remove_snapshots(&mut self, uuid: &str) -> Result<(Vec<String>, Vec<Value>), Error> {
        remove_snapshots(self.db, uuid)
    }

    fn remove_restore_images(&mut self, uuid: &str) -> Result<(Vec<String>, Vec<Value>), Error> {
        crate::recovery_point::remove_restore_images(self.db, uuid)
    }
}

struct DurableLifecycleDb<'a> {
    store: &'a mut dyn DurableStore,
}

impl DurableLifecycleDb<'_> {
    fn query_one(&mut self, sql: &str, values: &[Stored]) -> Result<Option<StoreRow>, Error> {
        Ok(self.store.query_one(sql, values)?)
    }

    fn query(&mut self, sql: &str, values: &[Stored]) -> Result<Vec<StoreRow>, Error> {
        Ok(self.store.query(sql, values)?)
    }

    fn execute(&mut self, sql: &str, values: &[Stored]) -> Result<u64, Error> {
        Ok(self.store.execute(sql, values)?)
    }

    fn restored_here(&mut self, uuid: &str, container_id: &str) -> Result<bool, Error> {
        let row = self.query_one(
            "SELECT o.request, o.result FROM migration_restore_claims k JOIN operations o ON o.id = k.operation_id \
             WHERE k.universe_uuid = ? AND k.container_id = ? AND k.state = 'restored' AND o.status = 'verified'",
            &[Stored::from(uuid), Stored::from(container_id)],
        )?;
        let Some(row) = row else { return Ok(false) };
        let request: Value = serde_json::from_str(row.text(0)?)?;
        let result: Value = serde_json::from_str(row.value(1)?.text().unwrap_or("null"))?;
        Ok(request["operation"] == "migration_restore"
            && request["universe_uuid"].as_str() == Some(uuid)
            && result["container_id"].as_str() == Some(container_id))
    }

    fn restored_locally(&mut self, uuid: &str, container_id: &str) -> Result<bool, Error> {
        let rows = self.query(
            "SELECT request, result FROM operations WHERE status = 'verified' AND result LIKE ?",
            &[Stored::from(format!("%{container_id}%"))],
        )?;
        for row in rows {
            let request: Value = serde_json::from_str(row.text(0)?)?;
            let result: Value = serde_json::from_str(row.value(1)?.text().unwrap_or("null"))?;
            if request["operation"] == "migration_restore_local"
                && request["universe_uuid"].as_str() == Some(uuid)
                && result["container_id"].as_str() == Some(container_id)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn promoted_live_here(&mut self, uuid: &str, container_id: &str) -> Result<bool, Error> {
        Ok(self
            .query_one(
                "SELECT 1 FROM recovery_point_live_promotions WHERE universe_uuid = ? AND container_id = ?",
                &[Stored::from(uuid), Stored::from(container_id)],
            )?
            .is_some())
    }

    fn transferred_away(&mut self, uuid: &str, container_id: &str) -> Result<bool, Error> {
        Ok(self
            .query_one(
                "SELECT universe_uuid FROM migration_reservations WHERE universe_uuid = ? AND container_id = ? AND state = 'transferred' \
                 UNION ALL SELECT universe_uuid FROM migration_reservation_history \
                 WHERE universe_uuid = ? AND container_id = ? AND state = 'transferred' LIMIT 1",
                &[
                    Stored::from(uuid),
                    Stored::from(container_id),
                    Stored::from(uuid),
                    Stored::from(container_id),
                ],
            )?
            .is_some())
    }

    fn collected_absent(&mut self, uuid: &str, container_id: &str) -> Result<bool, Error> {
        Ok(self
            .query_one(
                "SELECT universe_uuid FROM migration_universe_tombstones \
                 WHERE universe_uuid = ? AND container_id = ? AND container_absent_at_collection = 1 \
                 UNION ALL SELECT universe_uuid FROM migration_collection_history \
                 WHERE universe_uuid = ? AND container_id = ? AND container_absent_at_collection = 1 LIMIT 1",
                &[
                    Stored::from(uuid),
                    Stored::from(container_id),
                    Stored::from(uuid),
                    Stored::from(container_id),
                ],
            )?
            .is_some())
    }

    fn snapshot_recorded(&mut self, uuid: &str, image: &Value) -> Result<bool, Error> {
        let labels = &image["Labels"];
        let Some(operation) = labels[SNAPSHOT_OPERATION].as_str() else {
            return Ok(false);
        };
        let row = self.query_one(
            "SELECT request, status, result FROM operations WHERE id = ?",
            &[Stored::from(operation)],
        )?;
        let Some(row) = row else { return Ok(false) };
        let request: Value = serde_json::from_str(row.text(0)?)?;
        if request["operation"] != "clone"
            || request["universe_uuid"].as_str() != Some(uuid)
            || request["source_uuid"].as_str() != labels[UNIVERSE].as_str()
        {
            return Ok(false);
        }
        if row.text(1)? != "verified" {
            return Ok(true);
        }
        let result: Value = serde_json::from_str(row.value(2)?.text().unwrap_or("null"))?;
        Ok(result["snapshot_image"].as_str() == Some(image_id(image))
            && result["source_container_id"] == labels[SNAPSHOT_SOURCE])
    }
}

impl LifecycleDb for DurableLifecycleDb<'_> {
    fn secret_mounts(&mut self, request: &Value) -> Result<(Vec<String>, Option<String>), Error> {
        let Some(list) = request.get("secrets") else {
            return Ok((vec![], None));
        };
        let list = list
            .as_array()
            .ok_or("secrets must be a list of {name, target}")?;
        let mut args = vec![];
        let mut labels = vec![];
        for entry in list {
            let name = text(entry, "name")?;
            token(name)?;
            let target = text(entry, "target")?;
            if !target.starts_with('/')
                || target.contains("..")
                || target.contains(',')
                || target.ends_with('/')
            {
                return Err(format!(
                    "secret target {target} must be an absolute file path without '..' or ','"
                )
                .into());
            }
            if self
                .query_one(
                    "SELECT sha256 FROM secrets WHERE name = ? AND removed_at IS NULL AND state = 'effective'",
                    &[Stored::from(name)],
                )?
                .is_none()
            {
                return Err(format!("secret {name} is not declared on this host (secret_declare)").into());
            }
            let present = Command::new("podman")
                .args(["secret", "exists", name])
                .output();
            match present {
                Ok(output) if output.status.success() => {}
                Ok(_) => {
                    return Err(format!("secret {name} is declared here but absent from Podman's store; declare it again").into())
                }
                Err(_) => {
                    return Err(format!("Podman's secret store could not be asked about {name}; refusing on an unknown state").into())
                }
            }
            args.push("--secret".to_string());
            args.push(format!(
                "source={name},type=mount,target={target},mode=0600,uid=0,gid=0"
            ));
            labels.push(format!("{name}:{target}"));
        }
        Ok(if labels.is_empty() {
            (args, None)
        } else {
            (
                args,
                Some(format!("io.podmesh.secrets={}", labels.join(";"))),
            )
        })
    }

    fn owned(&mut self, c: &Value, uuid: &str, role: &str) -> Result<(), Error> {
        if let Some(container_id) = c["Id"].as_str() {
            if self.restored_here(uuid, container_id)?
                || self.restored_locally(uuid, container_id)?
                || self.promoted_live_here(uuid, container_id)?
            {
                return Ok(());
            }
        }
        let operation =
            label(c, CREATION).ok_or_else(|| format!("{role} has no creation operation"))?;
        let row = self.query_one(
            "SELECT request, result FROM operations WHERE id = ? AND status = 'verified'",
            &[Stored::from(operation)],
        )?;
        let Some(row) = row else {
            return Err(format!(
                "{role} is not recorded as a verified PodMesh universe on this host"
            )
            .into());
        };
        let request: Value = serde_json::from_str(row.text(0)?)?;
        let result: Value = serde_json::from_str(row.value(1)?.text().unwrap_or("null"))?;
        if !["create", "clone"].contains(&request["operation"].as_str().unwrap_or(""))
            || request["universe_uuid"].as_str() != Some(uuid)
            || result["container_id"] != c["Id"]
        {
            return Err(format!("{role} container does not match its recorded creation").into());
        }
        let container_id = c["Id"].as_str().unwrap_or("");
        if self.transferred_away(uuid, container_id)? {
            return Err(format!(
                "{role} container was transferred to another host by a completed migration; its original creation no longer grants ownership"
            )
            .into());
        }
        if self.collected_absent(uuid, container_id)? {
            return Err(format!(
                "{role} container was proven absent when this universe was collected; its original creation no longer grants ownership"
            )
            .into());
        }
        Ok(())
    }

    fn refuse_reserved(&mut self, uuid: &str, operation: &str) -> Result<(), Error> {
        if let Some(row) = self.query_one(
            "SELECT operation_id, state, container_id, image_id, source_host_uuid, destination_host_uuid, \
             container_started_at, created_at, updated_at, detail FROM migration_reservations WHERE universe_uuid = ?",
            &[Stored::from(uuid)],
        )? {
            let state = row.text(1)?;
            if state != "released" && state != "collected" {
                return Err(failure(
                    format!(
                        "Universe {uuid} is reserved by migration operation {} (state {state}); {operation} is refused while the reservation exists",
                        row.text(0)?
                    ),
                    json!({"reservation": {
                        "operation_id": row.text(0)?, "state": state, "container_id": row.text(2)?,
                        "image_id": row.text(3)?, "source_host_uuid": row.text(4)?,
                        "destination_host_uuid": row.text(5)?, "container_started_at": row.text(6)?,
                        "created_at": row.integer(7)?, "updated_at": row.integer(8)?,
                        "detail": row.value(9)?.text().and_then(|detail| serde_json::from_str::<Value>(detail).ok())
                    }}),
                ));
            }
        }
        if let Some(row) = self.query_one(
            "SELECT authorization_id, state FROM migration_restore_claims \
             WHERE universe_uuid = ? AND state IN ('restoring', 'restore_failed') ORDER BY created_at LIMIT 1",
            &[Stored::from(uuid)],
        )? {
            return Err(failure(
                format!(
                    "Universe {uuid} has an unresolved restore claim for authorization {} (state {}); {operation} is refused until migration_restore verifies it or migration_restore_abort closes it",
                    row.text(0)?,
                    row.text(1)?
                ),
                json!({"restore_claim": {"authorization_id": row.text(0)?, "state": row.text(1)?}}),
            ));
        }
        Ok(())
    }

    fn refuse_activation(&mut self, uuid: &str, operation: &str) -> Result<(), Error> {
        if self
            .query_one(
                "SELECT universe_uuid FROM activation_policy WHERE universe_uuid = ?",
                &[Stored::from(uuid)],
            )?
            .is_none()
        {
            return Ok(());
        }
        let held = self.query_one(
            "SELECT holder_host_uuid, expires_at, epoch FROM activation_leases WHERE universe_uuid = ?",
            &[Stored::from(uuid)],
        )?;
        let this_host = self
            .query_one(
                "SELECT value FROM metadata WHERE `key` = ?",
                &[Stored::from("host_uuid")],
            )?
            .ok_or("The journal carries no host_uuid")?;
        let this_host = this_host.text(0)?;
        let Some(lease) = held else {
            return Err(format!(
                "{operation} refused: this universe requires an activation lease and none is held"
            )
            .into());
        };
        if lease.text(0)? != this_host {
            return Err(format!(
                "{operation} refused: the activation lease is held by another host"
            )
            .into());
        }
        let now = crate::now() as i64;
        if lease.integer(1)? <= now {
            return Err(format!(
                "{operation} refused: this host's activation lease expired {} seconds ago",
                now - lease.integer(1)?
            )
            .into());
        }
        if let Some(epoch) = self.query_one(
            "SELECT epoch FROM activation_epochs WHERE universe_uuid = ? AND epoch > ? ORDER BY epoch DESC LIMIT 1",
            &[Stored::from(uuid), Stored::from(lease.integer(2)?)],
        )? {
            return Err(format!(
                "{operation} refused: this host's activation was superseded by epoch {}",
                epoch.integer(0)?
            )
            .into());
        }
        Ok(())
    }

    fn refuse_identity_reuse(&mut self, uuid: &str, operation: &str) -> Result<(), Error> {
        if let Some(row) = self.query_one(
            "SELECT class, class_number, collected_by_operation, collected_at \
             FROM migration_universe_tombstones WHERE universe_uuid = ?",
            &[Stored::from(uuid)],
        )? {
            return Err(failure(
                format!(
                    "Universe {uuid} was collected on this host by garbage collection operation {} (class {}); {operation} with this universe UUID is refused, because reusing a collected identity requires a verified handoff restore or an explicit replacement procedure",
                    row.text(2)?,
                    row.text(0)?
                ),
                json!({"tombstone": {
                    "universe_uuid": uuid, "class": row.text(0)?, "class_number": row.integer(1)?,
                    "collected_by_operation": row.text(2)?, "collected_at": row.integer(3)?
                }}),
            ));
        }
        Ok(())
    }

    fn allocate_network(
        &mut self,
        uuid: &str,
        id: &str,
        requested: Option<&str>,
    ) -> Result<(String, String, String), Error> {
        let declaration = self
            .query_one(
                "SELECT network_uuid, pool, gateway, bridge, state FROM network_declaration LIMIT 1",
                &[],
            )?
            .ok_or("The managed profile needs a network declared on this host (network_declare); none is")?;
        if declaration.text(4)? != "effective" {
            return Err(format!(
                "The network declaration on this host is in state {}, not effective",
                declaration.text(4)?
            )
            .into());
        }
        let network_uuid = declaration.text(0)?.to_string();
        let pool_text = declaration.text(1)?;
        let gateway: Ipv4Addr = declaration.text(2)?.parse()?;
        let bridge = declaration.text(3)?.to_string();
        if let Some(row) = self.query_one(
            "SELECT ip FROM network_allocations WHERE universe_uuid = ? AND released_at IS NULL",
            &[Stored::from(uuid)],
        )? {
            return Ok((bridge, row.text(0)?.to_string(), network_uuid));
        }
        let (network, bits) = pool_text.split_once('/').ok_or("pool must be a.b.c.d/n")?;
        let network: Ipv4Addr = network.parse().map_err(|_| "pool has an invalid address")?;
        let bits: u8 = bits
            .parse()
            .map_err(|_| "pool has an invalid prefix length")?;
        if !(8..=30).contains(&bits) {
            return Err("pool prefix length must be from 8 to 30".into());
        }
        let mask = u32::MAX << (32 - bits);
        let first = u32::from(network) & mask;
        if first != u32::from(network) {
            return Err("pool must name the network address, not a host in it".into());
        }
        let last = first | !mask;
        let used: BTreeSet<u32> = self
            .query(
                "SELECT ip FROM network_allocations WHERE released_at IS NULL",
                &[],
            )?
            .into_iter()
            .filter_map(|row| {
                row.text(0)
                    .ok()
                    .and_then(|ip| ip.parse::<Ipv4Addr>().ok())
                    .map(u32::from)
            })
            .collect();
        let selected = match requested {
            Some(text) => {
                let address: Ipv4Addr = text
                    .parse()
                    .map_err(|_| "network_address must be an IPv4 address")?;
                let raw = u32::from(address);
                if raw & mask != first || raw == first || raw == last {
                    return Err(format!(
                        "network_address {address} is outside the local pool {pool_text}"
                    )
                    .into());
                }
                if address == gateway {
                    return Err(format!("network_address {address} is the pool's gateway").into());
                }
                if used.contains(&raw) {
                    return Err(format!(
                        "network_address {address} is allocated to another universe"
                    )
                    .into());
                }
                address
            }
            None => {
                let mut candidate = first + 1;
                loop {
                    if candidate >= last {
                        return Err("The local pool has no free address left".into());
                    }
                    let address = Ipv4Addr::from(candidate);
                    if address != gateway && !used.contains(&candidate) {
                        break address;
                    }
                    candidate += 1;
                }
            }
        };
        self.execute(
            "INSERT INTO network_allocations(universe_uuid, network_uuid, ip, allocated_at, operation_id) \
             VALUES(?, ?, ?, ?, ?)",
            &[
                Stored::from(uuid),
                Stored::from(&network_uuid),
                Stored::from(selected.to_string()),
                Stored::from(crate::now() as i64),
                Stored::from(id),
            ],
        )?;
        Ok((bridge, selected.to_string(), network_uuid))
    }

    fn release_network(&mut self, uuid: &str, id: &str) -> Result<Option<String>, Error> {
        let held = self.query_one(
            "SELECT ip FROM network_allocations WHERE universe_uuid = ? AND released_at IS NULL",
            &[Stored::from(uuid)],
        )?;
        let Some(row) = held else { return Ok(None) };
        let ip = row.text(0)?.to_string();
        self.execute(
            "UPDATE network_allocations SET released_at = ?, released_by = ? \
             WHERE universe_uuid = ? AND released_at IS NULL",
            &[
                Stored::from(crate::now() as i64),
                Stored::from(id),
                Stored::from(uuid),
            ],
        )?;
        Ok(Some(ip))
    }

    fn earlier_attempt(&mut self, id: &str, attempt: i64) -> Result<Option<i64>, Error> {
        let row = self.query_one(
            "SELECT MIN(started_at) FROM operation_attempts WHERE operation_id = ? AND id < ?",
            &[Stored::from(id), Stored::from(attempt)],
        )?;
        Ok(row.and_then(|row| row.value(0).ok().and_then(Stored::integer)))
    }

    fn reservation_state(&mut self, uuid: &str) -> Result<Option<String>, Error> {
        Ok(self
            .query_one(
                "SELECT state FROM migration_reservations WHERE universe_uuid = ?",
                &[Stored::from(uuid)],
            )?
            .map(|row| row.text(0).map(str::to_string))
            .transpose()?)
    }

    fn remove_snapshots(&mut self, uuid: &str) -> Result<(Vec<String>, Vec<Value>), Error> {
        let (mut removed, mut retained) = (vec![], vec![]);
        for image in images()?
            .iter()
            .filter(|image| image["Labels"][SNAPSHOT_FOR].as_str() == Some(uuid))
        {
            let id = image_id(image).to_string();
            let tags = names(image);
            if tags
                .iter()
                .any(|name| !name.starts_with(SNAPSHOT_REPOSITORY))
            {
                retained.push(json!({"image": id, "reason": "image has names outside the PodMesh snapshot repository"}));
                continue;
            }
            if !self.snapshot_recorded(uuid, image)? {
                retained.push(json!({"image": id, "reason": "snapshot provenance does not match this host's journal"}));
                continue;
            }
            let targets = if tags.is_empty() {
                vec![id.as_str()]
            } else {
                tags
            };
            let mut args = vec!["image", "rm"];
            args.extend(targets);
            match podman(QUICK, &args) {
                Ok(_) => removed.push(id),
                Err(error) => retained.push(json!({"image": id, "reason": error.to_string()})),
            }
        }
        let remaining = images()?;
        removed.retain(|id| {
            let present = remaining.iter().any(|image| image_id(image) == id.as_str());
            if present {
                retained.push(json!({"image": id, "reason": "untagged but still present; a dependent image or container uses it"}));
            }
            !present
        });
        Ok((removed, retained))
    }

    fn remove_restore_images(&mut self, uuid: &str) -> Result<(Vec<String>, Vec<Value>), Error> {
        let mut ids: Vec<String> = self
            .query(
                "SELECT imported_image_id FROM recovery_point_restores WHERE restored_universe_uuid = ? \
                 UNION SELECT r.imported_image_id FROM recovery_point_promotions p \
                 JOIN recovery_point_restores r ON r.restored_universe_uuid = p.restored_universe_uuid \
                 WHERE p.universe_uuid = ?",
                &[Stored::from(uuid), Stored::from(uuid)],
            )?
            .iter()
            .map(|row| row.text(0).map(str::to_string))
            .collect::<crate::store::Result<Vec<_>>>()?;
        ids.sort();
        ids.dedup();
        let (mut removed, mut retained) = (vec![], vec![]);
        if ids.is_empty() {
            return Ok((removed, retained));
        }
        let present = images()?;
        for id in ids {
            let bare = id.trim_start_matches("sha256:").to_string();
            let Some(image) = present.iter().find(|image| image_id(image) == bare) else {
                continue;
            };
            let names: Vec<&str> = image["Names"]
                .as_array()
                .map(|held| held.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if names
                .iter()
                .any(|name| !name.starts_with("localhost/podmesh-recovery:"))
            {
                retained.push(json!({"image": bare, "reason": "image has names outside the PodMesh restore repository"}));
                continue;
            }
            match run_podman(QUICK, &["image", "rm", &bare]) {
                Ok(output) if output.status.success() => removed.push(bare),
                Ok(output) => retained.push(json!({"image": bare, "reason": String::from_utf8_lossy(&output.stderr).trim()})),
                Err(error) => retained.push(json!({"image": bare, "reason": error.to_string()})),
            }
        }
        Ok((removed, retained))
    }
}
pub fn execute(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = text(request, "operation")?;
    let id = text(request, "operation_id")?;
    token(id)?;
    let uuid = text(request, "universe_uuid")?;
    if !is_uuid(uuid) {
        return Err("Invalid universe UUID".into());
    }
    text(request, "authorization_ref")?;
    if !OPERATIONS.contains(&operation) {
        return Err("Unsupported lifecycle operation".into());
    }
    let params = parse(db, operation, uuid, request)?;
    ensure_schema(db)?;
    let canonical = request.to_string();
    let previous: Option<(String, String, Option<String>)> = db
        .query_row(
            "SELECT request,status,result FROM operations WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((saved, status, result)) = previous {
        if saved != canonical {
            return Err("Operation ID already belongs to a different request".into());
        }
        if status == "verified" {
            return replay(db, id, uuid, operation, result);
        }
        // pending (interrupted) or failed: re-evaluate from the observed state below.
    } else {
        db.execute(
            "INSERT INTO operations VALUES(?1,?2,'pending',NULL)",
            params![id, canonical],
        )?;
    }
    db.execute(
        "INSERT INTO operation_attempts(operation_id,started_at) VALUES(?1,?2)",
        params![id, crate::now() as i64],
    )?;
    let attempt = db.last_insert_rowid();
    let outcome = perform(db, attempt, id, uuid, &params);
    let finished = crate::now() as i64;
    match &outcome {
        Ok(result) => {
            db.execute(
                "UPDATE operations SET status='verified', result=?2 WHERE id=?1",
                params![id, result.to_string()],
            )?;
            db.execute(
                "UPDATE operation_attempts SET finished_at=?2, outcome='verified' WHERE id=?1",
                params![attempt, finished],
            )?;
        }
        Err(e) => {
            let details = e
                .downcast_ref::<Failure>()
                .map(|f| f.details.clone())
                .unwrap_or(Value::Null);
            let record = json!({"error": e.to_string(), "details": details}).to_string();
            db.execute(
                "UPDATE operations SET status='failed', result=?2 WHERE id=?1",
                params![id, record],
            )?;
            db.execute(
                "UPDATE operation_attempts SET finished_at=?2, outcome='failed', detail=?3 WHERE id=?1",
                params![attempt, finished, record],
            )?;
        }
    }
    outcome
}

/// The lifecycle operations whose journal has been ported to the engine-neutral store contract.
///
/// The schema is installed when the node store opens. This path therefore performs no DDL and
/// uses only positional `?` placeholders. A pending operation and its attempt are committed
/// before Podman is touched; the terminal journal update is committed as one transaction.
pub fn execute_store(store: &mut dyn DurableStore, request: &Value) -> Result<Value, Error> {
    let operation = text(request, "operation")?;
    if ![
        "create",
        "delete",
        "clone",
        "start",
        "stop",
        "pause",
        "resume",
        "resources",
    ]
    .contains(&operation)
    {
        return Err("Unsupported durable lifecycle operation".into());
    }
    let id = text(request, "operation_id")?;
    token(id)?;
    let uuid = text(request, "universe_uuid")?;
    if !is_uuid(uuid) {
        return Err("Invalid universe UUID".into());
    }
    text(request, "authorization_ref")?;
    let params = {
        let mut db = DurableLifecycleDb { store };
        parse_store(&mut db, operation, uuid, request)?
    };
    let canonical = request.to_string();
    let previous = store.query_one(
        "SELECT request, status, result FROM operations WHERE id = ?",
        &[Stored::from(id)],
    )?;
    if let Some(ref row) = previous {
        if row.text(0)? != canonical {
            return Err("Operation ID already belongs to a different request".into());
        }
        if row.text(1)? == "verified" {
            return replay_store(store, id, uuid, row.value(2)?.text().map(str::to_string));
        }
    }

    let started = crate::now() as i64;
    let attempt = {
        let mut tx = store.transaction()?;
        if previous.is_none() {
            tx.execute(
                "INSERT INTO operations(id, request, status, result) VALUES(?, ?, 'pending', NULL)",
                &[Stored::from(id), Stored::from(&canonical)],
            )?;
        }
        tx.execute(
            "INSERT INTO operation_attempts(operation_id, started_at) VALUES(?, ?)",
            &[Stored::from(id), Stored::from(started)],
        )?;
        let attempt = tx
            .query_one(
                "SELECT id FROM operation_attempts WHERE operation_id = ? ORDER BY id DESC LIMIT 1",
                &[Stored::from(id)],
            )?
            .ok_or("The lifecycle attempt was inserted but could not be read back")?
            .integer(0)?;
        tx.commit()?;
        attempt
    };

    let outcome = {
        let mut db = DurableLifecycleDb { store };
        perform_managed(&mut db, attempt, id, uuid, &params)
    };
    let finished = crate::now() as i64;
    let mut tx = store.transaction()?;
    match &outcome {
        Ok(result) => {
            tx.execute(
                "UPDATE operations SET status = 'verified', result = ? WHERE id = ?",
                &[Stored::from(result.to_string()), Stored::from(id)],
            )?;
            tx.execute(
                "UPDATE operation_attempts SET finished_at = ?, outcome = 'verified' WHERE id = ?",
                &[Stored::from(finished), Stored::from(attempt)],
            )?;
        }
        Err(error) => {
            let details = error
                .downcast_ref::<Failure>()
                .map(|failure| failure.details.clone())
                .unwrap_or(Value::Null);
            let record = json!({"error": error.to_string(), "details": details}).to_string();
            tx.execute(
                "UPDATE operations SET status = 'failed', result = ? WHERE id = ?",
                &[Stored::from(&record), Stored::from(id)],
            )?;
            tx.execute(
                "UPDATE operation_attempts SET finished_at = ?, outcome = 'failed', detail = ? WHERE id = ?",
                &[
                    Stored::from(finished),
                    Stored::from(record),
                    Stored::from(attempt),
                ],
            )?;
        }
    }
    tx.commit()?;
    outcome
}

fn parse_store<'a>(
    db: &mut dyn LifecycleDb,
    operation: &str,
    uuid: &str,
    request: &'a Value,
) -> Result<Params<'a>, Error> {
    Ok(match operation {
        "create" => {
            let image = text(request, "image")?;
            if image.len() != 71
                || !image.starts_with("sha256:")
                || !image[7..]
                    .bytes()
                    .all(|character| character.is_ascii_hexdigit())
            {
                return Err("Use a full local sha256 image ID".into());
            }
            let command = request["command"]
                .as_array()
                .ok_or("command must be an array")?;
            let command: Vec<&str> = command
                .iter()
                .map(|value| value.as_str().ok_or("command items must be strings"))
                .collect::<Result<_, _>>()?;
            if command.is_empty() {
                return Err("Explicit command required".into());
            }
            let profile = text(request, "network_profile")
                .map_err(|_| "network_profile is required: isolated or managed (docs/UNIVERSE-NETWORK-CONTRACT.md)")?;
            if profile != crate::network::PROFILE_ISOLATED
                && profile != crate::network::PROFILE_MANAGED
            {
                return Err("network_profile must be isolated or managed".into());
            }
            let address = request
                .get("network_address")
                .map(|value| value.as_str().ok_or("network_address must be a string"))
                .transpose()?;
            if address.is_some() && profile != crate::network::PROFILE_MANAGED {
                return Err("network_address is only meaningful with the managed profile".into());
            }
            let universe_profile = parse_universe_profile(request, profile)?;
            let secrets = db.secret_mounts(request)?;
            let host_state = request
                .get("manager_host_state")
                .map(|value| {
                    value
                        .as_str()
                        .filter(|name| !name.is_empty())
                        .ok_or("manager_host_state must be a non-empty name")
                })
                .transpose()?;
            if let Some(name) = host_state {
                token(name)?;
            }
            Params::Create {
                image,
                command,
                universe_profile,
                profile,
                address,
                secrets,
                host_state,
            }
        }
        "clone" => {
            let source = text(request, "source_uuid")?;
            if !is_uuid(source) {
                return Err("Invalid source UUID".into());
            }
            if source == uuid {
                return Err("A clone requires a new universe UUID".into());
            }
            Params::Clone { source }
        }
        "delete" => Params::Delete,
        "start" => {
            let observe_seconds = match request.get("observe_seconds") {
                None => DEFAULT_OBSERVE_SECONDS,
                Some(value) => value
                    .as_u64()
                    .filter(|seconds| *seconds <= MAX_OBSERVE_SECONDS)
                    .ok_or("observe_seconds must be an integer from 0 to 30")?,
            };
            Params::Start { observe_seconds }
        }
        "stop" => {
            let timeout = request
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .filter(|seconds| *seconds <= MAX_STOP_TIMEOUT_SECONDS)
                .ok_or("timeout_seconds must be an integer from 0 to 300")?;
            let on_timeout = request
                .get("on_timeout")
                .and_then(Value::as_str)
                .filter(|value| ["kill", "leave_running"].contains(value))
                .ok_or("on_timeout must be \"kill\" or \"leave_running\"")?;
            Params::Stop {
                timeout,
                on_timeout,
            }
        }
        "pause" => Params::Pause,
        "resume" => Params::Resume,
        "resources" => {
            let memory_bytes = match request.get("memory_bytes") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let memory = value
                        .as_u64()
                        .ok_or("memory_bytes must be a positive integer of bytes")?;
                    let total = host_memory_bytes()?;
                    if memory < MIN_MEMORY_BYTES || memory > total {
                        return Err(format!("memory_bytes must be from {MIN_MEMORY_BYTES} to this host's {total} bytes").into());
                    }
                    Some(memory)
                }
            };
            let cpus = match request.get("cpus") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let cpus = value
                        .as_f64()
                        .ok_or("cpus must be a number of cores, fractions allowed")?;
                    let cores = host_cpus() as f64;
                    if !(MIN_CPUS..=cores).contains(&cpus) {
                        return Err(format!(
                            "cpus must be from {MIN_CPUS} to this host's {cores} cores"
                        )
                        .into());
                    }
                    Some((cpus * 100.0).round() / 100.0)
                }
            };
            if memory_bytes.is_none() && cpus.is_none() {
                return Err("resources requires memory_bytes, cpus or both".into());
            }
            Params::Resources { memory_bytes, cpus }
        }
        _ => return Err("Unsupported durable lifecycle operation".into()),
    })
}

fn replay_store(
    store: &mut dyn DurableStore,
    id: &str,
    uuid: &str,
    result: Option<String>,
) -> Result<Value, Error> {
    let original: Value = serde_json::from_str(&result.ok_or("Missing persisted result")?)?;
    let verified_at = store
        .query_one(
            "SELECT MAX(finished_at) FROM operation_attempts WHERE operation_id = ? AND outcome = 'verified'",
            &[Stored::from(id)],
        )?
        .and_then(|row| row.value(0).ok().and_then(Stored::integer));
    let current = observe(uuid)?;
    let same_container = match (
        original["container_id"].as_str(),
        current["container_id"].as_str(),
    ) {
        (Some(recorded), Some(observed)) => json!(recorded == observed),
        (Some(_), None) => json!(false),
        _ => Value::Null,
    };
    Ok(json!({
        "replayed": true,
        "historical": true,
        "notice": "original_result is the persisted result from when this operation was verified, not current state; current is a fresh Podman observation",
        "verified_at": verified_at,
        "original_result": original,
        "current": current,
        "current_matches_recorded_container": same_container,
    }))
}
/// The journal contract, for modules whose operations are not lifecycle operations: a stable
/// operation ID, one request per ID, a verified operation replayed as its persisted result
/// (flat, marked `replayed` and `historical`) and never executed again, a pending one -- an
/// interrupted attempt -- re-evaluated by running again, and a durable attempt record either
/// way. The canonical form compared is the request's own serialization, exactly as above.
/// Lab-only fault injection, read from `PODMESH_FAULT`: `<point>` makes the daemon fail at that
/// point as a storage failure would; `<point>:crash` ends the process there, as a crash would;
/// `<point>:delay` holds it three seconds there, so that a race can be forced from outside.
/// Nothing sets this variable in the packaged units; the laboratory sets it on the transient
/// unit and restarts the daemon to watch what follows.
pub(crate) fn fault(point: &str) -> Result<(), Error> {
    // One or several points, comma-separated: `PODMESH_FAULT=publisher-after-connector,publisher-during-compensation:crash`
    // fails the first and crashes in the compensation the failure starts.
    let Ok(spec) = std::env::var("PODMESH_FAULT") else {
        return Ok(());
    };
    for one in spec.split(',').map(str::trim) {
        if one == format!("{point}:crash") {
            eprintln!("PodMesh fault injection: crashing at {point}");
            std::process::exit(70);
        }
        if one == format!("{point}:delay") {
            eprintln!("PodMesh fault injection: holding three seconds at {point}");
            std::thread::sleep(std::time::Duration::from_secs(3));
            return Ok(());
        }
        if one == point {
            return Err(format!(
                "simulated storage failure at {point}; nothing is recorded as effective"
            )
            .into());
        }
    }
    Ok(())
}

pub(crate) fn journaled(
    db: &Connection,
    request: &Value,
    run: impl FnOnce(&Connection) -> Result<Value, Error>,
) -> Result<Value, Error> {
    let id = text(request, "operation_id")?;
    token(id)?;
    ensure_schema(db)?;
    let canonical = request.to_string();
    let previous: Option<(String, String, Option<String>)> = db
        .query_row(
            "SELECT request,status,result FROM operations WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((saved, status, result)) = previous {
        if saved != canonical {
            return Err("Operation ID already belongs to a different request".into());
        }
        if status == "verified" {
            let mut original: Value =
                serde_json::from_str(&result.ok_or("Missing persisted result")?)?;
            original["replayed"] = json!(true);
            original["historical"] = json!(true);
            original["notice"] = json!("this is the result persisted when the operation was verified, not current state; a replay repeats no effect");
            return Ok(original);
        }
    } else {
        db.execute(
            "INSERT INTO operations VALUES(?1,?2,'pending',NULL)",
            params![id, canonical],
        )?;
    }
    db.execute(
        "INSERT INTO operation_attempts(operation_id,started_at) VALUES(?1,?2)",
        params![id, crate::now() as i64],
    )?;
    let attempt = db.last_insert_rowid();
    let outcome = run(db);
    let finished = crate::now() as i64;
    match &outcome {
        Ok(result) => {
            db.execute(
                "UPDATE operations SET status='verified', result=?2 WHERE id=?1",
                params![id, result.to_string()],
            )?;
            db.execute(
                "UPDATE operation_attempts SET finished_at=?2, outcome='verified' WHERE id=?1",
                params![attempt, finished],
            )?;
        }
        Err(e) => {
            let record = json!({"error": e.to_string()}).to_string();
            db.execute(
                "UPDATE operations SET status='failed', result=?2 WHERE id=?1",
                params![id, record],
            )?;
            db.execute(
                "UPDATE operation_attempts SET finished_at=?2, outcome='failed', detail=?3 WHERE id=?1",
                params![attempt, finished, record],
            )?;
        }
    }
    outcome
}

/// A verified operation is never executed again. Its persisted result is returned as
/// history, next to a fresh observation that may contradict it.
fn replay(
    db: &Connection,
    id: &str,
    uuid: &str,
    operation: &str,
    result: Option<String>,
) -> Result<Value, Error> {
    let original: Value = serde_json::from_str(&result.ok_or("Missing persisted result")?)?;
    // A replay never repeats an effect. Migration replays add a fresh re-hash or state of what the operation produced.
    let extra = match operation {
        "migration_checkpoint" => Some((
            "current_artifacts",
            migration::verify_artifacts(
                id,
                original["archive"]["sha256"].as_str(),
                original["manifest"]["sha256"].as_str(),
            )?,
        )),
        "migration_authorize_transfer" => {
            Some(("current_artifacts", transfer::verify_outbox(&original)?))
        }
        "migration_restore" | "migration_restore_abort" => {
            Some(("current_outcome", restore::verify_outcome(&original)?))
        }
        "migration_complete_transfer" | "migration_retire_source" => Some((
            "current_reservation",
            json!(migration::reservation(db, uuid)?.map(|r| r.view())),
        )),
        _ => None,
    };
    let verified_at: Option<i64> = db.query_row(
        "SELECT MAX(finished_at) FROM operation_attempts WHERE operation_id=?1 AND outcome='verified'",
        [id],
        |r| r.get(0),
    )?;
    let current = observe(uuid)?;
    let same_container = match (
        original["container_id"].as_str(),
        current["container_id"].as_str(),
    ) {
        (Some(recorded), Some(observed)) => json!(recorded == observed),
        (Some(_), None) => json!(false),
        _ => Value::Null,
    };
    let mut response = json!({
        "replayed": true,
        "historical": true,
        "notice": "original_result is the persisted result from when this operation was verified, not current state; current is a fresh Podman observation",
        "verified_at": verified_at,
        "original_result": original,
        "current": current,
        "current_matches_recorded_container": same_container,
    });
    if let Some((key, value)) = extra {
        response[key] = value;
    }
    Ok(response)
}
fn perform(
    db: &Connection,
    attempt: i64,
    id: &str,
    uuid: &str,
    params: &Params,
) -> Result<Value, Error> {
    if matches!(
        params,
        Params::Create { .. }
            | Params::Clone { .. }
            | Params::Delete
            | Params::Start { .. }
            | Params::Stop { .. }
            | Params::Pause
            | Params::Resume
            | Params::Resources { .. }
    ) {
        let mut managed = SqliteLifecycleDb { db };
        return perform_managed(&mut managed, attempt, id, uuid, params);
    }
    let name = format!("podmesh-{uuid}");
    let existing = inspect(&name)?;
    // On a migration destination an occupied name is a reported blocker, not an early refusal.
    let on_destination = matches!(
        params,
        Params::DestinationPreflight { .. }
            | Params::MigrationRestore { .. }
            | Params::MigrationRestoreAbort { .. }
    );
    if let Some(ref c) = existing {
        if !on_destination && label(c, UNIVERSE) != Some(uuid) {
            return Err("Target is not managed by PodMesh".into());
        }
    }
    // A migration reservation or an unresolved restore claim blocks every generic operation that could run, replace
    // or remove the universe. Stop stays available: it cannot run, replace or remove the source, although stopping
    // a source whose checkpoint failed does end its process. Migration operations apply their own state checks.
    // Pausing, resuming and changing a cgroup's limits all touch the process set a checkpoint in
    // flight is about, so they wait for the reservation like the rest.
    if matches!(
        params,
        Params::Create { .. }
            | Params::Clone { .. }
            | Params::Delete
            | Params::Start { .. }
            | Params::Pause
            | Params::Resume
            | Params::Resources { .. }
    ) {
        migration::refuse_if_reserved(db, uuid, params.name())?;
    }
    // A universe under an activation policy runs only where a live lease says it may. This is
    // a self-restraint on THIS host, not exclusion across hosts: it stops an agent that names
    // the wrong host, and it does not stop a host that never asks. `stop` stays available for
    // the same reason it survives a reservation -- stopping can never produce a second writer.
    // The gate covers every operation that leaves a RUNNING universe behind, and two of them
    // are not `start`: a destination restore and a local recovery restore both end with a
    // process serving requests. A gate that only knew about `start` let the migration chain
    // produce a writer on a host that held no entitlement to run it.
    if matches!(
        params,
        Params::Start { .. }
            | Params::Resume
            | Params::Clone { .. }
            | Params::MigrationRestore { .. }
            | Params::MigrationRestoreLocal { .. }
    ) {
        crate::activation::refuse_if_not_activated(db, uuid, params.name())?;
    }
    // Only the host entitled to run a universe may hand it off. Authorizing a transfer from a
    // host whose lease has lapsed, or that never held one, would let a fenced host originate
    // the very handoff the fence exists to prevent.
    if matches!(params, Params::MigrationAuthorize { .. }) {
        crate::activation::refuse_if_not_activated(db, uuid, params.name())?;
    }
    // A garbage collection leaves a tombstone: the identity of a collected universe is never given a new
    // meaning by a blind create, or by a clone into it. Operating the container a collection released, and
    // restoring a verified handoff, stay available (docs/GARBAGE-COLLECTION.md).
    if matches!(params, Params::Create { .. } | Params::Clone { .. }) {
        migration::refuse_identity_reuse(db, uuid, params.name())?;
    }
    match params {
        Params::Create { .. }
        | Params::Clone { .. }
        | Params::Delete
        | Params::Start { .. }
        | Params::Stop { .. }
        | Params::Pause
        | Params::Resume
        | Params::Resources { .. } => unreachable!("managed lifecycle operations returned above"),
        Params::MigrationPreflight(binding) => migration::preflight(db, uuid, binding, existing),
        Params::MigrationCheckpoint(binding) => {
            migration::checkpoint(db, attempt, id, uuid, &name, binding, existing)
        }
        Params::MigrationAuthorize {
            checkpoint,
            destination,
            authorization_ref,
        } => transfer::authorize(
            db,
            id,
            uuid,
            checkpoint,
            destination,
            authorization_ref,
            existing,
        ),
        Params::MigrationComplete { authorization } => {
            let result = transfer::complete(db, id, uuid, authorization, existing)?;
            // The universe now runs elsewhere, so this host's entitlement is surrendered. This is
            // cleanup rather than the safety mechanism: a completed reservation already refuses
            // `start` here in every state but released or collected, so a release lost to a
            // crash between the two writes blocks and never permits. It is still done, because
            // a lease that outlives the universe it was for is a lie in the journal.
            crate::activation::release_by_handoff(db, uuid, id)?;
            Ok(result)
        }
        Params::MigrationRetire { authorization } => {
            transfer::retire(db, id, uuid, &name, authorization, existing)
        }
        Params::MigrationRelease { checkpoint } => {
            recovery::release(db, id, uuid, checkpoint, existing)
        }
        Params::MigrationAbandon { checkpoint } => {
            recovery::abandon(db, id, uuid, checkpoint, existing)
        }
        Params::MigrationRestoreLocal { checkpoint } => {
            recovery::restore_local(db, attempt, id, uuid, &name, checkpoint, existing)
        }
        Params::DestinationPreflight { authorization } => {
            restore::preflight(db, uuid, authorization, existing)
        }
        Params::MigrationRestore { authorization } => {
            restore::restore(db, attempt, id, uuid, &name, authorization, existing)
        }
        Params::MigrationRestoreAbort {
            authorization,
            reference,
            reclaim_processes,
        } => restore::abort(
            db,
            id,
            uuid,
            &name,
            authorization,
            reference,
            *reclaim_processes,
            existing,
        ),
    }
}

fn perform_managed(
    db: &mut dyn LifecycleDb,
    attempt: i64,
    id: &str,
    uuid: &str,
    params: &Params,
) -> Result<Value, Error> {
    let name = format!("podmesh-{uuid}");
    let existing = inspect(&name)?;
    if let Some(ref container) = existing {
        if label(container, UNIVERSE) != Some(uuid) {
            return Err("Target is not managed by PodMesh".into());
        }
    }
    if matches!(
        params,
        Params::Create { .. }
            | Params::Clone { .. }
            | Params::Delete
            | Params::Start { .. }
            | Params::Pause
            | Params::Resume
            | Params::Resources { .. }
    ) {
        db.refuse_reserved(uuid, params.name())?;
    }
    if matches!(
        params,
        Params::Start { .. } | Params::Resume | Params::Clone { .. }
    ) {
        db.refuse_activation(uuid, params.name())?;
    }
    if matches!(params, Params::Create { .. } | Params::Clone { .. }) {
        db.refuse_identity_reuse(uuid, params.name())?;
    }
    match params {
        Params::Create {
            image,
            command,
            universe_profile,
            profile,
            address,
            secrets,
            host_state,
        } => create(
            db,
            id,
            uuid,
            &name,
            existing,
            image,
            command,
            universe_profile,
            profile,
            *address,
            secrets,
            *host_state,
        ),
        Params::Clone { source } => clone(db, id, uuid, source, &name, existing),
        Params::Delete => delete(db, uuid, &name, existing),
        Params::Start { observe_seconds } => {
            start(db, attempt, id, uuid, &name, existing, *observe_seconds)
        }
        Params::Stop {
            timeout,
            on_timeout,
        } => stop(db, attempt, id, uuid, &name, existing, *timeout, on_timeout),
        Params::Pause => pause(db, uuid, &name, existing),
        Params::Resume => resume(db, uuid, &name, existing),
        Params::Resources { memory_bytes, cpus } => {
            resources(db, uuid, &name, existing, *memory_bytes, *cpus)
        }
        _ => Err("Unsupported durable lifecycle operation".into()),
    }
}
#[allow(clippy::too_many_arguments)]
fn create(
    db: &mut dyn LifecycleDb,
    id: &str,
    uuid: &str,
    name: &str,
    existing: Option<Value>,
    image: &str,
    command: &[&str],
    universe_profile: &str,
    profile: &str,
    address: Option<&str>,
    secrets: &(Vec<String>, Option<String>),
    host_state: Option<&str>,
) -> Result<Value, Error> {
    let host_state_name = host_state;
    if let Some(ref c) = existing {
        if label(c, CREATION) != Some(id) {
            return Err("Universe already exists under another creation operation".into());
        }
    } else {
        let label = format!("{UNIVERSE}={uuid}");
        let provenance = format!("{CREATION}={id}");
        let profile_label = format!("{}={profile}", crate::network::LABEL_PROFILE);
        let universe_profile_label =
            format!("{}={universe_profile}", crate::network::LABEL_UNIVERSE_PROFILE);
        let mut args = vec!["create", "--pull=never"];
        if universe_profile == crate::network::UNIVERSE_PROFILE_NESTED {
            args.push("--privileged");
        }
        // The managed profile: one stable address allocated to the universe UUID from this host's
        // pool, on the host's bridge. The isolated profile: no network, as every lot before had it.
        let managed = if profile == crate::network::PROFILE_MANAGED {
            Some(db.allocate_network(uuid, id, address)?)
        } else {
            None
        };
        let (network_arg, ip_label, network_label);
        match managed.as_ref() {
            Some((bridge, ip, network_uuid)) => {
                network_arg = format!("--network={bridge}");
                ip_label = format!("{}={ip}", crate::network::LABEL_IP);
                network_label = format!("{}={network_uuid}", crate::network::LABEL_NETWORK);
                args.extend([
                    "--network",
                    bridge,
                    "--ip",
                    ip,
                    "--label",
                    &ip_label,
                    "--label",
                    &network_label,
                ]);
                let _ = &network_arg;
            }
            None => args.push("--network=none"),
        }
        args.extend([
            "--name",
            name,
            "--label",
            &label,
            "--label",
            &provenance,
            "--label",
            &profile_label,
            "--label",
            &universe_profile_label,
        ]);
        for a in &secrets.0 {
            args.push(a.as_str());
        }
        if let Some(l) = &secrets.1 {
            args.extend(["--label", l.as_str()]);
        }
        // Claimed before anything is made: refused while another universe holds the name.
        let host_state = host_state
            .map(|n| crate::manager::claim_host_state(n, uuid))
            .transpose()?;
        if let Some((mounts, label)) = &host_state {
            args.extend(mounts.iter().map(String::as_str));
            args.extend(["--label", label.as_str()]);
        }
        args.push(image);
        args.extend(command);
        if let Err(e) = podman(QUICK, &args) {
            // A managed create that did not produce a container releases its address; nothing is kept
            // that Podman does not carry.
            if managed.is_some() {
                let _ = db.release_network(uuid, id);
            }
            if let Some(name) = host_state_name {
                let _ = crate::manager::release_host_state(name);
            }
            return Err(e);
        }
    }
    let c = inspect(name)?.ok_or("Created container not observable")?;
    let network = crate::network::of_container(&c);
    if profile == crate::network::PROFILE_MANAGED {
        let requested = network["requested"]["ip"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let bridge = c["Config"]["Labels"][crate::network::LABEL_PROFILE]
            .as_str()
            .map(|_| crate::network::BRIDGE)
            .unwrap_or("");
        // Before a start the address is not up, and Podman records a created container's static
        // address only in the command it will run; after a start it is in the network settings.
        // Both are read back from Podman, never from this service's own allocation table, and a
        // container that carries neither is removed and its address released: nothing is kept that
        // Podman does not carry.
        let effective = network["effective"].as_array().is_some_and(|v| {
            v.iter()
                .any(|n| n["ip"].as_str() == Some(requested.as_str()))
        });
        let create_command: Vec<&str> = c["Config"]["CreateCommand"]
            .as_array()
            .map(|v| v.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let carried = create_command
            .windows(2)
            .any(|w| w[0] == "--ip" && w[1] == requested)
            && c["NetworkSettings"]["Networks"]
                .as_object()
                .is_some_and(|m| m.contains_key(bridge));
        if !effective && !carried {
            let _ = podman(QUICK, &["rm", name]);
            let _ = db.release_network(uuid, id);
            return Err(format!("the container does not carry the allocated address {requested}; it was removed and the address released").into());
        }
    }
    Ok(
        json!({"status":"verified","state":c["State"]["Status"],"universe_uuid":uuid,"universe_profile":universe_profile,"container_id":c["Id"],"network":network,"started":false}),
    )
}
fn delete(
    db: &mut dyn LifecycleDb,
    uuid: &str,
    name: &str,
    existing: Option<Value>,
) -> Result<Value, Error> {
    if let Some(c) = existing {
        db.owned(&c, uuid, "Target")?;
        stopped(&c).map_err(|e| format!("Refusing to delete: {e}; stop it explicitly first"))?;
        // Do not force removal and do not remove volumes.
        podman(QUICK, &["rm", name])?;
        if inspect(name)?.is_some() {
            return Err("Container still present after removal".into());
        }
        // A manager replica's host state is released once its universe is gone: renamed, never
        // removed, for the next create of the name (V3-5).
        if let Some(state) = c["Config"]["Labels"][crate::manager::LABEL_HOST_STATE].as_str() {
            crate::manager::release_host_state(state).map_err(|e| {
                format!("the universe was removed, and its host state {state} was not released ({e}); nothing will claim it until the operator renames it")
            })?;
        }
    }
    if inspect(name)?.is_some() {
        return Err("Container still present after removal".into());
    }
    let (removed, retained) = db.remove_snapshots(uuid)?;
    // A restored or promoted universe's imported image goes the same way, once nothing uses it.
    let (restore_removed, restore_retained) = db.remove_restore_images(uuid)?;
    // A managed universe's address is released once its container is gone: bound to the UUID
    // while the universe exists here, free afterwards, recorded either way.
    let released_ip = db.release_network(uuid, "delete")?;
    Ok(
        json!({"status":"verified","universe_uuid":uuid,"absent":true,"volumes":"retained","snapshot_images_removed":removed,"snapshot_images_retained":retained,
               "restore_images_removed":restore_removed,"restore_images_retained":restore_retained,"network_address_released":released_ip}),
    )
}
/// Start time of the earliest earlier attempt of this operation, if an attempt was interrupted
/// or failed. A retry must not repeat a start or stop that may already have happened.
fn earlier_attempt(db: &Connection, id: &str, attempt: i64) -> Result<Option<i64>, Error> {
    Ok(db.query_row(
        "SELECT MIN(started_at) FROM operation_attempts WHERE operation_id=?1 AND id<?2",
        params![id, attempt],
        |r| r.get(0),
    )?)
}
fn start_result(c: &Value, uuid: &str, action: &str, observe_seconds: u64, note: &str) -> Value {
    let mut result = state_view(c);
    let running = result["running"] == true;
    result["status"] = json!("verified");
    result["operation"] = json!("start");
    result["universe_uuid"] = json!(uuid);
    result["action"] = json!(action);
    result["observed_state"] = c["State"]["Status"].clone();
    result["observation_seconds"] = json!(observe_seconds);
    result["application_outcome"] = json!(if running {
        "running_when_observed"
    } else {
        "not_running_when_observed"
    });
    result["note"] = json!(note);
    result
}
fn start(
    db: &mut dyn LifecycleDb,
    attempt: i64,
    id: &str,
    uuid: &str,
    name: &str,
    existing: Option<Value>,
    observe_seconds: u64,
) -> Result<Value, Error> {
    let c = existing.ok_or("Universe container not found")?;
    db.owned(&c, uuid, "Target")?;
    let started_at = c["State"]["StartedAt"].as_str().unwrap_or("").to_string();
    if let Some(first) = db.earlier_attempt(id, attempt)? {
        match epoch(&started_at) {
            Some(t) if t >= first => {
                return Ok(start_result(&c, uuid, "none_start_observed_since_first_attempt", 0,
                    "an earlier attempt of this operation did not complete and the container has started since it began; not started again"));
            }
            Some(_) => {}
            None => {
                return Err(failure(
                    "Cannot compare the container start time with the earlier attempt; not starting again. Inspect the universe and use a new operation ID.",
                    json!({"observed": state_view(&c)}),
                ))
            }
        }
    }
    let state = status(&c);
    if state == "running" {
        return Ok(start_result(
            &c,
            uuid,
            "none_already_running",
            0,
            "the container was already running; no start was issued",
        ));
    }
    if !STOPPED.contains(&state) {
        return Err(failure(
            format!("Container state {state} is outside the start contract"),
            json!({"observed": state_view(&c)}),
        ));
    }
    if let Err(e) = podman(QUICK, &["start", name]) {
        let observed = inspect(name)?.map(|c| {
            let mut view = state_view(&c);
            view["runtime_error"] = c["State"]["Error"].clone();
            view
        });
        return Err(failure(
            format!("Start failed: {e}"),
            json!({"observed": observed}),
        ));
    }
    // The application may exit at once. Report only what is observed within the window.
    let deadline = Instant::now() + Duration::from_secs(observe_seconds);
    let c = loop {
        let c = inspect(name)?.ok_or("Container disappeared after start")?;
        if c["State"]["StartedAt"].as_str() == Some(started_at.as_str()) {
            return Err(failure(
                "Podman reported success but no new start was observed",
                json!({"observed": state_view(&c)}),
            ));
        }
        if status(&c) != "running" || Instant::now() >= deadline {
            break c;
        }
        thread::sleep(POLL);
    };
    let note = if status(&c) == "running" {
        "running when observed at the end of the observation window; later exits are not tracked by this result"
    } else {
        "the application exited during the observation window"
    };
    let mut result = start_result(&c, uuid, "started", observe_seconds, note);
    // Starting a released universe begins its application afresh. Say so: the caller chose this over
    // migration_restore_local, which is the operation that resumes the checkpointed memory.
    if let Some(state) = db.reservation_state(uuid)? {
        result["memory_restored"] = json!(false);
        result["memory_note"] = json!(format!(
            "this is an ordinary start of a universe whose reservation is {}: the checkpointed memory was not restored and the application began afresh; migration_restore_local resumes it instead",
            state
        ));
    }
    Ok(result)
}
fn host_memory_bytes() -> Result<u64, Error> {
    let info = std::fs::read_to_string("/proc/meminfo")?;
    let kib: u64 = info
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .ok_or("MemTotal not found in /proc/meminfo")?;
    Ok(kib * 1024)
}

fn host_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

fn state_result(c: &Value, uuid: &str, operation: &str, action: &str, note: &str) -> Value {
    let mut result = state_view(c);
    result["status"] = json!("verified");
    result["operation"] = json!(operation);
    result["universe_uuid"] = json!(uuid);
    result["action"] = json!(action);
    result["observed_state"] = c["State"]["Status"].clone();
    result["note"] = json!(note);
    result
}

/// Freeze a running universe. Its processes stop being scheduled, its memory and its network
/// address stay; Podman reports `paused`. A paused universe is already stopped for every purpose
/// of a second writer, so this is never gated by the lease, and it is idempotent by state.
fn pause(
    db: &mut dyn LifecycleDb,
    uuid: &str,
    name: &str,
    existing: Option<Value>,
) -> Result<Value, Error> {
    let c = existing.ok_or("Universe container not found")?;
    db.owned(&c, uuid, "Target")?;
    let state = status(&c);
    if state == "paused" {
        return Ok(state_result(
            &c,
            uuid,
            "pause",
            "none_already_paused",
            "the universe was already paused; nothing was sent",
        ));
    }
    if state != "running" {
        return Err(failure(format!("Container state {state} is outside the pause contract: only a running universe is paused"), json!({"observed": state_view(&c)})));
    }
    if let Err(e) = podman(QUICK, &["pause", name]) {
        let observed = inspect(name)?.map(|c| state_view(&c));
        return Err(failure(
            format!("Pause failed: {e}"),
            json!({"observed": observed}),
        ));
    }
    let c = inspect(name)?.ok_or("Container disappeared after pause")?;
    if status(&c) != "paused" {
        return Err(failure(
            "Podman reported success but the universe is not paused",
            json!({"observed": state_view(&c)}),
        ));
    }
    Ok(state_result(
        &c,
        uuid,
        "pause",
        "paused",
        "every process of the universe is frozen; its memory and its address stay; resume thaws it",
    ))
}

/// Thaw a paused universe. It leaves a running writer behind, so the caller passed the same gate
/// as `start`; a universe that is running already is left alone.
fn resume(
    db: &mut dyn LifecycleDb,
    uuid: &str,
    name: &str,
    existing: Option<Value>,
) -> Result<Value, Error> {
    let c = existing.ok_or("Universe container not found")?;
    db.owned(&c, uuid, "Target")?;
    let state = status(&c);
    if state == "running" {
        return Ok(state_result(
            &c,
            uuid,
            "resume",
            "none_already_running",
            "the universe was running; nothing was sent",
        ));
    }
    if state != "paused" {
        return Err(failure(format!("Container state {state} is outside the resume contract: only a paused universe is resumed (a stopped one is started)"), json!({"observed": state_view(&c)})));
    }
    if let Err(e) = podman(QUICK, &["unpause", name]) {
        let observed = inspect(name)?.map(|c| state_view(&c));
        return Err(failure(
            format!("Resume failed: {e}"),
            json!({"observed": observed}),
        ));
    }
    let c = inspect(name)?.ok_or("Container disappeared after resume")?;
    if status(&c) != "running" {
        return Err(failure(
            "Podman reported success but the universe is not running",
            json!({"observed": state_view(&c)}),
        ));
    }
    Ok(state_result(
        &c,
        uuid,
        "resume",
        "resumed",
        "the universe's processes run again from where they were frozen",
    ))
}

/// What the kernel enforces for this container right now: its cgroup's memory.max and cpu.max,
/// read from the unified hierarchy. `None` when the container has no cgroup (not running).
fn cgroup_limits(c: &Value) -> Option<Value> {
    let path = c["State"]["CgroupPath"]
        .as_str()
        .filter(|p| !p.is_empty())?;
    let dir = std::path::Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/'));
    let read = |f: &str| {
        std::fs::read_to_string(dir.join(f))
            .ok()
            .map(|s| s.trim().to_string())
    };
    let memory_max = read("memory.max")?;
    let cpu_max = read("cpu.max")?;
    // cpu.max is "<quota> <period>" in microseconds, or "max <period>".
    let cpus = {
        let mut it = cpu_max.split_whitespace();
        match (it.next(), it.next().and_then(|p| p.parse::<f64>().ok())) {
            (Some("max"), _) => None,
            (Some(q), Some(period)) => q
                .parse::<f64>()
                .ok()
                .map(|q| (q / period * 100.0).round() / 100.0),
            _ => None,
        }
    };
    Some(json!({
        "cgroup": path,
        "memory_max_bytes": memory_max.parse::<u64>().ok(),
        "memory_unlimited": memory_max == "max",
        "cpu_max": cpu_max,
        "cpus": cpus,
    }))
}

/// Set the memory limit and the CPU allowance of a universe (`podman update`). On a running
/// universe the kernel applies them to its cgroup at once, and the result reads them back from
/// there; on a stopped one they are kept for its next start and read back from Podman's record.
fn resources(
    db: &mut dyn LifecycleDb,
    uuid: &str,
    name: &str,
    existing: Option<Value>,
    memory_bytes: Option<u64>,
    cpus: Option<f64>,
) -> Result<Value, Error> {
    let c = existing.ok_or("Universe container not found")?;
    db.owned(&c, uuid, "Target")?;
    let state = status(&c);
    if !(state == "running" || state == "paused" || STOPPED.contains(&state)) {
        return Err(failure(
            format!("Container state {state} is outside the resources contract"),
            json!({"observed": state_view(&c)}),
        ));
    }
    let mut args: Vec<String> = vec!["update".into()];
    if let Some(m) = memory_bytes {
        // The same value for swap: a limit on memory alone lets the universe spill into swap
        // instead of being limited.
        args.push("--memory".into());
        args.push(m.to_string());
        args.push("--memory-swap".into());
        args.push(m.to_string());
    }
    if let Some(cp) = cpus {
        args.push("--cpus".into());
        args.push(format!("{cp}"));
    }
    args.push(name.into());
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    if let Err(e) = podman(QUICK, &argv) {
        let observed = inspect(name)?.map(|c| state_view(&c));
        return Err(failure(
            format!("Resources update failed: {e}"),
            json!({"observed": observed}),
        ));
    }
    let c = inspect(name)?.ok_or("Container disappeared after the update")?;
    let recorded_memory = c["HostConfig"]["Memory"].as_u64().unwrap_or(0);
    let recorded_nano = c["HostConfig"]["NanoCpus"].as_u64().unwrap_or(0);
    let recorded_cpus = (recorded_nano as f64 / 1e7).round() / 100.0;
    let memory_agrees = memory_bytes.is_none_or(|m| recorded_memory == m);
    let cpus_agree = cpus.is_none_or(|cp| (recorded_cpus - cp).abs() <= 0.011);
    // Measured on Podman 5.4.2: on a running, paused or never-started universe the record follows
    // the update at once; on an exited one the record shown by inspect keeps the previous values
    // while the next start applies the new ones. So a disagreement is a failure everywhere but on
    // an exited universe, where the verification is deferred to that start and said to be.
    let exited = !(state == "running" || state == "paused" || state == "created");
    if !exited {
        if !memory_agrees {
            return Err(failure(
                "Podman reported success but records another memory limit",
                json!({"requested": memory_bytes, "recorded": recorded_memory}),
            ));
        }
        if !cpus_agree {
            return Err(failure(
                "Podman reported success but records another CPU allowance",
                json!({"requested": cpus, "recorded": recorded_cpus}),
            ));
        }
    }
    let kernel = cgroup_limits(&c);
    if let Some(k) = kernel.as_ref() {
        if let Some(m) = memory_bytes {
            if k["memory_max_bytes"].as_u64() != Some(m) {
                return Err(failure(
                    "The kernel does not enforce the requested memory limit",
                    json!({"requested": m, "kernel": k}),
                ));
            }
        }
        if let Some(cp) = cpus {
            if k["cpus"].as_f64().is_none_or(|v| (v - cp).abs() > 0.011) {
                return Err(failure(
                    "The kernel does not enforce the requested CPU allowance",
                    json!({"requested": cp, "kernel": k}),
                ));
            }
        }
    }
    let verification = if kernel.is_some() {
        "kernel"
    } else if memory_agrees && cpus_agree {
        "recorded"
    } else {
        "deferred"
    };
    let mut result = state_result(&c, uuid, "resources", "updated", match verification {
        "kernel" => "applied to the running universe's cgroup and read back from the kernel",
        "recorded" => "recorded by Podman for the universe's next start; nothing runs to apply it to now",
        _ => "accepted by Podman for the universe's next start, which applies it; Podman's record of an exited universe shows the previous values until then (measured on 5.4.2), so this is not verified here",
    });
    result["verification"] = json!(verification);
    result["requested"] = json!({"memory_bytes": memory_bytes, "cpus": cpus});
    result["recorded"] = json!({"memory_bytes": recorded_memory, "memory_unlimited": recorded_memory == 0, "cpus": if recorded_nano == 0 { Value::Null } else { json!(recorded_cpus) }});
    result["kernel"] = kernel.unwrap_or(Value::Null);
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn stop_result(
    c: &Value,
    uuid: &str,
    action: &str,
    timeout: u64,
    on_timeout: &str,
    signal: Value,
    forced: bool,
    elapsed_ms: u128,
) -> Value {
    let mut result = state_view(c);
    result["status"] = json!("verified");
    result["operation"] = json!("stop");
    result["universe_uuid"] = json!(uuid);
    result["action"] = json!(action);
    result["observed_state"] = c["State"]["Status"].clone();
    result["stop_signal"] = signal;
    result["graceful_timeout_seconds"] = json!(timeout);
    result["on_timeout"] = json!(on_timeout);
    result["forced"] = json!(forced);
    result["forced_evidence"] = json!(match (action, on_timeout, forced) {
        ("stopped", "kill", true) =>
            "podman reported escalation to SIGKILL after the graceful timeout",
        ("stopped", "kill", false) => "no escalation reported by podman",
        ("stopped", _, _) => "only the stop signal was sent",
        _ => "no signal was sent",
    });
    result["elapsed_ms"] = json!(elapsed_ms);
    result["data"] = json!("stop does not remove the container, its filesystem or volumes");
    result
}
#[allow(clippy::too_many_arguments)]
fn stop(
    db: &mut dyn LifecycleDb,
    attempt: i64,
    id: &str,
    uuid: &str,
    name: &str,
    existing: Option<Value>,
    timeout: u64,
    on_timeout: &str,
) -> Result<Value, Error> {
    let c = existing.ok_or("Universe container not found")?;
    db.owned(&c, uuid, "Target")?;
    let state = status(&c);
    if STOPPED.contains(&state) {
        return Ok(stop_result(
            &c,
            uuid,
            "none_already_stopped",
            timeout,
            on_timeout,
            Value::Null,
            false,
            0,
        ));
    }
    // A container left 'stopping' by an interrupted stop may be stopped again to complete it.
    if state != "running" && state != "stopping" {
        return Err(failure(
            format!("Container state {state} is outside the stop contract"),
            json!({"observed": state_view(&c)}),
        ));
    }
    let started_at = c["State"]["StartedAt"].as_str().unwrap_or("").to_string();
    if let Some(first) = db.earlier_attempt(id, attempt)? {
        match epoch(&started_at) {
            Some(t) if t < first => {}
            _ => {
                return Err(failure(
                    "The container may have been started after this stop operation was first attempted; retrying would stop a newer run. Use a new operation ID.",
                    json!({"observed": state_view(&c)}),
                ))
            }
        }
    }
    let signal = c["Config"]["StopSignal"]
        .as_str()
        .unwrap_or("SIGTERM")
        .to_string();
    let begin = Instant::now();
    let mut forced = false;
    if on_timeout == "kill" {
        // podman stop sends the stop signal, waits the declared period, then sends SIGKILL.
        let limit = timeout + STOP_MARGIN_SECONDS;
        let out = run_podman(limit, &["stop", "--time", &timeout.to_string(), name])?;
        // Podman reports its escalation only as a warning on stderr.
        forced = String::from_utf8_lossy(&out.stderr).contains("resorting to SIGKILL");
        if !out.status.success() {
            let observed = inspect(name)?.map(|c| state_view(&c));
            return Err(failure(
                format!("Stop failed: {}", podman_error(limit, &["stop"], &out)),
                json!({"observed": observed, "escalation_reported": forced}),
            ));
        }
    } else {
        // Send only the stop signal; never escalate.
        if let Err(e) = podman(QUICK, &["kill", "--signal", &signal, name]) {
            let observed = inspect(name)?.map(|c| state_view(&c));
            return Err(failure(
                format!("Stop signal failed: {e}"),
                json!({"observed": observed, "forced": false}),
            ));
        }
        let deadline = begin + Duration::from_secs(timeout);
        loop {
            let now = inspect(name)?.ok_or("Container disappeared during stop")?;
            if !process_active(&now) {
                break;
            }
            if Instant::now() >= deadline {
                return Err(failure(
                    format!("Graceful stop did not complete within {timeout} s; container left running as requested by on_timeout=leave_running"),
                    json!({"observed": state_view(&now), "stop_signal": signal, "graceful_timeout_seconds": timeout, "forced": false}),
                ));
            }
            thread::sleep(POLL);
        }
    }
    let after = inspect(name)?.ok_or("Container disappeared during stop")?;
    if process_active(&after) || after["State"]["StartedAt"].as_str() != Some(started_at.as_str()) {
        return Err(failure(
            "The container is running or was restarted after stop",
            json!({"observed": state_view(&after), "forced": forced}),
        ));
    }
    Ok(stop_result(
        &after,
        uuid,
        "stopped",
        timeout,
        on_timeout,
        json!(signal),
        forced,
        begin.elapsed().as_millis(),
    ))
}
/// Clone a stopped, mount-free universe: commit its root filesystem into a snapshot image
/// tagged by operation ID, then create a new network-disabled container from that image.
/// A retry reuses a snapshot committed by an interrupted attempt of the same operation.
fn clone(
    db: &mut dyn LifecycleDb,
    id: &str,
    uuid: &str,
    source: &str,
    name: &str,
    existing: Option<Value>,
) -> Result<Value, Error> {
    let reference = format!("{SNAPSHOT_REPOSITORY}{id}");
    let mut reused = false;
    if let Some(ref c) = existing {
        if label(c, CREATION) != Some(id) {
            return Err("Clone target already belongs to another operation".into());
        }
    } else {
        let prior = images()?
            .into_iter()
            .find(|i| names(i).contains(&reference.as_str()));
        let image = if let Some(snapshot) = prior {
            let l = &snapshot["Labels"];
            if l[SNAPSHOT_FOR].as_str() != Some(uuid)
                || l[SNAPSHOT_OPERATION].as_str() != Some(id)
                || l[UNIVERSE].as_str() != Some(source)
            {
                return Err(
                    "Snapshot reference exists with different provenance; refusing to reuse it"
                        .into(),
                );
            }
            reused = true;
            image_id(&snapshot).to_string()
        } else {
            let source_name = format!("podmesh-{source}");
            let before = inspect(&source_name)?.ok_or("Clone source not found")?;
            if label(&before, UNIVERSE) != Some(source) {
                return Err("Clone source is not managed by PodMesh".into());
            }
            db.owned(&before, source, "Clone source")?;
            db.refuse_reserved(source, "clone from this source")?;
            stopped(&before).map_err(|e| {
                format!("Clone source must be stopped for a coherent filesystem snapshot: {e}")
            })?;
            if before["Mounts"]
                .as_array()
                .map(|m| !m.is_empty())
                .unwrap_or(true)
            {
                return Err("Volume and bind mount cloning is not supported yet".into());
            }
            let source_id = before["Id"]
                .as_str()
                .ok_or("Missing source container identity")?;
            let changes = [
                format!("LABEL {SNAPSHOT_FOR}={uuid}"),
                format!("LABEL {SNAPSHOT_OPERATION}={id}"),
                format!("LABEL {SNAPSHOT_SOURCE}={source_id}"),
            ];
            // Never pause: a source started concurrently is detected below, not frozen.
            empty_scratch()?;
            podman(
                COMMIT,
                &[
                    "commit",
                    "--pause=false",
                    "--change",
                    &changes[0],
                    "--change",
                    &changes[1],
                    "--change",
                    &changes[2],
                    &source_name,
                    &reference,
                ],
            )?;
            let after = inspect(&source_name)?;
            let unchanged = after.as_ref().is_some_and(|a| {
                a["Id"] == before["Id"]
                    && a["State"]["StartedAt"] == before["State"]["StartedAt"]
                    && stopped(a).is_ok()
            });
            if !unchanged {
                let _ = podman(QUICK, &["image", "rm", &reference]);
                return Err(
                    "Clone source was started or replaced during the snapshot; snapshot discarded"
                        .into(),
                );
            }
            let snapshot = images()?
                .into_iter()
                .find(|i| names(i).contains(&reference.as_str()))
                .ok_or("Snapshot image not observable after commit")?;
            image_id(&snapshot).to_string()
        };
        let label = format!("{UNIVERSE}={uuid}");
        let provenance = format!("{CREATION}={id}");
        // A clone does not carry the managed profile yet: it is created isolated, labelled so, and its
        // answer says so (docs/UNIVERSE-NETWORK-CONTRACT.md).
        let profile_label = format!(
            "{}={}",
            crate::network::LABEL_PROFILE,
            crate::network::PROFILE_ISOLATED
        );
        podman(
            QUICK,
            &[
                "create",
                "--pull=never",
                "--network=none",
                "--name",
                name,
                "--label",
                &label,
                "--label",
                &provenance,
                "--label",
                &profile_label,
                &image,
            ],
        )?;
    }
    // Verify the observed clone, whether created now or by an interrupted attempt.
    let c = inspect(name)?.ok_or("Clone not observable")?;
    let image = c["Image"]
        .as_str()
        .unwrap_or("")
        .trim_start_matches("sha256:")
        .to_string();
    let snapshot = images()?
        .into_iter()
        .find(|i| image_id(i) == image)
        .ok_or("Clone snapshot image not observable")?;
    let l = &snapshot["Labels"];
    if label(&c, CREATION) != Some(id)
        || l[SNAPSHOT_FOR].as_str() != Some(uuid)
        || l[SNAPSHOT_OPERATION].as_str() != Some(id)
        || l[UNIVERSE].as_str() != Some(source)
    {
        return Err("Clone provenance does not match the operation".into());
    }
    if c["HostConfig"]["NetworkMode"].as_str() != Some("none")
        || c["Mounts"]
            .as_array()
            .map(|m| !m.is_empty())
            .unwrap_or(true)
    {
        return Err("Clone configuration does not match the supported scope".into());
    }
    Ok(
        json!({"status":"verified","universe_uuid":uuid,"source_uuid":source,"container_id":c["Id"],"state":c["State"]["Status"],
        "source_container_id":l[SNAPSHOT_SOURCE],"snapshot_image":image,"snapshot_reference":reference,"snapshot_reused":reused,
        "started":false,"network":"isolated (a clone does not carry the managed profile yet)","scope":"stopped container root filesystem; no volumes or bind mounts"}),
    )
}
/// A snapshot image may be removed for a universe only if this host's journal records the
/// clone operation named by its labels, for this universe and its source, and, once that
/// clone was verified, the same image and source container.
fn snapshot_recorded(db: &Connection, uuid: &str, i: &Value) -> Result<bool, Error> {
    let l = &i["Labels"];
    let Some(operation) = l[SNAPSHOT_OPERATION].as_str() else {
        return Ok(false);
    };
    let row: Option<(String, String, Option<String>)> = db
        .query_row(
            "SELECT request,status,result FROM operations WHERE id=?1",
            [operation],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((request, status, result)) = row else {
        return Ok(false);
    };
    let request: Value = serde_json::from_str(&request)?;
    if request["operation"] != "clone"
        || request["universe_uuid"].as_str() != Some(uuid)
        || request["source_uuid"].as_str() != l[UNIVERSE].as_str()
    {
        return Ok(false);
    }
    if status != "verified" {
        // Left by an interrupted or failed attempt of this recorded clone operation.
        return Ok(true);
    }
    let result: Value = serde_json::from_str(result.as_deref().unwrap_or("null"))?;
    Ok(result["snapshot_image"].as_str() == Some(image_id(i))
        && result["source_container_id"] == l[SNAPSHOT_SOURCE])
}
/// Remove snapshot images committed for this universe. Never forced: an image still used
/// by a container or by a dependent image, or without journal provenance, is retained and reported.
fn remove_snapshots(db: &Connection, uuid: &str) -> Result<(Vec<String>, Vec<Value>), Error> {
    let (mut removed, mut retained) = (vec![], vec![]);
    for i in images()?
        .iter()
        .filter(|i| i["Labels"][SNAPSHOT_FOR].as_str() == Some(uuid))
    {
        let id = image_id(i).to_string();
        let tags = names(i);
        if tags.iter().any(|n| !n.starts_with(SNAPSHOT_REPOSITORY)) {
            retained.push(json!({"image":id,"reason":"image has names outside the PodMesh snapshot repository"}));
            continue;
        }
        if !snapshot_recorded(db, uuid, i)? {
            retained.push(json!({"image":id,"reason":"snapshot provenance does not match this host's journal"}));
            continue;
        }
        let targets = if tags.is_empty() {
            vec![id.as_str()]
        } else {
            tags
        };
        let mut args = vec!["image", "rm"];
        args.extend(targets);
        match podman(QUICK, &args) {
            Ok(_) => removed.push(id),
            Err(e) => retained.push(json!({"image":id,"reason":e.to_string()})),
        }
    }
    // Removing a tag succeeds even when a dependent image keeps the image; report what remains.
    let remaining = images()?;
    removed.retain(|id| {
        let present = remaining.iter().any(|i| image_id(i) == id.as_str());
        if present {
            retained.push(json!({"image":id,"reason":"untagged but still present; a dependent image or container uses it"}));
        }
        !present
    });
    Ok((removed, retained))
}

#[cfg(test)]
mod tests {
    use super::{epoch, is_uuid};
    #[test]
    fn podman_timestamps() {
        assert_eq!(epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(epoch("2024-02-29T12:00:00Z"), Some(1_709_208_000));
        // Observed on the lab: StartedAt of a container whose start event had time 1789150856.
        assert_eq!(epoch("2026-09-11T18:20:56.109581375Z"), Some(1_789_150_856));
        assert_eq!(epoch("2026-09-11T18:20:56+02:00"), Some(1_789_143_656));
        assert_eq!(
            epoch("2026-10-07T20:24:33.526747397+02:00"),
            epoch("2026-10-07T20:24:33+02:00"),
        );
        assert!(epoch("0001-01-01T00:00:00Z").is_some_and(|t| t < 0));
    }
    #[test]
    fn uuids() {
        assert!(is_uuid("16926159-bf59-4537-8f6e-5cfea52540ea"));
        assert!(!is_uuid("16926159-bf59-4537-8f6e-5cfea52540e"));
        assert!(!is_uuid("16926159xbf59-4537-8f6e-5cfea52540ea"));
    }

    #[test]
    fn universe_profile_parse() {
        use super::parse_universe_profile;
        use crate::network::{PROFILE_ISOLATED, PROFILE_MANAGED, UNIVERSE_PROFILE_FLAT, UNIVERSE_PROFILE_NESTED};
        use serde_json::json;

        assert_eq!(
            parse_universe_profile(&json!({}), PROFILE_ISOLATED).unwrap(),
            UNIVERSE_PROFILE_FLAT
        );
        assert_eq!(
            parse_universe_profile(
                &json!({"universe_profile": "nested"}),
                PROFILE_ISOLATED
            )
            .unwrap(),
            UNIVERSE_PROFILE_NESTED
        );
        assert!(parse_universe_profile(
            &json!({"universe_profile": "nested"}),
            PROFILE_MANAGED
        )
        .is_err());
    }

    #[cfg(feature = "mariadb")]
    #[test]
    fn mariadb_lifecycle_write_round_trips_when_a_server_is_named() {
        use crate::store::config::DSN_ENVIRONMENT;
        use crate::store::{
            migrations, DurableStore, MariadbConfig, MariadbStore, Value as Stored,
        };
        use serde_json::json;

        let _serialized = crate::store::MARIADB_TEST_SERVER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(config) = MariadbConfig::from_environment() else {
            eprintln!("skipped: {DSN_ENVIRONMENT} names no MariaDB server");
            return;
        };
        let mut store = MariadbStore::open(&config).unwrap_or_else(|error| {
            panic!(
                "{DSN_ENVIRONMENT} names {}, which did not open: {error}",
                config.described()
            )
        });
        migrations::apply(&mut store).unwrap();
        super::prepare_scratch(
            &std::env::temp_dir().join(format!(
                "podmesh-mariadb-lifecycle-test-{}",
                std::process::id()
            )),
        )
        .unwrap();

        let operation_id = "mariadb-lifecycle-delete";
        store
            .execute(
                "DELETE FROM operation_attempts WHERE operation_id = ?",
                &[Stored::from(operation_id)],
            )
            .unwrap();
        store
            .execute(
                "DELETE FROM operations WHERE id = ?",
                &[Stored::from(operation_id)],
            )
            .unwrap();
        let request = json!({
            "operation": "delete",
            "operation_id": operation_id,
            "universe_uuid": "731b538e-8f8a-4f8d-a234-29df4d993c15",
            "authorization_ref": "mariadb lifecycle test"
        });
        let written = super::execute_store(&mut store, &request).unwrap();
        assert_eq!(written["absent"], true);
        let row = store
            .query_one(
                "SELECT status, result FROM operations WHERE id = ?",
                &[Stored::from(operation_id)],
            )
            .unwrap()
            .expect("the operation is durable");
        assert_eq!(row.text(0).unwrap(), "verified");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(row.text(1).unwrap()).unwrap()["absent"],
            true
        );
        let replayed = super::execute_store(&mut store, &request).unwrap();
        assert_eq!(replayed["replayed"], true);

        store
            .execute(
                "DELETE FROM operation_attempts WHERE operation_id = ?",
                &[Stored::from(operation_id)],
            )
            .unwrap();
        store
            .execute(
                "DELETE FROM operations WHERE id = ?",
                &[Stored::from(operation_id)],
            )
            .unwrap();
    }
}
