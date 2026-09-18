//! Signed votes (V3-4): a replica's promise for one exclusive decision, signed by its own key.
//!
//! A vote carries the decision as the node will read it: the canonical payload of a V3-2
//! certificate (`podmesh-takeover-proof/quorum-ed25519` for an epoch rotation or a same-holder
//! re-issue, `podmesh-policy-change/quorum-ed25519` for a change of the authority set). It carries
//! two signatures by the voter's key:
//!
//! - `signature`, over the payload's canonical bytes: exactly the entry the node counts in a
//!   certificate's `signatures`, so that k votes on one payload assemble into a certificate with no
//!   re-signing and nothing the voters did not sign;
//! - `envelope_signature`, over the vote itself (every field but this one): it binds the voter's
//!   ledger sequence number and ledger nonce to the payload, so that nobody but the key's owner can
//!   attribute a sequence number to it. The tripwire reads that number; an unsigned one would let
//!   any relay make a replica refuse to sign.
//!
//! The two signatures cover different bytes and cannot stand in for each other: a vote has no
//! top-level `kind`, and it carries a `signature` field, which the node refuses in a certificate
//! (`mixed_forms`).
//!
//! A vote counts only once its origin is established: the voter is a key of the policy, both
//! signatures verify strictly under that key, the payload is made under that policy, and, when the
//! vote arrives as a fact, the fact's origin replica is the replica that owns that key. A relaying
//! replica can carry a vote; it cannot make one.
use crate::quorum::{self, Quorum, Refusal as CertificateRefusal};
use ed25519_dalek::{Signer, SigningKey};
use podmesh_manager_ha_lab::Fact;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The form of a vote document.
pub const VOTE_FORM: &str = "podmesh-manager-vote/1";
/// Every replica's votes live under this scope prefix, in its own scope `votes/<replica_id>`.
pub const VOTE_SCOPE_PREFIX: &str = "votes";

/// The scope a replica's votes are written in.
#[must_use]
pub fn vote_scope(replica_id: &str) -> String {
    format!("{VOTE_SCOPE_PREFIX}/{replica_id}")
}

/// Why a vote does not count, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoteRejection {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for VoteRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "vote refused ({}): {}", self.code, self.detail)
    }
}

impl std::error::Error for VoteRejection {}

fn rejection(code: &'static str, detail: impl Into<String>) -> VoteRejection {
    VoteRejection {
        code,
        detail: detail.into(),
    }
}

/// The two kinds of promise a ledger keeps, and what each is keyed on besides the resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PromiseKind {
    /// A takeover certificate: keyed on `new_epoch`.
    Epoch,
    /// A policy-change certificate: keyed on `from_serial`.
    Serial,
}

/// What a payload decides, read from its fields: the resource, the number the promise is keyed on,
/// who the decision is for, and the decision's identity without its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub kind: PromiseKind,
    pub resource: String,
    /// `new_epoch` for an epoch, `from_serial` for a serial.
    pub number: i64,
    /// `new_holder` for an epoch, `new_policy_digest` for a serial.
    pub holder: String,
    /// SHA-256 of the canonical payload without `issued_at` and `expires_at`: the same decision
    /// re-issued with a fresh life has the same identity; anything else is another decision.
    pub decision_digest: String,
    /// SHA-256 of the canonical payload: this document.
    pub payload_digest: String,
    pub issued_at: i64,
    pub expires_at: i64,
}

