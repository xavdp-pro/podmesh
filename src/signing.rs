//! Verification of the authority's signed documents (Codex's step after P0: an Ed25519-signed
//! takeover document). PodMesh holds no key: a policy names the authority's public key, given by
//! the operator's tool when the policy is declared, and a document is accepted only when its
//! signature verifies under that key over the document's canonical form.
//!
//! Canonical form: the document without its `signature` field, serialised as compact JSON with
//! the keys of every object sorted (what `serde_json` produces for a `Value`, and what the tool
//! produces with `sort_keys=True, separators=(',', ':'), ensure_ascii=False`). Numbers are
//! integers in every document signed today; a document with a float is refused, since two
//! serialisers may print one differently.
//!
//! QUORUM CERTIFICATES (V3-2). A policy may name, instead of one key, an authority QUORUM: n replica
//! public keys, each under a stable `key_id`, and a threshold k with 2k > n. An exclusive decision is
//! then a CERTIFICATE: one document, a `policy_digest` naming the quorum it was made under, and a list
//! of `signatures`, each `{key_id, signature}` over the same canonical bytes (the document without
//! `signatures`). The node accepts it only when at least k DISTINCT keys of that quorum signed it, and
//! refuses with a named reason otherwise (`Refusal::code`): a duplicate, unknown or malformed signature
//! refuses the whole certificate rather than being skipped, so what the node accepts is exactly a set
//! of valid signatures from distinct keys of its own policy. Any two sets of k keys out of n share a
//! key when 2k > n, which is what lets a key's owner that signs once per (resource, epoch) keep two
//! decisions from both reaching k -- that promise is the manager's (V3-3), not this file's.
//!
//! The single `authority_key` of today's policies is the 1-of-1 quorum (`Quorum::single`): its
//! documents, the `signer`/`signature` kind above, verify exactly as before, and a certificate made
//! under its 1-of-1 digest verifies too, which is the migration's path from the gate's key to the
//! replicas' keys.
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

type Error = Box<dyn std::error::Error>;

pub const SIGNED_PROOF_KIND: &str = "podmesh-takeover-proof/ed25519";
pub const UNSIGNED_PROOF_KIND: &str = "podmesh-takeover-proof/lab-unsigned";
/// A takeover document as a certificate: k-of-n signatures of the policy's quorum.
pub const QUORUM_PROOF_KIND: &str = "podmesh-takeover-proof/quorum-ed25519";
/// A change of a resource's authority set, certified by the quorum it replaces.
pub const POLICY_CHANGE_KIND: &str = "podmesh-policy-change/quorum-ed25519";
/// The form of the canonical policy a `policy_digest` is taken over.
pub const QUORUM_POLICY_FORM: &str = "podmesh-authority-quorum/1";
/// The key identifier the 1-of-1 quorum of a single `authority_key` gives that key.
pub const SINGLE_KEY_ID: &str = "authority_key";
/// At most this many keys in a quorum. The daemon reads a request of at most 4096 bytes, and a policy
/// change carries the new policy and a certificate of the old one in one request: nine keys of 32
/// characters and five signatures fit with room to spare. Three or five replicas is the expected case.
pub const MAX_QUORUM_KEYS: usize = 9;
/// A key identifier's length bound, for the same reason.
pub const MAX_KEY_ID: usize = 32;

fn hex_decode(text: &str, what: &str, bytes: usize) -> Result<Vec<u8>, Error> {
    if text.len() != bytes * 2 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{what} must be {bytes} bytes as {} lowercase hex characters", bytes * 2).into());
    }
    if text.bytes().any(|b| b.is_ascii_uppercase()) {
        return Err(format!("{what} must be lowercase hex").into());
    }
    (0..bytes).map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|e| e.into())).collect()
}

/// The authority's public key as a policy records it: 32 bytes, lowercase hex. Checked to be a
/// valid Ed25519 point when the policy is declared, so a mistyped key is refused then.
pub fn verifying_key(hex: &str) -> Result<VerifyingKey, Error> {
    let bytes: [u8; 32] = hex_decode(hex, "authority_key", 32)?.try_into().map_err(|_| "authority_key length")?;
    VerifyingKey::from_bytes(&bytes).map_err(|e| format!("authority_key is not a valid Ed25519 public key: {e}").into())
}

fn has_float(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.is_f64(),
        Value::Array(a) => a.iter().any(has_float),
        Value::Object(o) => o.values().any(has_float),
        _ => false,
    }
}

/// The canonical bytes a signature covers: the document without `signature`, keys sorted,
/// compact.
pub fn canonical(document: &Value) -> Result<Vec<u8>, Error> {
    let mut payload = document.as_object().ok_or("a signed document must be an object")?.clone();
    payload.remove("signature");
    let payload = Value::Object(payload);
    if has_float(&payload) {
        return Err("a signed document must not carry a floating-point number".into());
    }
    Ok(serde_json::to_vec(&payload)?)
}

/// Verifies `document.signature` under `key_hex` and requires `document.signer` to name that
/// key: a document signed by another key is refused as such, before the signature is even tried,
/// so that the refusal says which it was. Returns nothing but the verdict; the caller checks the
/// document's bindings afterwards, on a document whose origin is now established.
pub fn verify(document: &Value, key_hex: &str) -> Result<(), Error> {
    let key = verifying_key(key_hex)?;
    let signer = document["signer"].as_str().ok_or("the document names no signer")?;
    if signer != key_hex {
        return Err("the document is signed by a key this resource's policy does not name".into());
    }
    let signature = document["signature"].as_str().ok_or("the document carries no signature")?;
    let bytes: [u8; 64] = hex_decode(signature, "signature", 64)?.try_into().map_err(|_| "signature length")?;
    let signature = Signature::from_bytes(&bytes);
    key.verify(&canonical(document)?, &signature).map_err(|_| "the document's signature does not verify: altered, or not signed over this content".into())
}

