//! The PodMesh node's quorum certificates, as the replicas must produce them (V3-2, ported).
//!
//! This is a port of the verification half of `src/signing.rs` of the PodMesh node at `5022a7b`
//! ("QUORUM CERTIFICATES (V3-2)"), kept byte-for-byte in what it accepts: the canonical form, the
//! policy digest, the certificate kinds, the payload fields and the order and names of the
//! refusals. The replicas use it for two things: to check, before a certificate leaves a replica,
//! that the node will accept it, and to count votes under the rules the node counts signatures by.
//! The node remains the verifier that matters; the tests of this file pin the node's own vectors
//! (its policy digests and a certificate signed by another implementation), so that a drift between
//! the two copies fails here.
//!
//! Canonical form: the document without its `signatures` field (a vote's payload has none), every
//! object's keys sorted by their bytes, compact JSON, no floating-point number. It is what
//! `serde_json` prints for a `Value` when its maps are sorted, and what Python prints with
//! `sort_keys=True, separators=(',', ':'), ensure_ascii=False`. `canonical_json` sorts explicitly, so
//! the bytes do not depend on how this build's `serde_json` orders its maps.
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// A takeover document as a certificate: k-of-n signatures of the policy's quorum.
pub const QUORUM_PROOF_KIND: &str = "podmesh-takeover-proof/quorum-ed25519";
/// A change of a resource's authority set, certified by the quorum it replaces.
pub const POLICY_CHANGE_KIND: &str = "podmesh-policy-change/quorum-ed25519";
/// The form of the canonical policy a `policy_digest` is taken over.
pub const QUORUM_POLICY_FORM: &str = "podmesh-authority-quorum/1";
/// The key identifier the 1-of-1 quorum of a single `authority_key` gives that key.
pub const SINGLE_KEY_ID: &str = "authority_key";
/// At most this many keys in a quorum, as the node bounds it.
pub const MAX_QUORUM_KEYS: usize = 9;
/// A key identifier's length bound, as the node bounds it.
pub const MAX_KEY_ID: usize = 32;

pub(crate) fn hex_decode(text: &str, what: &str, bytes: usize) -> Result<Vec<u8>, Error> {
    if text.len() != bytes * 2 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "{what} must be {bytes} bytes as {} lowercase hex characters",
            bytes * 2
        )
        .into());
    }
    if text.bytes().any(|b| b.is_ascii_uppercase()) {
        return Err(format!("{what} must be lowercase hex").into());
    }
    (0..bytes)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(Into::into))
        .collect()
}

/// Lowercase hex of `bytes`.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A public key as a policy records it: 32 bytes, lowercase hex, a valid Ed25519 point.
///
/// # Errors
/// The text is not 64 lowercase hex characters, or not a point.
pub fn verifying_key(text: &str) -> Result<VerifyingKey, Error> {
    let bytes: [u8; 32] = hex_decode(text, "public_key", 32)?
        .try_into()
        .map_err(|_| "public_key length")?;
    VerifyingKey::from_bytes(&bytes)
        .map_err(|e| format!("public_key is not a valid Ed25519 public key: {e}").into())
}

/// A signature as a certificate records it: 64 bytes, 128 lowercase hex characters.
pub(crate) fn signature_from_hex(text: &str) -> Option<Signature> {
    let bytes: [u8; 64] = hex_decode(text, "signature", 64).ok()?.try_into().ok()?;
    Some(Signature::from_bytes(&bytes))
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) -> Result<(), Error> {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            out.push(b'{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend_from_slice(&serde_json::to_vec(key)?);
                out.push(b':');
                write_canonical(&map[key], out)?;
            }
            out.push(b'}');
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out)?;
            }
            out.push(b']');
        }
        Value::Number(n) if n.is_f64() => {
            return Err("a signed document must not carry a floating-point number".into())
        }
        other => out.extend_from_slice(&serde_json::to_vec(other)?),
    }
    Ok(())
}

/// The canonical bytes of `value`: keys sorted by their bytes, compact, no floating-point number.
///
/// # Errors
/// The value carries a floating-point number.
pub fn canonical_json(value: &Value) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    write_canonical(value, &mut out)?;
    Ok(out)
}

