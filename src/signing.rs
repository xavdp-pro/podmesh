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
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::Value;

type Error = Box<dyn std::error::Error>;

pub const SIGNED_PROOF_KIND: &str = "podmesh-takeover-proof/ed25519";
pub const UNSIGNED_PROOF_KIND: &str = "podmesh-takeover-proof/lab-unsigned";

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
