//! The manager decides (V3-5): proposals, the voters' rules, and the decision a replica reads.
//!
//! A **proposal** is a fact: the full payload of a takeover certificate (the resource, the target
//! epoch, the new holder and its boot, the grant, the method, the barrier and the life), written by
//! one replica in its own scope `proposals/<replica_id>`. Anyone the proposing replica lets write
//! may propose; nothing about a proposal is trusted. Every replica that votes reads the proposals
//! its store holds, checks each one against **its own view**, and votes (a V3-4 vote, under its
//! signing ledger) only for one that passes. k votes of distinct keys on one payload are a
//! certificate, which any replica can assemble and read, and which a PodMesh node verifies itself.
//!
//! **The view** of a resource is what the replica's store proves: the highest-epoch certificate its
//! votes assemble into (the current epoch, its holder, its barrier and its expiry), or, before any
//! certificate, the operator's recorded baseline (the epoch the gate reached, for a resource that
//! moves from the gate to the replicas). A replica whose store lags sees a lower epoch and refuses
//! a proposal for the next one until it catches up: that costs a vote, never safety, since the
//! promise of one decision per epoch is the ledger's.
//!
//! **The rules**, in the order they are checked (`check`), each refusal named:
//! - the payload is a takeover certificate of this replica's policy, for a resource it decides, with
//!   no field the node does not bind, live on this clock, living at most the longest life;
//! - its holder is a node of the configuration, its barrier is no later than its expiry;
//! - it is the **next** epoch after the view's, with the view's holder as its previous holder;
//! - its method binds, as `MANAGER-PUBLISHER-CONTRACT.md` states the barrier ("The barrier, as it
//!   is"):
//!   - `first`: no epoch exists in the view, and no baseline;
//!   - `same_holder`: the new holder is the view's holder, and the barrier is carried: no earlier
//!     than the current epoch's own;
//!   - `lease_barrier`: the barrier is carried and, when the holder changes, it also covers every
//!     way the previous holder may still hold its lease without being told: its re-acquisition
//!     under the current certificate until that certificate expires, a renewal by itself until the
//!     latest second any holder may renew by itself (`renewal_not_after`, the operator's recorded
//!     bound: the follow mandates' `not_after`), and an acquisition at the proposal's issue; each
//!     plus the lease and the margin. Until the majority extends leases (V3-6), a change of holder
//!     is refused unless the operator recorded an explicit renewal bound, not 0, no earlier than the
//!     current certificate's expiry and the proposal's issue (review of V3-5, finding 2): the
//!     replicas cannot see a follow mandate, and a bound that does not cover the present would let
//!     the previous holder renew past the barrier. A holder change from a baseline is refused while
//!     the baseline does not name the gate's last proof's expiry (finding 1);
//!   - `fence_receipt`: refused. A receipt is the previous holder node's unsigned answer to its own
//!     fence; no replica can verify it, and a receipt that shortened the barrier would let one
//!     proposer start a second holder. It returns when a host reports its fence into its own
//!     replica, which the peers can read (detection and takeover, V3-7).
//!
//! What the majority does not decide here: that a host is lost. A rotation away from a holder that
//! may still be running waits for everything above, whatever a proposer says.
use crate::quorum::{Quorum, QUORUM_PROOF_KIND, TAKEOVER_FIELDS};
use crate::vote::{self, Decision, PromiseKind, VerifiedVote};
use podmesh_manager_ha_lab::Fact;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The form of a proposal fact's value.
pub const PROPOSAL_FORM: &str = "podmesh-manager-proposal/1";
/// Every replica's proposals live under this scope prefix, in its own scope `proposals/<replica>`.
pub const PROPOSAL_SCOPE_PREFIX: &str = "proposals";
/// How far ahead of a voter's clock a payload may be issued: the node's allowance.
pub const CLOCK_ALLOWANCE_SECONDS: i64 = 30;
/// How many proposals a decision read lists.
pub const PENDING_LISTED: usize = 8;

/// The scope a replica's proposals are written in.
#[must_use]
pub fn proposal_scope(replica_id: &str) -> String {
    format!("{PROPOSAL_SCOPE_PREFIX}/{replica_id}")
}