/// The bytes every signature of a certificate covers: the document without `signatures`.
///
/// # Errors
/// Not an object, or it carries a floating-point number.
pub fn certificate_message(document: &Value) -> Result<Vec<u8>, Error> {
    let mut payload = document
        .as_object()
        .ok_or("a certificate must be an object")?
        .clone();
    payload.remove("signatures");
    canonical_json(&Value::Object(payload))
}

/// SHA-256, lowercase hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// The digest of a certificate's payload (the document without its signatures): the decision's
/// identity, whichever set of signers presented it. The node's `payload_digest`.
///
/// # Errors
/// As `certificate_message`.
pub fn payload_digest(document: &Value) -> Result<String, Error> {
    Ok(sha256_hex(&certificate_message(document)?))
}

/// Why a certificate was refused, by name, with the node's codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "certificate refused ({}): {}", self.code, self.detail)
    }
}

impl std::error::Error for Refusal {}

fn refusal(code: &'static str, detail: impl Into<String>) -> Refusal {
    Refusal {
        code,
        detail: detail.into(),
    }
}

/// The type a certificate's payload field must have.
#[derive(Clone, Copy)]
pub enum Field {
    Text,
    Integer,
    /// A string, or null: `previous_holder` of a first epoch.
    TextOrNull,
}

/// What every takeover certificate binds, as the node requires it.
pub const TAKEOVER_FIELDS: &[(&str, Field)] = &[
    ("resource", Field::Text),
    ("new_epoch", Field::Integer),
    ("previous_epoch", Field::Integer),
    ("new_holder", Field::Text),
    ("previous_holder", Field::TextOrNull),
    ("holder_boot_id", Field::Text),
    ("grant_id", Field::Text),
    ("method", Field::Text),
    ("eligible_after", Field::Integer),
    ("issued_at", Field::Integer),
    ("expires_at", Field::Integer),
];

/// What a policy-change certificate binds, as the node requires it.
pub const POLICY_CHANGE_FIELDS: &[(&str, Field)] = &[
    ("resource", Field::Text),
    ("new_policy_digest", Field::Text),
    ("from_serial", Field::Integer),
    ("new_serial", Field::Integer),
    ("issued_at", Field::Integer),
    ("expires_at", Field::Integer),
];

/// The fields a certificate of `kind` must bind, or none for a kind the node does not know.
#[must_use]
pub fn fields_of(kind: &str) -> Option<&'static [(&'static str, Field)]> {
    match kind {
        QUORUM_PROOF_KIND => Some(TAKEOVER_FIELDS),
        POLICY_CHANGE_KIND => Some(POLICY_CHANGE_FIELDS),
        _ => None,
    }
}

/// One key of a quorum: its stable identifier and its public half.
pub struct QuorumKey {
    pub key_id: String,
    pub public_key: String,
    pub(crate) key: VerifyingKey,
}

/// The authority a policy names, as a quorum: n keys, a threshold k, the authority's identifier and
/// the authority set's serial. The same structure, digest and checks as the node's.
pub struct Quorum {
    pub authority_id: String,
    pub threshold: usize,
    /// Sorted by `key_id`.
    pub keys: Vec<QuorumKey>,
    pub single_key: bool,
    pub serial: u64,
}

fn key_id(value: &str) -> Result<(), Error> {
    let mut bytes = value.bytes();
    let head = bytes.next().ok_or("key_id must not be empty")?;
    if !head.is_ascii_alphanumeric()
        || value.len() > MAX_KEY_ID
        || !bytes
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b':' || b == b'-')
    {
        return Err(format!(
            "key_id must be 1-{MAX_KEY_ID} ASCII characters from [A-Za-z0-9_.:-], starting alphanumeric"
        )
        .into());
    }
    Ok(())
}

/// Checks a key identifier as the node checks it.
///
/// # Errors
/// The identifier is empty, too long, or uses another character.
pub fn check_key_id(value: &str) -> Result<(), Error> {
    key_id(value)
}

