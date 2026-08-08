//! Music-only asymmetric authorization for the user-owned Music daemon.
//!
//! The root DRM shell signs a short-lived, exact-body-bound capability with a
//! host-provisioned Ed25519 seed. The user `mde-musicd` process receives only
//! the matching public key, so validating a Music mutation never requires
//! copying the root cloud-arm secret into a user service.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde_json::Value;

/// Contract version for Music workspace authorization.
pub const MUSIC_AUTH_SCHEMA_VERSION: u64 = 1;
/// Domain-separated key identifier and rotation label.
pub const MUSIC_AUTH_KEY_ID: &str = "music-action-ed25519-v1";
/// Domain separation for the signed capability payload.
pub const MUSIC_AUTH_DOMAIN: &str = "magic-mesh:music-action-ed25519:v1";
/// Root-only systemd credential leaf containing 64 lowercase hex seed bytes.
pub const MUSIC_AUTH_CREDENTIAL_NAME: &str = "music-action-private-key";
/// Public verification key installed as non-secret host configuration.
pub const MUSIC_AUTH_PUBLIC_KEY_PATH: &str = "/etc/mde/music-action-public-key";
/// Maximum capability lifetime accepted by Music.
pub const MUSIC_AUTH_MAX_TTL_MS: i64 = 30_000;
/// Maximum nonce length admitted by the contract.
pub const MUSIC_AUTH_MAX_NONCE_BYTES: usize = 128;

/// The closed semantic scope bound into one Music capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MusicAuthContext<'a> {
    /// Exact capability verb.
    pub verb: &'a str,
    /// Local node identity.
    pub node: &'a str,
    /// Stable mutation scope.
    pub target: &'a str,
}

/// The authenticated fields carried inside a Music request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicAuthToken {
    /// Contract version.
    pub schema_version: u64,
    /// Key rotation identifier.
    pub key_id: String,
    /// Single-use nonce.
    pub nonce: String,
    /// Epoch-millisecond expiry.
    pub expires_at_ms: i64,
    /// Signed capability verb.
    pub verb: String,
    /// Signed local node.
    pub node: String,
    /// Signed mutation target.
    pub target: String,
    /// SHA-256 digest of the request with this auth object removed.
    pub request_sha256: String,
    /// Lowercase Ed25519 signature over the domain-separated fields.
    pub signature: String,
}

impl MusicAuthToken {
    fn from_value(value: &Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "music_auth is not an object".to_string())?;
        let string = |name: &str| {
            object
                .get(name)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("music_auth.{name} is missing"))
        };
        Ok(Self {
            schema_version: object
                .get("schema_version")
                .and_then(Value::as_u64)
                .ok_or_else(|| "music_auth.schema_version is missing".to_string())?,
            key_id: string("key_id")?,
            nonce: string("nonce")?,
            expires_at_ms: object
                .get("expires_at_ms")
                .and_then(Value::as_i64)
                .ok_or_else(|| "music_auth.expires_at_ms is missing".to_string())?,
            verb: string("verb")?,
            node: string("node")?,
            target: string("target")?,
            request_sha256: string("request_sha256")?,
            signature: string("signature")?,
        })
    }

    fn signing_bytes(&self) -> Vec<u8> {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            MUSIC_AUTH_DOMAIN,
            self.schema_version,
            self.key_id,
            self.nonce,
            self.expires_at_ms,
            self.verb,
            self.node,
            self.target,
            self.request_sha256,
        )
        .into_bytes()
    }

    fn to_value(&self) -> Value {
        serde_json::json!({
            "schema_version": self.schema_version,
            "key_id": self.key_id,
            "nonce": self.nonce,
            "expires_at_ms": self.expires_at_ms,
            "verb": self.verb,
            "node": self.node,
            "target": self.target,
            "request_sha256": self.request_sha256,
            "signature": self.signature,
        })
    }
}

/// Sign a request body with the root-only seed and insert `music_auth`.
pub fn sign_request(
    body: &str,
    context: MusicAuthContext<'_>,
    seed: &[u8; 32],
    nonce: &str,
    expires_at_ms: i64,
) -> Result<String, String> {
    let request_sha256 = request_digest(body)?;
    let signing_key = SigningKey::from_bytes(seed);
    let mut token = MusicAuthToken {
        schema_version: MUSIC_AUTH_SCHEMA_VERSION,
        key_id: MUSIC_AUTH_KEY_ID.to_string(),
        nonce: nonce.to_string(),
        expires_at_ms,
        verb: context.verb.to_string(),
        node: context.node.to_string(),
        target: context.target.to_string(),
        request_sha256,
        signature: String::new(),
    };
    token.signature = hex_encode(&signing_key.sign(&token.signing_bytes()).to_bytes());
    let mut document: Value = serde_json::from_str(body)
        .map_err(|_| "Music mutation body is not valid JSON".to_string())?;
    let object = document
        .as_object_mut()
        .ok_or_else(|| "Music mutation body is not a JSON object".to_string())?;
    object.insert("music_auth".to_string(), token.to_value());
    serde_json::to_string(&document).map_err(|error| error.to_string())
}

