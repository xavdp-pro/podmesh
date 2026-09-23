//! The resident's side of the signed votes (V3-4): its configuration, its control operations and
//! its status. The rules are in `ledger.rs`, `vote.rs` and `readmission.rs`; this file reads the
//! host-provided directory, the store and the configuration, and answers on the control socket.
//!
//! The host provides two things outside the universe's state, through the environment:
//! `PODMESH_MANAGER_VOTE_DIR`, the private directory holding the replica's key (`<key_id>.key`) and
//! its signing ledger (`<key_id>.ledger`), and `PODMESH_MANAGER_HOST_ID_FILE`, the host's machine-id
//! mounted read-only (default `/etc/machine-id`; checked read-only at the start and at every
//! operation). Neither is in the store or in any recovery point. The configuration names a third,
//! `votes.evidence_dir`: the operator's evidence for readmission, which the replica cannot write.
//!
//! Operations, each one JSON object on the control socket:
//!
//! - `vote_ledger_init` (the operator): creates the ledger of this replica's key, unadmitted;
//! - `vote_ledger_mark_unadmitted` (the operator): the restore procedure's mark;
//! - `vote_ledger_readmit` (the operator): readmission, which fails closed, reading only the evidence
//!   files whose SHA-256 the request states (`evidence_sha256`, file name to digest);
//! - `vote_sign` (the observation writer): signs a vote for a certificate payload under the ledger's
//!   rules, reading the votes in this replica's store for the tripwire, and appends it as a fact in
//!   this replica's vote scope `votes/<replica_id>` before it answers with it.
//!
//! The manager decides (V3-5, `decisions.rs`): with `votes.decisions` configured,
//!
//! - `decision_propose` (the observation writer or the operator): records a proposal, the full payload
//!   of a takeover certificate, as a fact in this replica's scope `proposals/<replica_id>`;
//! - `decision_read` (the same callers): the current decision of a resource with its assembled
//!   certificate, or each live proposal above it with its voters, what is missing and this replica's
//!   own verdict;
//! - the voter, a worker of the resident, reads the proposals its store holds at every
//!   `voter_interval_ms`, checks each against its own view and votes for one that passes;
//! - `vote_sign` checks the same rules before it signs a takeover payload, and refuses a policy change
//!   (which the replicas do not decide here); it is idempotent: a payload this replica already voted
//!   for, with the promise in its ledger, answers the recorded vote again.
use crate::{
    decisions::{self, DecisionConfiguration, Proposal},
    ledger::{HostIdentity, LedgerRefusal, Signer, SigningRules, TRIPWIRE_CODES},
    quorum::{self, Quorum},
    readmission::{self, Scope},
    vote::{self, PromiseKind},
    Configuration,
};
use podmesh_manager_ha_lab::{
    durable::{Request, Response, Store},
    Fact,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::PathBuf,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The environment variable naming the host-provided vote directory.
pub const VOTE_DIR_ENV: &str = "PODMESH_MANAGER_VOTE_DIR";
/// The environment variable naming the host's machine-id file, mounted read-only.
pub const HOST_ID_FILE_ENV: &str = "PODMESH_MANAGER_HOST_ID_FILE";
/// Optional, live hypervisor generation witness; mandatory if the vote config requires it.
pub const GENERATION_FILE_ENV: &str = "PODMESH_MANAGER_GENERATION_ID_FILE";
const DEFAULT_HOST_ID_FILE: &str = "/etc/machine-id";
/// How long a vote operation waits for the ledger's lock: inside the control deadline.
const LOCK_WAIT: Duration = Duration::from_millis(150);
/// How many alerts the status keeps.
const ALERTS_KEPT: usize = 16;

/// A key retired by a re-keying: readmission still reads its votes.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetiredKey {
    pub key_id: String,
    pub public_key: String,
}

/// The replica's vote configuration: the authority set it votes under, and whom it answers to.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VoteConfiguration {
    /// This replica's key; its seed is `<vote_dir>/<key_id>.key`.
    pub key_id: String,
    /// The authority the nodes' policies name.
    pub authority_id: String,
    /// The nodes' `authority_quorum`: `{"threshold", "keys": [{"key_id", "public_key"}]}`.
    pub authority_quorum: Value,
    /// The authority set's serial, as the nodes hold it.
    pub authority_serial: u64,
    /// Every replica of the topology and the key it owns.
    pub replica_keys: BTreeMap<String, String>,
    /// Keys retired by a re-keying, whose votes readmission reads (finding 2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired_keys: Vec<RetiredKey>,
    /// Every PodMesh node whose epoch screen readmission reads, by host UUID.
    pub nodes: Vec<String>,
    /// The operator's evidence directory for readmission: an absolute path outside the vote
    /// directory that the replica cannot write (owned by the operator, or mounted read-only).
    pub evidence_dir: PathBuf,
    /// The only UID that may create, mark and readmit the ledger.
    pub operator_uid: u32,
    /// The longest life a vote gives a certificate, in seconds; readmission waits it out.
    pub max_certificate_life_seconds: i64,
    /// Require a live hypervisor generation witness before any vote or readmission. True unless
    /// the configuration says otherwise, because a guest whose hypervisor can snapshot it cannot
    /// otherwise tell that it went back in time. Setting it false is the operator's recorded
    /// decision to sign without that defence; the signer then carries the waiver and shows it.
    #[serde(default = "require_generation_id_default")]
    pub require_generation_id: bool,
    /// The operator's own reason for voting with no generation witness, required whenever
    /// `require_generation_id` is false. The signer carries it as its waiver and `status` reports
    /// it, so that doing without the defence is a named act and not a sentence a tool produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_witness_waiver: Option<String>,
    /// The replicas' decisions (V3-5): which resources this replica decides and how often its voter
    /// reads the store. Absent, the replica proposes nothing, votes only through `vote_sign`, and
    /// checks no decision rule (V3-4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decisions: Option<DecisionConfiguration>,
}

