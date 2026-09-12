//! Bounding a restore attempt, and reclaiming what a failed one leaves behind.
//!
//! A restore that fails after Podman started was observed on the laboratory to leave CRIU writing its
//! restore log at about 20 MB/s, with the `podman container restore` command never returning; stopping
//! the transient scope does not stop it, because the writers live in the container's own
//! `libpod-<id>.scope` and in its `libpod-conmon-<id>.scope`, not in the scope PodMesh created.
//!
//! Two bounded mechanisms follow from that measurement:
//!
//! * **Prevention** ([`Bound`]): while the command runs, the free space of the Podman graph root is
//!   watched. When the attempt has consumed more than the space its own preflight required, or when
//!   the graph root falls below an absolute floor, the container's cgroup is **frozen** (which stops
//!   the writing without ending anything) and the transient scope is stopped. Freezing is reversible
//!   and is undone if the frozen cgroup turns out not to belong to the attempt.
//! * **Reclaim** ([`reclaim`]): ending processes is never implicit. It happens only through
//!   `migration_restore_abort` with an explicit `reclaim_processes: true`, and only for a process that
//!   is a member of one of those two exact cgroups and whose start time is at or after the durable
//!   claim. A pidfd is opened before both facts are re-read and is the only signal transport, so numeric
//!   PID reuse cannot redirect the signal. A command line that merely names the container is a diagnostic
//!   fallback once the cgroups are gone; it never authorizes a signal.
use crate::lifecycle::{self as lc, Error};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs, io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::raw::{c_long, c_void},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

const MACHINE_SLICE: &str = "/sys/fs/cgroup/machine.slice";
/// `/proc/<pid>/stat` reports a start time in clock ticks of USER_HZ, fixed at 100 for this interface
/// independently of the kernel's CONFIG_HZ. Checked on the laboratory against the container's own
/// `Created` timestamp (probe `prevent-reclaim-sequence.json`).
const USER_HZ: u64 = 100;
/// An attempt is stopped if the graph root falls below this, whatever its own allowance is.
pub(crate) const FREE_FLOOR_BYTES: u64 = 1024 * 1024 * 1024;
/// No attempt is stopped before it has been allowed at least this much, so that a small archive on a
/// busy host cannot be stopped by another writer's noise.
const MIN_ALLOWANCE_BYTES: u64 = 256 * 1024 * 1024;
const WAIT_FOR_CGROUPS_SECONDS: u64 = 30;
/// A freeze or thaw is asynchronous; this bounds the wait for the kernel to report the new state.
const FREEZE_SECONDS: u64 = 10;
const SIGKILL: i32 = 9;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const SYS_PIDFD_SEND_SIGNAL: c_long = 424;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const SYS_PIDFD_OPEN: c_long = 434;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
extern "C" {
    fn syscall(number: c_long, ...) -> c_long;
}

/// An owned kernel reference to one process identity. Once opened, PID reuse cannot redirect a later
/// signal to another process.
struct PidFd(OwnedFd);

impl PidFd {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn open(pid: u32) -> io::Result<Self> {
        // SAFETY: pidfd_open takes a numeric PID and zero flags and returns either a new file descriptor
        // or -1 with errno. The successful descriptor is immediately transferred to OwnedFd.
        let fd = unsafe { syscall(SYS_PIDFD_OPEN, pid as i32, 0_u32) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful pidfd_open returns a new descriptor owned by this call.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd as i32) }))
    }

    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    fn open(_pid: u32) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pidfd_open ABI is supported by this build only on Linux x86_64",
        ))
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn send_signal(&self, signal: i32) -> io::Result<()> {
        // SAFETY: the descriptor is an owned pidfd, the signal is an integer value, siginfo is null as
        // permitted by pidfd_send_signal, and flags must be zero.
        let result = unsafe {
            syscall(
                SYS_PIDFD_SEND_SIGNAL,
                self.0.as_raw_fd(),
                signal,
                std::ptr::null::<c_void>(),
                0_u32,
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    fn send_signal(&self, _signal: i32) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pidfd_send_signal ABI is supported by this build only on Linux x86_64",
        ))
    }
}