/// Why a certificate was refused, by name: what an operator or a test reads, and what stays stable when
/// the sentence after it is reworded.
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
    Refusal { code, detail: detail.into() }
}

/// The type a certificate's payload field must have.
#[derive(Clone, Copy)]
pub enum Field {
    Text,
    Integer,
    /// A string, or null: `previous_holder` of a first epoch.
    TextOrNull,
}

/// What every takeover certificate binds, whatever its method: the resource, both epochs, both holders,
/// the new holder's boot, the grant, the method, the barrier, and its own life. `policy_digest`,
/// `authority_id` and `kind` are checked before these, by `Quorum::verify`.
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

/// What a policy-change certificate binds: the resource, the policy it moves to, and its own life. The
/// policy it moves from is its `policy_digest`.
pub const POLICY_CHANGE_FIELDS: &[(&str, Field)] = &[
    ("resource", Field::Text),
    ("new_policy_digest", Field::Text),
    ("issued_at", Field::Integer),
    ("expires_at", Field::Integer),
];

/// One key of a quorum: its stable identifier and its public half. The identifier names the key, not a
/// replica's store: the private half is a secret the replica's host mounts outside the universe's state,
/// so a recovery point or a clone of the replica carries neither the key nor a new identity for it.
pub struct QuorumKey {
    pub key_id: String,
    pub public_key: String,
    key: VerifyingKey,
}

/// The authority a policy names, as a quorum: n keys, a threshold k, and the authority's identifier.
pub struct Quorum {
    pub authority_id: String,
    pub threshold: usize,
    /// Sorted by `key_id`, so that the canonical policy, and its digest, do not depend on the order the
    /// operator listed them in.
    pub keys: Vec<QuorumKey>,
    /// The 1-of-1 of a policy that names a single `authority_key`: its signer/signature documents verify
    /// as they always did. False for a declared `authority_quorum`, even a 1-of-1 one.
    pub single_key: bool,
}

fn key_id(value: &str) -> Result<(), Error> {
    let mut bytes = value.bytes();
    let head = bytes.next().ok_or("key_id must not be empty")?;
    if !head.is_ascii_alphanumeric() || value.len() > MAX_KEY_ID
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b':' || b == b'-')
    {
        return Err(format!("key_id must be 1-{MAX_KEY_ID} ASCII characters from [A-Za-z0-9_.:-], starting alphanumeric").into());
    }
    Ok(())
}

impl Quorum {
    /// A quorum as `activation_require` declares it: `{"threshold": k, "keys": [{"key_id", "public_key"}]}`,
    /// no other field. Refused: no key, more than `MAX_QUORUM_KEYS`, a key_id or a public key named twice
    /// (two identifiers for one key would let one signer count twice), a key that is not a valid Ed25519
    /// point or is of small order (anyone can sign for such a key), and a threshold that is not a strict
    /// majority of the keys -- with 2k <= n two disjoint sets of signers could each certify a decision.
    pub fn declared(authority_id: &str, value: &Value) -> Result<Quorum, Error> {
        let object = value.as_object().ok_or("authority_quorum must be an object")?;
        let mut fields: Vec<&str> = object.keys().map(String::as_str).collect();
        fields.sort_unstable();
        if fields != ["keys", "threshold"] {
            return Err("authority_quorum must carry exactly the fields keys and threshold".into());
        }
        let entries = object["keys"].as_array().ok_or("authority_quorum.keys must be a list")?;
        if entries.is_empty() || entries.len() > MAX_QUORUM_KEYS {
            return Err(format!("authority_quorum.keys must name 1 to {MAX_QUORUM_KEYS} keys").into());
        }
        let mut keys: Vec<QuorumKey> = Vec::new();
        for entry in entries {
            let e = entry.as_object().ok_or("each authority_quorum key must be an object")?;
            let mut names: Vec<&str> = e.keys().map(String::as_str).collect();
            names.sort_unstable();
            if names != ["key_id", "public_key"] {
                return Err("each authority_quorum key must carry exactly key_id and public_key".into());
            }
            let id = e["key_id"].as_str().ok_or("key_id must be a string")?;
            key_id(id)?;
            let hex = e["public_key"].as_str().ok_or("public_key must be a string")?;
            let key = verifying_key(hex).map_err(|e| format!("key {id}: {}", e.to_string().replace("authority_key", "public_key")))?;
            if key.is_weak() {
                return Err(format!("key {id}: a public key of small order is refused: anyone can sign for it").into());
            }
            if keys.iter().any(|k| k.key_id == id) {
                return Err(format!("key_id {id} is named twice").into());
            }
            if keys.iter().any(|k| k.public_key == hex) {
                return Err(format!("the public key of {id} is named twice, under two identifiers: one signer would count twice").into());
            }
            keys.push(QuorumKey { key_id: id.to_string(), public_key: hex.to_string(), key });
        }
        let threshold = object["threshold"].as_u64().ok_or("authority_quorum.threshold must be an integer")? as usize;
        if threshold < 1 || threshold > keys.len() {
            return Err(format!("authority_quorum.threshold must be from 1 to the {} keys named", keys.len()).into());
        }
        if 2 * threshold <= keys.len() {
            return Err(format!(
                "authority_quorum.threshold {threshold} of {} is not a strict majority: two disjoint sets of signers could each certify a decision",
                keys.len()
            )
            .into());
        }
        keys.sort_by(|a, b| a.key_id.cmp(&b.key_id));
        Ok(Quorum { authority_id: authority_id.to_string(), threshold, keys, single_key: false })
    }