fn require_generation_id_default() -> bool {
    true
}

/// Read a bounded live hypervisor witness. QEMU's fw_cfg item is 4096 bytes with the VM
/// generation ID at offset 40; another platform adapter may supply the bare 16 bytes.
pub(crate) fn generation_id_from(
    path: &std::path::Path,
) -> std::result::Result<String, LedgerRefusal> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|mut file| {
            // Best effort: the witness may be on a filesystem that caches nothing, and the call
            // may be refused. Neither is a reason to fail, but a cache that can be dropped is.
            let _ = rustix::fs::fadvise(&file, 0, None, rustix::fs::Advice::DontNeed);
            file.by_ref().take(4097).read_to_end(&mut bytes)
        })
        .map_err(|e| crate::ledger::refusal("generation_identity_unreadable", e.to_string()))?;
    let generation = match bytes.len() {
        16 => bytes.as_slice(),
        4096 => &bytes[40..56],
        _ => {
            return Err(crate::ledger::refusal(
                "generation_identity_unreadable",
                "expected a 16-byte generation ID or the 4096-byte QEMU fw_cfg item",
            ))
        }
    };
    if generation.iter().all(|b| *b == 0) {
        return Err(crate::ledger::refusal(
            "generation_identity_unreadable",
            "the hypervisor generation ID is zero",
        ));
    }
    Ok(quorum::hex(generation))
}

impl VoteConfiguration {
    /// The policy the replica votes under.
    ///
    /// # Errors
    /// The node's own refusal of the quorum.
    pub fn policy(&self) -> Result<Quorum> {
        Ok(
            Quorum::declared(&self.authority_id, &self.authority_quorum)?
                .at_serial(self.authority_serial),
        )
    }

    /// Checks the vote configuration against the topology.
    ///
    /// # Errors
    /// A named inconsistency.
    pub fn validate(&self, config: &Configuration) -> Result<()> {
        let policy = self.policy()?;
        let replica = &config.network.replica_id;
        if policy.key(&self.key_id).is_none() {
            return Err("votes.key_id is not a key of votes.authority_quorum".into());
        }
        let replicas: BTreeSet<&str> = config
            .network
            .manager
            .replicas
            .iter()
            .map(|r| r.replica_id.as_str())
            .collect();
        if self
            .replica_keys
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != replicas
        {
            return Err(
                "votes.replica_keys must name every replica of the topology exactly once".into(),
            );
        }
        let keys: BTreeSet<&String> = self.replica_keys.values().collect();
        if keys.len() != self.replica_keys.len() || keys.iter().any(|k| policy.key(k).is_none()) {
            return Err(
                "votes.replica_keys must give each replica its own key of the policy".into(),
            );
        }
        if self.replica_keys.get(replica) != Some(&self.key_id) {
            return Err("votes.replica_keys must give this replica votes.key_id".into());
        }
        let scope = vote::vote_scope(replica);
        if !config
            .network
            .manager
            .grants
            .iter()
            .any(|g| g.scope == scope && &g.owner_replica_id == replica)
        {
            return Err(format!("the topology must grant {scope} to this replica").into());
        }
        if self.nodes.is_empty() || self.nodes.len() > 64 {
            return Err("votes.nodes must name 1 to 64 nodes".into());
        }
        for node in &self.nodes {
            crate::validate_control_token(node)?;
        }
        for retired in &self.retired_keys {
            quorum::check_key_id(&retired.key_id)?;
            quorum::verifying_key(&retired.public_key)?;
            if policy.key(&retired.key_id).is_some() {
                return Err("a retired key is not a key of the policy".into());
            }
        }
        if !self.evidence_dir.is_absolute() {
            return Err("votes.evidence_dir must be an absolute path".into());
        }
        if !(1..=3600).contains(&self.max_certificate_life_seconds) {
            return Err("votes.max_certificate_life_seconds must be 1 to 3600".into());
        }
        if let Some(d) = &self.decisions {
            d.validate(&self.nodes)?;
        }
        Ok(())
    }
}

/// What the resident holds for its votes while it runs.
pub(crate) struct VoteRuntime {
    config: VoteConfiguration,
    manager: podmesh_manager_ha_lab::durable::Configuration,
    dir: PathBuf,
    host_id_file: PathBuf,
    alerts: Mutex<Vec<String>>,
    /// This replica's last verdict on each proposal its voter read, by payload digest.
    verdicts: Mutex<BTreeMap<String, Value>>,
    /// The voter's own account: passes, votes cast, the last pass's duration and error.
    voter: Mutex<VoterStatus>,
    /// The duration of the vote operations served on the control socket: the last and the longest.
    timings: Mutex<BTreeMap<&'static str, (u64, u64)>>,
}