impl Quorum {
    /// A quorum as `activation_require` declares it: `{"threshold": k, "keys": [{"key_id",
    /// "public_key"}]}`, no other field, with the node's refusals: no key or too many, a key_id or
    /// a public key named twice, an invalid or small-order key, a threshold that is not a strict
    /// majority.
    ///
    /// # Errors
    /// The declaration is refused, with the node's reason.
    pub fn declared(authority_id: &str, value: &Value) -> Result<Quorum, Error> {
        let object = value
            .as_object()
            .ok_or("authority_quorum must be an object")?;
        let mut fields: Vec<&str> = object.keys().map(String::as_str).collect();
        fields.sort_unstable();
        if fields != ["keys", "threshold"] {
            return Err("authority_quorum must carry exactly the fields keys and threshold".into());
        }
        let entries = object["keys"]
            .as_array()
            .ok_or("authority_quorum.keys must be a list")?;
        if entries.is_empty() || entries.len() > MAX_QUORUM_KEYS {
            return Err(
                format!("authority_quorum.keys must name 1 to {MAX_QUORUM_KEYS} keys").into(),
            );
        }
        let mut keys: Vec<QuorumKey> = Vec::new();
        for entry in entries {
            let e = entry
                .as_object()
                .ok_or("each authority_quorum key must be an object")?;
            let mut names: Vec<&str> = e.keys().map(String::as_str).collect();
            names.sort_unstable();
            if names != ["key_id", "public_key"] {
                return Err(
                    "each authority_quorum key must carry exactly key_id and public_key".into(),
                );
            }
            let id = e["key_id"].as_str().ok_or("key_id must be a string")?;
            key_id(id)?;
            let text = e["public_key"]
                .as_str()
                .ok_or("public_key must be a string")?;
            let key = verifying_key(text).map_err(|e| format!("key {id}: {e}"))?;
            if key.is_weak() {
                return Err(format!(
                    "key {id}: a public key of small order is refused: anyone can sign for it"
                )
                .into());
            }
            if keys.iter().any(|k| k.key_id == id) {
                return Err(format!("key_id {id} is named twice").into());
            }
            if keys.iter().any(|k| k.public_key == text) {
                return Err(format!("the public key of {id} is named twice, under two identifiers: one signer would count twice").into());
            }
            keys.push(QuorumKey {
                key_id: id.to_string(),
                public_key: text.to_string(),
                key,
            });
        }
        let threshold = usize::try_from(
            object["threshold"]
                .as_u64()
                .ok_or("authority_quorum.threshold must be an integer")?,
        )?;
        if threshold < 1 || threshold > keys.len() {
            return Err(format!(
                "authority_quorum.threshold must be from 1 to the {} keys named",
                keys.len()
            )
            .into());
        }
        if 2 * threshold <= keys.len() {
            return Err(format!(
                "authority_quorum.threshold {threshold} of {} is not a strict majority: two disjoint sets of signers could each certify a decision",
                keys.len()
            )
            .into());
        }
        keys.sort_by(|a, b| a.key_id.cmp(&b.key_id));
        Ok(Quorum {
            authority_id: authority_id.to_string(),
            threshold,
            keys,
            single_key: false,
            serial: 0,
        })
    }

    /// The 1-of-1 quorum of a policy that names a single `authority_key`.
    ///
    /// # Errors
    /// The key is not a valid point, or is of small order.
    pub fn single(authority_id: &str, key_hex: &str) -> Result<Quorum, Error> {
        let key = verifying_key(key_hex)?;
        if key.is_weak() {
            return Err(
                "authority_key: a public key of small order is refused: anyone can sign for it"
                    .into(),
            );
        }
        Ok(Quorum {
            authority_id: authority_id.to_string(),
            threshold: 1,
            keys: vec![QuorumKey {
                key_id: SINGLE_KEY_ID.to_string(),
                public_key: key_hex.to_string(),
                key,
            }],
            single_key: true,
            serial: 0,
        })
    }

    /// The same authority set at `serial`.
    #[must_use]
    pub fn at_serial(mut self, serial: u64) -> Quorum {
        self.serial = serial;
        self
    }

    /// The quorum as a policy stores it.
    #[must_use]
    pub fn stored(&self) -> Value {
        serde_json::json!({
            "threshold": self.threshold,
            "keys": self.keys.iter().map(|k| serde_json::json!({"key_id": k.key_id, "public_key": k.public_key})).collect::<Vec<_>>(),
        })
    }