/// The subject of a proposal fact: one revision chain per (resource, epoch) and proposer.
#[must_use]
pub fn proposal_subject(resource: &str, epoch: i64) -> String {
    format!("proposal:{resource}:{epoch}")
}

/// What the operator records about one resource the replicas decide.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResourceRules {
    /// The resource (a universe UUID), as the nodes' policies name it.
    pub resource: String,
    /// The longest lease any holder of the resource holds, in seconds (the nodes' policy).
    pub lease_seconds: i64,
    /// The takeover margin, in seconds: the clock-skew budget between two hosts.
    pub takeover_margin_seconds: i64,
    /// The latest second at which any holder may still renew its lease by itself (the `not_after`
    /// of the follow mandates standing on the hosts); 0 when no host renews by itself. The barrier of
    /// a rotation to another holder is never earlier than this plus the lease and the margin.
    pub renewal_not_after: i64,
    /// The operator's recorded starting point, for a resource that moves from the gate to the
    /// replicas: the gate's last epoch, its holder and its barrier. Absent for a new resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<Baseline>,
}

/// The operator's recorded starting point of a resource.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub epoch: i64,
    pub holder: String,
    pub eligible_after: i64,
    /// The gate's last proof's `expires_at`: its holder may re-acquire under it until then. Until a
    /// certificate the replicas assembled passes the baseline, it is the view's `expires_at`, and a
    /// change of holder is refused while it is absent (review of V3-5, finding 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

/// The replicas' decisions: which resources this replica decides, and how often its voter reads
/// the store for proposals.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DecisionConfiguration {
    /// How often the voter reads the store for proposals, in milliseconds (100 to 60,000).
    pub voter_interval_ms: u64,
    /// The resources this replica decides.
    pub resources: Vec<ResourceRules>,
}

impl DecisionConfiguration {
    /// # Errors
    /// A named inconsistency.
    pub fn validate(&self, nodes: &[String]) -> Result<(), String> {
        if !(100..=60_000).contains(&self.voter_interval_ms) {
            return Err("votes.decisions.voter_interval_ms must be 100 to 60000".into());
        }
        if self.resources.is_empty() || self.resources.len() > 64 {
            return Err("votes.decisions.resources must name 1 to 64 resources".into());
        }
        let mut seen = BTreeSet::new();
        for r in &self.resources {
            identifier(&r.resource).map_err(|e| format!("votes.decisions resource: {e}"))?;
            if !seen.insert(r.resource.as_str()) {
                return Err(format!("votes.decisions names {} twice", r.resource));
            }
            if !(5..=3600).contains(&r.lease_seconds)
                || !(5..=3600).contains(&r.takeover_margin_seconds)
            {
                return Err(format!(
                    "{}: lease_seconds and takeover_margin_seconds must be 5 to 3600, as the node bounds them",
                    r.resource
                ));
            }
            if r.renewal_not_after < 0 {
                return Err(format!(
                    "{}: renewal_not_after must be 0 or a time",
                    r.resource
                ));
            }
            if let Some(b) = &r.baseline {
                if !(1..=i64::from(i32::MAX)).contains(&b.epoch) || !nodes.contains(&b.holder) {
                    return Err(format!(
                        "{}: a baseline names an epoch from 1 and a holder among votes.nodes",
                        r.resource
                    ));
                }
            }
        }
        Ok(())
    }

    /// The rules of `resource`, if this replica decides it.
    #[must_use]
    pub fn rules(&self, resource: &str) -> Option<&ResourceRules> {
        self.resources.iter().find(|r| r.resource == resource)
    }
}

/// An identifier as the node defines one: `[A-Za-z0-9][A-Za-z0-9_.:-]{0,95}`.
fn identifier(value: &str) -> Result<(), String> {
    let mut bytes = value.bytes();
    let ok = bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
        && value.len() <= 96
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(format!("{value:?} is not an identifier the node accepts"))
    }
}

/// Why a replica does not vote for a proposal, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecisionRefusal {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for DecisionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no vote ({}): {}", self.code, self.detail)
    }
}