/// The voter's account, in the status.
#[derive(Default, Clone, Serialize)]
struct VoterStatus {
    passes: u64,
    votes_cast: u64,
    last_pass_ms: Option<u64>,
    longest_pass_ms: u64,
    last_error: Option<String>,
}

/// The vote operations the control socket takes.
pub(crate) enum VoteOperation {
    Init,
    MarkUnadmitted {
        reason: String,
    },
    Readmit {
        operation_id: String,
        evidence_sha256: BTreeMap<String, String>,
    },
    Sign {
        operation_id: String,
        payload: Value,
    },
    Propose {
        operation_id: String,
        payload: Value,
    },
    Read {
        resource: String,
    },
}

impl VoteOperation {
    /// Whether only the operator may ask it.
    pub(crate) fn operator_only(&self) -> bool {
        matches!(
            self,
            VoteOperation::Init
                | VoteOperation::MarkUnadmitted { .. }
                | VoteOperation::Readmit { .. }
        )
    }

    /// Whether the operator may ask it too, beside the observation writer.
    pub(crate) fn operator_too(&self) -> bool {
        matches!(
            self,
            VoteOperation::Propose { .. } | VoteOperation::Read { .. }
        )
    }

    /// Whether it writes a fact, and so waits for this process to have caught up with its peers.
    pub(crate) fn writes_a_fact(&self) -> bool {
        matches!(
            self,
            VoteOperation::Sign { .. } | VoteOperation::Propose { .. }
        )
    }

    fn name(&self) -> &'static str {
        match self {
            VoteOperation::Init => "vote_ledger_init",
            VoteOperation::MarkUnadmitted { .. } => "vote_ledger_mark_unadmitted",
            VoteOperation::Readmit { .. } => "vote_ledger_readmit",
            VoteOperation::Sign { .. } => "vote_sign",
            VoteOperation::Propose { .. } => "decision_propose",
            VoteOperation::Read { .. } => "decision_read",
        }
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn refused(e: &LedgerRefusal) -> Vec<u8> {
    serde_json::to_vec(&json!({"error": "vote_refused", "code": e.code, "detail": e.detail}))
        .unwrap_or_else(|_| b"{\"error\":\"vote_refused\"}".to_vec())
}

fn answer(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_else(|_| b"{\"error\":\"vote_refused\"}".to_vec())
}

fn decision_refused(error: &'static str, code: &str, detail: &str) -> Vec<u8> {
    answer(&json!({"error": error, "code": code, "detail": detail}))
}

/// Every fact of this replica's store.
fn export(store: &impl Fn() -> Result<Store>) -> Result<Vec<Fact>> {
    match store()?.execute(&Request::Export {})? {
        Response::Snapshot { snapshot } => Ok(snapshot.facts),
        _ => Err("unexpected export response".into()),
    }
}

/// A vote signed and recorded, or found already recorded.
struct Recorded {
    vote: Value,
    fact: String,
    replayed: bool,
}

impl VoteRuntime {
    /// Reads the environment for a configuration that votes. A resident configured to vote without a
    /// vote directory does not start.
    pub(crate) fn from_environment(config: &Configuration) -> Result<Option<VoteRuntime>> {
        let Some(votes) = &config.votes else {
            return Ok(None);
        };
        votes.validate(config)?;
        let dir = std::env::var_os(VOTE_DIR_ENV)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or("votes are configured: PODMESH_MANAGER_VOTE_DIR must name the host-provided vote directory")?;
        let host_id_file = std::env::var_os(HOST_ID_FILE_ENV)
            .map_or_else(|| PathBuf::from(DEFAULT_HOST_ID_FILE), PathBuf::from);
        // Checked at every signature too; said at once, so that a mount made writable by mistake
        // is seen at the start rather than at the first vote.
        if let Err(e) = HostIdentity::read(&host_id_file) {
            eprintln!(
                "resident vote alert: {}: {}; this replica signs nothing until it is fixed",
                e.code, e.detail
            );
        }
        let runtime = VoteRuntime {
            config: votes.clone(),
            manager: config.network.manager.clone(),
            dir,
            host_id_file,
            alerts: Mutex::new(Vec::new()),
            verdicts: Mutex::new(BTreeMap::new()),
            voter: Mutex::new(VoterStatus::default()),
            timings: Mutex::new(BTreeMap::new()),
        };
        // Try before peer exchange. An invalid read-only host mount already prevents signatures;
        // signer() repeats the guard before every later operation if the mount is repaired live.
        if let Err(e) = runtime.signer() {
            eprintln!(
                "resident vote alert: {}: {}; this replica signs nothing until it is fixed",
                e.code, e.detail
            );
        }
        Ok(Some(runtime))
    }

