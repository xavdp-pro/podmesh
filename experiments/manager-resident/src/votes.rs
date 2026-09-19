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
//! Proposing, deciding and delivering certificates are V3-5's.
use crate::{
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
    path::PathBuf,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The environment variable naming the host-provided vote directory.
pub const VOTE_DIR_ENV: &str = "PODMESH_MANAGER_VOTE_DIR";
/// The environment variable naming the host's machine-id file, mounted read-only.
pub const HOST_ID_FILE_ENV: &str = "PODMESH_MANAGER_HOST_ID_FILE";
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
}

impl VoteOperation {
    /// Whether only the operator may ask it.
    pub(crate) fn operator_only(&self) -> bool {
        !matches!(self, VoteOperation::Sign { .. })
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
        Ok(Some(VoteRuntime {
            config: votes.clone(),
            manager: config.network.manager.clone(),
            dir,
            host_id_file,
            alerts: Mutex::new(Vec::new()),
        }))
    }

    fn signer(&self) -> std::result::Result<Signer, LedgerRefusal> {
        let host = HostIdentity::read(&self.host_id_file)?;
        let policy = self
            .config
            .policy()
            .map_err(|e| crate::ledger::refusal("key_not_in_policy", e.to_string()))?;
        Signer::open(
            &self.dir,
            &self.config.key_id,
            host,
            policy,
            SigningRules {
                max_certificate_life: self.config.max_certificate_life_seconds,
                lock_wait: LOCK_WAIT,
            },
        )
    }

    fn alert(&self, text: String) {
        eprintln!("resident vote alert: {text}");
        if let Ok(mut alerts) = self.alerts.lock() {
            alerts.push(text);
            let excess = alerts.len().saturating_sub(ALERTS_KEPT);
            alerts.drain(..excess);
        }
    }

    /// The status's `votes` section: the ledger's state as a signer would find it, and the alerts.
    pub(crate) fn status(&self) -> Value {
        let alerts = self.alerts.lock().map(|a| a.clone()).unwrap_or_default();
        let ledger = self.signer().and_then(|s| s.load());
        let (state, detail) = match &ledger {
            Ok(l) if l.admitted => ("admitted", None),
            Ok(_) => ("unadmitted", None),
            Err(e) => (e.code, Some(e.detail.clone())),
        };
        let ledger = ledger.ok();
        json!({
            "key_id": self.config.key_id,
            "ledger_state": state,
            "ledger_detail": detail,
            "sequence": ledger.as_ref().map(|l| l.sequence),
            "unadmitted_since": ledger.as_ref().and_then(|l| l.unadmitted_since),
            "unadmitted_reason": ledger.as_ref().and_then(|l| l.unadmitted_reason.clone()),
            "admissions": ledger.as_ref().map(|l| l.admissions.len()),
            "alerts": alerts,
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
            VoteOperation::Sign { operation_id, payload } => self.sign(&signer, &operation_id, &payload, store, replica_id),
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
        let own_store = store().and_then(|mut s| match s.execute(&Request::Export {})? {
            Response::Snapshot { snapshot } => Ok(snapshot.facts),
            _ => Err("unexpected export response".into()),
        });
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

    fn sign(
        &self,
        signer: &Signer,
        operation_id: &str,
        payload: &Value,
        store: impl Fn() -> Result<Store>,
        replica_id: &str,
    ) -> Vec<u8> {
        // The votes this replica can see, its own and its peers', for the tripwire.
        let facts: Vec<Fact> =
            match store().and_then(|mut s| match s.execute(&Request::Export {})? {
                Response::Snapshot { snapshot } => Ok(snapshot.facts),
                _ => Err("unexpected export response".into()),
            }) {
                Ok(facts) => facts,
                Err(_) => return b"{\"error\":\"vote_store_unreadable\"}".to_vec(),
            };
        let seen = votes_in(&facts);
        let vote = match signer.sign(payload, &seen, now()) {
            Ok(vote) => vote,
            Err(e) => {
                if TRIPWIRE_CODES.contains(&e.code) {
                    self.alert(format!("{}: {}", e.code, e.detail));
                }
                return refused(&e);
            }
        };
        // The vote is recorded in this replica's own vote scope before it is released: the promise
        // is already durable in the ledger, and a vote that is not recorded is not answered.
        let Ok(decision) = vote::Decision::of_payload(payload) else {
            return b"{\"error\":\"vote_unrecorded\"}".to_vec();
        };
        let kind = match decision.kind {
            PromiseKind::Epoch => "epoch",
            PromiseKind::Serial => "serial",
        };
        let recorded = store().and_then(|mut s| {
            Ok(s.execute_with_receipt(&Request::Observe {
                operation_id: operation_id.to_string(),
                scope: vote::vote_scope(replica_id),
                subject: format!("{kind}:{}:{}", decision.resource, decision.number),
                exclusive_resource: None,
                active_claim: false,
                value: vote.to_string(),
            })?)
        });
        match recorded {
            Ok(executed) => match executed.response {
                Response::Observed { fact } => {
                    serde_json::to_vec(&json!({"vote": vote, "fact": fact.event_id}))
                        .unwrap_or_default()
                }
                _ => b"{\"error\":\"vote_unrecorded\"}".to_vec(),
            },
            Err(_) => b"{\"error\":\"vote_unrecorded\"}".to_vec(),
        }
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