    /// The 1-of-1 quorum of a policy that names a single `authority_key`: the migration's starting point.
    pub fn single(authority_id: &str, key_hex: &str) -> Result<Quorum, Error> {
        let key = verifying_key(key_hex)?;
        Ok(Quorum {
            authority_id: authority_id.to_string(),
            threshold: 1,
            keys: vec![QuorumKey { key_id: SINGLE_KEY_ID.to_string(), public_key: key_hex.to_string(), key }],
            single_key: true,
        })
    }

    /// The quorum as a policy stores it and `activation_status` reports it.
    pub fn stored(&self) -> Value {
        serde_json::json!({
            "threshold": self.threshold,
            "keys": self.keys.iter().map(|k| serde_json::json!({"key_id": k.key_id, "public_key": k.public_key})).collect::<Vec<_>>(),
        })
    }

    /// What `policy_digest` is taken over: the form, the authority, whether it is a single key's 1-of-1,
    /// the threshold and the keys in key_id order, canonical JSON. The resource is not in it -- one quorum
    /// may govern many resources -- and is bound by every certificate separately. `single_key` is in it
    /// because the two forms differ in what they accept (a single-key policy still takes the gate's
    /// permits), so moving between them is a change of the authority set even over the same key.
    pub fn canonical_policy(&self) -> Vec<u8> {
        let mut v = self.stored();
        v["form"] = Value::from(QUORUM_POLICY_FORM);
        v["authority_id"] = Value::from(self.authority_id.as_str());
        v["single_key"] = Value::from(self.single_key);
        serde_json::to_vec(&v).unwrap_or_default()
    }

    /// SHA-256 of the canonical policy, lowercase hex: what every certificate under this policy names.
    pub fn digest(&self) -> String {
        sha256_hex(&self.canonical_policy())
    }

    /// Verifies a certificate of `kind` under this quorum: the kind, the authority and the policy digest
    /// first, then the payload's fields, then every signature, and only then the count. Returns the key
    /// identifiers that signed, in the order given. Nothing here reads what the document decides: the
    /// caller checks the binding afterwards, on a document whose origin is now established.
    pub fn verify(&self, document: &Value, kind: &str, required: &[(&str, Field)]) -> Result<Vec<String>, Refusal> {
        let object = document.as_object().ok_or_else(|| refusal("payload_incomplete", "a certificate must be an object"))?;
        let found = object.get("kind").and_then(Value::as_str).unwrap_or("none");
        if found != kind {
            return Err(refusal("certificate_kind", format!("the document is {found}, not {kind}")));
        }
        if object.contains_key("signer") || object.contains_key("signature") {
            return Err(refusal("mixed_forms", "a certificate carries `signatures`, never the single-key `signer` or `signature`"));
        }
        if object.get("authority_id").and_then(Value::as_str) != Some(self.authority_id.as_str()) {
            return Err(refusal("authority_mismatch", "the certificate names another authority than this resource's policy"));
        }
        let digest = self.digest();
        match object.get("policy_digest").and_then(Value::as_str) {
            Some(d) if d == digest => {}
            Some(d) => return Err(refusal("policy_mismatch", format!("the certificate was made under policy {d}, this resource's policy is {digest}"))),
            None => return Err(refusal("policy_mismatch", "the certificate names no policy_digest")),
        }
        for (name, field) in required {
            let v = object.get(*name);
            let ok = match field {
                Field::Text => v.is_some_and(Value::is_string),
                Field::Integer => v.is_some_and(|v| v.is_i64()),
                Field::TextOrNull => v.is_some_and(|v| v.is_string() || v.is_null()),
            };
            if !ok {
                return Err(refusal("payload_incomplete", format!("the certificate lacks {name}, or it has the wrong type")));
            }
        }
        let mut payload = object.clone();
        payload.remove("signatures");
        let payload = Value::Object(payload);
        if has_float(&payload) {
            return Err(refusal("payload_incomplete", "a certificate must not carry a floating-point number"));
        }
        let message = serde_json::to_vec(&payload).map_err(|e| refusal("payload_incomplete", e.to_string()))?;
        let entries = match object.get("signatures").and_then(Value::as_array) {
            Some(a) if !a.is_empty() => a,
            _ => return Err(refusal("no_signatures", "the certificate carries no signatures")),
        };
        if entries.len() > self.keys.len() {
            return Err(refusal("too_many_signatures", format!("{} signatures for a policy of {} keys", entries.len(), self.keys.len())));
        }
        let mut signers: Vec<String> = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            let e = entry.as_object().filter(|e| e.len() == 2).ok_or_else(|| refusal("malformed_signature", format!("signature {i} is not exactly {{key_id, signature}}")))?;
            let (Some(id), Some(hex)) = (e.get("key_id").and_then(Value::as_str), e.get("signature").and_then(Value::as_str)) else {
                return Err(refusal("malformed_signature", format!("signature {i} is not exactly {{key_id, signature}} as strings")));
            };
            let bytes: [u8; 64] = hex_decode(hex, "signature", 64)
                .ok()
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| refusal("malformed_signature", format!("signature {i} ({id}) is not 64 bytes as 128 lowercase hex characters")))?;
            if signers.iter().any(|s| s == id) {
                return Err(refusal("duplicate_key", format!("{id} signed twice; a key counts once, and a certificate that lists it twice is refused")));
            }
            let key = self.keys.iter().find(|k| k.key_id == id).ok_or_else(|| refusal("unknown_key", format!("{id} is not a key of this resource's policy")))?;
            // Strict: a non-canonical signature or a small-order key is refused, so that no signature
            // counts that its key's owner did not make.
            key.key
                .verify_strict(&message, &Signature::from_bytes(&bytes))
                .map_err(|_| refusal("bad_signature", format!("{id}'s signature does not verify: altered, or not signed over this content by that key")))?;
            signers.push(id.to_string());
        }
        if signers.len() < self.threshold {
            return Err(refusal(
                "below_threshold",
                format!("{} distinct key(s) signed, the policy requires {} of its {}", signers.len(), self.threshold, self.keys.len()),
            ));
        }
        Ok(signers)
    }
}

