//! The operator's readmission of a signing ledger (V3-4, review finding 1: it fails closed).
//!
//! A ledger that may have gone backwards (restored with its host, reverted with a VM snapshot,
//! caught by the tripwire, or new) signs nothing until the operator readmits it. Readmission never
//! guesses what the key promised before. It reads every input that could show it:
//!
//! - this replica's own store, and every other replica's store (an inspection of its facts, as
//!   `--inspect-store --facts-only` prints it), for the votes they hold, of every key it knows;
//! - every other key's ledger, and any ledger of this host's retired keys still in the directory;
//! - every node's epoch screen (its `activation_status` answers), which moves on certificates only.
//!
//! If any of them cannot be read, is not what it claims to be, was collected before the ledger
//! stopped signing, or holds a vote of a key this replica does not know, it refuses and names each
//! one (`readmission_inputs_unreadable`). The operator then waits until the input can be read:
//! nothing replaces reading it. Re-keying (a new key and a new ledger for the replica, an
//! authority-set change) is the way out for a ledger that cannot itself be readmitted -- lost,
//! unreadable, or bound to another host -- and its new ledger is admitted by this same operation,
//! from the same inputs, reading the old key's votes as a retired key's, so it gets the same floors.
//! Readmission also waits: until the longest life a vote may give a certificate, plus the clock
//! skew, has passed since the ledger was marked,
//! so that a signature the ledger forgot and a proposer still holds has expired (review finding 1,
//! "the wait stays, for live grants, not as a substitute for the floor").
//!
//! Otherwise it sets, for every resource it saw, an epoch floor at the highest epoch anything showed
//! (a promise, a vote, a screen) and a serial floor at the highest `from_serial` promised or already
//! passed, so the key signs only above everything seen; it raises the sequence number above every
//! vote of this key it read, so the tripwire does not fire on its old votes; and it records what it
//! read, with digests, and what it set. Floors only rise.
use crate::ledger::{self, Admission, FloorSet, InputRead, Ledger, LedgerRefusal, Signer};
use crate::quorum;
use crate::vote::{self, PromiseKind};
use ed25519_dalek::VerifyingKey;
use podmesh_manager_ha_lab::{
    durable::{facts_history_sha256, Configuration as Manager},
    Fact,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
};

/// The form of an evidence file.
pub const EVIDENCE_FORM: &str = "podmesh-manager-readmission-evidence/1";
/// The directory, inside the vote directory, the operator places the evidence in.
pub const EVIDENCE_DIR: &str = "readmission";
const EVIDENCE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Why readmission refused, by name, with every input it could not read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReadmissionRefusal {
    pub code: &'static str,
    pub detail: String,
    pub unreadable: Vec<Unreadable>,
    pub retry_at: Option<i64>,
    pub alternative: &'static str,
}

impl std::fmt::Display for ReadmissionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "readmission refused ({}): {}", self.code, self.detail)?;
        for u in &self.unreadable {
            write!(f, "; {} {}: {}", u.input, u.source, u.reason)?;
        }
        Ok(())
    }
}

impl std::error::Error for ReadmissionRefusal {}

/// The operator's alternatives, said in every refusal.
pub const ALTERNATIVE: &str = "wait until every input can be read: nothing replaces reading it. A ledger that cannot itself be readmitted (lost, unreadable, or bound to another host) is replaced by re-keying: a new key and ledger (an authority-set change, serial + 1), admitted by this same operation from the same inputs, reading the old key's votes as a retired key's";

/// One input readmission could not read, and why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Unreadable {
    pub input: String,
    pub source: String,
    pub reason: String,
}

/// A node's screen for one resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenEntry {
    pub resource: String,
    pub highest_epoch_seen: Option<i64>,
    pub authority_serial: Option<i64>,
}

/// What an input held.
#[derive(Debug, Clone)]
pub enum Content {
    Facts(Vec<Fact>),
    Ledger(Box<Ledger>),
    Screen(Vec<ScreenEntry>),
}

/// One input read.
#[derive(Debug, Clone)]
pub struct Item {
    pub input: &'static str,
    pub source: String,
    pub sha256: String,
    pub collected_at: Option<i64>,
    pub content: Content,
}

/// Everything readmission read, and everything it could not.
#[derive(Debug, Clone, Default)]
pub struct Evidence {
    pub items: Vec<Item>,
    pub unreadable: Vec<Unreadable>,
}

/// What must be read, from the replica's configuration.
pub struct Scope<'a> {
    pub manager: &'a Manager,
    pub replica_id: &'a str,
    /// Every replica's key, by replica.
    pub replica_keys: &'a BTreeMap<String, String>,
    /// Every key of the policy, and the retired keys whose votes may still be read.
    pub known_keys: BTreeMap<String, VerifyingKey>,
    /// The policy's keys.
    pub policy_keys: BTreeSet<String>,
    /// This host's retired keys, whose ledgers may still sit in the vote directory.
    pub retired_keys: BTreeSet<String>,
    /// Every node whose screen must be read.
    pub nodes: &'a [String],
}