    /// What `policy_digest` is taken over, as the node computes it.
    #[must_use]
    pub fn canonical_policy(&self) -> Vec<u8> {
        let mut v = self.stored();
        v["form"] = Value::from(QUORUM_POLICY_FORM);
        v["authority_id"] = Value::from(self.authority_id.as_str());
        v["single_key"] = Value::from(self.single_key);
        v["serial"] = Value::from(self.serial);
        canonical_json(&v).unwrap_or_default()
    }

    /// SHA-256 of the canonical policy, lowercase hex.
    #[must_use]
    pub fn digest(&self) -> String {
        sha256_hex(&self.canonical_policy())
    }

    /// The key named `key_id`, if the policy names it.
    #[must_use]
    pub fn key(&self, key_id: &str) -> Option<&QuorumKey> {
        self.keys.iter().find(|k| k.key_id == key_id)
    }

    /// Verifies a certificate of `kind` under this quorum exactly as the node does: the kind, no
    /// single-key fields, the authority, the digest, the payload's fields, then every signature
    /// strictly, and only then the count. Returns the signers in the order given.
    ///
    /// # Errors
    /// The node's refusal, by name.
    pub fn verify(
        &self,
        document: &Value,
        kind: &str,
        required: &[(&str, Field)],
    ) -> Result<Vec<String>, Refusal> {
        let object = document
            .as_object()
            .ok_or_else(|| refusal("payload_incomplete", "a certificate must be an object"))?;
        let found = object.get("kind").and_then(Value::as_str).unwrap_or("none");
        if found != kind {
            return Err(refusal(
                "certificate_kind",
                format!("the document is {found}, not {kind}"),
            ));
        }
        if object.contains_key("signer") || object.contains_key("signature") {
            return Err(refusal(
                "mixed_forms",
                "a certificate carries `signatures`, never the single-key `signer` or `signature`",
            ));
        }
        if object.get("authority_id").and_then(Value::as_str) != Some(self.authority_id.as_str()) {
            return Err(refusal(
                "authority_mismatch",
                "the certificate names another authority than this resource's policy",
            ));
        }
        let digest = self.digest();
        match object.get("policy_digest").and_then(Value::as_str) {
            Some(d) if d == digest => {}
            Some(d) => {
                return Err(refusal(
                    "policy_mismatch",
                    format!(
                    "the certificate was made under policy {d}, this resource's policy is {digest}"
                ),
                ))
            }
            None => {
                return Err(refusal(
                    "policy_mismatch",
                    "the certificate names no policy_digest",
                ))
            }
        }
        for (name, field) in required {
            let v = object.get(*name);
            let ok = match field {
                Field::Text => v.is_some_and(Value::is_string),
                Field::Integer => v.is_some_and(Value::is_i64),
                Field::TextOrNull => v.is_some_and(|v| v.is_string() || v.is_null()),
            };
            if !ok {
                return Err(refusal(
                    "payload_incomplete",
                    format!("the certificate lacks {name}, or it has the wrong type"),
                ));
            }
        }
        let message = certificate_message(document).map_err(|_| {
            refusal(
                "payload_incomplete",
                "a certificate must not carry a floating-point number",
            )
        })?;
        let entries = match object.get("signatures").and_then(Value::as_array) {
            Some(a) if !a.is_empty() => a,
            _ => {
                return Err(refusal(
                    "no_signatures",
                    "the certificate carries no signatures",
                ))
            }
        };
        if entries.len() > self.keys.len() {
            return Err(refusal(
                "too_many_signatures",
                format!(
                    "{} signatures for a policy of {} keys",
                    entries.len(),
                    self.keys.len()
                ),
            ));
        }
        let mut signers: Vec<String> = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            let e = entry.as_object().filter(|e| e.len() == 2).ok_or_else(|| {
                refusal(
                    "malformed_signature",
                    format!("signature {i} is not exactly {{key_id, signature}}"),
                )
            })?;
            let (Some(id), Some(text)) = (
                e.get("key_id").and_then(Value::as_str),
                e.get("signature").and_then(Value::as_str),
            ) else {
                return Err(refusal(
                    "malformed_signature",
                    format!("signature {i} is not exactly {{key_id, signature}} as strings"),
                ));
            };
            let signature = signature_from_hex(text).ok_or_else(|| {
                refusal(
                    "malformed_signature",
                    format!("signature {i} ({id}) is not 64 bytes as 128 lowercase hex characters"),
                )
            })?;
            if signers.iter().any(|s| s == id) {
                return Err(refusal(
                    "duplicate_key",
                    format!("{id} signed twice; a key counts once, and a certificate that lists it twice is refused"),
                ));
            }
            let key = self.key(id).ok_or_else(|| {
                refusal(
                    "unknown_key",
                    format!("{id} is not a key of this resource's policy"),
                )
            })?;
            key.key.verify_strict(&message, &signature).map_err(|_| {
                refusal(
                    "bad_signature",
                    format!("{id}'s signature does not verify: altered, or not signed over this content by that key"),
                )
            })?;
            signers.push(id.to_string());
        }
        if signers.len() < self.threshold {
            return Err(refusal(
                "below_threshold",
                format!(
                    "{} distinct key(s) signed, the policy requires {} of its {}",
                    signers.len(),
                    self.threshold,
                    self.keys.len()
                ),
            ));
        }
        Ok(signers)
    }
}

