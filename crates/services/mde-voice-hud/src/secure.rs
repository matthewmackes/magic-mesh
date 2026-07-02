//! VOIP-GW-4 — secure SIP transport selection, re-REGISTER backoff, and the
//! per-node registration state published for the mackesd `voice_provision`
//! worker (VOIP-GW-3) to mirror to `state/voice/<node>`.
//!
//! This module is the **pure** half of the secure-register work: which
//! transport a policy resolves to (given a live TLS probe result), how long to
//! back off before the next re-REGISTER after a drop, and the JSON shape of the
//! published reg-state. The wire side (the real TLS REGISTER handshake, the
//! agent loop) lives in [`crate::sip`] where the digest/REGISTER core already
//! is — this half is toolkit- and socket-free so it is exhaustively
//! unit-tested. §7: nothing here fakes a secure session; it only *decides* and
//! *reports* — an unreachable TLS endpoint yields a downgrade or an honest
//! `Error`, never a pretend `Tls`.

use std::time::Duration;

/// Which wire transport actually carries this node's SIP signaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SipTransport {
    /// SIP over TLS (SIPS), RFC 3261 §26 — the confidential path (port 5061).
    Tls,
    /// SIP over plaintext UDP (port 5060) — the interop fallback.
    Udp,
}

impl SipTransport {
    /// The wire token used in the published state + `Via` transport.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Tls => "TLS",
            Self::Udp => "UDP",
        }
    }

    /// The IANA default SIP port for this transport (5061 TLS / 5060 UDP).
    #[must_use]
    pub const fn default_port(self) -> u16 {
        match self {
            Self::Tls => 5061,
            Self::Udp => 5060,
        }
    }
}

/// The confidentiality policy for a node's REGISTER leg (lock 17). Parsed from
/// the inbound sub-account's `transport = …` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecurityPolicy {
    /// Attempt TLS(5061); **honestly** fall back to UDP(5060) if the secure
    /// endpoint can't be reached — surfacing the downgrade. The default.
    #[default]
    PreferTls,
    /// TLS only — never fall back; an unreachable secure endpoint is an honest
    /// `Error`, not a silent plaintext register.
    RequireTls,
    /// UDP only — a provider/endpoint without TLS support (no downgrade to
    /// surface: plaintext is the configured intent).
    UdpOnly,
}

impl SecurityPolicy {
    /// Parse the `transport` config token. Unknown/blank → the secure default
    /// (`PreferTls`), so a mis-typed value fails safe toward confidentiality.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "udp" | "udp-only" | "plain" | "plaintext" => Self::UdpOnly,
            "require-tls" | "tls-only" | "strict" => Self::RequireTls,
            // "tls" / "auto" / "prefer-tls" / "" / anything else → prefer TLS.
            _ => Self::PreferTls,
        }
    }

    /// The config token this policy round-trips to.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::PreferTls => "prefer-tls",
            Self::RequireTls => "require-tls",
            Self::UdpOnly => "udp",
        }
    }
}

/// The resolved transport for a REGISTER attempt: the chosen wire, whether we
/// downgraded away from a secure preference (so the panel shows it honestly),
/// and whether SRTP media is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportChoice {
    /// The transport actually in use.
    pub transport: SipTransport,
    /// `true` when the node WANTED TLS but fell back to UDP — an honest
    /// downgrade the panel must surface (never silently swallowed).
    pub downgraded: bool,
    /// Whether SRTP protects the media stream. Honestly `false` today: media
    /// (`crate::media`) is still plain RTP; encrypted media is a follow-on
    /// unit. Reported (not assumed) so the panel never claims a green padlock
    /// the audio path doesn't earn.
    pub srtp: bool,
}