impl Decision {
    /// Reads the decision of a certificate payload (the document without `signatures`), after
    /// checking it is a well-formed payload of a kind the node knows.
    ///
    /// # Errors
    /// `payload_invalid`: not an object, an unknown kind, a missing or mistyped field, a float, a
    /// `signatures`, `signer` or `signature` field, or numbers the node would refuse.
    pub fn of_payload(payload: &Value) -> Result<Decision, VoteRejection> {
        let object = payload
            .as_object()
            .ok_or_else(|| rejection("payload_invalid", "a payload must be an object"))?;
        for forbidden in ["signatures", "signer", "signature"] {
            if object.contains_key(forbidden) {
                return Err(rejection(
                    "payload_invalid",
                    format!("a vote's payload carries no `{forbidden}`"),
                ));
            }
        }
        let kind_name = object.get("kind").and_then(Value::as_str).unwrap_or("none");
        let fields = quorum::fields_of(kind_name).ok_or_else(|| {
            rejection(
                "payload_invalid",
                format!("{kind_name} is not a certificate kind the node knows"),
            )
        })?;
        for (name, field) in fields.iter().chain(
            [
                ("authority_id", quorum::Field::Text),
                ("policy_digest", quorum::Field::Text),
            ]
            .iter(),
        ) {
            let v = object.get(*name);
            let ok = match field {
                quorum::Field::Text => v.is_some_and(Value::is_string),
                quorum::Field::Integer => v.is_some_and(Value::is_i64),
                quorum::Field::TextOrNull => v.is_some_and(|v| v.is_string() || v.is_null()),
            };
            if !ok {
                return Err(rejection(
                    "payload_invalid",
                    format!("the payload lacks {name}, or it has the wrong type"),
                ));
            }
        }
        let message = quorum::canonical_json(payload)
            .map_err(|e| rejection("payload_invalid", e.to_string()))?;
        let text = |name: &str| object[name].as_str().unwrap_or_default().to_string();
        let integer = |name: &str| object[name].as_i64().unwrap_or_default();
        let (kind, number, holder) = if kind_name == quorum::QUORUM_PROOF_KIND {
            let epoch = integer("new_epoch");
            // The node's own bounds: epoch 1 to 2^31 - 1, exactly one after the previous.
            if !(1..=i64::from(i32::MAX)).contains(&epoch) || integer("previous_epoch") != epoch - 1
            {
                return Err(rejection(
                    "payload_invalid",
                    "new_epoch must be 1 to 2^31-1 and previous_epoch exactly one before it",
                ));
            }
            (PromiseKind::Epoch, epoch, text("new_holder"))
        } else {
            let from = integer("from_serial");
            if from < 0 || integer("new_serial") != from + 1 {
                return Err(rejection(
                    "payload_invalid",
                    "from_serial must be at least 0 and new_serial exactly one more",
                ));
            }
            (PromiseKind::Serial, from, text("new_policy_digest"))
        };
        let resource = text("resource");
        if resource.is_empty() {
            return Err(rejection(
                "payload_invalid",
                "the payload names no resource",
            ));
        }
        let mut identity = object.clone();
        identity.remove("issued_at");
        identity.remove("expires_at");
        let identity = quorum::canonical_json(&Value::Object(identity))
            .map_err(|e| rejection("payload_invalid", e.to_string()))?;
        Ok(Decision {
            kind,
            resource,
            number,
            holder,
            decision_digest: quorum::sha256_hex(&identity),
            payload_digest: quorum::sha256_hex(&message),
            issued_at: integer("issued_at"),
            expires_at: integer("expires_at"),
        })
    }
}

/// What the ledger adds to a vote once its entry is durable.
pub struct LedgerStamp<'a> {
    pub nonce: &'a str,
    pub sequence: u64,
}

/// Builds and signs a vote. Called by the signing ledger only, after the entry is durable: nothing
/// here is a promise until then.
pub(crate) fn seal(
    key: &SigningKey,
    key_id: &str,
    payload: &Value,
    stamp: &LedgerStamp<'_>,
) -> Result<Value, VoteRejection> {
    let message =
        quorum::canonical_json(payload).map_err(|e| rejection("payload_invalid", e.to_string()))?;
    let kind = payload["kind"].as_str().unwrap_or_default();
    let mut vote = json!({
        "form": VOTE_FORM,
        "voter": key_id,
        "certificate_kind": kind,
        "payload": payload,
        "signature": quorum::hex(&key.sign(&message).to_bytes()),
        "ledger_nonce": stamp.nonce,
        "ledger_sequence": stamp.sequence,
    });
    let envelope =
        quorum::canonical_json(&vote).map_err(|e| rejection("payload_invalid", e.to_string()))?;
    vote["envelope_signature"] = json!(quorum::hex(&key.sign(&envelope).to_bytes()));
    Ok(vote)
}