/// Deterministic test keys (a one-byte seed, as the node's own tests make them) and certificates
/// signed by any set of them. Test-only: no real key is ever made or read here.
#[cfg(test)]
pub(crate) mod testkit {
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{json, Value};

    pub fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    pub fn public(seed: u8) -> String {
        super::hex(&key(seed).verifying_key().to_bytes())
    }

    pub fn policy(threshold: usize, keys: &[(&str, u8)]) -> Value {
        json!({"threshold": threshold, "keys": keys.iter().map(|(id, s)| json!({"key_id": id, "public_key": public(*s)})).collect::<Vec<_>>()})
    }

    pub fn sign(document: &Value, signers: &[(&str, u8)]) -> Value {
        let mut d = document.clone();
        d.as_object_mut().unwrap().remove("signatures");
        let message = super::canonical_json(&d).unwrap();
        d["signatures"] = json!(signers
            .iter()
            .map(|(id, s)| json!({"key_id": id, "signature": super::hex(&key(*s).sign(&message).to_bytes())}))
            .collect::<Vec<_>>());
        d
    }

    /// An unsigned takeover certificate at fixed times, method `lease_barrier`.
    pub fn takeover(
        authority: &str,
        digest: &str,
        resource: &str,
        epoch: i64,
        holder: &str,
        boot: &str,
    ) -> Value {
        json!({"kind": super::QUORUM_PROOF_KIND, "authority_id": authority, "policy_digest": digest, "resource": resource,
               "new_epoch": epoch, "previous_epoch": epoch - 1, "new_holder": holder, "previous_holder": "00000000-0000-4000-8000-00000000000b",
               "holder_boot_id": boot, "grant_id": format!("g{epoch}"), "method": "lease_barrier",
               "eligible_after": 1_700_000_599, "issued_at": 1_700_000_000, "expires_at": 1_700_000_300})
    }
}

/// The node's verifier tests (`src/signing.rs`, `quorum_tests`, at `5022a7b`), ported: the same
/// cases and the same pinned vectors, so that this copy accepts exactly what the node accepts.
#[cfg(test)]
mod node_tests {
    use super::testkit::*;
    use super::*;
    use serde_json::json;

    const R: &str = "91eeb6bf-5489-405b-b77a-53105b0aff7a";
    const HOST: &str = "5d1c0b8e-3f59-4d0e-9d7a-2a1e7c4b9f10";
    const BOOT: &str = "0b7f6f1e-6a55-4c1d-8f53-1c2d3e4f5a6b";
    const ABC: &[(&str, u8)] = &[("replica-a", 1), ("replica-b", 2), ("replica-c", 3)];
    const FIVE: &[(&str, u8)] = &[("r1", 11), ("r2", 12), ("r3", 13), ("r4", 14), ("r5", 15)];