fn unreadable(input: &str, source: &str, reason: impl Into<String>) -> Unreadable {
    Unreadable {
        input: input.into(),
        source: source.into(),
        reason: reason.into(),
    }
}

fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(EVIDENCE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > EVIDENCE_MAX_BYTES {
        return Err(std::io::Error::other("larger than its bound"));
    }
    Ok(bytes)
}

/// Checks and reads a store inspection's facts: its count and its digest recomputed, every fact
/// valid under the topology.
fn facts_of(manager: &Manager, replica_id: &str, content: &Value) -> Result<Vec<Fact>, String> {
    let facts: Vec<Fact> = serde_json::from_value(content["ordered_facts"].clone())
        .map_err(|e| format!("ordered_facts: {e}"))?;
    if content["history_count"].as_u64() != Some(facts.len() as u64) {
        return Err("history_count is not the number of facts".into());
    }
    let digest = facts_history_sha256(manager, &facts).map_err(|e| e.to_string())?;
    if content["logical_history_sha256"].as_str() != Some(digest.as_str()) {
        return Err("logical_history_sha256 does not match the facts: altered, truncated or of another manager".into());
    }
    let mut replica = manager
        .topology()
        .and_then(|t| t.instantiate(replica_id))
        .map_err(|e| e.to_string())?;
    for fact in &facts {
        replica
            .ingest(fact.clone())
            .map_err(|e| format!("fact {}: {e}", fact.event_id))?;
    }
    Ok(facts)
}

fn screen_of(node: &str, content: &Value) -> Result<Vec<ScreenEntry>, String> {
    let answers = content["activation_status"]
        .as_array()
        .ok_or("activation_status must be the list of the node's answers")?;
    answers
        .iter()
        .map(|a| {
            if a["this_host_uuid"].as_str() != Some(node) {
                return Err(format!("an answer of another host than {node}"));
            }
            let resource = a["universe_uuid"]
                .as_str()
                .ok_or("an answer names no universe_uuid")?;
            let integer = |name: &str| -> Result<Option<i64>, String> {
                match &a[name] {
                    Value::Null => Ok(None),
                    v => v
                        .as_i64()
                        .map(Some)
                        .ok_or(format!("{name} is not an integer")),
                }
            };
            Ok(ScreenEntry {
                resource: resource.into(),
                highest_epoch_seen: integer("highest_epoch_seen")?,
                authority_serial: integer("authority_serial")?,
            })
        })
        .collect()
}