/// A vote whose origin is established: its voter, its decision and its ledger stamp.
#[derive(Debug, Clone)]
pub struct VerifiedVote {
    pub voter: String,
    pub decision: Decision,
    pub payload: Value,
    pub signature: String,
    pub ledger_nonce: String,
    pub ledger_sequence: u64,
}

/// The shape of a vote, before anything is verified.
fn shape(vote: &Value) -> Result<(&serde_json::Map<String, Value>, &str), VoteRejection> {
    let object = vote
        .as_object()
        .ok_or_else(|| rejection("vote_malformed", "a vote must be an object"))?;
    let mut names: Vec<&str> = object.keys().map(String::as_str).collect();
    names.sort_unstable();
    if names
        != [
            "certificate_kind",
            "envelope_signature",
            "form",
            "ledger_nonce",
            "ledger_sequence",
            "payload",
            "signature",
            "voter",
        ]
    {
        return Err(rejection(
            "vote_malformed",
            "a vote carries exactly its eight fields",
        ));
    }
    if object["form"].as_str() != Some(VOTE_FORM) {
        return Err(rejection(
            "vote_malformed",
            format!("a vote's form is {VOTE_FORM}"),
        ));
    }
    let voter = object["voter"]
        .as_str()
        .ok_or_else(|| rejection("vote_malformed", "voter must be a key identifier"))?;
    if object["ledger_sequence"].as_u64().is_none() || object["ledger_nonce"].as_str().is_none() {
        return Err(rejection(
            "vote_malformed",
            "ledger_sequence and ledger_nonce are required",
        ));
    }
    if object["certificate_kind"] != object["payload"]["kind"] {
        return Err(rejection(
            "vote_malformed",
            "certificate_kind is not the payload's kind",
        ));
    }
    Ok((object, voter))
}