/// Decide the actual transport from the policy + whether a live TLS connection
/// could be established (the probe result is injected — this stays pure).
///
/// - `UdpOnly` → UDP, no downgrade (plaintext was the intent).
/// - `RequireTls` + reachable → TLS; unreachable → an honest `Err`.
/// - `PreferTls` + reachable → TLS; unreachable → UDP with `downgraded = true`.
///
/// # Errors
/// Returns `Err(reason)` only for `RequireTls` when no secure endpoint is
/// reachable — the caller publishes it as the reg-state `Error` reason.
pub fn select_transport(
    policy: SecurityPolicy,
    tls_available: bool,
) -> Result<TransportChoice, String> {
    match (policy, tls_available) {
        // UDP-only: plaintext was the configured intent — never a downgrade.
        (SecurityPolicy::UdpOnly, _) => Ok(TransportChoice {
            transport: SipTransport::Udp,
            downgraded: false,
            srtp: false,
        }),
        // Prefer/Require TLS with a reachable secure endpoint → TLS.
        (_, true) => Ok(TransportChoice {
            transport: SipTransport::Tls,
            downgraded: false,
            srtp: false,
        }),
        // Require TLS but none reachable → honest error, no plaintext fallback.
        (SecurityPolicy::RequireTls, false) => {
            Err("TLS required but no secure SIP endpoint reachable on 5061".to_string())
        }
        // Prefer TLS but none reachable → honest downgrade to UDP.
        (SecurityPolicy::PreferTls, false) => Ok(TransportChoice {
            transport: SipTransport::Udp,
            downgraded: true,
            srtp: false,
        }),
    }
}

/// Exponential backoff for the next re-REGISTER after a failed attempt (lock
/// 19: "nodes auto-re-REGISTER on drop with backoff").
///
/// `consecutive_failures` is the number of attempts that have failed **in a
/// row** (0 = the first retry after a healthy registration dropped): the delay
/// is `base * 2^failures`, clamped to `cap`. A success resets the caller's
/// counter to 0, after which the normal (non-backoff) refresh period applies.
#[must_use]
pub fn reregister_backoff(consecutive_failures: u32, base: Duration, cap: Duration) -> Duration {
    // Cap the shift so `1 << shift` can't overflow, then saturate the product.
    let shift = consecutive_failures.min(16);
    let base_secs = base.as_secs().max(1);
    let secs = base_secs.saturating_mul(1_u64 << shift);
    Duration::from_secs(secs.min(cap.as_secs().max(1)))
}

/// The published registration phase (lock 9 shape) — what a subscriber
/// (VOIP-GW-3, the Fleet-tab board) sees for a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegPhase {
    /// The registrar returned 200 OK — inbound reachable.
    Registered,
    /// No account, or the registration lapsed and is not being re-attempted.
    Unregistered,
    /// A REGISTER (or provisioning step) is in flight.
    Provisioning,
    /// The attempt failed — carries the real reason (never a fake online).
    Error,
}

impl RegPhase {
    /// The state string in the published JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "Registered",
            Self::Unregistered => "Unregistered",
            Self::Provisioning => "Provisioning",
            Self::Error => "Error",
        }
    }
}

/// This node's name — the `<node>` in `state/voice/<node>`.
///
/// The kernel hostname (matching [`crate::roster`] + `local_identity`),
/// lowercased; also the local part of its `<hostname>@<realm>` inbound address.
#[must_use]
pub fn node_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "mde".to_string())
}

/// The Bus topic a node publishes its own reg-state to, for VOIP-GW-3 to
/// mirror. `<node>` is [`node_name`].
#[must_use]
pub fn node_reg_topic(node: &str) -> String {
    format!("state/voice/{node}")
}

