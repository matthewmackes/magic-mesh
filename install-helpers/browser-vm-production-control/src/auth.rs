//! HMAC authentication for the host-to-guest controller API.

use crate::{hex_decode, hex_encode, unix_seconds};
use anyhow::{bail, ensure, Context, Result};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

type HmacSha256 = Hmac<Sha256>;

pub const MAX_CLOCK_SKEW_SECONDS: i64 = 30;
const REPLAY_RETENTION_SECONDS: i64 = 120;

#[must_use]
pub fn body_digest(body: &[u8]) -> String {
    hex_encode(&Sha256::digest(body))
}

pub fn request_signature(
    secret: &[u8; 32],
    method: &str,
    path: &str,
    timestamp: i64,
    nonce: &str,
    body: &[u8],
) -> Result<String> {
    let payload = format!(
        "{method}\n{path}\n{timestamp}\n{nonce}\n{}",
        body_digest(body)
    );
    sign(secret, payload.as_bytes())
}

pub fn verify_request_signature(
    secret: &[u8; 32],
    method: &str,
    path: &str,
    timestamp: i64,
    nonce: &str,
    body: &[u8],
    signature: &str,
) -> Result<()> {
    validate_nonce(nonce)?;
    let payload = format!(
        "{method}\n{path}\n{timestamp}\n{nonce}\n{}",
        body_digest(body)
    );
    verify(secret, payload.as_bytes(), signature)
}

pub fn response_signature(
    secret: &[u8; 32],
    request_nonce: &str,
    status: u16,
    body: &[u8],
) -> Result<String> {
    let payload = format!("response\n{request_nonce}\n{status}\n{}", body_digest(body));
    sign(secret, payload.as_bytes())
}

pub fn verify_response_signature(
    secret: &[u8; 32],
    request_nonce: &str,
    status: u16,
    body: &[u8],
    signature: &str,
) -> Result<()> {
    let payload = format!("response\n{request_nonce}\n{status}\n{}", body_digest(body));
    verify(secret, payload.as_bytes(), signature)
}

fn sign(secret: &[u8; 32], payload: &[u8]) -> Result<String> {
    let mut mac = HmacSha256::new_from_slice(secret).context("initialize HMAC")?;
    mac.update(payload);
    Ok(hex_encode(&mac.finalize().into_bytes()))
}

fn verify(secret: &[u8; 32], payload: &[u8], signature: &str) -> Result<()> {
    let bytes = hex_decode::<32>(signature).context("signature is not SHA-256 hex")?;
    let mut mac = HmacSha256::new_from_slice(secret).context("initialize HMAC")?;
    mac.update(payload);
    mac.verify_slice(&bytes)
        .context("controller authentication failed")
}

fn validate_nonce(nonce: &str) -> Result<()> {
    if nonce.len() != 64
        || !nonce
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
    {
        bail!("request nonce must be 256-bit lowercase hex");
    }
    Ok(())
}

#[derive(Default)]
pub struct ReplayCache {
    seen: BTreeMap<String, i64>,
}

impl ReplayCache {
    pub fn admit(&mut self, timestamp: i64, nonce: &str) -> Result<()> {
        let now = unix_seconds()?;
        ensure!(
            timestamp >= now - MAX_CLOCK_SKEW_SECONDS && timestamp <= now + MAX_CLOCK_SKEW_SECONDS,
            "authenticated request timestamp is outside the clock-skew window"
        );
        validate_nonce(nonce)?;
        self.seen
            .retain(|_, observed| *observed >= now - REPLAY_RETENTION_SECONDS);
        ensure!(!self.seen.contains_key(nonce), "replayed request nonce");
        self.seen.insert(nonce.to_owned(), timestamp);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{request_signature, verify_request_signature, ReplayCache};
    use crate::unix_seconds;

    #[test]
    fn request_auth_binds_method_path_time_nonce_and_body() {
        let secret = [7_u8; 32];
        let nonce = "a".repeat(64);
        let now = unix_seconds().unwrap_or_default();
        let signature = request_signature(&secret, "POST", "/v1/jobs", now, &nonce, b"body")
            .unwrap_or_default();
        assert!(verify_request_signature(
            &secret, "POST", "/v1/jobs", now, &nonce, b"body", &signature
        )
        .is_ok());
        assert!(verify_request_signature(
            &secret, "POST", "/v1/jobs", now, &nonce, b"changed", &signature
        )
        .is_err());
    }

    #[test]
    fn replay_cache_rejects_the_same_nonce() {
        let now = unix_seconds().unwrap_or_default();
        let nonce = "b".repeat(64);
        let mut cache = ReplayCache::default();
        assert!(cache.admit(now, &nonce).is_ok());
        assert!(cache.admit(now, &nonce).is_err());
    }
}