/// Verifies a vote's origin under the keys `keys` knows (the policy's, and for evidence the
/// retired ones), without asking which policy its payload was made under. Used by the tripwire and
/// by readmission, which must read a vote made under an older authority set too.
///
/// # Errors
/// `vote_malformed`, `unknown_voter`, `bad_envelope_signature`, `bad_signature`, `payload_invalid`.
pub fn verify_origin(
    vote: &Value,
    keys: &BTreeMap<String, ed25519_dalek::VerifyingKey>,
) -> Result<VerifiedVote, VoteRejection> {
    let (object, voter) = shape(vote)?;
    let key = keys.get(voter).ok_or_else(|| {
        rejection(
            "unknown_voter",
            format!("{voter} is not a key this replica knows"),
        )
    })?;
    let mut envelope = object.clone();
    envelope.remove("envelope_signature");
    let envelope = quorum::canonical_json(&Value::Object(envelope))
        .map_err(|e| rejection("vote_malformed", e.to_string()))?;
    let envelope_signature = object["envelope_signature"]
        .as_str()
        .and_then(quorum::signature_from_hex)
        .ok_or_else(|| {
            rejection(
                "vote_malformed",
                "envelope_signature must be 128 lowercase hex characters",
            )
        })?;
    key.verify_strict(&envelope, &envelope_signature)
        .map_err(|_| {
            rejection(
                "bad_envelope_signature",
                format!(
                    "the vote is not signed by {voter}'s key: forged, or altered after signing"
                ),
            )
        })?;
    let payload = &object["payload"];
    let decision = Decision::of_payload(payload)?;
    let signature_text = object["signature"].as_str().unwrap_or_default();
    let signature = quorum::signature_from_hex(signature_text).ok_or_else(|| {
        rejection(
            "vote_malformed",
            "signature must be 128 lowercase hex characters",
        )
    })?;
    let message =
        quorum::canonical_json(payload).map_err(|e| rejection("payload_invalid", e.to_string()))?;
    key.verify_strict(&message, &signature).map_err(|_| {
        rejection(
            "bad_signature",
            format!("{voter}'s signature does not verify over the payload"),
        )
    })?;
    Ok(VerifiedVote {
        voter: voter.to_string(),
        decision,
        payload: payload.clone(),
        signature: signature_text.to_string(),
        ledger_nonce: object["ledger_nonce"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        ledger_sequence: object["ledger_sequence"].as_u64().unwrap_or_default(),
    })
}

/// The keys of a quorum, by identifier.
#[must_use]
pub fn keys_of(quorum: &Quorum) -> BTreeMap<String, ed25519_dalek::VerifyingKey> {
    quorum
        .keys
        .iter()
        .map(|k| (k.key_id.clone(), k.key))
        .collect()
}

/// Verifies a vote for counting under `policy`: its origin under the policy's keys only (a vote by a
/// key the policy does not name is refused, `unknown_voter`), then that its payload was made under
/// this policy (`policy_mismatch`).
///
/// # Errors
/// As `verify_origin`, and `policy_mismatch`.
pub fn verify_vote(vote: &Value, policy: &Quorum) -> Result<VerifiedVote, VoteRejection> {
    let verified = verify_origin(vote, &keys_of(policy))?;
    if verified.payload["authority_id"].as_str() != Some(policy.authority_id.as_str())
        || verified.payload["policy_digest"].as_str() != Some(policy.digest().as_str())
    {
        return Err(rejection(
            "policy_mismatch",
            "the vote's payload was made under another authority set than this policy",
        ));
    }
    Ok(verified)
}

/// Reads a vote from a fact: a fact in a vote scope whose value is a vote. `replica_keys` maps each
/// replica to the key it owns: a vote counts only from the replica that owns its key
/// (`origin_mismatch`), so one replica's scope never carries another's vote as its own.
///
/// Returns `None` for a fact that is not in a vote scope.
#[must_use]
pub fn vote_of_fact(
    fact: &Fact,
    replica_keys: &BTreeMap<String, String>,
) -> Option<Result<Value, VoteRejection>> {
    let owner = fact
        .scope
        .strip_prefix(VOTE_SCOPE_PREFIX)?
        .strip_prefix('/')?;
    Some((|| {
        if owner != fact.origin_replica_id {
            return Err(rejection(
                "origin_mismatch",
                "a vote scope names another replica than the fact's origin",
            ));
        }
        let vote: Value = serde_json::from_str(&fact.value)
            .map_err(|_| rejection("vote_malformed", "a vote fact's value is not JSON"))?;
        let voter = vote["voter"].as_str().unwrap_or_default();
        if replica_keys
            .get(&fact.origin_replica_id)
            .map(String::as_str)
            != Some(voter)
        {
            return Err(rejection(
                "origin_mismatch",
                format!(
                    "replica {} does not own key {voter}",
                    fact.origin_replica_id
                ),
            ));
        }
        Ok(vote)
    })())
}

/// Why no certificate was assembled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyRefusal {
    pub code: &'static str,
    pub detail: String,
    /// The votes that did not count, and why.
    pub rejected: Vec<(usize, VoteRejection)>,
}

impl std::fmt::Display for AssemblyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no certificate ({}): {}", self.code, self.detail)
    }
}

impl std::error::Error for AssemblyRefusal {}

