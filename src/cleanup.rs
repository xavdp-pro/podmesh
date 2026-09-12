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
//!   claim, both re-read immediately before the signal. A command line that merely names the container
//!   is a diagnostic fallback once the cgroups are gone; it never authorizes a signal.
use crate::lifecycle::Error;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
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

pub(crate) fn container_scope(container_id: &str) -> PathBuf {
    PathBuf::from(format!("{MACHINE_SLICE}/libpod-{container_id}.scope"))
}
pub(crate) fn conmon_scope(container_id: &str) -> PathBuf {
    PathBuf::from(format!("{MACHINE_SLICE}/libpod-conmon-{container_id}.scope"))
}
fn is_container_id(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
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
        let Some(rest) = name.strip_prefix("libpod-") else { continue };
        let Some(rest) = rest.strip_suffix(".scope") else { continue };
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
        .find_map(|l| l.strip_prefix("btime ").and_then(|v| v.trim().parse::<u64>().ok()))
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
    read_trimmed(format!("/proc/{pid}/cgroup")).map(|v| v.rsplit("::").next().unwrap_or("").to_string())
}
fn cmdline(pid: u32) -> Option<String> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if raw.len() > 64 * 1024 {
        return None;
    }
    Some(String::from_utf8_lossy(&raw).replace('\0', " "))
}
/// Every PID in a cgroup and its descendants, with the `cgroup.procs` file that listed it.
fn members(dir: &Path, out: &mut Vec<(u32, String)>) {
    if let Ok(procs) = fs::read_to_string(dir.join("cgroup.procs")) {
        for pid in procs.lines().filter_map(|l| l.trim().parse::<u32>().ok()) {
            out.push((pid, dir.display().to_string()));
        }
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                members(&entry.path(), out);
            }
        }
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
    let mut cgroups = vec![];
    let mut found: Vec<(u32, String)> = vec![];
    for (role, dir) in [("container", container_scope(container_id)), ("conmon", conmon_scope(container_id))] {
        let exists = dir.is_dir();
        let mut here = vec![];
        if exists {
            members(&dir, &mut here);
        }
        cgroups.push(json!({"role": role, "path": dir, "exists": exists,
            "member_pids": here.iter().map(|(p, _)| *p).collect::<Vec<_>>()}));
        found.extend(here);
    }
    found.sort_unstable();
    found.dedup();
    let any_cgroup = cgroups.iter().any(|c| c["exists"] == true);
    let processes: Vec<Value> = if any_cgroup {
        found.iter().map(|(pid, file)| process_view(*pid, file, claim)).collect()
    } else {
        // Diagnostic only: a command line naming the container proves nothing about ownership.
        let mut scanned = vec![];
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Some(pid) = entry.file_name().to_str().and_then(|p| p.parse::<u32>().ok()) else {
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
        "source": if any_cgroup { "cgroup_residency" } else { "cmdline_fallback" },
        "authorizes_reclaim": any_cgroup,
        "cgroups": cgroups, "processes": processes, "count": processes.len(),
    })
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
        let gone = !container_scope(container_id).is_dir() && !conmon_scope(container_id).is_dir();
        if gone || Instant::now() >= deadline {
            return (gone, begin.elapsed().as_secs_f64());
        }
        thread::sleep(Duration::from_millis(200));
    }
}
/// Ends only the processes this host can prove belong to the failed restore it claimed. Every
/// candidate's cgroup and start time are re-read immediately before the signal, so a PID reused by
/// another process between the two reads is skipped rather than signalled.
pub(crate) fn reclaim(container_id: &str, claim: i64) -> Value {
    let before = runtime_processes(container_id, claim);
    let mut signalled = vec![];
    if before["authorizes_reclaim"] == true {
        for candidate in before["processes"].as_array().cloned().unwrap_or_default() {
            let pid = candidate["pid"].as_u64().unwrap_or(0) as u32;
            // Re-read: the proof must describe the process that is about to receive the signal.
            let fresh_cgroup = live_cgroup(pid);
            let fresh_start = start_epoch(pid);
            let in_scope = fresh_cgroup.as_deref().is_some_and(|c| {
                c.starts_with(&format!("/machine.slice/libpod-{container_id}.scope"))
                    || c.starts_with(&format!("/machine.slice/libpod-conmon-{container_id}.scope"))
            });
            let recent = fresh_start.is_some_and(|s| s as i64 >= claim);
            let mut entry = json!({"pid": pid, "comm": comm(pid), "cgroup_at_signal": fresh_cgroup,
                "start_epoch_at_signal": fresh_start, "member_of_claimed_cgroup": in_scope,
                "started_at_or_after_claim": recent});
            let (decision, result) = match (fresh_start, in_scope, recent) {
                (None, _, _) => ("skipped", "the process is gone or its start time cannot be read".to_string()),
                (_, false, _) => ("skipped", "no longer a member of a cgroup of the claimed container".to_string()),
                (_, _, false) => ("skipped", "start time is earlier than the claim".to_string()),
                _ => ("sigkill", sigkill(pid)),
            };
            entry["decision"] = json!(decision);
            entry["result"] = json!(result);
            signalled.push(entry);
        }
    }
    let (gone, waited) = wait_for_cgroups(container_id);
    let after = runtime_processes(container_id, claim);
    let survivors = after["processes"].as_array().map(|p| p.len()).unwrap_or(0);
    let complete = gone && survivors == 0 && before["authorizes_reclaim"] == true;
    json!({
        "requested": true, "before": before, "signalled": signalled, "waited_seconds": waited,
        "cgroups_gone": gone, "after": after, "surviving_processes": survivors, "complete": complete,
        "incomplete_reason": if complete { Value::Null } else if before["authorizes_reclaim"] != true {
            json!("no cgroup of the claimed container existed, so nothing could be proven to belong to this attempt and nothing was signalled")
        } else if !gone {
            json!("a cgroup of the claimed container did not disappear within the bounded wait")
        } else {
            json!("a process of the claimed container survived the signal, or a new one appeared")
        },
    })
}
/// The only signal PodMesh ever sends to a process it did not start, behind the explicit request field.
/// A fixed program with a numeric argument; no shell, and no dependency added for one call.
fn sigkill(pid: u32) -> String {
    match Command::new("/usr/bin/kill").args(["-s", "KILL", "--", &pid.to_string()]).output() {
        Ok(out) if out.status.success() => "signal sent".to_string(),
        Ok(out) => format!("signal refused: {}", String::from_utf8_lossy(&out.stderr).trim()),
        Err(e) => format!("signal could not be sent: {e}"),
    }
}