impl std::error::Error for DecisionRefusal {}

fn refuse(code: &'static str, detail: impl Into<String>) -> DecisionRefusal {
    DecisionRefusal {
        code,
        detail: detail.into(),
    }
}

/// Where a view's current epoch comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewSource {
    /// No certificate and no baseline: nothing was ever decided.
    None,
    /// The operator's recorded baseline.
    Baseline,
    /// A certificate the store's votes assemble into.
    Certificate,
}

/// What one replica's store proves about one resource.
#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub resource: String,
    /// The current epoch: 0 when nothing was decided.
    pub epoch: i64,
    pub holder: Option<String>,
    /// The current epoch's barrier, which every later decision carries.
    pub barrier: i64,
    /// The current certificate's expiry: its holder may re-acquire under it until then.
    pub expires_at: i64,
    pub source: ViewSource,
    /// The current certificate, assembled.
    pub certificate: Option<Value>,
    pub payload_digest: Option<String>,
    /// Epochs of this resource for which two different decisions were each certified: a broken
    /// promise. The view decides nothing while one is present.
    pub conflicts: Vec<i64>,
}

/// Every vote of `votes` whose origin and policy verify under `policy`, verified once: what a view, a
/// read and a voter pass count.
#[must_use]
pub fn verified(policy: &Quorum, votes: &[Value]) -> Vec<VerifiedVote> {
    votes
        .iter()
        .filter_map(|v| vote::verify_vote(v, policy).ok())
        .collect()
}

/// The certificate of one payload from its verified votes, signatures in key order, checked with the
/// node's rules before it is returned.
fn certificate_of(policy: &Quorum, payload: &Value, votes: &[&VerifiedVote]) -> Option<Value> {
    let signatures: BTreeMap<&str, &str> = votes
        .iter()
        .map(|v| (v.voter.as_str(), v.signature.as_str()))
        .collect();
    let mut certificate = payload.clone();
    certificate["signatures"] = json!(signatures
        .iter()
        .map(|(key_id, signature)| json!({"key_id": key_id, "signature": signature}))
        .collect::<Vec<_>>());
    policy
        .verify(&certificate, QUORUM_PROOF_KIND, TAKEOVER_FIELDS)
        .ok()?;
    Some(certificate)
}

/// The view of `rules.resource` from `votes`, each already verified under `policy`.
#[must_use]
pub fn view(policy: &Quorum, rules: &ResourceRules, votes: &[VerifiedVote]) -> View {
    // The votes of this resource's epochs, by payload; a payload is certified when the threshold's
    // number of distinct keys voted for it.
    let mut by_payload: BTreeMap<&str, Vec<&VerifiedVote>> = BTreeMap::new();
    for v in votes {
        if v.decision.kind == PromiseKind::Epoch && v.decision.resource == rules.resource {
            let entry = by_payload
                .entry(v.decision.payload_digest.as_str())
                .or_default();
            if !entry.iter().any(|w| w.voter == v.voter) {
                entry.push(v);
            }
        }
    }
    let certified: Vec<&Vec<&VerifiedVote>> = by_payload
        .values()
        .filter(|v| v.len() >= policy.threshold)
        .collect();
    let mut decided: BTreeMap<i64, BTreeSet<&str>> = BTreeMap::new();
    for c in &certified {
        decided
            .entry(c[0].decision.number)
            .or_default()
            .insert(c[0].decision.decision_digest.as_str());
    }
    let conflicts: Vec<i64> = decided
        .into_iter()
        .filter(|(_, d)| d.len() > 1)
        .map(|(n, _)| n)
        .collect();
    let best = certified.into_iter().max_by_key(|c| {
        (
            c[0].decision.number,
            c[0].decision.issued_at,
            c[0].decision.payload_digest.clone(),
        )
    });
    let mut out = View {
        resource: rules.resource.clone(),
        epoch: 0,
        holder: None,
        barrier: 0,
        expires_at: 0,
        source: ViewSource::None,
        certificate: None,
        payload_digest: None,
        conflicts,
    };
    if let Some(b) = &rules.baseline {
        out.epoch = b.epoch;
        out.holder = Some(b.holder.clone());
        out.barrier = b.eligible_after;
        out.expires_at = b.expires_at.unwrap_or(0);
        out.source = ViewSource::Baseline;
    }
    if let Some(votes) = best {
        let d = &votes[0].decision;
        let payload = &votes[0].payload;
        if d.number > out.epoch {
            if let Some(certificate) = certificate_of(policy, payload, votes) {
                out.epoch = d.number;
                out.holder = Some(d.holder.clone());
                out.barrier = payload["eligible_after"]
                    .as_i64()
                    .unwrap_or(0)
                    .max(out.barrier);
                out.expires_at = d.expires_at;
                out.source = ViewSource::Certificate;
                out.payload_digest = Some(d.payload_digest.clone());
                out.certificate = Some(certificate);
            }
        }
    }
    out
}