/// Assembles the certificate for `payload` from `votes`: every vote is verified under `policy`
/// (origin first, then the policy); votes on another payload are set aside; each voter counts once;
/// and only when at least the threshold's number of distinct keys voted for exactly this payload is
/// the certificate built, the payload with `signatures` in key order. The result is then verified
/// with the node's rules (`Quorum::verify`) before it is returned, so what leaves here is what the
/// node accepts.
///
/// # Errors
/// `payload_invalid`, `policy_mismatch`, `below_threshold` (with every rejected vote named), or the
/// node's own refusal if the self-check fails (`certificate_self_check`).
pub fn assemble(
    policy: &Quorum,
    payload: &Value,
    votes: &[Value],
) -> Result<Value, AssemblyRefusal> {
    let refuse = |code, detail: String, rejected| AssemblyRefusal {
        code,
        detail,
        rejected,
    };
    let target = Decision::of_payload(payload)
        .map_err(|e| refuse("payload_invalid", e.detail, Vec::new()))?;
    if payload["authority_id"].as_str() != Some(policy.authority_id.as_str())
        || payload["policy_digest"].as_str() != Some(policy.digest().as_str())
    {
        return Err(refuse(
            "policy_mismatch",
            "the payload was made under another authority set".into(),
            Vec::new(),
        ));
    }
    let mut rejected = Vec::new();
    let mut signatures: BTreeMap<String, String> = BTreeMap::new();
    for (i, vote) in votes.iter().enumerate() {
        match verify_vote(vote, policy) {
            Ok(v) if v.decision.payload_digest == target.payload_digest => {
                signatures.entry(v.voter).or_insert(v.signature);
            }
            Ok(_) => rejected.push((i, rejection("other_payload", "a vote for another payload"))),
            Err(e) => rejected.push((i, e)),
        }
    }
    if signatures.len() < policy.threshold {
        return Err(refuse(
            "below_threshold",
            format!(
                "{} distinct key(s) voted for this payload, the policy requires {} of its {}",
                signatures.len(),
                policy.threshold,
                policy.keys.len()
            ),
            rejected,
        ));
    }
    let mut certificate = payload.clone();
    certificate["signatures"] = json!(signatures
        .iter()
        .map(|(key_id, signature)| json!({"key_id": key_id, "signature": signature}))
        .collect::<Vec<_>>());
    let kind = payload["kind"].as_str().unwrap_or_default();
    let fields = quorum::fields_of(kind).unwrap_or_default();
    policy
        .verify(&certificate, kind, fields)
        .map_err(|e: CertificateRefusal| {
            refuse("certificate_self_check", e.to_string(), Vec::new())
        })?;
    Ok(certificate)
}

/// Every distinct decision `votes` reached a certificate for under `policy`, one per payload, and the
/// promises they conflict on: two payloads with different decisions for one (kind, resource,
/// number), each certified. That second list is empty whenever every signer kept its promise; a
/// non-empty one is evidence of a broken promise, whatever caused it.
#[must_use]
pub fn tally(policy: &Quorum, votes: &[Value]) -> (Vec<Value>, Vec<(PromiseKind, String, i64)>) {
    let mut payloads: BTreeMap<String, (Value, BTreeSet<String>, Decision)> = BTreeMap::new();
    for vote in votes {
        if let Ok(v) = verify_vote(vote, policy) {
            payloads
                .entry(v.decision.payload_digest.clone())
                .or_insert_with(|| (v.payload.clone(), BTreeSet::new(), v.decision.clone()))
                .1
                .insert(v.voter);
        }
    }
    let mut certificates = Vec::new();
    let mut decided: BTreeMap<(PromiseKind, String, i64), BTreeSet<String>> = BTreeMap::new();
    for (payload, voters, decision) in payloads.values() {
        if voters.len() >= policy.threshold {
            if let Ok(c) = assemble(policy, payload, votes) {
                certificates.push(c);
                decided
                    .entry((decision.kind, decision.resource.clone(), decision.number))
                    .or_default()
                    .insert(decision.decision_digest.clone());
            }
        }
    }
    let conflicts = decided
        .into_iter()
        .filter(|(_, decisions)| decisions.len() > 1)
        .map(|(key, _)| key)
        .collect();
    (certificates, conflicts)
}