/// Build the per-node reg-state JSON body (lock 9).
///
/// Pure so it is unit-tested without a Bus. `choice` is `None` while
/// unregistered/provisioning (no transport resolved yet); `reason` is empty
/// except for `Error`.
#[must_use]
pub fn node_reg_state_json(
    node: &str,
    phase: RegPhase,
    reason: &str,
    choice: Option<TransportChoice>,
    server: &str,
    caller_id: &str,
    ts: u64,
) -> String {
    let (transport, downgraded, srtp) = choice.map_or(("", false, false), |c| {
        (c.transport.token(), c.downgraded, c.srtp)
    });
    serde_json::json!({
        "node": node,
        "state": phase.as_str(),
        "reason": reason,
        "transport": transport,
        "downgraded": downgraded,
        "srtp": srtp,
        "server": server,
        "caller_id": caller_id,
        "ts": ts,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_parses_leniently_and_fails_safe() {
        assert_eq!(SecurityPolicy::parse("udp"), SecurityPolicy::UdpOnly);
        assert_eq!(SecurityPolicy::parse("Plaintext"), SecurityPolicy::UdpOnly);
        assert_eq!(
            SecurityPolicy::parse("require-tls"),
            SecurityPolicy::RequireTls
        );
        assert_eq!(
            SecurityPolicy::parse("tls-only"),
            SecurityPolicy::RequireTls
        );
        assert_eq!(SecurityPolicy::parse("tls"), SecurityPolicy::PreferTls);
        assert_eq!(SecurityPolicy::parse(""), SecurityPolicy::PreferTls);
        // An unknown token fails safe toward confidentiality, not plaintext.
        assert_eq!(SecurityPolicy::parse("wat"), SecurityPolicy::PreferTls);
        assert_eq!(SecurityPolicy::default(), SecurityPolicy::PreferTls);
    }

    #[test]
    fn prefer_tls_takes_tls_when_available_no_downgrade() {
        let c = select_transport(SecurityPolicy::PreferTls, true).unwrap();
        assert_eq!(c.transport, SipTransport::Tls);
        assert!(!c.downgraded);
    }

    #[test]
    fn prefer_tls_downgrades_honestly_when_unavailable() {
        let c = select_transport(SecurityPolicy::PreferTls, false).unwrap();
        assert_eq!(c.transport, SipTransport::Udp);
        assert!(c.downgraded, "a fallback must be surfaced, not silent");
    }

    #[test]
    fn require_tls_errors_rather_than_downgrade() {
        assert!(select_transport(SecurityPolicy::RequireTls, false).is_err());
        let c = select_transport(SecurityPolicy::RequireTls, true).unwrap();
        assert_eq!(c.transport, SipTransport::Tls);
        assert!(!c.downgraded);
    }

    #[test]
    fn udp_only_never_downgrades() {
        // No TLS was wanted, so falling to UDP is not a "downgrade".
        let c = select_transport(SecurityPolicy::UdpOnly, false).unwrap();
        assert_eq!(c.transport, SipTransport::Udp);
        assert!(!c.downgraded);
        // Even if TLS were available, UdpOnly stays UDP.
        let c2 = select_transport(SecurityPolicy::UdpOnly, true).unwrap();
        assert_eq!(c2.transport, SipTransport::Udp);
    }

    #[test]
    fn srtp_is_honestly_false_until_media_is_encrypted() {
        // Never claim SRTP the plain-RTP media path doesn't deliver.
        for avail in [true, false] {
            if let Ok(c) = select_transport(SecurityPolicy::PreferTls, avail) {
                assert!(!c.srtp);
            }
        }
    }

    #[test]
    fn backoff_grows_exponentially_and_caps() {
        let base = Duration::from_secs(2);
        let cap = Duration::from_secs(300);
        assert_eq!(reregister_backoff(0, base, cap), Duration::from_secs(2));
        assert_eq!(reregister_backoff(1, base, cap), Duration::from_secs(4));
        assert_eq!(reregister_backoff(2, base, cap), Duration::from_secs(8));
        assert_eq!(reregister_backoff(3, base, cap), Duration::from_secs(16));
        // Deep into the failure streak it clamps at the cap, never overflows.
        assert_eq!(reregister_backoff(20, base, cap), Duration::from_secs(300));
        assert_eq!(
            reregister_backoff(u32::MAX, base, cap),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn reg_state_json_carries_the_lock9_shape_with_transport() {
        let choice = TransportChoice {
            transport: SipTransport::Udp,
            downgraded: true,
            srtp: false,
        };
        let body = node_reg_state_json(
            "eagle",
            RegPhase::Registered,
            "",
            Some(choice),
            "sip.vitelity.net:5060",
            "+15551230000",
            1_700_000_000,
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["node"], "eagle");
        assert_eq!(v["state"], "Registered");
        assert_eq!(v["transport"], "UDP");
        assert_eq!(v["downgraded"], true, "downgrade surfaced, not hidden");
        assert_eq!(v["srtp"], false);
        assert_eq!(v["caller_id"], "+15551230000");
        assert_eq!(v["ts"], 1_700_000_000_u64);
    }

    #[test]
    fn reg_state_json_error_carries_reason_and_no_transport() {
        let body = node_reg_state_json(
            "pine",
            RegPhase::Error,
            "TLS required but no secure SIP endpoint reachable on 5061",
            None,
            "",
            "",
            42,
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["state"], "Error");
        assert!(v["reason"].as_str().unwrap().contains("TLS required"));
        assert_eq!(v["transport"], "");
    }

    #[test]
    fn reg_topic_is_per_node() {
        assert_eq!(node_reg_topic("eagle"), "state/voice/eagle");
    }
}