/// The fields a proposal's payload may carry: the kind, the authority, the digest and what every
/// takeover certificate binds. Nothing else: a field the node does not read would still be covered
/// by every signature.
fn allowed_field(name: &str) -> bool {
    matches!(name, "kind" | "authority_id" | "policy_digest")
        || TAKEOVER_FIELDS.iter().any(|(f, _)| *f == name)
}

/// The barrier a proposal must carry, at least, under `view` and `rules`; `None` for a method this
/// replica does not vote for.
#[must_use]
pub fn required_barrier(rules: &ResourceRules, view: &View, payload: &Value) -> Option<i64> {
    let method = payload["method"].as_str().unwrap_or_default();
    let holder = payload["new_holder"].as_str();
    let issued = payload["issued_at"].as_i64().unwrap_or(0);
    let wait = rules.lease_seconds + rules.takeover_margin_seconds;
    match method {
        "first" | "same_holder" => Some(view.barrier),
        "lease_barrier" => {
            let changes = view.holder.is_some() && view.holder.as_deref() != holder;
            Some(if changes {
                view.barrier
                    .max(issued + wait)
                    .max(view.expires_at + wait)
                    .max(rules.renewal_not_after + wait)
            } else {
                view.barrier
            })
        }
        _ => None,
    }
}