    fn quorum(threshold: usize, keys: &[(&str, u8)]) -> Quorum {
        Quorum::declared("replicas", &policy(threshold, keys)).unwrap()
    }

    fn certificate(q: &Quorum, signers: &[(&str, u8)]) -> Value {
        sign(
            &takeover("replicas", &q.digest(), R, 12, HOST, BOOT),
            signers,
        )
    }

    fn code(q: &Quorum, doc: &Value) -> &'static str {
        match q.verify(doc, QUORUM_PROOF_KIND, TAKEOVER_FIELDS) {
            Ok(_) => "accepted",
            Err(r) => r.code,
        }
    }

    /// Proves: the explicit canonical form is `serde_json`'s sorted form, byte for byte.
    #[test]
    fn canonical_form_sorts_keys_and_refuses_floats() {
        let doc = json!({"z": 1, "a": {"y": [1, 2], "b": "é"}, "B": null});
        assert_eq!(
            String::from_utf8(canonical_json(&doc).unwrap()).unwrap(),
            r#"{"B":null,"a":{"b":"é","y":[1,2]},"z":1}"#
        );
        assert!(canonical_json(&json!({"a": 1.5})).is_err());
        assert!(canonical_json(&json!({"a": [{"b": 0.5}]})).is_err());
    }

    #[test]
    fn k_of_n_is_accepted_at_exactly_k_and_above() {
        let q = quorum(2, ABC);
        for signers in [&ABC[..2], &ABC[1..], &[ABC[2], ABC[0]], ABC] {
            let doc = certificate(&q, signers);
            let who = q.verify(&doc, QUORUM_PROOF_KIND, TAKEOVER_FIELDS).unwrap();
            assert_eq!(
                who,
                signers
                    .iter()
                    .map(|(id, _)| id.to_string())
                    .collect::<Vec<_>>()
            );
        }
        let q5 = quorum(3, FIVE);
        for n in 3..=5 {
            assert_eq!(
                code(&q5, &certificate(&q5, &FIVE[5 - n..])),
                "accepted",
                "{n} of 5"
            );
        }
    }

    #[test]
    fn k_minus_one_is_refused() {
        let q = quorum(2, ABC);
        for one in ABC {
            assert_eq!(code(&q, &certificate(&q, &[*one])), "below_threshold");
        }
        let q5 = quorum(3, FIVE);
        assert_eq!(code(&q5, &certificate(&q5, &FIVE[..2])), "below_threshold");
        let e = q5
            .verify(
                &certificate(&q5, &FIVE[..2]),
                QUORUM_PROOF_KIND,
                TAKEOVER_FIELDS,
            )
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("2 distinct key(s) signed, the policy requires 3 of its 5"),
            "{e}"
        );
    }

    #[test]
    fn the_same_key_twice_is_counted_once() {
        let q = quorum(2, ABC);
        assert_eq!(
            code(&q, &certificate(&q, &[ABC[0], ABC[0]])),
            "duplicate_key"
        );
        assert_eq!(
            code(&q, &certificate(&q, &[ABC[0], ABC[1], ABC[0]])),
            "duplicate_key"
        );
        let alias = json!({"threshold": 2, "keys": [
            {"key_id": "replica-a", "public_key": public(1)}, {"key_id": "alias-of-a", "public_key": public(1)}, {"key_id": "replica-c", "public_key": public(3)}]});
        assert!(Quorum::declared("replicas", &alias)
            .err()
            .unwrap()
            .to_string()
            .contains("named twice, under two identifiers"));
    }

    #[test]
    fn a_key_from_another_policy_is_refused() {
        let q = quorum(2, ABC);
        assert_eq!(
            code(&q, &certificate(&q, &[ABC[0], ("replica-d", 4)])),
            "unknown_key"
        );
        assert_eq!(
            code(&q, &certificate(&q, &[ABC[0], ("replica-b", 4)])),
            "bad_signature"
        );
        let other = quorum(2, &[ABC[0], ABC[1], ("replica-d", 4)]);
        let foreign = sign(
            &takeover("replicas", &other.digest(), R, 12, HOST, BOOT),
            &[ABC[0], ABC[1]],
        );
        assert_eq!(code(&other, &foreign), "accepted");
        assert_eq!(code(&q, &foreign), "policy_mismatch");
        let mut elsewhere = certificate(&q, &ABC[..2]);
        elsewhere["authority_id"] = json!("another-authority");
        assert_eq!(code(&q, &sign(&elsewhere, &ABC[..2])), "authority_mismatch");
    }

    #[test]
    fn a_payload_relabelled_for_another_resource_or_epoch_is_refused() {
        let q = quorum(2, ABC);
        let doc = certificate(&q, &ABC[..2]);
        for (field, value) in [
            ("resource", json!("00000000-0000-4000-8000-000000000001")),
            ("new_epoch", json!(13)),
            ("previous_epoch", json!(12)),
            ("new_holder", json!("00000000-0000-4000-8000-00000000000b")),
            ("method", json!("first")),
            ("eligible_after", json!(0)),
            ("expires_at", json!(4_102_444_800_i64)),
            ("holder_boot_id", json!("another-boot")),
            ("grant_id", json!("g13")),
        ] {
            let mut moved = doc.clone();
            moved[field] = value;
            assert_eq!(code(&q, &moved), "bad_signature", "{field}");
        }
    }

    /// Proves, against the node's pinned vectors: the policy digests of a 2-of-3 at serial 0 and 1
    /// and of the single key's 1-of-1 are the node's, and a certificate signed by Python's
    /// `cryptography` (the node's cross-language vector, verbatim) verifies here.
    #[test]
    fn the_node_pinned_digests_and_its_python_certificate_agree() {
        let q = quorum(2, ABC);
        assert_eq!(
            q.digest(),
            "965bd61aaad93361c01a54635b3f3ee6c40953e0680589f064cf56ca06c19d86"
        );
        let reversed: Vec<(&str, u8)> = ABC.iter().rev().copied().collect();
        assert_eq!(quorum(2, &reversed).digest(), q.digest());
        let later = quorum(2, ABC).at_serial(1);
        assert_eq!(
            later.digest(),
            "cd720df9baf1151f94fec5f5055460255fdc675fd3c2ef6525e563ca60e07a80"
        );
        assert_eq!(code(&later, &certificate(&q, &ABC[..2])), "policy_mismatch");
        let single = Quorum::single(
            "lab-gate",
            "197f6b23e16c8532c6abc838facd5ea789be0c76b2920334039bfa8b3d368d61",
        )
        .unwrap();
        assert_eq!(
            public(42),
            "197f6b23e16c8532c6abc838facd5ea789be0c76b2920334039bfa8b3d368d61"
        );
        assert_eq!(
            single.digest(),
            "7cfa96e40596eb1f6d54ed7485e3674280925995abab60beb6503ded09b635c0"
        );
        let python: Value = serde_json::from_str(r#"{"kind": "podmesh-takeover-proof/quorum-ed25519", "authority_id": "replicas", "policy_digest": "965bd61aaad93361c01a54635b3f3ee6c40953e0680589f064cf56ca06c19d86", "resource": "91eeb6bf-5489-405b-b77a-53105b0aff7a", "new_epoch": 12, "previous_epoch": 11, "new_holder": "5d1c0b8e-3f59-4d0e-9d7a-2a1e7c4b9f10", "previous_holder": null, "holder_boot_id": "0b7f6f1e-6a55-4c1d-8f53-1c2d3e4f5a6b", "grant_id": "g12", "method": "lease_barrier", "eligible_after": 1700000600, "issued_at": 1700000000, "expires_at": 4102444800, "note": "decided by two replicas — naïve JSON", "signatures": [{"key_id": "replica-c", "signature": "cbcbfdb7b05696a80eaf173c8fc9edbc38f68b1a6a4d1bda7a2b2ca937dbdc7016f0b53926a32b728a8055598e9b3618bc0c3c6e0938481394a633004d0ada06"}, {"key_id": "replica-a", "signature": "54780a60c484034192ebc49570866c8e72df15a4ceccd4289cfbb91cc476a4e4d53c5c3a5b91e405de524ccac34226ab073d1e8f9ee729ee7a661c94557ebd0c"}]}"#).unwrap();
        assert_eq!(
            q.verify(&python, QUORUM_PROOF_KIND, TAKEOVER_FIELDS)
                .unwrap(),
            ["replica-c", "replica-a"]
        );
    }

    #[test]
    fn flipping_one_byte_of_the_payload_or_a_signature_is_refused() {
        let q = quorum(2, ABC);
        let doc = certificate(&q, &ABC[..2]);
        let signatures = doc["signatures"].clone();
        let mut payload = doc.clone();
        payload.as_object_mut().unwrap().remove("signatures");
        let bytes = canonical_json(&payload).unwrap();
        let mut presented = 0;
        for i in 0..bytes.len() {
            let mut b = bytes.clone();
            b[i] ^= 0x01;
            let Ok(mut v) = serde_json::from_slice::<Value>(&b) else {
                continue;
            };
            if !v.is_object() {
                continue;
            }
            v["signatures"] = signatures.clone();
            presented += 1;
            assert_ne!(code(&q, &v), "accepted", "payload byte {i} flipped");
        }
        assert!(presented > bytes.len() / 2);
        for s in 0..2 {
            for i in 0..64 {
                let mut v = doc.clone();
                let text = v["signatures"][s]["signature"]
                    .as_str()
                    .unwrap()
                    .to_string();
                let mut raw = hex_decode(&text, "signature", 64).unwrap();
                raw[i] ^= 0x01;
                v["signatures"][s]["signature"] = json!(hex(&raw));
                assert_eq!(code(&q, &v), "bad_signature", "signature {s} byte {i}");
            }
        }
    }

    #[test]
    fn malformed_certificates_are_refused_by_name() {
        let q = quorum(2, ABC);
        let good = certificate(&q, &ABC[..2]);
        let with = |f: &dyn Fn(&mut Value)| {
            let mut v = good.clone();
            f(&mut v);
            code(&q, &v)
        };
        assert_eq!(with(&|v| v["signatures"] = json!([])), "no_signatures");
        let s = good["signatures"][0].clone();
        assert_eq!(
            with(&|v| v["signatures"] = json!([s, s, s, s])),
            "too_many_signatures"
        );
        assert_eq!(
            with(&|v| v["signatures"][0]["extra"] = json!(1)),
            "malformed_signature"
        );
        assert_eq!(
            with(&|v| {
                let up = v["signatures"][0]["signature"]
                    .as_str()
                    .unwrap()
                    .to_uppercase();
                v["signatures"][0]["signature"] = json!(up);
            }),
            "malformed_signature"
        );
        assert_eq!(
            with(&|v| v["signatures"][0]["signature"] = json!("00".repeat(64))),
            "bad_signature"
        );
        assert_eq!(
            with(&|v| v["kind"] = json!(POLICY_CHANGE_KIND)),
            "certificate_kind"
        );
        assert_eq!(with(&|v| v["signature"] = json!("00")), "mixed_forms");
        assert_eq!(
            with(&|v| {
                v.as_object_mut().unwrap().remove("policy_digest");
            }),
            "policy_mismatch"
        );
        for (field, _) in TAKEOVER_FIELDS {
            assert_eq!(
                with(&|v| {
                    v.as_object_mut().unwrap().remove(*field);
                }),
                "payload_incomplete",
                "{field}"
            );
        }
        assert_eq!(with(&|v| v["note"] = json!(1.5)), "payload_incomplete");
    }

    #[test]
    fn a_quorum_is_declared_with_a_strict_majority_of_valid_distinct_keys() {
        let refused = |v: Value| {
            Quorum::declared("replicas", &v)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        assert!(refused(policy(1, &ABC[..2])).contains("not a strict majority"));
        assert!(
            refused(policy(2, &[ABC[0], ABC[1], ABC[2], ("replica-d", 4)]))
                .contains("not a strict majority")
        );
        assert!(refused(policy(0, ABC)).contains("from 1 to"));
        let weak =
            json!({"threshold": 1, "keys": [{"key_id": "weak", "public_key": "00".repeat(32)}]});
        assert!(refused(weak).contains("small order"));
        let mut extra = policy(2, ABC);
        extra["note"] = json!("x");
        assert!(refused(extra).contains("exactly the fields keys and threshold"));
    }
}