/// SHA-256, lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The digest of a certificate's payload (the document without its signatures): the decision's identity,
/// whichever set of signers presented it.
pub fn payload_digest(document: &Value) -> Result<String, Error> {
    let mut payload = document.as_object().ok_or("a certificate must be an object")?.clone();
    payload.remove("signatures");
    Ok(sha256_hex(&serde_json::to_vec(&Value::Object(payload))?))
}

/// A takeover document under a policy's authority, in whichever form that authority accepts: the single
/// key's signed document (verified exactly as before, by `verify`) under a single-key policy, or a quorum
/// certificate under any keyed policy. Returns the signers, for the answer.
pub fn verify_takeover(document: &Value, quorum: &Quorum) -> Result<Vec<String>, Error> {
    let kind = document["kind"].as_str().unwrap_or("none");
    if quorum.single_key && kind == SIGNED_PROOF_KIND {
        verify(document, &quorum.keys[0].public_key)?;
        return Ok(vec![SINGLE_KEY_ID.to_string()]);
    }
    Ok(quorum.verify(document, QUORUM_PROOF_KIND, TAKEOVER_FIELDS)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_form_sorts_keys_and_drops_the_signature() {
        let doc = json!({"z": 1, "a": {"y": [1, 2], "b": "é"}, "signature": "x"});
        assert_eq!(String::from_utf8(canonical(&doc).unwrap()).unwrap(), r#"{"a":{"b":"é","y":[1,2]},"z":1}"#);
        assert!(canonical(&json!({"a": 1.5})).is_err());
    }

    #[test]
    fn a_known_vector_verifies_and_an_altered_one_does_not() {
        // RFC 8032 test 1: the empty message under the first test key.
        let key = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
        let sig = "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b";
        // canonical({}) is "{}" -- not the empty message; so the vector is checked directly.
        let vk = verifying_key(key).unwrap();
        let bytes: [u8; 64] = hex_decode(sig, "signature", 64).unwrap().try_into().unwrap();
        assert!(vk.verify(b"", &Signature::from_bytes(&bytes)).is_ok());
        assert!(vk.verify(b"x", &Signature::from_bytes(&bytes)).is_err());
        assert!(verify(&json!({"signer": key, "signature": sig}), key).is_err()); // canonical form is "{...}", not ""
        assert!(verify(&json!({"signer": "00", "signature": sig}), key).unwrap_err().to_string().contains("does not name"));
        assert!(verifying_key("zz").is_err());
    }
}

#[cfg(test)]
mod point_tests {
    #[test]
    fn a_y_coordinate_off_the_curve_is_refused_and_zero_is_a_point() {
        assert!(super::verifying_key("0200000000000000000000000000000000000000000000000000000000000000").is_err());
        assert!(super::verifying_key("0000000000000000000000000000000000000000000000000000000000000000").is_ok());
    }
}

#[cfg(test)]
mod cross_language {
    /// A document signed by the tool's Python helper (`PODMESH_SIGNING_VECTOR` names the file it
    /// wrote: `{"key": <public hex>, "document": <signed document>}`): the two canonical forms
    /// agree when it verifies here. Skipped without the file.
    #[test]
    fn a_document_signed_by_the_tool_verifies_here() {
        let Ok(path) = std::env::var("PODMESH_SIGNING_VECTOR") else { return };
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let key = v["key"].as_str().unwrap();
        super::verify(&v["document"], key).unwrap();
        let mut altered = v["document"].clone();
        altered["issued_at"] = serde_json::Value::from(altered["issued_at"].as_i64().unwrap() + 1);
        assert!(super::verify(&altered, key).is_err());
    }
}

/// Keys and certificates for the tests of this file, of `activation` and of `publisher`: deterministic
/// keys from a one-byte seed, and a certificate signed by any set of them over its canonical form.
#[cfg(test)]
pub(crate) mod testkit {
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{json, Value};

    pub fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    pub fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn public(seed: u8) -> String {
        hex(&key(seed).verifying_key().to_bytes())
    }

    /// A declared quorum: `threshold` of the keys (key_id, seed).
    pub fn policy(threshold: usize, keys: &[(&str, u8)]) -> Value {
        json!({"threshold": threshold, "keys": keys.iter().map(|(id, s)| json!({"key_id": id, "public_key": public(*s)})).collect::<Vec<_>>()})
    }

    /// `document` with its `signatures` replaced by one per (key_id, seed), each over the canonical
    /// form of the document without `signatures`.
    pub fn sign(document: &Value, signers: &[(&str, u8)]) -> Value {
        let mut d = document.clone();
        d.as_object_mut().unwrap().remove("signatures");
        let message = serde_json::to_vec(&d).unwrap();
        d["signatures"] = json!(signers
            .iter()
            .map(|(id, s)| json!({"key_id": id, "signature": hex(&key(*s).sign(&message).to_bytes())}))
            .collect::<Vec<_>>());
        d
    }

    /// An unsigned takeover certificate, live now, eligible now, method `lease_barrier`.
    #[allow(clippy::too_many_arguments)]
    pub fn takeover(authority: &str, digest: &str, resource: &str, epoch: i64, holder: &str, boot: &str) -> Value {
        let now = crate::now() as i64;
        json!({"kind": super::QUORUM_PROOF_KIND, "authority_id": authority, "policy_digest": digest, "resource": resource,
               "new_epoch": epoch, "previous_epoch": epoch - 1, "new_holder": holder, "previous_holder": "00000000-0000-4000-8000-00000000000b",
               "holder_boot_id": boot, "grant_id": format!("g{epoch}"), "method": "lease_barrier",
               "eligible_after": now - 1, "issued_at": now, "expires_at": now + 3600})
    }
}

#[cfg(test)]
mod quorum_tests {
    //! The verifier of V3-2, alone: what a certificate needs to count, and every way it is refused.
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
        sign(&testkit::takeover("replicas", &q.digest(), R, 12, HOST, BOOT), signers)
    }

    fn code(q: &Quorum, doc: &Value) -> &'static str {
        match q.verify(doc, QUORUM_PROOF_KIND, TAKEOVER_FIELDS) {
            Ok(_) => "accepted",
            Err(r) => r.code,
        }
    }

    /// Proves: a certificate counts once k distinct keys of the policy signed it, at exactly k and at
    /// every number above, for 2-of-3 and 3-of-5, whatever order the signatures come in.
    #[test]
    fn k_of_n_is_accepted_at_exactly_k_and_above() {
        let q = quorum(2, ABC);
        for signers in [&ABC[..2], &ABC[1..], &[ABC[2], ABC[0]], ABC] {
            let doc = certificate(&q, signers);
            let who = q.verify(&doc, QUORUM_PROOF_KIND, TAKEOVER_FIELDS).unwrap();
            assert_eq!(who, signers.iter().map(|(id, _)| id.to_string()).collect::<Vec<_>>());
        }
        let q5 = quorum(3, FIVE);
        for n in 3..=5 {
            assert_eq!(code(&q5, &certificate(&q5, &FIVE[5 - n..])), "accepted", "{n} of 5");
        }
    }

    /// Proves: one signature short of the threshold is a minority, and is refused by name.
    #[test]
    fn k_minus_one_is_refused() {
        let q = quorum(2, ABC);
        for one in ABC {
            assert_eq!(code(&q, &certificate(&q, &[*one])), "below_threshold");
        }
        let q5 = quorum(3, FIVE);
        assert_eq!(code(&q5, &certificate(&q5, &FIVE[..2])), "below_threshold");
        assert_eq!(code(&q5, &certificate(&q5, &FIVE[3..])), "below_threshold");
        let e = q5.verify(&certificate(&q5, &FIVE[..2]), QUORUM_PROOF_KIND, TAKEOVER_FIELDS).unwrap_err().to_string();
        assert!(e.contains("2 distinct key(s) signed, the policy requires 3 of its 5"), "{e}");
    }

    /// Proves: a key counts once. A certificate listing one key twice does not reach 2-of-3, and is
    /// refused whole (duplicate_key) even beside another valid signature; and a policy cannot name one
    /// public key under two identifiers, which would be the same double count by another door.
    #[test]
    fn the_same_key_twice_is_counted_once() {
        let q = quorum(2, ABC);
        let doc = certificate(&q, &[ABC[0], ABC[0]]);
        assert_eq!(code(&q, &doc), "duplicate_key");
        let e = q.verify(&doc, QUORUM_PROOF_KIND, TAKEOVER_FIELDS).unwrap_err().to_string();
        assert!(e.contains("a key counts once"), "{e}");
        assert_eq!(code(&q, &certificate(&q, &[ABC[0], ABC[1], ABC[0]])), "duplicate_key");
        let alias = json!({"threshold": 2, "keys": [
            {"key_id": "replica-a", "public_key": public(1)}, {"key_id": "alias-of-a", "public_key": public(1)}, {"key_id": "replica-c", "public_key": public(3)}]});
        let e = Quorum::declared("replicas", &alias).err().unwrap().to_string();
        assert!(e.contains("named twice, under two identifiers"), "{e}");
        let twice = json!({"threshold": 2, "keys": [
            {"key_id": "replica-a", "public_key": public(1)}, {"key_id": "replica-a", "public_key": public(2)}, {"key_id": "replica-c", "public_key": public(3)}]});
        assert!(Quorum::declared("replicas", &twice).err().unwrap().to_string().contains("key_id replica-a is named twice"));
    }

    /// Proves: a key the policy does not name never counts. Under an identifier the policy does not know
    /// it is `unknown_key`; claiming the identifier of a policy key it is `bad_signature`; a certificate
    /// made under another policy (another key set, or the same keys under another threshold or
    /// authority) is `policy_mismatch` or `authority_mismatch` before any signature is tried.
    #[test]
    fn a_key_from_another_policy_is_refused() {
        let q = quorum(2, ABC);
        assert_eq!(code(&q, &certificate(&q, &[ABC[0], ("replica-d", 4)])), "unknown_key");
        assert_eq!(code(&q, &certificate(&q, &[ABC[0], ("replica-b", 4)])), "bad_signature");
        let other = quorum(2, &[ABC[0], ABC[1], ("replica-d", 4)]);
        let foreign = sign(&testkit::takeover("replicas", &other.digest(), R, 12, HOST, BOOT), &[ABC[0], ABC[1]]);
        assert_eq!(code(&other, &foreign), "accepted");
        assert_eq!(code(&q, &foreign), "policy_mismatch", "signed by two keys this policy names, but under another policy");
        let three = quorum(3, ABC);
        assert_ne!(three.digest(), q.digest());
        assert_eq!(code(&q, &certificate(&three, ABC)), "policy_mismatch");
        let mut elsewhere = certificate(&q, &ABC[..2]);
        elsewhere["authority_id"] = json!("another-authority");
        assert_eq!(code(&q, &sign(&elsewhere, &ABC[..2])), "authority_mismatch");
    }

    /// Proves: the payload binds the resource and the epoch. A certificate for one resource or epoch,
    /// relabelled for another, no longer verifies; re-signed for the other it verifies here, and it is
    /// the binding in `activation` that refuses it there (tested beside that code).
    #[test]
    fn a_payload_relabelled_for_another_resource_or_epoch_is_refused() {
        let q = quorum(2, ABC);
        let doc = certificate(&q, &ABC[..2]);
        for (field, value) in [("resource", json!("00000000-0000-4000-8000-000000000001")), ("new_epoch", json!(13)), ("previous_epoch", json!(12)),
                               ("new_holder", json!("00000000-0000-4000-8000-00000000000b")), ("method", json!("first")),
                               ("eligible_after", json!(0)), ("expires_at", json!(4102444800i64)), ("holder_boot_id", json!("another-boot")),
                               ("grant_id", json!("g13"))] {
            let mut moved = doc.clone();
            moved[field] = value;
            assert_eq!(code(&q, &moved), "bad_signature", "{field}");
        }
    }

    /// Proves: the canonical form and the digest are what another implementation computes. The digest of
    /// a fixed 2-of-3 policy is pinned to the value Python's `json.dumps(sort_keys=True, separators=(',',
    /// ':'), ensure_ascii=False)` and `hashlib.sha256` give, and does not depend on the order the keys
    /// were declared in; a certificate signed by Python's `cryptography` over that form verifies here.
    #[test]
    fn the_policy_digest_and_a_certificate_signed_elsewhere_agree() {
        let q = quorum(2, ABC);
        assert_eq!(q.digest(), "9ff65bd48dbffd3be07a34977033007bd0085afe399fb414e27ceeaff5c67d62");
        let reversed: Vec<(&str, u8)> = ABC.iter().rev().copied().collect();
        assert_eq!(quorum(2, &reversed).digest(), q.digest());
        let python: Value = serde_json::from_str(r#"{"kind": "podmesh-takeover-proof/quorum-ed25519", "authority_id": "replicas", "policy_digest": "9ff65bd48dbffd3be07a34977033007bd0085afe399fb414e27ceeaff5c67d62", "resource": "91eeb6bf-5489-405b-b77a-53105b0aff7a", "new_epoch": 12, "previous_epoch": 11, "new_holder": "5d1c0b8e-3f59-4d0e-9d7a-2a1e7c4b9f10", "previous_holder": null, "holder_boot_id": "0b7f6f1e-6a55-4c1d-8f53-1c2d3e4f5a6b", "grant_id": "g12", "method": "lease_barrier", "eligible_after": 1700000600, "issued_at": 1700000000, "expires_at": 4102444800, "note": "decided by two replicas — naïve JSON", "signatures": [{"key_id": "replica-c", "signature": "9c722fa1724ee8b15966931dbabd728edb22712ca5393597de05bddde226ea8775e714d41ad4ab58b13597ef6d16f4c3368d0e74426c0902cbb6467f6b6d120f"}, {"key_id": "replica-a", "signature": "2035ab67576d9ca04900e013bda417c4f6630617e3f2904ce9da7367398f49a5f37ffa695b181a9945afb8a64d86ff25fd85efca896719a9a23c1e953296790d"}]}"#).unwrap();
        assert_eq!(q.verify(&python, QUORUM_PROOF_KIND, TAKEOVER_FIELDS).unwrap(), ["replica-c", "replica-a"]);
        assert_eq!(verify_takeover(&python, &q).unwrap(), ["replica-c", "replica-a"]);
    }

    /// The gate's document as `tools/ha-standby.py`'s `sign` writes it today (key from seed 42, one
    /// field with non-ASCII text), verbatim.
    const TOOL_SIGNED: &str = r#"{"kind": "podmesh-takeover-proof/ed25519", "authority_id": "lab-gate", "resource": "91eeb6bf-5489-405b-b77a-53105b0aff7a", "new_holder": "5d1c0b8e-3f59-4d0e-9d7a-2a1e7c4b9f10", "previous_holder": "00000000-0000-4000-8000-00000000000b", "new_epoch": 158, "previous_epoch": 157, "method": "lease_barrier", "eligible_after": 1700000600, "issued_at": 1700000000, "expires_at": 4102444800, "barrier_basis": "the previous lease plus the margin — naïve clocks", "signer": "197f6b23e16c8532c6abc838facd5ea789be0c76b2920334039bfa8b3d368d61", "note": "signed by the authority: the host verifies the signature under the key its policy names, then the binding", "signature": "9e9e9f25c1ee813e95827a0b81ed4c8bf0c963d80c6e7762958a91d3c4dc731391f82e5d1f4639920b3b2636ba7c1d5f6cac0b89de15a814025cb7b800c3a30e"}"#;

    /// Proves: the single key's path is byte-compatible with today's documents. The tool's signed
    /// document verifies through the 1-of-1 quorum exactly as through the unchanged `verify`, counted as
    /// the one key; any altered field fails both; a certificate made under the 1-of-1's digest verifies
    /// too (the migration's path); and the 1-of-1's digest is pinned, distinct from a declared 1-of-1
    /// quorum over the same key, whose form accepts no permit.
    #[test]
    fn the_single_key_path_is_byte_compatible_with_todays_documents() {
        let doc: Value = serde_json::from_str(TOOL_SIGNED).unwrap();
        let key = "197f6b23e16c8532c6abc838facd5ea789be0c76b2920334039bfa8b3d368d61";
        assert_eq!(public(42), key);
        verify(&doc, key).unwrap();
        let single = Quorum::single("lab-gate", key).unwrap();
        assert_eq!(verify_takeover(&doc, &single).unwrap(), [SINGLE_KEY_ID]);
        assert_eq!(single.digest(), "b6f2fbdbc0317d1991baa407f8c97a0b4fbbe2d7f7737fa4040c291790436fed");
        for field in ["resource", "new_holder", "method", "barrier_basis", "note", "signer"] {
            let mut altered = doc.clone();
            altered[field] = json!(format!("{}x", altered[field].as_str().unwrap()));
            assert!(verify(&altered, key).is_err() && verify_takeover(&altered, &single).is_err(), "{field}");
        }
        for field in ["new_epoch", "previous_epoch", "eligible_after", "issued_at", "expires_at"] {
            let mut altered = doc.clone();
            altered[field] = json!(altered[field].as_i64().unwrap() + 1);
            assert!(verify(&altered, key).is_err() && verify_takeover(&altered, &single).is_err(), "{field}");
        }
        // A certificate under the 1-of-1 digest, signed by the gate's key under its key id.
        let cert = sign(&testkit::takeover("lab-gate", &single.digest(), R, 12, HOST, BOOT), &[(SINGLE_KEY_ID, 42)]);
        assert_eq!(verify_takeover(&cert, &single).unwrap(), [SINGLE_KEY_ID]);
        let declared = Quorum::declared("lab-gate", &json!({"threshold": 1, "keys": [{"key_id": SINGLE_KEY_ID, "public_key": key}]})).unwrap();
        assert_ne!(declared.digest(), single.digest());
        assert_eq!(code(&declared, &cert), "policy_mismatch");
        // Under a declared quorum the single key's document is not a certificate.
        assert!(verify_takeover(&doc, &declared).unwrap_err().to_string().contains("certificate_kind"));
    }

    /// Proves, by mutation: no single flipped byte of a certificate is accepted. Every byte of the
    /// canonical payload is flipped in turn (each bit pattern XOR 0x01); a result that is still a JSON
    /// object is presented, re-assembled with the original signatures, and refused. Every byte of each
    /// signature is flipped in turn and refused. The same for the single key's document.
    #[test]
    fn flipping_one_byte_of_the_payload_or_a_signature_is_refused() {
        let q = quorum(2, ABC);
        let doc = certificate(&q, &ABC[..2]);
        let signatures = doc["signatures"].clone();
        let mut payload = doc.clone();
        payload.as_object_mut().unwrap().remove("signatures");
        let bytes = serde_json::to_vec(&payload).unwrap();
        let mut presented = 0;
        for i in 0..bytes.len() {
            let mut b = bytes.clone();
            b[i] ^= 0x01;
            let Ok(mut v) = serde_json::from_slice::<Value>(&b) else { continue };
            if !v.is_object() {
                continue;
            }
            v["signatures"] = signatures.clone();
            presented += 1;
            assert_ne!(code(&q, &v), "accepted", "payload byte {i} flipped: {}", String::from_utf8_lossy(&b));
        }
        assert!(presented > bytes.len() / 2, "only {presented} of {} mutations parsed", bytes.len());
        for s in 0..2 {
            for i in 0..64 {
                let mut v = doc.clone();
                let hex = v["signatures"][s]["signature"].as_str().unwrap().to_string();
                let mut raw = hex_decode(&hex, "signature", 64).unwrap();
                raw[i] ^= 0x01;
                v["signatures"][s]["signature"] = json!(testkit::hex(&raw));
                assert_eq!(code(&q, &v), "bad_signature", "signature {s} byte {i}");
            }
        }
        let legacy: Value = serde_json::from_str(TOOL_SIGNED).unwrap();
        let single = Quorum::single("lab-gate", &public(42)).unwrap();
        let bytes = canonical(&legacy).unwrap();
        for i in 0..bytes.len() {
            let mut b = bytes.clone();
            b[i] ^= 0x01;
            let Ok(mut v) = serde_json::from_slice::<Value>(&b) else { continue };
            if !v.is_object() {
                continue;
            }
            v["signature"] = legacy["signature"].clone();
            assert!(verify_takeover(&v, &single).is_err(), "legacy payload byte {i} flipped");
        }
        for i in 0..64 {
            let mut raw = hex_decode(legacy["signature"].as_str().unwrap(), "signature", 64).unwrap();
            raw[i] ^= 0x01;
            let mut v = legacy.clone();
            v["signature"] = json!(testkit::hex(&raw));
            assert!(verify_takeover(&v, &single).is_err(), "legacy signature byte {i}");
        }
    }

    /// Proves: every malformed certificate is refused by name, before any count.
    #[test]
    fn malformed_certificates_are_refused_by_name() {
        let q = quorum(2, ABC);
        let good = certificate(&q, &ABC[..2]);
        let with = |f: &dyn Fn(&mut Value)| { let mut v = good.clone(); f(&mut v); code(&q, &v) };
        assert_eq!(with(&|v| v["signatures"] = json!([])), "no_signatures");
        assert_eq!(with(&|v| { v.as_object_mut().unwrap().remove("signatures"); }), "no_signatures");
        assert_eq!(with(&|v| v["signatures"] = json!("x")), "no_signatures");
        let s = good["signatures"][0].clone();
        assert_eq!(with(&|v| v["signatures"] = json!([s, s, s, s])), "too_many_signatures");
        assert_eq!(with(&|v| v["signatures"][0] = json!("x")), "malformed_signature");
        assert_eq!(with(&|v| { v["signatures"][0].as_object_mut().unwrap().remove("key_id"); }), "malformed_signature");
        assert_eq!(with(&|v| v["signatures"][0]["extra"] = json!(1)), "malformed_signature");
        assert_eq!(with(&|v| v["signatures"][0]["key_id"] = json!(1)), "malformed_signature");
        assert_eq!(with(&|v| v["signatures"][0]["signature"] = json!("00")), "malformed_signature");
        assert_eq!(with(&|v| { let up = v["signatures"][0]["signature"].as_str().unwrap().to_uppercase(); v["signatures"][0]["signature"] = json!(up); }), "malformed_signature");
        assert_eq!(with(&|v| v["signatures"][0]["signature"] = json!("zz".repeat(64))), "malformed_signature");
        assert_eq!(with(&|v| v["signatures"][0]["signature"] = json!("00".repeat(64))), "bad_signature");
        assert_eq!(with(&|v| v["kind"] = json!(SIGNED_PROOF_KIND)), "certificate_kind");
        assert_eq!(with(&|v| v["signer"] = json!(public(1))), "mixed_forms");
        assert_eq!(with(&|v| v["signature"] = json!("00")), "mixed_forms");
        assert_eq!(with(&|v| { v.as_object_mut().unwrap().remove("policy_digest"); }), "policy_mismatch");
        for field in ["resource", "new_epoch", "previous_epoch", "new_holder", "previous_holder", "holder_boot_id", "grant_id", "method",
                      "eligible_after", "issued_at", "expires_at"] {
            assert_eq!(with(&|v| { v.as_object_mut().unwrap().remove(field); }), "payload_incomplete", "{field}");
        }
        assert_eq!(with(&|v| v["new_epoch"] = json!("12")), "payload_incomplete");
        assert_eq!(with(&|v| v["note"] = json!(1.5)), "payload_incomplete");
        assert_eq!(code(&q, &json!([1])), "payload_incomplete");
    }

    /// Proves: a policy that could certify two decisions for one epoch, or that a key could sign for
    /// without its owner, is refused at declaration.
    #[test]
    fn a_quorum_is_declared_with_a_strict_majority_of_valid_distinct_keys() {
        let refused = |v: Value| Quorum::declared("replicas", &v).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(refused(policy(1, &ABC[..2])).contains("not a strict majority"));
        assert!(refused(policy(1, ABC)).contains("not a strict majority"));
        assert!(refused(policy(2, &[ABC[0], ABC[1], ABC[2], ("replica-d", 4)])).contains("not a strict majority"));
        assert!(Quorum::declared("replicas", &policy(3, &[ABC[0], ABC[1], ABC[2], ("replica-d", 4)])).is_ok());
        assert!(refused(policy(0, ABC)).contains("from 1 to"));
        assert!(refused(policy(4, ABC)).contains("from 1 to"));
        assert!(refused(policy(1, &[])).contains("1 to 9 keys"));
        let ten: Vec<(String, u8)> = (0..10u8).map(|i| (format!("r{i}"), 20 + i)).collect();
        let ten: Vec<(&str, u8)> = ten.iter().map(|(s, i)| (s.as_str(), *i)).collect();
        assert!(refused(policy(6, &ten)).contains("1 to 9 keys"));
        assert!(Quorum::declared("replicas", &policy(5, &ten[..9])).is_ok());
        assert!(refused(policy(2, &[("-a", 1), ABC[1], ABC[2]])).contains("key_id must be"));
        assert!(refused(policy(2, &[(&"a".repeat(33), 1), ABC[1], ABC[2]])).contains("key_id must be"));
        let weak = json!({"threshold": 1, "keys": [{"key_id": "weak", "public_key": "00".repeat(32)}]});
        assert!(refused(weak).contains("small order"));
        let off = json!({"threshold": 1, "keys": [{"key_id": "off", "public_key": format!("02{}", "00".repeat(31))}]});
        assert!(refused(off).contains("not a valid Ed25519 public key"));
        let mut extra = policy(2, ABC);
        extra["note"] = json!("x");
        assert!(refused(extra).contains("exactly the fields keys and threshold"));
        let mut extra = policy(2, ABC);
        extra["keys"][0]["custody"] = json!("host");
        assert!(refused(extra).contains("exactly key_id and public_key"));
    }
}