pub(crate) fn container_scope(container_id: &str) -> PathBuf {
    PathBuf::from(format!("{MACHINE_SLICE}/libpod-{container_id}.scope"))
}
pub(crate) fn conmon_scope(container_id: &str) -> PathBuf {
    PathBuf::from(format!(
        "{MACHINE_SLICE}/libpod-conmon-{container_id}.scope"
    ))
}
fn is_container_id(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
/// Container IDs that own a cgroup under `machine.slice` right now, read from the kernel alone.
/// Podman itself does not answer while a restore is in flight (observed: `podman ps` did not return
/// within 15 s), so the attempt's container is identified from this set, never by asking Podman.
pub(crate) fn libpod_ids() -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let Ok(entries) = fs::read_dir(MACHINE_SLICE) else {
        return ids;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix("libpod-") else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(".scope") else {
            continue;
        };
        let id = rest.strip_prefix("conmon-").unwrap_or(rest);
        if is_container_id(id) {
            ids.insert(id.to_string());
        }
    }
    ids
}
fn boot_epoch() -> Option<u64> {
    fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| {
            l.strip_prefix("btime ")
                .and_then(|v| v.trim().parse::<u64>().ok())
        })
}
/// Seconds since the Unix epoch at which a process started, from `/proc/<pid>/stat`. `None` when the
/// process is gone or its start time cannot be read: such a process is never signalled.
pub(crate) fn start_epoch(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is parenthesised and may contain spaces; every numeric field follows it.
    let after = stat.rfind(')').map(|i| &stat[i + 1..])?;
    let ticks: u64 = after.split_whitespace().nth(19)?.parse().ok()?;
    Some(boot_epoch()? + ticks / USER_HZ)
}
fn read_trimmed(path: String) -> Option<String> {
    fs::read_to_string(path).ok().map(|v| v.trim().to_string())
}
fn comm(pid: u32) -> Option<String> {
    read_trimmed(format!("/proc/{pid}/comm"))
}
/// The cgroup a process is in right now, as the kernel reports it (cgroup v2: `0::<path>`).
fn live_cgroup(pid: u32) -> Option<String> {
    read_trimmed(format!("/proc/{pid}/cgroup"))
        .map(|v| v.rsplit("::").next().unwrap_or("").to_string())
}
fn cmdline(pid: u32) -> Option<String> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if raw.len() > 64 * 1024 {
        return None;
    }
    Some(String::from_utf8_lossy(&raw).replace('\0', " "))
}
fn environment_has(pid: u32, key: &str, expected: &str) -> Result<bool, String> {
    let path = format!("/proc/{pid}/environ");
    let raw = fs::read(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    if raw.len() > 256 * 1024 {
        return Err(format!("{path} exceeds the bounded environment size"));
    }
    let wanted = format!("{key}={expected}");
    Ok(raw
        .split(|byte| *byte == 0)
        .any(|entry| entry == wanted.as_bytes()))
}
fn command_has_pair(pid: u32, short: &str, long: &str, expected: &str) -> Result<bool, String> {
    let path = format!("/proc/{pid}/cmdline");
    let raw = fs::read(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    if raw.len() > 64 * 1024 {
        return Err(format!("{path} exceeds the bounded command-line size"));
    }
    let arguments: Vec<&[u8]> = raw
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .collect();
    let expected = expected.as_bytes();
    Ok(arguments.windows(2).any(|pair| {
        (pair[0] == short.as_bytes() || pair[0] == long.as_bytes()) && pair[1] == expected
    }) || arguments
        .iter()
        .any(|argument| argument.strip_prefix(format!("{long}=").as_bytes()) == Some(expected)))
}
fn exact_scope_or_descendant(observed: &str, scope: &str) -> bool {
    observed == scope
        || observed
            .strip_prefix(scope)
            .is_some_and(|suffix| suffix.starts_with('/'))
}
/// Every PID in a cgroup and its descendants, with the `cgroup.procs` file that listed it.
fn members(dir: &Path, out: &mut Vec<(u32, String)>) -> Result<(), String> {
    let procs = fs::read_to_string(dir.join("cgroup.procs"))
        .map_err(|e| format!("cannot read {}/cgroup.procs: {e}", dir.display()))?;
    for line in procs.lines() {
        let pid = line
            .trim()
            .parse::<u32>()
            .map_err(|e| format!("invalid PID in {}/cgroup.procs: {e}", dir.display()))?;
        if pid == 0 {
            return Err(format!("invalid PID 0 in {}/cgroup.procs", dir.display()));
        }
        out.push((pid, dir.display().to_string()));
    }
    let entries =
        fs::read_dir(dir).map_err(|e| format!("cannot enumerate {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read entry in {}: {e}", dir.display()))?;
        if entry
            .file_type()
            .map_err(|e| format!("cannot inspect {}: {e}", entry.path().display()))?
            .is_dir()
        {
            members(&entry.path(), out)?;
        }
    }
    Ok(())
}
/// Absence is a successful kernel lookup, not the default result of any I/O error.
fn scope_members(dir: &Path) -> Result<Option<Vec<(u32, String)>>, String> {
    match fs::metadata(dir) {
        Ok(metadata) if metadata.is_dir() => {
            let mut found = vec![];
            members(dir, &mut found)?;
            Ok(Some(found))
        }
        Ok(_) => Err(format!("{} is not a cgroup directory", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot inspect {}: {e}", dir.display())),
    }
}
fn process_view(pid: u32, cgroup_file: &str, claim: i64) -> Value {
    let start = start_epoch(pid);
    json!({
        "pid": pid, "comm": comm(pid), "program": cmdline(pid).map(|c| c.split_whitespace().next().unwrap_or("").to_string()),
        "cgroup": live_cgroup(pid), "cgroup_procs_file": cgroup_file, "start_epoch": start,
        "started_at_or_after_claim": start.map(|s| s as i64 >= claim),
    })
}

/// Processes of a failed restore, by cgroup residency: membership of the container's own
/// `libpod-<id>.scope` or of its `libpod-conmon-<id>.scope`, with each PID's start time compared to the
/// claim. Once both cgroups are gone, a command-line scan is reported as a diagnostic fallback that
/// authorizes nothing.
pub(crate) fn runtime_processes(container_id: &str, claim: i64) -> Value {
    if !is_container_id(container_id) {
        return json!({
            "observed_at": crate::now(), "container_id": container_id, "claim_created_at": claim,
            "known": false, "observation_errors": ["the container ID is not a 64-character lowercase hexadecimal ID"],
            "reason": "cgroup membership cannot be observed for an invalid container ID",
            "cgroups_known_absent": false, "source": "invalid_container_id",
            "authorizes_reclaim": false, "cgroups": [], "processes": [], "count": 0,
        });
    }
    runtime_processes_in(
        container_id,
        claim,
        [
            ("container", container_scope(container_id)),
            ("conmon", conmon_scope(container_id)),
        ],
    )
}
fn runtime_processes_in(container_id: &str, claim: i64, scopes: [(&str, PathBuf); 2]) -> Value {
    let mut cgroups = vec![];
    let mut found: Vec<(u32, String)> = vec![];
    let mut errors = vec![];
    for (role, dir) in scopes {
        match scope_members(&dir) {
            Ok(Some(here)) => {
                cgroups.push(json!({"role": role, "path": dir, "exists": true,
                    "member_pids": here.iter().map(|(p, _)| *p).collect::<Vec<_>>()}));
                found.extend(here);
            }
            Ok(None) => {
                cgroups.push(json!({"role": role, "path": dir, "exists": false, "member_pids": []}))
            }
            Err(error) => {
                cgroups.push(json!({"role": role, "path": dir, "exists": null, "member_pids": null, "error": error}));
                errors.push(error);
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    let any_cgroup = cgroups.iter().any(|c| c["exists"] == true);
    let processes: Vec<Value> = if any_cgroup || !errors.is_empty() {
        found
            .iter()
            .map(|(pid, file)| process_view(*pid, file, claim))
            .collect()
    } else {
        // Diagnostic only: a command line naming the container proves nothing about ownership.
        let mut scanned = vec![];
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|p| p.parse::<u32>().ok())
                else {
                    continue;
                };
                if cmdline(pid).is_some_and(|c| c.contains(container_id)) {
                    let mut view = process_view(pid, "", claim);
                    view["cgroup_procs_file"] = Value::Null;
                    view["authorizes_reclaim"] = json!(false);
                    scanned.push(view);
                }
                if scanned.len() >= 32 {
                    break;
                }
            }
        }
        scanned
    };
    json!({
        "observed_at": crate::now(), "container_id": container_id, "claim_created_at": claim,
        "known": errors.is_empty(), "observation_errors": errors,
        "reason": if errors.is_empty() { Value::Null } else { json!("cgroup membership could not be established; an unreadable scope is not empty") },
        "cgroups_known_absent": errors.is_empty() && !any_cgroup,
        "source": if any_cgroup { "cgroup_residency" } else { "cmdline_fallback" },
        "authorizes_reclaim": any_cgroup && errors.is_empty(),
        "cgroups": cgroups, "processes": processes, "count": processes.len(),
    })
}
#[cfg(test)]
pub(crate) fn runtime_processes_for_test(container: &Path, conmon: &Path) -> Value {
    runtime_processes_in(
        "test-container",
        0,
        [
            ("container", container.to_path_buf()),
            ("conmon", conmon.to_path_buf()),
        ],
    )
}
fn freeze_file(container_id: &str) -> PathBuf {
    container_scope(container_id).join("cgroup.freeze")
}
/// What a freeze or thaw request achieved. A request whose confirmation is still pending must never
/// read as one that never happened: the kernel confirms only once every task has actually stopped, and
/// a task blocked in the writing this bound exists to stop can take longer than the poll allows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Freeze {
    /// No freezer file: nothing was written and nothing is pending.
    Absent,
    /// The kernel reported the requested state.
    Confirmed,
    /// The write succeeded; the kernel had not confirmed it within the bound.
    Unconfirmed,
}
/// Freezes the container's own cgroup: the writing stops and nothing is ended. Reversible.
pub(crate) fn freeze(container_id: &str) -> Result<Freeze, Error> {
    set_freeze(container_id, true)
}
pub(crate) fn thaw(container_id: &str) -> Result<Freeze, Error> {
    set_freeze(container_id, false)
}
fn set_freeze(container_id: &str, freeze: bool) -> Result<Freeze, Error> {
    if !freeze_file(container_id).exists() {
        return Ok(Freeze::Absent);
    }
    fs::write(freeze_file(container_id), if freeze { b"1" } else { b"0" })?;
    let deadline = Instant::now() + Duration::from_secs(FREEZE_SECONDS);
    loop {
        if frozen(container_id) == freeze {
            return Ok(Freeze::Confirmed);
        }
        if !freeze_file(container_id).exists() {
            // The cgroup disappeared: there is nothing left to hold frozen.
            return Ok(Freeze::Absent);
        }
        if Instant::now() >= deadline {
            return Ok(Freeze::Unconfirmed);
        }
        thread::sleep(Duration::from_millis(50));
    }
}
pub(crate) fn frozen(container_id: &str) -> bool {
    fs::read_to_string(container_scope(container_id).join("cgroup.events"))
        .map(|e| e.lines().any(|l| l == "frozen 1"))
        .unwrap_or(false)
}
/// Waits, bounded, for both cgroups of a container to disappear. A cgroup that stays is a failed
/// reclaim, never a verified one.
fn wait_for_cgroups(container_id: &str) -> (bool, f64) {
    let begin = Instant::now();
    let deadline = begin + Duration::from_secs(WAIT_FOR_CGROUPS_SECONDS);
    loop {
        let gone = matches!(scope_members(&container_scope(container_id)), Ok(None))
            && matches!(scope_members(&conmon_scope(container_id)), Ok(None));
        if gone || Instant::now() >= deadline {
            return (gone, begin.elapsed().as_secs_f64());
        }
        thread::sleep(Duration::from_millis(200));
    }
}
#[derive(Debug)]
struct LiveIdentity {
    cgroup: Option<String>,
    start_epoch: Option<u64>,
    comm: Option<String>,
}

fn no_such_process(error: &io::Error) -> bool {
    error.raw_os_error() == Some(3)
}

/// Opens the immutable process handle before re-reading identity, then signals only through that handle.
/// The injected form keeps ordering and fail-closed behavior directly testable without sending a signal.
fn signal_candidate_with<T>(
    candidate: &Value,
    container_id: &str,
    claim: i64,
    open: impl FnOnce(u32) -> io::Result<T>,
    observe: impl FnOnce(u32) -> LiveIdentity,
    send: impl FnOnce(&T) -> io::Result<()>,
) -> Value {
    let pid = candidate["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid <= i32::MAX as u32)
        .unwrap_or(0);
    let mut entry = json!({
        "pid": pid,
        "comm": candidate["comm"],
        "cgroup_at_signal": Value::Null,
        "start_epoch_at_signal": Value::Null,
        "member_of_claimed_cgroup": false,
        "started_at_or_after_claim": false,
        "pidfd_opened": false,
        "signal_transport": "pidfd_send_signal",
        "numeric_pid_fallback": false,
        "signal_attempted": false,
    });
    if pid == 0 {
        entry["decision"] = json!("refused");
        entry["signal_outcome"] = json!("refused");
        entry["result"] = json!("signal refused: the candidate has no valid PID");
        return entry;
    }
    let handle = match open(pid) {
        Ok(handle) => handle,
        Err(error) if no_such_process(&error) => {
            entry["decision"] = json!("skipped");
            entry["signal_outcome"] = json!("already_gone");
            entry["result"] = json!("the process was already gone before pidfd_open");
            return entry;
        }
        Err(error) => {
            entry["decision"] = json!("refused");
            entry["signal_outcome"] = json!("refused");
            entry["result"] = json!(format!(
                "signal refused: pidfd_open is unavailable or failed: {error}"
            ));
            return entry;
        }
    };
    entry["pidfd_opened"] = json!(true);

    // These reads occur after pidfd_open. The descriptor pins the identity while cgroup and start time
    // are verified and while the later signal is issued.
    let fresh = observe(pid);
    let container_path = format!("/machine.slice/libpod-{container_id}.scope");
    let conmon_path = format!("/machine.slice/libpod-conmon-{container_id}.scope");
    let in_scope = fresh.cgroup.as_deref().is_some_and(|c| {
        exact_scope_or_descendant(c, &container_path) || exact_scope_or_descendant(c, &conmon_path)
    });
    let recent = fresh.start_epoch.is_some_and(|s| s as i64 >= claim);
    entry["comm"] = json!(fresh.comm);
    entry["cgroup_at_signal"] = json!(fresh.cgroup);
    entry["start_epoch_at_signal"] = json!(fresh.start_epoch);
    entry["member_of_claimed_cgroup"] = json!(in_scope);
    entry["started_at_or_after_claim"] = json!(recent);
    let refusal = if fresh.start_epoch.is_none() {
        Some("the process identity could not be re-read after pidfd_open")
    } else if !in_scope {
        Some("the pidfd-bound process is no longer in a cgroup of the claimed container")
    } else if !recent {
        Some("the pidfd-bound process started before the claim")
    } else {
        None
    };
    if let Some(reason) = refusal {
        entry["decision"] = json!("refused");
        entry["signal_outcome"] = json!("refused");
        entry["result"] = json!(format!("signal refused: {reason}"));
        return entry;
    }

    entry["signal_attempted"] = json!(true);
    entry["decision"] = json!("sigkill");
    match send(&handle) {
        Ok(()) => {
            entry["signal_outcome"] = json!("delivered");
            entry["result"] = json!("signal delivered through pidfd");
        }
        Err(error) if no_such_process(&error) => {
            entry["signal_outcome"] = json!("already_gone");
            entry["result"] = json!("the pidfd-bound process exited before signal delivery");
        }
        Err(error) => {
            entry["signal_outcome"] = json!("refused");
            entry["result"] = json!(format!("pidfd_send_signal failed: {error}"));
        }
    }
    entry
}

fn signal_candidate(candidate: &Value, container_id: &str, claim: i64) -> Value {
    signal_candidate_with(
        candidate,
        container_id,
        claim,
        PidFd::open,
        |pid| LiveIdentity {
            cgroup: live_cgroup(pid),
            start_epoch: start_epoch(pid),
            comm: comm(pid),
        },
        |pidfd| pidfd.send_signal(SIGKILL),
    )
}

pub(crate) fn signal_metrics(entries: &[Value]) -> Value {
    let mut attempts = 0_u64;
    let mut delivered = 0_u64;
    let mut already_gone = 0_u64;
    let mut refused = 0_u64;
    for entry in entries {
        let attempted = entry["signal_attempted"] == true || entry["decision"] == "sigkill";
        attempts += u64::from(attempted);
        match entry["signal_outcome"].as_str() {
            Some("delivered") => delivered += 1,
            Some("already_gone") => already_gone += 1,
            Some("refused") => refused += 1,
            // Compatibility when reconciling evidence written before pidfd metrics existed.
            None if entry["decision"] == "sigkill" && entry["result"] == "signal sent" => {
                delivered += 1
            }
            None if entry["decision"] == "sigkill"
                && entry["result"]
                    .as_str()
                    .is_some_and(|result| result.contains("No such process")) =>
            {
                already_gone += 1
            }
            _ => refused += 1,
        }
    }
    json!({
        "signal_candidates": entries.len(),
        "signal_attempts": attempts,
        "signals_delivered": delivered,
        "processes_signalled": delivered,
        "processes_already_gone": already_gone,
        "signals_refused": refused,
    })
}

/// Ends only the processes this host can prove belong to the failed restore it claimed. A pidfd is
/// opened before cgroup/start-time revalidation and is the only signal transport, so PID reuse cannot
/// redirect SIGKILL. If pidfd is unavailable, the signal is refused and there is no numeric fallback.
pub(crate) fn reclaim(container_id: &str, claim: i64) -> Value {
    let before = runtime_processes(container_id, claim);
    let mut signalled = vec![];
    if before["authorizes_reclaim"] == true {
        for candidate in before["processes"].as_array().cloned().unwrap_or_default() {
            signalled.push(signal_candidate(&candidate, container_id, claim));
        }
    }
    let metrics = signal_metrics(&signalled);
    let (gone, waited) = wait_for_cgroups(container_id);
    let after = runtime_processes(container_id, claim);
    let survivors = after["processes"].as_array().map(|p| p.len()).unwrap_or(0);
    let signal_failures = metrics["signals_refused"].as_u64().unwrap_or(0) > 0;
    let complete = gone
        && after["known"] == true
        && after["cgroups_known_absent"] == true
        && survivors == 0
        && before["authorizes_reclaim"] == true
        && !signal_failures;
    json!({
        "requested": true, "before": before, "signalled": signalled, "waited_seconds": waited,
        "signal_candidates": metrics["signal_candidates"],
        "signal_attempts": metrics["signal_attempts"],
        "signals_delivered": metrics["signals_delivered"],
        "processes_signalled": metrics["processes_signalled"],
        "processes_already_gone": metrics["processes_already_gone"],
        "signals_refused": metrics["signals_refused"],
        "cgroups_gone": gone, "after": after, "surviving_processes": survivors, "complete": complete,
        "incomplete_reason": if complete { Value::Null } else if before["authorizes_reclaim"] != true {
            json!("no cgroup of the claimed container existed, so nothing could be proven to belong to this attempt and nothing was signalled")
        } else if signal_failures {
            json!("one or more pidfd opens, identity rechecks, or pidfd signal attempts failed closed")
        } else if !gone {
            json!("a cgroup of the claimed container did not disappear within the bounded wait")
        } else {
            json!("a process of the claimed container survived the signal, or a new one appeared")
        },
    })
}

/// What one restore attempt may consume on the Podman graph root before it is stopped, together with the
/// producer markers required before its exact immutable cgroup may be frozen. Global cgroup appearance is
/// diagnostic only; the current operation-attempt's conmon must attest the complete binding.
pub(crate) struct Bound {
    pub graph_root: PathBuf,
    /// Bytes the attempt may consume: the space its own preflight required for it.
    pub allowance: u64,
    /// Container IDs that already owned a cgroup when the attempt started.
    pub before: BTreeSet<String>,
    pub unit: String,
    pub expected_name: String,
    pub universe_uuid: String,
    pub operation_id: String,
    pub operation_attempt_id: i64,
    pub authority_id: String,
    pub expected_image_id: String,
    pub attempt_started_at: i64,
    pub immutable_container_id: Option<String>,
}
impl Bound {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        graph_root: &Path,
        required: u64,
        unit: &str,
        expected_name: &str,
        universe_uuid: &str,
        operation_id: &str,
        operation_attempt_id: i64,
        authority_id: &str,
        expected_image_id: &str,
        attempt_started_at: i64,
        immutable_container_id: Option<&str>,
    ) -> Result<Self, Error> {
        if unit != format!("podmesh-restore-{operation_id}.scope")
            && unit != format!("podmesh-restore-local-{operation_id}.scope")
        {
            return Err("restore watchdog unit is not bound to its operation ID".into());
        }
        if !lc::is_uuid(universe_uuid) || expected_name != format!("podmesh-{universe_uuid}") {
            return Err("restore watchdog name is not bound to its universe UUID".into());
        }
        if operation_attempt_id <= 0 {
            return Err("restore watchdog operation-attempt ID is invalid".into());
        }
        if immutable_container_id.is_some_and(|id| !is_container_id(id)) {
            return Err("restore watchdog immutable container ID is invalid".into());
        }
        Ok(Bound {
            graph_root: graph_root.to_path_buf(),
            allowance: required.max(MIN_ALLOWANCE_BYTES),
            before: libpod_ids(),
            unit: unit.to_string(),
            expected_name: expected_name.to_string(),
            universe_uuid: universe_uuid.to_string(),
            operation_id: operation_id.to_string(),
            operation_attempt_id,
            authority_id: authority_id.to_string(),
            expected_image_id: expected_image_id.trim_start_matches("sha256:").to_string(),
            attempt_started_at,
            immutable_container_id: immutable_container_id.map(str::to_string),
        })
    }
    fn binding(&self, global_new: Vec<String>, operation_attested: bool) -> Binding {
        let result = match self.immutable_container_id.as_ref() {
            Some(id) => Ok(id.clone()),
            None if global_new.len() == 1 => Ok(global_new[0].clone()),
            None if global_new.is_empty() => Err("no fresh libpod cgroup is available for operation attestation"),
            None => Err("more than one fresh libpod cgroup makes the restore identity ambiguous"),
        }
        .and_then(|id| {
            if !is_container_id(&id) {
                Err("the restore candidate has no valid immutable container ID")
            } else if !operation_attested {
                Err("the candidate conmon does not attest this restore operation, authority, universe, name, and image")
            } else {
                Ok(id)
            }
        });
        match result {
            Ok(container_id) => Binding {
                container_id: Some(container_id),
                global_new,
                error: None,
            },
            Err(error) => Binding {
                container_id: None,
                global_new,
                error: Some(error.to_string()),
            },
        }
    }
    fn operation_attested(&self, container_id: &str) -> Result<bool, String> {
        let Some(members) = scope_members(&conmon_scope(container_id))? else {
            return Ok(false);
        };
        for (pid, _) in members {
            let recent =
                start_epoch(pid).is_some_and(|started| started as i64 >= self.attempt_started_at);
            if recent
                && environment_has(pid, "PODMESH_RESTORE_OPERATION_ID", &self.operation_id)?
                && environment_has(
                    pid,
                    "PODMESH_RESTORE_ATTEMPT_ID",
                    &self.operation_attempt_id.to_string(),
                )?
                && environment_has(pid, "PODMESH_RESTORE_AUTHORITY_ID", &self.authority_id)?
                && environment_has(pid, "PODMESH_RESTORE_UNIVERSE_UUID", &self.universe_uuid)?
                && environment_has(pid, "PODMESH_RESTORE_CONTAINER_NAME", &self.expected_name)?
                && environment_has(pid, "PODMESH_RESTORE_IMAGE_ID", &self.expected_image_id)?
                && command_has_pair(pid, "-c", "--cid", container_id)?
                && command_has_pair(pid, "-n", "--name", &self.expected_name)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn observe_binding(&self) -> Binding {
        let global_new: Vec<String> = libpod_ids().difference(&self.before).cloned().collect();
        let candidate = self
            .immutable_container_id
            .as_deref()
            .or_else(|| (global_new.len() == 1).then(|| global_new[0].as_str()));
        match candidate.map(|id| self.operation_attested(id)).transpose() {
            Ok(attested) => self.binding(global_new, attested.unwrap_or(false)),
            Err(error) => Binding {
                container_id: None,
                global_new,
                error: Some(format!(
                    "the restore operation attestation could not be read: {error}"
                )),
            },
        }
    }
}
struct Binding {
    container_id: Option<String>,
    global_new: Vec<String>,
    error: Option<String>,
}
/// Watches the graph root while a restore command runs and stops the attempt when it crosses its
/// allowance or the absolute floor. Returns the measurement, whether or not it had to act.
pub(crate) struct Watch {
    pub baseline: u64,
    pub minimum: u64,
    pub consumed_max: u64,
    pub container_id: Option<String>,
    pub triggered: Option<String>,
    pub frozen: bool,
    pub freeze_requested: bool,
    pub stopped_scope: bool,
    /// New global libpod cgroups are diagnostics only. They never select a freeze target.
    pub ambiguous: Vec<String>,
    pub binding_errors: Vec<String>,
    pub allowance: u64,
    pub floor: u64,
    pub samples: Vec<Value>,
    pub measurement_errors: Vec<String>,
    last_space: f64,
}
impl Watch {
    pub(crate) fn start(bound: &Bound) -> Result<Watch, Error> {
        let baseline = crate::migration::available_bytes(&bound.graph_root)?;
        Ok(Watch {
            baseline,
            minimum: baseline,
            consumed_max: 0,
            container_id: None,
            triggered: None,
            frozen: false,
            freeze_requested: false,
            stopped_scope: false,
            ambiguous: vec![],
            binding_errors: vec![],
            allowance: bound.allowance,
            floor: FREE_FLOOR_BYTES,
            samples: vec![],
            measurement_errors: vec![],
            last_space: -1.0,
        })
    }
    /// One observation. Returns true when the attempt was stopped. The cgroup delta is read on every
    /// call (a directory listing); free space once a second (one `df`), which bounds the overshoot to
    /// about a second of writing.
    pub(crate) fn step(&mut self, bound: &Bound, elapsed: f64) -> bool {
        if self.triggered.is_some() {
            return false;
        }
        if self.container_id.is_none() {
            let binding = bound.observe_binding();
            self.ambiguous = binding.global_new;
            if let Some(error) = binding.error {
                if self.binding_errors.last() != Some(&error) {
                    self.binding_errors.push(error);
                }
            }
            self.container_id = binding.container_id;
        }
        if elapsed - self.last_space < 1.0 {
            return false;
        }
        self.last_space = elapsed;
        let free = match crate::migration::available_bytes(&bound.graph_root) {
            Ok(free) => free,
            Err(error) => {
                let error = error.to_string();
                self.samples
                    .push(json!({"t": (elapsed * 100.0).round() / 100.0,
                    "known": false, "error": error}));
                self.measurement_errors.push(error.clone());
                self.triggered = Some(format!(
                    "free space on {} became unknown while the restore was running: {error}",
                    bound.graph_root.display()
                ));
                self.stop(bound);
                return true;
            }
        };
        let consumed = self.baseline.saturating_sub(free);
        self.minimum = self.minimum.min(free);
        self.consumed_max = self.consumed_max.max(consumed);
        self.samples.push(
            json!({"t": (elapsed * 100.0).round() / 100.0, "free": free, "consumed": consumed}),
        );
        if self.samples.len() > 240 {
            self.samples.remove(0);
        }
        let reason = if consumed > bound.allowance {
            Some(format!(
                "the attempt consumed {consumed} bytes on {}, more than the {} bytes its preflight required for it",
                bound.graph_root.display(),
                bound.allowance
            ))
        } else if free < FREE_FLOOR_BYTES {
            Some(format!(
                "{free} bytes left on {}, below the {FREE_FLOOR_BYTES} byte floor a restore attempt may not cross",
                bound.graph_root.display()
            ))
        } else {
            None
        };
        let Some(reason) = reason else { return false };
        self.triggered = Some(reason);
        self.stop(bound);
        true
    }
    fn stop(&mut self, bound: &Bound) {
        // Freeze only the immutable ID obtained by inspecting the exact expected name and validating its
        // universe/claim binding. If no such ID was established, stop only this operation's own scope.
        if let Some(ref id) = self.container_id {
            let outcome = freeze(id).unwrap_or(Freeze::Absent);
            self.frozen = outcome == Freeze::Confirmed;
            // A freeze that was written but not yet confirmed still has to be undone if it caught the
            // wrong container, so the request is recorded apart from its confirmation.
            self.freeze_requested = outcome != Freeze::Absent;
        }
        self.stopped_scope = Command::new("/usr/bin/timeout")
            .args([
                "--signal=KILL",
                "30",
                "/usr/bin/systemctl",
                "stop",
                &bound.unit,
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
    pub(crate) fn view(&self, bound: &Bound) -> Value {
        let final_observation = match crate::migration::available_bytes(&bound.graph_root) {
            Ok(free) => json!({"known": true, "available_bytes": free}),
            Err(error) => {
                json!({"known": false, "available_bytes": Value::Null, "error": error.to_string()})
            }
        };
        let measurement_known =
            self.measurement_errors.is_empty() && final_observation["known"] == true;
        json!({
            "watched": true, "graph_root": bound.graph_root, "allowance_bytes": self.allowance,
            "free_floor_bytes": self.floor, "free_bytes_before": self.baseline,
            "free_bytes_after": final_observation["available_bytes"],
            "measurement_known": measurement_known, "measurement_errors": self.measurement_errors,
            "final_space_observation": final_observation,
            "minimum_free_bytes": self.minimum, "maximum_consumed_bytes": self.consumed_max,
            "attempt_container_id": self.container_id, "ambiguous_container_cgroups": self.ambiguous,
            "binding": {"expected_name": bound.expected_name, "universe_uuid": bound.universe_uuid,
                "operation_id": bound.operation_id, "authority_id": bound.authority_id,
                "operation_attempt_id": bound.operation_attempt_id,
                "attempt_started_at": bound.attempt_started_at,
                "expected_image_id": bound.expected_image_id,
                "expected_immutable_container_id": bound.immutable_container_id,
                "errors": self.binding_errors,
                "global_new_cgroups_are_diagnostic_only": true},
            "stopped": self.triggered.is_some(), "reason": self.triggered,
            "container_cgroup_frozen": self.frozen, "container_cgroup_freeze_requested": self.freeze_requested,
            "container_cgroup_freeze_unconfirmed": self.freeze_requested && !self.frozen,
            "transient_scope_stopped": self.stopped_scope,
            "samples": self.samples,
        })
    }
}
/// Confirms that the already-attested immutable ID is also the container Podman reports after the command.
/// This is consistency reporting, not authorization for the earlier freeze.
pub(crate) fn confirm_or_thaw(view: &mut Value, observed: Option<&str>) {
    let frozen_id = view["attempt_container_id"].as_str().map(str::to_string);
    // A request that was written counts, confirmed or not: an unconfirmed freeze on the wrong container
    // is exactly the case that must still be undone.
    let requested = view["container_cgroup_freeze_requested"] == true
        || view["container_cgroup_frozen"] == true;
    let (Some(id), true) = (frozen_id, requested) else {
        return;
    };
    match observed {
        Some(observed) if observed == id => {
            view["freeze_confirmed_by_observation"] = json!(true);
            view["container_cgroup_frozen_now"] = json!(frozen(&id));
        }
        _ => {
            view["freeze_confirmed_by_observation"] = json!(false);
            view["thawed_after_misattribution"] =
                json!(thaw(&id).unwrap_or(Freeze::Absent) == Freeze::Confirmed);
        }
    }
}

#[cfg(test)]
mod observation_tests {
    use super::*;

    struct Tree(PathBuf);
    impl Tree {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "podmesh-cgroup-observation-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn restore_bound(expected_id: Option<&str>) -> Bound {
        let uuid = "d1111111-1111-4111-8111-111111111111";
        Bound {
            graph_root: PathBuf::from("/graph"),
            allowance: 1,
            before: BTreeSet::new(),
            unit: "podmesh-restore-operation-1.scope".into(),
            expected_name: format!("podmesh-{uuid}"),
            universe_uuid: uuid.into(),
            operation_id: "operation-1".into(),
            operation_attempt_id: 7,
            authority_id: "authorization-1".into(),
            expected_image_id: "b".repeat(64),
            attempt_started_at: 1,
            immutable_container_id: expected_id.map(str::to_string),
        }
    }

    #[test]
    fn absent_and_observed_empty_scopes_are_distinct() {
        let tree = Tree::new();
        assert!(matches!(scope_members(&tree.0.join("absent")), Ok(None)));
        fs::write(tree.0.join("cgroup.procs"), "").unwrap();
        assert!(matches!(scope_members(&tree.0), Ok(Some(members)) if members.is_empty()));
    }

    #[test]
    fn unreadable_or_malformed_membership_never_becomes_empty() {
        let tree = Tree::new();
        // A directory in place of cgroup.procs produces a real read error even as root.
        fs::create_dir(tree.0.join("cgroup.procs")).unwrap();
        assert!(scope_members(&tree.0).is_err());
        fs::remove_dir(tree.0.join("cgroup.procs")).unwrap();
        fs::write(tree.0.join("cgroup.procs"), "123\ninvalid\n").unwrap();
        assert!(scope_members(&tree.0).is_err());
        fs::write(tree.0.join("cgroup.procs"), "0\n").unwrap();
        assert!(scope_members(&tree.0).is_err());
    }

    #[test]
    fn failed_descendant_read_invalidates_the_whole_observation() {
        let tree = Tree::new();
        fs::write(tree.0.join("cgroup.procs"), "123\n").unwrap();
        let child = tree.0.join("child");
        fs::create_dir(&child).unwrap();
        assert!(
            scope_members(&tree.0).is_err(),
            "a missing descendant procs file is not an empty descendant"
        );
        fs::write(child.join("cgroup.procs"), "456\n").unwrap();
        let members = scope_members(&tree.0).unwrap().unwrap();
        assert_eq!(
            members.iter().map(|(pid, _)| *pid).collect::<Vec<_>>(),
            vec![123, 456]
        );
    }

    #[test]
    fn unrelated_or_ambiguous_global_cgroups_never_select_a_freeze_target() {
        let expected = "a".repeat(64);
        let unrelated = "c".repeat(64);
        let bound = restore_bound(None);

        let wrong_sole = bound.binding(vec![unrelated], false);
        assert!(wrong_sole.container_id.is_none());
        assert!(wrong_sole.error.unwrap().contains("does not attest"));

        let ambiguous = bound.binding(vec![expected.clone(), "d".repeat(64)], true);
        assert!(ambiguous.container_id.is_none());
        assert!(ambiguous.error.unwrap().contains("more than one"));
    }

    #[test]
    fn only_exact_attested_restore_identity_binds_an_immutable_id() {
        let expected = "a".repeat(64);
        let imported = restore_bound(None);
        let proven = imported.binding(vec![expected.clone()], true);
        assert_eq!(proven.container_id.as_deref(), Some(expected.as_str()));

        let unattested = imported.binding(vec![expected.clone()], false);
        assert!(unattested.container_id.is_none());
        assert!(unattested.error.unwrap().contains("does not attest"));

        let in_place = restore_bound(Some(&expected));
        let noisy = in_place.binding(vec!["e".repeat(64), "f".repeat(64)], true);
        assert_eq!(noisy.container_id.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn cgroup_membership_uses_exact_scope_boundaries() {
        let scope = "/machine.slice/libpod-aaaaaaaa.scope";
        assert!(exact_scope_or_descendant(scope, scope));
        assert!(exact_scope_or_descendant(&format!("{scope}/child"), scope));
        assert!(!exact_scope_or_descendant(&format!("{scope}-evil"), scope));
        assert!(!exact_scope_or_descendant(
            &format!("{scope}evil/child"),
            scope
        ));
        assert!(!exact_scope_or_descendant(
            "/machine.slice/libpod-bbbbbbbb.scope",
            scope
        ));
    }

    #[test]
    fn pidfd_is_opened_before_identity_recheck_and_is_the_only_signal_transport() {
        let events = std::cell::RefCell::new(Vec::new());
        let container_id = "a".repeat(64);
        let candidate = json!({"pid": 1234, "comm": "criu"});
        let result = signal_candidate_with(
            &candidate,
            &container_id,
            100,
            |_| {
                events.borrow_mut().push("pidfd_open");
                Ok(())
            },
            |_| {
                events.borrow_mut().push("identity_recheck");
                LiveIdentity {
                    cgroup: Some(format!(
                        "/machine.slice/libpod-{container_id}.scope/container"
                    )),
                    start_epoch: Some(101),
                    comm: Some("criu".into()),
                }
            },
            |_| {
                events.borrow_mut().push("pidfd_send_signal");
                Ok(())
            },
        );
        assert_eq!(
            *events.borrow(),
            ["pidfd_open", "identity_recheck", "pidfd_send_signal"]
        );
        assert_eq!(result["signal_outcome"], "delivered");
        assert_eq!(result["signal_transport"], "pidfd_send_signal");
        assert_eq!(result["numeric_pid_fallback"], false);
        assert_eq!(result["signal_attempted"], true);
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn configured_pidfd_abi_opens_and_checks_the_current_process_without_a_signal() {
        let pidfd = PidFd::open(std::process::id())
            .expect("this target must provide pidfd_open; product reclaim otherwise fails closed");
        pidfd
            .send_signal(0)
            .expect("pidfd_send_signal with signal 0 must validate the bound process identity");
    }

    #[test]
    fn unavailable_pidfd_or_invalid_pid_refuses_without_observation_or_signal() {
        let observed = std::cell::Cell::new(false);
        let sent = std::cell::Cell::new(false);
        let candidate = json!({"pid": 1234, "comm": "criu"});
        let result = signal_candidate_with(
            &candidate,
            &"b".repeat(64),
            100,
            |_| Err::<(), _>(io::Error::new(io::ErrorKind::Unsupported, "no pidfd")),
            |_| {
                observed.set(true);
                LiveIdentity {
                    cgroup: None,
                    start_epoch: None,
                    comm: None,
                }
            },
            |_| {
                sent.set(true);
                Ok(())
            },
        );
        assert_eq!(result["signal_outcome"], "refused");
        assert_eq!(result["pidfd_opened"], false);
        assert!(!observed.get());
        assert!(!sent.get());

        let wrapped = json!({"pid": u64::from(u32::MAX) + 1, "comm": "malformed"});
        let opened = std::cell::Cell::new(false);
        let result = signal_candidate_with(
            &wrapped,
            &"b".repeat(64),
            100,
            |_| {
                opened.set(true);
                Ok(())
            },
            |_| unreachable!(),
            |_| unreachable!(),
        );
        assert_eq!(result["signal_outcome"], "refused");
        assert!(
            !opened.get(),
            "an out-of-range PID must not wrap into another PID"
        );
    }

    #[test]
    fn signal_metrics_distinguish_attempts_delivery_disappearance_and_refusal() {
        let entries = vec![
            json!({"decision": "sigkill", "signal_attempted": true, "signal_outcome": "delivered"}),
            json!({"decision": "sigkill", "signal_attempted": true, "signal_outcome": "already_gone"}),
            json!({"decision": "refused", "signal_attempted": false, "signal_outcome": "refused"}),
            json!({"decision": "skipped", "signal_attempted": false, "signal_outcome": "already_gone"}),
        ];
        let metrics = signal_metrics(&entries);
        assert_eq!(metrics["signal_candidates"], 4);
        assert_eq!(metrics["signal_attempts"], 2);
        assert_eq!(metrics["signals_delivered"], 1);
        assert_eq!(metrics["processes_signalled"], 1);
        assert_eq!(metrics["processes_already_gone"], 2);
        assert_eq!(metrics["signals_refused"], 1);

        let legacy = signal_metrics(&[
            json!({"decision": "sigkill", "result": "signal sent"}),
            json!({"decision": "sigkill", "result": "signal refused: No such process"}),
        ]);
        assert_eq!(legacy["signal_attempts"], 2);
        assert_eq!(legacy["signals_delivered"], 1);
        assert_eq!(legacy["processes_already_gone"], 1);
    }
}