/// What one restore attempt may consume on the Podman graph root before it is stopped.
pub(crate) struct Bound {
    pub graph_root: PathBuf,
    /// Bytes the attempt may consume: the space its own preflight required for it.
    pub allowance: u64,
    /// Container IDs that already owned a cgroup when the attempt started.
    pub before: BTreeSet<String>,
    pub unit: String,
}
impl Bound {
    pub(crate) fn new(graph_root: &Path, required: u64, unit: &str) -> Self {
        Bound {
            graph_root: graph_root.to_path_buf(),
            allowance: required.max(MIN_ALLOWANCE_BYTES),
            before: libpod_ids(),
            unit: unit.to_string(),
        }
    }
    /// The container this attempt created, when exactly one appeared since it started. Ambiguity is
    /// reported and nothing is frozen: PodMesh never acts on a cgroup it cannot attribute.
    fn appeared(&self) -> Result<String, Vec<String>> {
        let fresh: Vec<String> = libpod_ids().difference(&self.before).cloned().collect();
        match fresh.len() {
            1 => Ok(fresh[0].clone()),
            _ => Err(fresh),
        }
    }
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
    pub ambiguous: Vec<String>,
    pub allowance: u64,
    pub floor: u64,
    pub samples: Vec<Value>,
    last_space: f64,
}
impl Watch {
    pub(crate) fn start(bound: &Bound) -> Watch {
        let baseline = crate::migration::available_bytes(&bound.graph_root);
        Watch {
            baseline,
            minimum: baseline,
            consumed_max: 0,
            container_id: None,
            triggered: None,
            frozen: false,
            freeze_requested: false,
            stopped_scope: false,
            ambiguous: vec![],
            allowance: bound.allowance,
            floor: FREE_FLOOR_BYTES,
            samples: vec![],
            last_space: -1.0,
        }
    }
    /// One observation. Returns true when the attempt was stopped. The cgroup delta is read on every
    /// call (a directory listing); free space once a second (one `df`), which bounds the overshoot to
    /// about a second of writing.
    pub(crate) fn step(&mut self, bound: &Bound, elapsed: f64) -> bool {
        if self.triggered.is_some() {
            return false;
        }
        if self.container_id.is_none() {
            match bound.appeared() {
                Ok(id) => self.container_id = Some(id),
                Err(fresh) => {
                    if fresh.len() > 1 {
                        self.ambiguous = fresh;
                    }
                }
            }
        }
        if elapsed - self.last_space < 1.0 {
            return false;
        }
        self.last_space = elapsed;
        let free = crate::migration::available_bytes(&bound.graph_root);
        let consumed = self.baseline.saturating_sub(free);
        self.minimum = self.minimum.min(free);
        self.consumed_max = self.consumed_max.max(consumed);
        self.samples
            .push(json!({"t": (elapsed * 100.0).round() / 100.0, "free": free, "consumed": consumed}));
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
        // Freezing stops the writing without ending anything, and is undone if the cgroup turns out
        // not to belong to this attempt.
        if let Some(ref id) = self.container_id {
            let outcome = freeze(id).unwrap_or(Freeze::Absent);
            self.frozen = outcome == Freeze::Confirmed;
            // A freeze that was written but not yet confirmed still has to be undone if it caught the
            // wrong container, so the request is recorded apart from its confirmation.
            self.freeze_requested = outcome != Freeze::Absent;
        }
        self.stopped_scope = Command::new("/usr/bin/timeout")
            .args(["--signal=KILL", "30", "/usr/bin/systemctl", "stop", &bound.unit])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        self.triggered = Some(reason);
        true
    }
    pub(crate) fn view(&self, bound: &Bound) -> Value {
        let free = crate::migration::available_bytes(&bound.graph_root);
        json!({
            "watched": true, "graph_root": bound.graph_root, "allowance_bytes": self.allowance,
            "free_floor_bytes": self.floor, "free_bytes_before": self.baseline, "free_bytes_after": free,
            "minimum_free_bytes": self.minimum, "maximum_consumed_bytes": self.consumed_max,
            "attempt_container_id": self.container_id, "ambiguous_container_cgroups": self.ambiguous,
            "stopped": self.triggered.is_some(), "reason": self.triggered,
            "container_cgroup_frozen": self.frozen, "container_cgroup_freeze_requested": self.freeze_requested,
            "container_cgroup_freeze_unconfirmed": self.freeze_requested && !self.frozen,
            "transient_scope_stopped": self.stopped_scope,
            "samples": self.samples,
        })
    }
}
/// Undoes a freeze that turns out to have caught a container which is not the attempt's own.
pub(crate) fn confirm_or_thaw(view: &mut Value, observed: Option<&str>) {
    let frozen_id = view["attempt_container_id"].as_str().map(str::to_string);
    // A request that was written counts, confirmed or not: an unconfirmed freeze on the wrong container
    // is exactly the case that must still be undone.
    let requested = view["container_cgroup_freeze_requested"] == true || view["container_cgroup_frozen"] == true;
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
            view["thawed_after_misattribution"] = json!(thaw(&id).unwrap_or(Freeze::Absent) == Freeze::Confirmed);
        }
    }
}