/// Reads one evidence file of the evidence directory.
fn evidence_file(
    scope: &Scope<'_>,
    dir: &Path,
    input: &'static str,
    source: &str,
) -> Result<Item, Unreadable> {
    let path = dir
        .join(EVIDENCE_DIR)
        .join(format!("{input}.{source}.json"));
    let fail = |reason: String| unreadable(input, source, reason);
    let bytes = read_file(&path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let envelope: Value =
        serde_json::from_slice(&bytes).map_err(|e| fail(format!("does not parse: {e}")))?;
    if envelope["form"].as_str() != Some(EVIDENCE_FORM)
        || envelope["input"].as_str() != Some(input)
        || envelope["source"].as_str() != Some(source)
    {
        return Err(fail(format!(
            "not an evidence file of form {EVIDENCE_FORM} for {input} {source}"
        )));
    }
    let collected_at = envelope["collected_at"]
        .as_i64()
        .ok_or_else(|| fail("collected_at is required".into()))?;
    let content = &envelope["content"];
    let content = match input {
        "store" => {
            Content::Facts(facts_of(scope.manager, scope.replica_id, content).map_err(fail)?)
        }
        "ledger" => {
            let text = serde_json::to_vec(content).map_err(|e| fail(e.to_string()))?;
            let ledger = Ledger::parse(&text).map_err(|e| fail(e.detail))?;
            if ledger.key_id != source {
                return Err(fail(format!("a ledger of key {}", ledger.key_id)));
            }
            Content::Ledger(Box::new(ledger))
        }
        _ => Content::Screen(screen_of(source, content).map_err(fail)?),
    };
    Ok(Item {
        input,
        source: source.into(),
        sha256: quorum::sha256_hex(&bytes),
        collected_at: Some(collected_at),
        content,
    })
}

/// Gathers every input the scope requires: the evidence files in `<vote_dir>/readmission/`
/// (`store.<replica>.json` for every other replica, `ledger.<key>.json` for every other key of the
/// policy, `screen.<node>.json` for every node), this replica's own store as the caller read it, and
/// any retired key's ledger in the vote directory. What cannot be read is named, never skipped.
#[must_use]
pub fn gather(
    scope: &Scope<'_>,
    vote_dir: &Path,
    own_key: &str,
    own_store: Result<Vec<Fact>, String>,
) -> Evidence {
    let mut evidence = Evidence::default();
    let mut take = |result: Result<Item, Unreadable>| match result {
        Ok(item) => evidence.items.push(item),
        Err(u) => evidence.unreadable.push(u),
    };
    take(
        own_store
            .map_err(|e| unreadable("own_store", scope.replica_id, e))
            .map(|facts| Item {
                input: "own_store",
                source: scope.replica_id.into(),
                sha256: quorum::sha256_hex(&serde_json::to_vec(&facts).unwrap_or_default()),
                collected_at: None,
                content: Content::Facts(facts),
            }),
    );
    for replica in scope.manager.replicas.iter().map(|r| r.replica_id.as_str()) {
        if replica != scope.replica_id {
            take(evidence_file(scope, vote_dir, "store", replica));
        }
    }
    for key in &scope.policy_keys {
        if key != own_key {
            take(evidence_file(scope, vote_dir, "ledger", key));
        }
    }
    for node in scope.nodes {
        take(evidence_file(scope, vote_dir, "screen", node));
    }
    for key in &scope.retired_keys {
        let path = vote_dir.join(format!("{key}.ledger"));
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => take(
                read_file(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| {
                        let ledger = Ledger::parse(&bytes).map_err(|e| e.detail)?;
                        Ok(Item {
                            input: "retired_ledger",
                            source: key.clone(),
                            sha256: quorum::sha256_hex(&bytes),
                            collected_at: None,
                            content: Content::Ledger(Box::new(ledger)),
                        })
                    })
                    .map_err(|e| unreadable("retired_ledger", key, e)),
            ),
        }
    }
    evidence
}

/// The highest numbers seen, per resource and kind, and this key's highest vote sequence.
#[derive(Default)]
struct Seen {
    highest: BTreeMap<(String, PromiseKind), i64>,
    own_sequence: u64,
    votes_counted: usize,
    votes_ignored: Vec<String>,
    unknown: Vec<Unreadable>,
}

impl Seen {
    fn raise(&mut self, resource: &str, kind: PromiseKind, number: i64) {
        let entry = self
            .highest
            .entry((resource.to_string(), kind))
            .or_insert(number);
        *entry = (*entry).max(number);
    }

    fn ledger(&mut self, ledger: &Ledger) {
        for (resource, r) in &ledger.resources {
            for kind in [PromiseKind::Epoch, PromiseKind::Serial] {
                if let Some(n) = r.highest(kind) {
                    self.raise(resource, kind, n);
                }
            }
        }
    }

    fn facts(
        &mut self,
        item: &Item,
        facts: &[Fact],
        known: &BTreeMap<String, VerifyingKey>,
        own_key: &str,
    ) {
        for fact in facts {
            if !fact
                .scope
                .starts_with(&format!("{}/", vote::VOTE_SCOPE_PREFIX))
            {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&fact.value) else {
                self.votes_ignored.push(format!(
                    "{} in {} {}: not a vote",
                    fact.event_id, item.input, item.source
                ));
                continue;
            };
            match vote::verify_origin(&value, known) {
                Ok(v) => {
                    self.votes_counted += 1;
                    self.raise(&v.decision.resource, v.decision.kind, v.decision.number);
                    if v.voter == own_key {
                        self.own_sequence = self.own_sequence.max(v.ledger_sequence);
                    }
                }
                // A vote of a key this replica does not know may be a real promise of a retired key
                // that is not configured: an input it cannot read, not one it may skip.
                Err(e) if e.code == "unknown_voter" => self.unknown.push(unreadable(
                    item.input,
                    &item.source,
                    format!(
                        "{}: {}; name the key among the retired keys",
                        fact.event_id, e.detail
                    ),
                )),
                Err(e) => self.votes_ignored.push(format!(
                    "{} in {} {}: {e}",
                    fact.event_id, item.input, item.source
                )),
            }
        }
    }
}

fn refuse(
    code: &'static str,
    detail: impl Into<String>,
    unreadable: Vec<Unreadable>,
    retry_at: Option<i64>,
) -> ReadmissionRefusal {
    ReadmissionRefusal {
        code,
        detail: detail.into(),
        unreadable,
        retry_at,
        alternative: ALTERNATIVE,
    }
}