    /// The voter's interval, when this replica decides.
    pub(crate) fn voter_interval(&self) -> Option<Duration> {
        self.config
            .decisions
            .as_ref()
            .map(|d| Duration::from_millis(d.voter_interval_ms))
    }

    fn signer(&self) -> std::result::Result<Signer, LedgerRefusal> {
        let host = HostIdentity::read(&self.host_id_file)?;
        let policy = self
            .config
            .policy()
            .map_err(|e| crate::ledger::refusal("key_not_in_policy", e.to_string()))?;
        let mut signer = Signer::open(
            &self.dir,
            &self.config.key_id,
            host,
            policy,
            SigningRules {
                max_certificate_life: self.config.max_certificate_life_seconds,
                lock_wait: LOCK_WAIT,
            },
        )?;
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map_err(|e| crate::ledger::refusal("boot_identity_unreadable", e.to_string()))?;
        signer.guard_boot(boot_id.trim_end_matches('\n'), now())?;
        if let Some(path) = std::env::var_os(GENERATION_FILE_ENV) {
            let path = PathBuf::from(path);
            // Whether the path could be a witness at all is settled before `guard_generation`,
            // which persists a marker and can mark the ledger unadmitted. A path that could never
            // be a witness must not be able to quarantine a ledger by being named.
            // One ordered act: the path is judged, read, guarded and watched, so that nothing
            // durable is marked on the strength of a path that could never be a witness, and so
            // that there is one order rather than one per caller.
            match signer.adopt_generation_witness(path, now()) {
                Ok(_) => {}
                Err(refused) if refused.code == crate::ledger::GENERATION_WITNESS_UNTRUSTED => {
                    // A witness that proves nothing is the same position as no witness: it needs
                    // the operator's recorded decision, and it marked nothing on the way there.
                    let waiver = self.waiver(Some(&refused))?;
                    signer.waive_generation(waiver)?;
                }
                Err(other) => return Err(other),
            }
        } else {
            let waiver = self.waiver(None)?;
            signer.waive_generation(waiver)?;
        }
        Ok(signer)
    }

    /// The operator's recorded decision to vote without the generation witness, refused unless the
    /// configuration carries it in the operator's own words. `untrusted`, when present, is why the
    /// witness that was offered cannot serve; the waiver must still be the operator's.
    fn waiver(
        &self,
        untrusted: Option<&LedgerRefusal>,
    ) -> std::result::Result<String, LedgerRefusal> {
        let reason = self
            .config
            .generation_witness_waiver
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty());
        let Some(reason) = reason else {
            let missing = match untrusted {
                Some(u) => format!(
                    "the generation witness mounted cannot serve ({}: {})",
                    u.code, u.detail
                ),
                None => "no generation witness is mounted".to_string(),
            };
            return Err(crate::ledger::refusal(
                "generation_identity_unreadable",
                format!("{missing}, and this configuration records no generation_witness_waiver. Mount a witness the hypervisor answers for, or write the operator's own reason for voting without one; a tool cannot decide that for them"),
            ));
        };
        if self.config.require_generation_id {
            return Err(crate::ledger::refusal(
                "generation_identity_unreadable",
                "this vote configuration requires a mounted hypervisor generation witness; a waiver does not override require_generation_id, which the operator sets to false in the same act",
            ));
        }
        Ok(match untrusted {
            Some(u) => format!("operator waiver: {reason} (the witness offered cannot serve: {}: {})", u.code, u.detail),
            None => format!("operator waiver: {reason}"),
        })
    }

    fn alert(&self, text: String) {
        eprintln!("resident vote alert: {text}");
        if let Ok(mut alerts) = self.alerts.lock() {
            alerts.push(text);
            let excess = alerts.len().saturating_sub(ALERTS_KEPT);
            alerts.drain(..excess);
        }
    }