/// Checks a proposal against this replica's view, on its clock `now`. `nodes` are the hosts that
/// may hold a resource; `max_life` is the longest life a vote gives a certificate.
///
/// # Errors
/// The rule the proposal breaks, by name.
pub fn check(
    policy: &Quorum,
    rules: &ResourceRules,
    nodes: &[String],
    view: &View,
    payload: &Value,
    now: i64,
    max_life: i64,
) -> Result<Decision, DecisionRefusal> {
    let object = payload
        .as_object()
        .ok_or_else(|| refuse("payload_invalid", "a proposal's payload must be an object"))?;
    if object.get("kind").and_then(Value::as_str) != Some(QUORUM_PROOF_KIND) {
        return Err(refuse(
            "decision_kind",
            format!("the replicas decide takeover certificates ({QUORUM_PROOF_KIND}) only"),
        ));
    }
    if let Some(extra) = object.keys().find(|k| !allowed_field(k)) {
        return Err(refuse(
            "payload_unknown_field",
            format!("a proposal carries no field the node does not bind: {extra}"),
        ));
    }
    let decision =
        Decision::of_payload(payload).map_err(|e| refuse("payload_invalid", e.detail))?;
    if decision.resource != rules.resource {
        return Err(refuse(
            "resource_not_decided_here",
            "another resource than these rules'",
        ));
    }
    if payload["authority_id"].as_str() != Some(policy.authority_id.as_str())
        || payload["policy_digest"].as_str() != Some(policy.digest().as_str())
    {
        return Err(refuse(
            "policy_mismatch",
            "the payload names another authority set than the one this replica votes under",
        ));
    }
    if decision.issued_at > now + CLOCK_ALLOWANCE_SECONDS
        || decision.expires_at <= now
        || decision.expires_at <= decision.issued_at
        || decision.expires_at - decision.issued_at > max_life
    {
        return Err(refuse(
            "certificate_life",
            format!("a proposal is voted only while live on this clock, issued at most {CLOCK_ALLOWANCE_SECONDS} s ahead, living at most {max_life} s"),
        ));
    }
    if !nodes.iter().any(|n| n == &decision.holder) {
        return Err(refuse(
            "holder_not_a_node",
            format!("{} is not a node this replica decides for", decision.holder),
        ));
    }
    for field in ["holder_boot_id", "grant_id"] {
        identifier(payload[field].as_str().unwrap_or_default())
            .map_err(|e| refuse("payload_invalid", format!("{field}: {e}")))?;
    }
    let eligible = payload["eligible_after"].as_i64().unwrap_or(0);
    if eligible > decision.expires_at {
        return Err(refuse(
            "barrier_after_expiry",
            "the barrier falls after the certificate's expiry: no node could ever accept it",
        ));
    }
    if !view.conflicts.is_empty() {
        return Err(refuse(
            "conflict_in_view",
            format!("two decisions were certified for epoch(s) {:?} of this resource: a broken promise, which the operator settles; this replica decides nothing for it meanwhile", view.conflicts),
        ));
    }
    if decision.number != view.epoch + 1 {
        return Err(refuse(
            "epoch_not_next",
            format!(
                "this replica's view is at epoch {} ({:?}); it votes for epoch {} only",
                view.epoch,
                view.source,
                view.epoch + 1
            ),
        ));
    }
    if payload["previous_holder"].as_str() != view.holder.as_deref() {
        return Err(refuse(
            "previous_holder_mismatch",
            format!(
                "the previous holder is {:?} in this replica's view",
                view.holder
            ),
        ));
    }
    let method = payload["method"].as_str().unwrap_or_default();
    match method {
        "first" if view.source != ViewSource::None => {
            return Err(refuse(
                "first_after_an_epoch",
                "an epoch was already decided for this resource",
            ));
        }
        "same_holder" if view.holder.as_deref() != Some(decision.holder.as_str()) => {
            return Err(refuse(
                "not_the_same_holder",
                format!(
                    "same_holder names {}, the current holder is {:?}",
                    decision.holder, view.holder
                ),
            ));
        }
        "fence_receipt" => {
            return Err(refuse(
                "fence_receipt_unverifiable",
                "a fence receipt is the previous holder node's unsigned answer: no replica can verify it, so none shortens a barrier",
            ));
        }
        "first" | "same_holder" | "lease_barrier" => {}
        other => {
            return Err(refuse(
                "method_unknown",
                format!("method {other} is not one the node knows"),
            ))
        }
    }
    let changes_holder =
        method == "lease_barrier" && view.holder.as_deref().is_some_and(|h| h != decision.holder);
    if changes_holder {
        if view.source == ViewSource::Baseline
            && rules
                .baseline
                .as_ref()
                .is_some_and(|b| b.expires_at.is_none())
        {
            return Err(refuse(
                "baseline_expiry_unknown",
                "the baseline does not name the gate's last proof's expiry: its holder may re-acquire under that proof until an unknown time",
            ));
        }
        if rules.renewal_not_after == 0 {
            return Err(refuse(
                "renewal_unbounded",
                "no renewal bound is recorded (renewal_not_after 0): until the majority extends leases (V3-6), a change of holder needs the operator's explicit bound on self-renewal",
            ));
        }
        let covered = view.expires_at.max(decision.issued_at);
        if rules.renewal_not_after < covered {
            return Err(refuse(
                "renewal_bound_too_early",
                format!(
                    "the recorded renewal bound {} is earlier than {covered}, the later of the current proof's expiry and this proposal's issue: a follow mandate may stand past it; freeze the mandates and record their latest not_after first",
                    rules.renewal_not_after
                ),
            ));
        }
    }
    let required = required_barrier(rules, view, payload).unwrap_or(i64::MAX);
    if eligible < required {
        return Err(refuse(
            "barrier_too_early",
            format!("the barrier is {eligible}; this replica's view requires at least {required}"),
        ));
    }
    Ok(decision)
}