fn from_ledger(e: LedgerRefusal) -> ReadmissionRefusal {
    let detail = match e.code {
        "ledger_missing" => format!(
            "{}; a new key's ledger is created with vote_ledger_init, then readmitted",
            e.detail
        ),
        _ => e.detail,
    };
    refuse(e.code, detail, Vec::new(), None)
}

/// Readmits `signer`'s ledger from `evidence`, or refuses and names why. Under the ledger's lock, so
/// no signature interleaves.
///
/// # Errors
/// The ledger's own refusals (`ledger_missing`, `ledger_unreadable`, `ledger_foreign_key`,
/// `ledger_foreign_host`), `ledger_admitted`, `readmission_inputs_unreadable`,
/// `readmission_too_early`, and the storage refusal.
pub fn readmit(
    signer: &Signer,
    scope: &Scope<'_>,
    evidence: Evidence,
    operation_id: &str,
    by_uid: Option<u32>,
    now: i64,
) -> Result<Admission, ReadmissionRefusal> {
    let _lock = signer.lock().map_err(from_ledger)?;
    let mut ledger = signer.load().map_err(from_ledger)?;
    if ledger.admitted {
        return Err(refuse(
            "ledger_admitted",
            "the ledger is admitted; the restore procedure marks it unadmitted first",
            Vec::new(),
            None,
        ));
    }
    let since = ledger.unadmitted_since.unwrap_or(ledger.created_at);
    let mut missing = evidence.unreadable;
    for item in &evidence.items {
        if item.collected_at.is_some_and(|at| at < since) {
            missing.push(unreadable(
                item.input,
                &item.source,
                format!("collected at {}, before the ledger was marked unadmitted at {since}: it cannot show what was signed since", item.collected_at.unwrap_or_default()),
            ));
        }
    }
    let own_key = signer.key_id().to_string();
    let mut seen = Seen::default();
    seen.ledger(&ledger);
    for item in &evidence.items {
        match &item.content {
            Content::Facts(facts) => seen.facts(item, facts, &scope.known_keys, &own_key),
            Content::Ledger(l) => seen.ledger(l),
            Content::Screen(entries) => {
                for e in entries {
                    if let Some(epoch) = e.highest_epoch_seen {
                        seen.raise(&e.resource, PromiseKind::Epoch, epoch);
                    }
                    // A node at serial s has passed every change from below s.
                    if let Some(serial) = e.authority_serial.filter(|s| *s > 0) {
                        seen.raise(&e.resource, PromiseKind::Serial, serial - 1);
                    }
                }
            }
        }
    }
    missing.append(&mut seen.unknown);
    if !missing.is_empty() {
        return Err(refuse(
            "readmission_inputs_unreadable",
            format!(
                "{} input(s) could not be read; the ledger stays unadmitted and nothing is guessed",
                missing.len()
            ),
            missing,
            None,
        ));
    }
    let ready_at = since + signer.rules().max_certificate_life + ledger::CLOCK_SKEW_SECONDS;
    if now < ready_at {
        return Err(refuse(
            "readmission_too_early",
            format!("a signature this ledger forgot may still be live in a proposer's hands until {ready_at}"),
            Vec::new(),
            Some(ready_at),
        ));
    }
    let mut floors_set = BTreeMap::new();
    for ((resource, kind), number) in &seen.highest {
        let r = ledger.resources.entry(resource.clone()).or_default();
        match kind {
            PromiseKind::Epoch => r.epoch_floor = r.epoch_floor.max(*number),
            PromiseKind::Serial => r.serial_floor = r.serial_floor.max(Some(*number)),
        }
        floors_set.insert(
            resource.clone(),
            FloorSet {
                epoch_floor: r.epoch_floor,
                serial_floor: r.serial_floor,
            },
        );
    }
    let sequence_before = ledger.sequence;
    ledger.sequence = ledger.sequence.max(seen.own_sequence);
    let admission = Admission {
        at: now,
        operation_id: operation_id.into(),
        by_uid,
        unadmitted_since: since,
        unadmitted_reason: ledger.unadmitted_reason.clone().unwrap_or_default(),
        inputs_read: evidence
            .items
            .iter()
            .map(|i| InputRead {
                input: i.input.into(),
                source: i.source.clone(),
                sha256: i.sha256.clone(),
                collected_at: i.collected_at,
            })
            .collect(),
        votes_counted: seen.votes_counted,
        votes_ignored: seen.votes_ignored,
        floors_set,
        sequence_before,
        sequence_set: ledger.sequence,
    };
    ledger.admitted = true;
    ledger.admitted_at_sequence = ledger.sequence;
    ledger.unadmitted_since = None;
    ledger.unadmitted_reason = None;
    ledger.admissions.push(admission.clone());
    signer.store(&mut ledger).map_err(from_ledger)?;
    Ok(admission)
}