    /// The status's `votes` section: the ledger's state as a signer would find it, the alerts, and
    /// whether this replica votes without a generation witness, on whose recorded reason.
    pub(crate) fn status(&self) -> Value {
        let alerts = self.alerts.lock().map(|a| a.clone()).unwrap_or_default();
        let signer = self.signer();
        // Whether this replica is defended against a snapshot rollback, and on whose recorded word
        // if it is not. A waiver that no one can read is a waiver no one can withdraw.
        let waiver = signer
            .as_ref()
            .ok()
            .and_then(|s| s.generation_waiver().map(str::to_string));
        let ledger = signer.and_then(|s| s.load());
        let (state, detail) = match &ledger {
            Ok(l) if l.admitted => ("admitted", None),
            Ok(_) => ("unadmitted", None),
            Err(e) => (e.code, Some(e.detail.clone())),
        };
        let ledger = ledger.ok();
        let timings: BTreeMap<&str, Value> = self
            .timings
            .lock()
            .map(|t| {
                t.iter()
                    .map(|(k, (last, longest))| {
                        (*k, json!({"last_ms": last, "longest_ms": longest}))
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!({
            "key_id": self.config.key_id,
            "ledger_state": state,
            "ledger_detail": detail,
            "sequence": ledger.as_ref().map(|l| l.sequence),
            "unadmitted_since": ledger.as_ref().and_then(|l| l.unadmitted_since),
            "unadmitted_reason": ledger.as_ref().and_then(|l| l.unadmitted_reason.clone()),
            "admissions": ledger.as_ref().map(|l| l.admissions.len()),
            "alerts": alerts,
            "generation_witness": if waiver.is_some() { "waived" } else { "watched" },
            "generation_witness_waiver": waiver,
            "decides": self.config.decisions.as_ref().map(|d| d.resources.iter().map(|r| r.resource.clone()).collect::<Vec<_>>()),
            "voter": self.config.decisions.as_ref().map(|_| self.voter.lock().map(|v| json!(v.clone())).unwrap_or(Value::Null)),
            "operations": timings,
        })
    }

    /// Runs one vote operation. `store` opens this replica's store when the operation needs it.
    pub(crate) fn execute(
        &self,
        operation: VoteOperation,
        uid: u32,
        store: impl Fn() -> Result<Store>,
        replica_id: &str,
    ) -> Vec<u8> {
        let started = std::time::Instant::now();
        let name = operation.name();
        let response = self.perform(operation, uid, store, replica_id);
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Ok(mut t) = self.timings.lock() {
            let entry = t.entry(name).or_insert((0, 0));
            *entry = (ms, entry.1.max(ms));
        }
        response
    }

    fn perform(
        &self,
        operation: VoteOperation,
        uid: u32,
        store: impl Fn() -> Result<Store>,
        replica_id: &str,
    ) -> Vec<u8> {
        // Reading and proposing need no signer: a replica whose ledger cannot sign still tells what
        // its store holds, and still carries a proposal to its peers.
        match operation {
            VoteOperation::Read { resource } => return self.read(&resource, &store),
            VoteOperation::Propose {
                operation_id,
                payload,
            } => return self.propose(&operation_id, &payload, &store, replica_id),
            _ => {}
        }
        let signer = match self.signer() {
            Ok(s) => s,
            Err(e) => return refused(&e),
        };
        match operation {
            VoteOperation::Init => match signer.init(now()) {
                Ok(l) => serde_json::to_vec(&json!({"vote_ledger": "created", "admitted": l.admitted, "creation_nonce": l.creation_nonce})).unwrap_or_default(),
                Err(e) => refused(&e),
            },
            VoteOperation::MarkUnadmitted { reason } => match signer.mark_unadmitted(&reason, now()) {
                Ok(l) => serde_json::to_vec(&json!({"vote_ledger": "unadmitted", "unadmitted_since": l.unadmitted_since})).unwrap_or_default(),
                Err(e) => refused(&e),
            },
            VoteOperation::Readmit {
                operation_id,
                evidence_sha256,
            } => self.readmit(&signer, &operation_id, &evidence_sha256, uid, store, replica_id),
            VoteOperation::Sign { operation_id, payload } => self.sign(&signer, &operation_id, &payload, &store, replica_id),
            VoteOperation::Read { .. } | VoteOperation::Propose { .. } => b"{\"error\":\"vote_refused\"}".to_vec(),
        }
    }

    fn readmit(
        &self,
        signer: &Signer,
        operation_id: &str,
        evidence_sha256: &BTreeMap<String, String>,
        uid: u32,
        store: impl Fn() -> Result<Store>,
        replica_id: &str,
    ) -> Vec<u8> {
        let config = &self.config;
        let Ok(policy) = config.policy() else {
            return b"{\"error\":\"vote_configuration_invalid\"}".to_vec();
        };
        let mut known = vote::keys_of(&policy);
        let mut retired = BTreeSet::new();
        for key in &config.retired_keys {
            if let Ok(k) = quorum::verifying_key(&key.public_key) {
                known.insert(key.key_id.clone(), k);
                retired.insert(key.key_id.clone());
            }
        }
        let own_store = export(&store);
        let manager = &self.manager;
        let scope = Scope {
            manager,
            replica_id,
            replica_keys: &config.replica_keys,
            known_keys: known,
            policy_keys: policy.keys.iter().map(|k| k.key_id.clone()).collect(),
            retired_keys: retired,
            nodes: &config.nodes,
        };
        let refused = |e: readmission::ReadmissionRefusal| {
            serde_json::to_vec(&json!({"error": "readmission_refused", "code": e.code, "detail": e.detail,
                                       "unreadable": e.unreadable, "retry_at": e.retry_at, "alternative": e.alternative}))
            .unwrap_or_default()
        };
        let names = readmission::evidence_names(&scope, &config.key_id);
        if let Err(e) = readmission::check_evidence_dir(&config.evidence_dir, &self.dir, &names) {
            return refused(e);
        }
        let evidence = readmission::gather(
            &scope,
            &readmission::EvidenceSource {
                dir: &config.evidence_dir,
                stated: evidence_sha256,
            },
            &self.dir,
            &config.key_id,
            own_store.map_err(|e| e.to_string()),
        );
        match readmission::readmit(signer, &scope, evidence, operation_id, Some(uid), now()) {
            Ok(admission) => {
                serde_json::to_vec(&json!({"vote_ledger": "admitted", "admission": admission}))
                    .unwrap_or_default()
            }
            Err(e) => refused(e),
        }
    }

    /// The decision rules for `payload`, when this replica decides (V3-5): a takeover certificate of
    /// a resource it decides, checked against its own view of `facts`. Without `votes.decisions`,
    /// nothing is checked here (V3-4).
    fn check_rules(
        &self,
        payload: &Value,
        facts: &[Fact],
    ) -> std::result::Result<(), decisions::DecisionRefusal> {
        let Some(d) = &self.config.decisions else {
            return Ok(());
        };
        let policy = self
            .config
            .policy()
            .map_err(|e| decisions::DecisionRefusal {
                code: "vote_configuration_invalid",
                detail: e.to_string(),
            })?;
        if payload["kind"].as_str() != Some(quorum::QUORUM_PROOF_KIND) {
            return Err(decisions::DecisionRefusal {
                code: "decision_kind",
                detail: "a replica that decides signs takeover certificates only; a change of the authority set is the operator's".into(),
            });
        }
        let resource = payload["resource"].as_str().unwrap_or_default();
        let Some(rules) = d.rules(resource) else {
            return Err(decisions::DecisionRefusal {
                code: "resource_not_decided_here",
                detail: format!("{resource} is not among the resources this replica decides"),
            });
        };
        let counted =
            decisions::verified(&policy, &counted_votes(facts, &self.config.replica_keys));
        let view = decisions::view(&policy, rules, &counted);
        decisions::check(
            &policy,
            rules,
            &self.config.nodes,
            &view,
            payload,
            now(),
            self.config.max_certificate_life_seconds,
        )
        .map(|_| ())
    }

    /// This replica's own recorded vote for exactly `payload`, if its ledger still holds the promise
    /// it was made under: what a retry answers again, with nothing signed or appended twice.
    fn recorded_vote(
        &self,
        signer: &Signer,
        payload: &Value,
        facts: &[Fact],
        replica_id: &str,
    ) -> Option<Recorded> {
        let target = vote::Decision::of_payload(payload).ok()?;
        let ledger = signer.load().ok().filter(|l| l.admitted)?;
        let promise = ledger.promise(target.kind, &target.resource, target.number)?;
        if promise.decision_digest != target.decision_digest {
            return None;
        }
        let own: BTreeMap<String, ed25519_dalek::VerifyingKey> =
            [(self.config.key_id.clone(), signer.verifying_key())].into();
        let scope = vote::vote_scope(replica_id);
        facts.iter().rev().find_map(|fact| {
            if fact.scope != scope || fact.origin_replica_id != replica_id {
                return None;
            }
            let value: Value = serde_json::from_str(&fact.value).ok()?;
            let verified = vote::verify_origin(&value, &own).ok()?;
            (verified.voter == self.config.key_id
                && verified.decision.payload_digest == target.payload_digest)
                .then(|| Recorded {
                    vote: value,
                    fact: fact.event_id.clone(),
                    replayed: true,
                })
        })
    }

    /// Signs a vote for `payload` under the ledger and records it in this replica's vote scope
    /// before it is released, or answers the vote already recorded for it.
    fn sign_and_record(
        &self,
        signer: &Signer,
        operation_id: &str,
        payload: &Value,
        facts: &[Fact],
        store: &impl Fn() -> Result<Store>,
        replica_id: &str,
    ) -> std::result::Result<Recorded, LedgerRefusal> {
        if let Some(recorded) = self.recorded_vote(signer, payload, facts, replica_id) {
            return Ok(recorded);
        }
        // The votes this replica can see, its own and its peers', for the tripwire.
        let seen = votes_in(facts);
        let vote = signer.sign(payload, &seen, now()).inspect_err(|e| {
            if TRIPWIRE_CODES.contains(&e.code) {
                self.alert(format!("{}: {}", e.code, e.detail));
            }
        })?;
        // The vote is recorded in this replica's own vote scope before it is released: the promise
        // is already durable in the ledger, and a vote that is not recorded is not answered.
        let unrecorded = |detail: String| crate::ledger::refusal("vote_unrecorded", detail);
        let decision = vote::Decision::of_payload(payload).map_err(|e| unrecorded(e.detail))?;
        let kind = match decision.kind {
            PromiseKind::Epoch => "epoch",
            PromiseKind::Serial => "serial",
        };
        let executed = store()
            .and_then(|mut s| {
                Ok(s.execute_with_receipt(&Request::Observe {
                    operation_id: operation_id.to_string(),
                    scope: vote::vote_scope(replica_id),
                    subject: format!("{kind}:{}:{}", decision.resource, decision.number),
                    exclusive_resource: None,
                    active_claim: false,
                    value: vote.to_string(),
                })?)
            })
            .map_err(|e| unrecorded(e.to_string()))?;
        match executed.response {
            Response::Observed { fact } => Ok(Recorded {
                vote,
                fact: fact.event_id,
                replayed: false,
            }),
            _ => Err(unrecorded(
                "the store did not answer with the recorded fact".into(),
            )),
        }
    }

    fn sign(
        &self,
        signer: &Signer,
        operation_id: &str,
        payload: &Value,
        store: &impl Fn() -> Result<Store>,
        replica_id: &str,
    ) -> Vec<u8> {
        let Ok(facts) = export(store) else {
            return b"{\"error\":\"vote_store_unreadable\"}".to_vec();
        };
        // A retry first: the vote already recorded for this very payload is answered again, even
        // once the view has moved past it (its certificate formed meanwhile, say).
        if let Some(r) = self.recorded_vote(signer, payload, &facts, replica_id) {
            return answer(&json!({"vote": r.vote, "fact": r.fact, "replayed": r.replayed}));
        }
        if let Err(e) = self.check_rules(payload, &facts) {
            return decision_refused("vote_refused", e.code, &e.detail);
        }
        match self.sign_and_record(signer, operation_id, payload, &facts, store, replica_id) {
            Ok(r) => answer(&json!({"vote": r.vote, "fact": r.fact, "replayed": r.replayed})),
            Err(e) if e.code == "vote_unrecorded" => b"{\"error\":\"vote_unrecorded\"}".to_vec(),
            Err(e) => refused(&e),
        }
    }

    /// Records a proposal: the full payload of a takeover certificate, in this replica's scope
    /// `proposals/<replica_id>`. The proposal is checked for its shape, its policy, its resource and
    /// its life only; whether anyone votes for it is each voter's own check, and the answer carries
    /// this replica's (`here`).
    fn propose(
        &self,
        operation_id: &str,
        payload: &Value,
        store: &impl Fn() -> Result<Store>,
        replica_id: &str,
    ) -> Vec<u8> {
        let refused = |code: &str, detail: &str| decision_refused("proposal_refused", code, detail);
        let Some(d) = &self.config.decisions else {
            return refused(
                "decisions_not_configured",
                "this replica decides nothing: votes.decisions is absent",
            );
        };
        let scope = decisions::proposal_scope(replica_id);
        if !self
            .manager
            .grants
            .iter()
            .any(|g| g.scope == scope && g.owner_replica_id == replica_id)
        {
            return refused(
                "proposals_not_granted",
                &format!("the topology does not grant {scope} to this replica"),
            );
        }
        let Ok(policy) = self.config.policy() else {
            return refused(
                "vote_configuration_invalid",
                "the vote configuration's policy",
            );
        };
        let decision = match vote::Decision::of_payload(payload) {
            Ok(decision) if decision.kind == PromiseKind::Epoch => decision,
            Ok(_) => {
                return refused(
                    "decision_kind",
                    "a proposal is a takeover certificate's payload",
                )
            }
            Err(e) => return refused("payload_invalid", &e.detail),
        };
        if d.rules(&decision.resource).is_none() {
            return refused(
                "resource_not_decided_here",
                &format!(
                    "{} is not among the resources this replica decides",
                    decision.resource
                ),
            );
        }
        if payload["authority_id"].as_str() != Some(policy.authority_id.as_str())
            || payload["policy_digest"].as_str() != Some(policy.digest().as_str())
        {
            return refused(
                "policy_mismatch",
                "the payload names another authority set than the one the replicas vote under",
            );
        }
        let now = now();
        if decision.expires_at <= now
            || decision.expires_at - decision.issued_at > self.config.max_certificate_life_seconds
        {
            return refused("certificate_life", "a proposal is recorded only while live, living at most the longest certificate life");
        }
        let value = decisions::proposal_value(payload).to_string();
        if value.len() > 4096 {
            return refused("payload_invalid", "a proposal is at most 4096 bytes");
        }
        let recorded = store().and_then(|mut s| {
            Ok(s.execute_with_receipt(&Request::Observe {
                operation_id: operation_id.to_string(),
                scope: scope.clone(),
                subject: decisions::proposal_subject(&decision.resource, decision.number),
                exclusive_resource: None,
                active_claim: false,
                value,
            })?)
        });
        let fact = match recorded {
            Ok(executed) => match executed.response {
                Response::Observed { fact } => fact.event_id,
                _ => return b"{\"error\":\"proposal_unrecorded\"}".to_vec(),
            },
            Err(e) => return refused("proposal_unrecorded", &e.to_string()),
        };
        let here = export(store)
            .map_err(|e| decisions::DecisionRefusal {
                code: "vote_store_unreadable",
                detail: e.to_string(),
            })
            .and_then(|facts| self.check_rules(payload, &facts));
        answer(&json!({
            "proposal": fact,
            "payload_digest": decision.payload_digest,
            "here": match here { Ok(()) => json!({"verdict": "passes"}), Err(e) => json!({"verdict": "refused", "code": e.code, "detail": e.detail}) },
        }))
    }

    /// What this replica's store says about one resource's decision.
    fn read(&self, resource: &str, store: &impl Fn() -> Result<Store>) -> Vec<u8> {
        let Some(rules) = self
            .config
            .decisions
            .as_ref()
            .and_then(|d| d.rules(resource))
        else {
            return decision_refused(
                "decision_refused",
                "resource_not_decided_here",
                "this replica decides no such resource",
            );
        };
        let Ok(policy) = self.config.policy() else {
            return decision_refused(
                "decision_refused",
                "vote_configuration_invalid",
                "the vote configuration's policy",
            );
        };
        let facts = match export(store) {
            Ok(facts) => facts,
            Err(e) => {
                return decision_refused(
                    "decision_refused",
                    "vote_store_unreadable",
                    &e.to_string(),
                )
            }
        };
        let counted =
            decisions::verified(&policy, &counted_votes(&facts, &self.config.replica_keys));
        let proposals = decisions::proposals_in(&facts);
        let here = self.verdicts.lock().map(|v| v.clone()).unwrap_or_default();
        answer(
            &json!({"decision": decisions::read(&policy, rules, &counted, &proposals, &here, now())}),
        )
    }

    /// One pass of the voter: reads the proposals the store holds, checks each live one above the
    /// view against this replica's view, and votes for one that passes, under the ledger. Records a
    /// verdict for each. Returns how many votes it recorded.
    ///
    /// # Errors
    /// The store could not be read; nothing was voted.
    pub(crate) fn decide_once(
        &self,
        store: impl Fn() -> Result<Store>,
        replica_id: &str,
    ) -> Result<usize> {
        let started = std::time::Instant::now();
        let outcome = self.decide_pass(&store, replica_id);
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Ok(mut v) = self.voter.lock() {
            v.passes += 1;
            v.last_pass_ms = Some(ms);
            v.longest_pass_ms = v.longest_pass_ms.max(ms);
            match &outcome {
                Ok(cast) => {
                    v.votes_cast += *cast as u64;
                    v.last_error = None;
                }
                Err(e) => v.last_error = Some(e.to_string()),
            }
        }
        outcome
    }

    fn decide_pass(&self, store: &impl Fn() -> Result<Store>, replica_id: &str) -> Result<usize> {
        let Some(d) = &self.config.decisions else {
            return Ok(0);
        };
        let policy = self.config.policy()?;
        let facts = export(store)?;
        let counted =
            decisions::verified(&policy, &counted_votes(&facts, &self.config.replica_keys));
        let proposals = decisions::proposals_in(&facts);
        let voted = decisions::voters_by_payload(&counted);
        let now = now();
        let mut verdicts = BTreeMap::new();
        let mut cast = 0;
        for rules in &d.resources {
            let view = decisions::view(&policy, rules, &counted);
            let mut candidates: Vec<&Proposal> = proposals
                .iter()
                .filter(|p| p.decision.resource == rules.resource && p.decision.number > view.epoch)
                .collect();
            candidates.sort_by(|a, b| {
                (
                    a.decision.number,
                    a.decision.issued_at,
                    &a.decision.payload_digest,
                )
                    .cmp(&(
                        b.decision.number,
                        b.decision.issued_at,
                        &b.decision.payload_digest,
                    ))
            });
            for p in candidates {
                let digest = p.decision.payload_digest.clone();
                let verdict = if voted
                    .get(&digest)
                    .is_some_and(|v| v.contains(&self.config.key_id))
                {
                    json!({"verdict": "voted"})
                } else if p.decision.expires_at <= now {
                    json!({"verdict": "expired"})
                } else {
                    match decisions::check(
                        &policy,
                        rules,
                        &self.config.nodes,
                        &view,
                        &p.payload,
                        now,
                        self.config.max_certificate_life_seconds,
                    ) {
                        Err(e) => json!({"verdict": "refused", "code": e.code, "detail": e.detail}),
                        // A pass can vote for more than one proposal. Re-read the host and VM
                        // witnesses before each signature, not just once at the start of the pass.
                        Ok(_) => match self.signer() {
                            Err(e) => {
                                json!({"verdict": "refused", "code": e.code, "detail": e.detail})
                            }
                            Ok(signer) => match self.sign_and_record(
                                &signer,
                                &format!("decide-{digest}"),
                                &p.payload,
                                &facts,
                                store,
                                replica_id,
                            ) {
                                Ok(r) => {
                                    if !r.replayed {
                                        cast += 1;
                                    }
                                    json!({"verdict": "voted", "fact": r.fact})
                                }
                                Err(e) => {
                                    json!({"verdict": "refused", "code": e.code, "detail": e.detail})
                                }
                            },
                        },
                    }
                };
                verdicts.insert(digest, verdict);
            }
        }
        if let Ok(mut v) = self.verdicts.lock() {
            *v = verdicts;
        }
        Ok(cast)
    }
}

/// Every vote a set of facts carries, in any replica's vote scope, as JSON: what the tripwire reads.
/// Whether each one counts is decided by whoever reads it, by its signatures.
#[must_use]
pub fn votes_in(facts: &[Fact]) -> Vec<Value> {
    facts
        .iter()
        .filter(|f| {
            f.scope
                .starts_with(&format!("{}/", vote::VOTE_SCOPE_PREFIX))
        })
        .filter_map(|f| serde_json::from_str(&f.value).ok())
        .collect()
}

/// The votes a set of facts carries that a decision counts: each read from its fact with
/// `vote_of_fact`, so that it comes from the scope of the replica that owns its voter's key. Their
/// signatures are verified again wherever they are counted.
#[must_use]
pub fn counted_votes(facts: &[Fact], replica_keys: &BTreeMap<String, String>) -> Vec<Value> {
    facts
        .iter()
        .filter_map(|f| vote::vote_of_fact(f, replica_keys))
        .filter_map(std::result::Result::ok)
        .collect()
}
