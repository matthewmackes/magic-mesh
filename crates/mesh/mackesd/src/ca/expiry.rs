//! EFF-11 — CA-cert expiry probe.
//!
//! Peer certs deliberately don't carry a per-cert expiry (the sign
//! module writes `expires_at = 0`, the epoch-lifetime sentinel —
//! turnover is rotation/revocation, never a quiet expiry). The real
//! standing risk is the **CA cert** itself: when the Nebula CA cert
//! reaches its `notAfter`, every peer cert signed under it becomes
//! invalid at once, so a small (≤8-peer) mesh can silently lose every
//! tunnel at turnover with no warning.
//!
//! This probe reads the CA cert's `notAfter` via the authoritative
//! source — `nebula-cert print -json` — and reduces it to days
//! remaining. The [`crate::workers::metrics_exporter`] worker calls
//! it each tick to emit `mackesd_ca_cert_days_remaining` + a
//! threshold warning, giving operators (and a Prometheus alert rule)
//! lead time to `mackesd ca rotate` before the cliff.

/// Days-remaining at or below which the mesh is at risk: a CA-cert
/// turnover invalidates every peer cert simultaneously, so operators
/// need lead time to rotate. 30 days is a full ops cycle of warning.
pub const CERT_EXPIRY_WARN_DAYS: i64 = 30;

/// Days until the Nebula CA cert at `ca_cert_path` expires, relative
/// to `now_unix` (Unix seconds). `None` when `nebula-cert` is
/// unavailable, the cert can't be read, or `notAfter` can't be
/// parsed — callers treat that as "unknown, don't alert" rather than
/// "expired". A negative result means the cert is already past
/// `notAfter`.
#[must_use]
pub fn ca_cert_days_remaining(ca_cert_path: &std::path::Path, now_unix: i64) -> Option<i64> {
    let out = std::process::Command::new("nebula-cert")
        .args(["print", "-json", "-path"])
        .arg(ca_cert_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let not_after = parse_not_after_unix(&String::from_utf8_lossy(&out.stdout))?;
    Some((not_after - now_unix) / 86_400)
}

/// Parse the `details.notAfter` RFC-3339 timestamp out of a
/// `nebula-cert print -json` document and return it as Unix seconds.
/// Pure (no subprocess) so the parse path is unit-tested against a
/// captured document. `None` on any missing field or unparseable
/// timestamp.
#[must_use]
pub fn parse_not_after_unix(raw_json: &str) -> Option<i64> {
    let v: serde_json::Value = serde_json::from_str(raw_json.trim()).ok()?;
    // Nebula Cert V1 prints a single object `{"details":…}`; V2 prints a JSON
    // array of certs `[{"details":…}]`. Accept both (same shape change that
    // broke fingerprint parsing — found live 2026-07-01: without this the
    // self-test cert probe showed "expiry not probed" on the V2 fleet).
    let cert = if v.is_array() { v.get(0)? } else { &v };
    let not_after = cert.get("details")?.get("notAfter")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(not_after)
        .ok()
        .map(|dt| dt.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A representative `nebula-cert print -json` document. Only the
    // fields the probe reads are asserted; the rest mirror the real
    // shape so the selector path is exercised end-to-end.
    const SAMPLE: &str = r#"{
        "details": {
            "name": "magic-mesh-ca",
            "notBefore": "2026-01-01T00:00:00Z",
            "notAfter": "2027-01-01T00:00:00Z",
            "isCa": true,
            "groups": []
        },
        "fingerprint": "deadbeef"
    }"#;

    #[test]
    fn parses_not_after_to_unix_seconds() {
        // 2027-01-01T00:00:00Z == 1_798_761_600.
        assert_eq!(parse_not_after_unix(SAMPLE), Some(1_798_761_600));
    }

    #[test]
    fn parses_not_after_from_v2_array() {
        // Nebula Cert V2: `nebula-cert print -json` wraps the cert in a JSON
        // array. The same notAfter must parse (found live 2026-07-01 — the V2
        // fleet showed "expiry not probed" until parse_not_after_unix took the
        // array's first element).
        let v2 = format!("[{SAMPLE}]");
        assert_eq!(parse_not_after_unix(&v2), Some(1_798_761_600));
        assert!(parse_not_after_unix("[]").is_none()); // empty array ⇒ no cert
    }

    #[test]
    fn days_remaining_is_positive_before_expiry() {
        let not_after = parse_not_after_unix(SAMPLE).unwrap();
        // 60 days before notAfter.
        let now = not_after - 60 * 86_400;
        let days = (not_after - now) / 86_400;
        assert_eq!(days, 60);
        assert!(days > CERT_EXPIRY_WARN_DAYS);
    }

    #[test]
    fn days_remaining_goes_negative_after_expiry() {
        let not_after = parse_not_after_unix(SAMPLE).unwrap();
        let now = not_after + 5 * 86_400; // 5 days past expiry
        assert_eq!((not_after - now) / 86_400, -5);
    }

    #[test]
    fn malformed_json_yields_none() {
        assert!(parse_not_after_unix("{not json").is_none());
        assert!(parse_not_after_unix(r#"{"details":{}}"#).is_none());
        assert!(parse_not_after_unix(r#"{"details":{"notAfter":"nope"}}"#).is_none());
    }
}