/// One proposal found in a store.
#[derive(Debug, Clone)]
pub struct Proposal {
    /// The replica whose scope holds it.
    pub proposer: String,
    pub event_id: String,
    pub payload: Value,
    pub decision: Decision,
}

/// Every well-formed proposal a store holds, once per payload (the first fact that carries it), in
/// the order of their facts. A fact in a proposal scope whose origin is not the scope's replica, or
/// whose value is not a proposal, is skipped: it proposes nothing.
#[must_use]
pub fn proposals_in(facts: &[Fact]) -> Vec<Proposal> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for fact in facts {
        let Some(owner) = fact
            .scope
            .strip_prefix(PROPOSAL_SCOPE_PREFIX)
            .and_then(|s| s.strip_prefix('/'))
        else {
            continue;
        };
        if owner != fact.origin_replica_id {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&fact.value) else {
            continue;
        };
        if value["form"].as_str() != Some(PROPOSAL_FORM) {
            continue;
        }
        let payload = value["payload"].clone();
        let Ok(decision) = Decision::of_payload(&payload) else {
            continue;
        };
        if decision.kind != PromiseKind::Epoch || !seen.insert(decision.payload_digest.clone()) {
            continue;
        }
        out.push(Proposal {
            proposer: owner.to_string(),
            event_id: fact.event_id.clone(),
            payload,
            decision,
        });
    }
    out
}

/// The value of a proposal fact.
#[must_use]
pub fn proposal_value(payload: &Value) -> Value {
    json!({"form": PROPOSAL_FORM, "payload": payload})
}

/// The keys that voted for each payload.
#[must_use]
pub fn voters_by_payload(votes: &[VerifiedVote]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for v in votes {
        out.entry(v.decision.payload_digest.clone())
            .or_default()
            .insert(v.voter.clone());
    }
    out
}

/// What a replica answers to `decision_read` for one resource: the current decision with its
/// certificate, or, above it, each live proposal with who voted, what is missing, and this
/// replica's own verdict (`here`).
#[must_use]
pub fn read(
    policy: &Quorum,
    rules: &ResourceRules,
    votes: &[VerifiedVote],
    proposals: &[Proposal],
    here: &BTreeMap<String, Value>,
    now: i64,
) -> Value {
    let view = view(policy, rules, votes);
    let voters = voters_by_payload(votes);
    let mut pending: Vec<&Proposal> = proposals
        .iter()
        .filter(|p| p.decision.resource == rules.resource && p.decision.number > view.epoch)
        .collect();
    pending.sort_by_key(|p| std::cmp::Reverse((p.decision.number, p.decision.issued_at)));
    let pending: Vec<Value> = pending
        .into_iter()
        .take(PENDING_LISTED)
        .map(|p| {
            let who = voters.get(&p.decision.payload_digest).cloned().unwrap_or_default();
            json!({
                "payload_digest": p.decision.payload_digest,
                "new_epoch": p.decision.number,
                "new_holder": p.decision.holder,
                "method": p.payload["method"],
                "eligible_after": p.payload["eligible_after"],
                "expires_at": p.decision.expires_at,
                "live": p.decision.expires_at > now,
                "proposed_by": p.proposer,
                "voters": who,
                "votes": who.len(),
                "threshold": policy.threshold,
                "missing": policy.threshold.saturating_sub(who.len()),
                "here": here.get(&p.decision.payload_digest).cloned().unwrap_or(json!({"verdict": "not_evaluated"})),
            })
        })
        .collect();
    let current = view.certificate.as_ref().map(|c| {
        json!({
            "epoch": view.epoch,
            "holder": view.holder,
            "payload_digest": view.payload_digest,
            "voters": c["signatures"].as_array().map(|s| s.iter().filter_map(|e| e["key_id"].as_str()).collect::<Vec<_>>()),
            "certificate": c,
        })
    });
    json!({
        "resource": rules.resource,
        "view": {"epoch": view.epoch, "holder": view.holder, "barrier": view.barrier, "expires_at": view.expires_at, "source": view.source},
        "current": current,
        "pending": pending,
        "conflicts": view.conflicts,
        "threshold": policy.threshold,
        "keys": policy.keys.len(),
        "read_at": now,
    })
}
