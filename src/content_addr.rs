//! Crypto-free canonical encoding + content-addressing.
//!
//! These four helpers are the content-addressing substrate WG uses everywhere a
//! stable, tamper-evident id is needed for a JSON value:
//!
//! - [`canonical_json`] — recursively sorted-key, compact JSON bytes.
//! - [`blake3_32`] — a raw 32-byte BLAKE3 digest of arbitrary bytes.
//! - [`content_cid`] — `b3:<hex>` over [`canonical_json`].
//! - [`signing_digest`] — the 32-byte digest a signature covers (canonical bytes
//!   with the top-level `sig` field removed).
//!
//! They historically lived in `crate::identity` (they were introduced with the
//! WG-Fed wire envelopes, doc 04 §1.4), but they carry **no cryptography and no
//! identity semantics** — only `serde_json`, `blake3`, and `hex`. The completion
//! evidence / land / review planes (the daily-use path) depend on them just as
//! much as the federation envelopes do, so they live in this small standalone
//! module. That keeps the always-built evidence path independent of the
//! (feature-gated) federation cluster while every call site keeps byte-identical
//! behavior: these functions are moved verbatim, not rewritten.
//!
//! Canonicalization is done over `serde_json::Value` so it does not depend on
//! serde_json's map ordering: keys are emitted in sorted order at every level.

use serde_json::Value;

/// Recursively emit `value` as canonical JSON (object keys sorted, compact).
pub fn canonical_json(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Object(map) => {
            out.push(b'{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                // serde_json renders a string Value with correct JSON escaping.
                out.extend_from_slice(serde_json::to_string(k).unwrap().as_bytes());
                out.push(b':');
                write_canonical(&map[*k], out);
            }
            out.push(b'}');
        }
        Value::Array(arr) => {
            out.push(b'[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(v, out);
            }
            out.push(b']');
        }
        other => out.extend_from_slice(serde_json::to_string(other).unwrap().as_bytes()),
    }
}

/// BLAKE3 a byte slice into a raw 32-byte digest.
pub fn blake3_32(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// Content id of a value: `b3:<hex>` over its canonical serialization (doc 04 §1.4).
pub fn content_cid(value: &Value) -> String {
    format!("b3:{}", hex::encode(blake3_32(&canonical_json(value))))
}

/// The 32-byte digest that a signature covers: the canonical serialization of
/// `value` with any top-level `sig` field removed. Symmetric across sign/verify.
pub fn signing_digest(value: &Value) -> [u8; 32] {
    let mut v = value.clone();
    if let Value::Object(map) = &mut v {
        map.remove("sig");
    }
    blake3_32(&canonical_json(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_sorts_keys_recursively() {
        let a = serde_json::json!({"b": 1, "a": {"y": 2, "x": 3}});
        let b = serde_json::json!({"a": {"x": 3, "y": 2}, "b": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_eq!(canonical_json(&a), br#"{"a":{"x":3,"y":2},"b":1}"#);
    }

    #[test]
    fn signing_digest_ignores_sig_field() {
        let with_sig = serde_json::json!({"from": "alice", "sig": "deadbeef"});
        let other_sig = serde_json::json!({"from": "alice", "sig": "00000000"});
        let no_sig = serde_json::json!({"from": "alice"});
        assert_eq!(signing_digest(&with_sig), signing_digest(&other_sig));
        assert_eq!(signing_digest(&with_sig), signing_digest(&no_sig));
    }

    #[test]
    fn content_cid_is_stable_and_prefixed() {
        let v = serde_json::json!({"x": 1, "y": [1, 2, 3]});
        let cid = content_cid(&v);
        assert!(cid.starts_with("b3:"), "{cid}");
        assert_eq!(content_cid(&v), cid);
        // A change in content changes the cid.
        let v2 = serde_json::json!({"x": 2, "y": [1, 2, 3]});
        assert_ne!(content_cid(&v2), cid);
    }

    #[test]
    fn blake3_32_is_the_raw_digest_of_the_bytes() {
        let digest = blake3_32(b"wg");
        assert_eq!(digest, *blake3::hash(b"wg").as_bytes());
        // A different input gives a different digest (no accidental constant).
        assert_ne!(digest, blake3_32(b"wg "));
    }

    #[test]
    fn content_cid_agrees_with_blake3_over_canonical_bytes() {
        let v = serde_json::json!({"b": [1, {"z": true, "a": null}], "a": "s"});
        assert_eq!(
            content_cid(&v),
            format!("b3:{}", hex::encode(blake3_32(&canonical_json(&v))))
        );
    }
}