/// Verify a request's public-key signature and return its bounded token.
pub fn verify_request(
    body: &str,
    context: MusicAuthContext<'_>,
    public_key: &VerifyingKey,
) -> Result<MusicAuthToken, String> {
    let document: Value = serde_json::from_str(body)
        .map_err(|_| "Music mutation body is not valid JSON".to_string())?;
    let token_value = document
        .get("music_auth")
        .ok_or_else(|| "music_auth is missing".to_string())?;
    let token = MusicAuthToken::from_value(token_value)?;
    if token.schema_version != MUSIC_AUTH_SCHEMA_VERSION
        || token.key_id != MUSIC_AUTH_KEY_ID
        || token.verb != context.verb
        || token.node != context.node
        || token.target != context.target
    {
        return Err("music_auth scope or key id is invalid".to_string());
    }
    if token.nonce.len() > MUSIC_AUTH_MAX_NONCE_BYTES
        || token.nonce.chars().any(char::is_control)
        || token.request_sha256.len() != 64
        || !token
            .request_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || token.signature.len() != 128
        || !token.signature.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("music_auth field bounds are invalid".to_string());
    }
    if request_digest(body)? != token.request_sha256 {
        return Err("music_auth request body digest does not match".to_string());
    }
    let signature_bytes = decode_hex::<64>(&token.signature)
        .ok_or_else(|| "music_auth signature is malformed".to_string())?;
    public_key
        .verify(
            &token.signing_bytes(),
            &Signature::from_bytes(&signature_bytes),
        )
        .map_err(|_| "music_auth signature did not verify".to_string())?;
    Ok(token)
}

/// Compute the digest of a request with the auth object removed.
pub fn request_digest(body: &str) -> Result<String, String> {
    let mut document: Value = serde_json::from_str(body)
        .map_err(|_| "Music mutation body is not valid JSON".to_string())?;
    document
        .as_object_mut()
        .ok_or_else(|| "Music mutation body is not a JSON object".to_string())?
        .remove("music_auth");
    let mut canonical = String::new();
    write_canonical_json(&document, &mut canonical)?;
    Ok(hex_encode(&sha256(canonical.as_bytes())))
}

fn write_canonical_json(value: &Value, output: &mut String) -> Result<(), String> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => {
            output.push_str(&serde_json::to_string(value).map_err(|error| error.to_string())?)
        }
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut keys: Vec<&str> = values.keys().map(String::as_str).collect();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key).map_err(|error| error.to_string())?);
                output.push(':');
                write_canonical_json(&values[key], output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

fn sha256(value: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value);
    hasher.finalize().into()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut bytes = [0_u8; N];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        bytes[index] = (hex_nibble(chunk[0])? << 4) | hex_nibble(chunk[1])?;
    }
    Some(bytes)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: [u8; 32] = [7; 32];
    const CONTEXT: MusicAuthContext<'static> = MusicAuthContext {
        verb: "music-workspace",
        node: "seat-15",
        target: "workspace",
    };

    #[test]
    fn signed_request_round_trips_and_body_tamper_fails() {
        let body = r#"{"action":"play","request_id":"r1","schema_version":1}"#;
        let signed =
            sign_request(body, CONTEXT, &SEED, "nonce-12345678", 1_700_000_030_000).unwrap();
        let key = SigningKey::from_bytes(&SEED).verifying_key();
        assert_eq!(
            verify_request(&signed, CONTEXT, &key).unwrap().nonce,
            "nonce-12345678"
        );
        let tampered = signed.replace("\"play\"", "\"stop\"");
        assert!(verify_request(&tampered, CONTEXT, &key).is_err());
    }

    #[test]
    fn wrong_scope_and_wrong_key_fail_closed() {
        let body = r#"{"action":"pause","request_id":"r2","schema_version":1}"#;
        let signed =
            sign_request(body, CONTEXT, &SEED, "nonce-abcdefgh", 1_700_000_030_000).unwrap();
        let key = SigningKey::from_bytes(&[8; 32]).verifying_key();
        assert!(verify_request(&signed, CONTEXT, &key).is_err());
        let wrong_context = MusicAuthContext {
            target: "queue",
            ..CONTEXT
        };
        let key = SigningKey::from_bytes(&SEED).verifying_key();
        assert!(verify_request(&signed, wrong_context, &key).is_err());
    }

    #[test]
    fn digest_is_independent_of_json_object_order() {
        let first = r#"{"schema_version":1,"action":"play"}"#;
        let second = r#"{"action":"play","schema_version":1}"#;
        assert_eq!(
            request_digest(first).unwrap(),
            request_digest(second).unwrap()
        );
    }
}
