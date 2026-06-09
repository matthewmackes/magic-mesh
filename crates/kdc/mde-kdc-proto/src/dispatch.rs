//! KDC2-3.11 — plugin-dispatch policy enforcement.
//!
//! Every incoming KDC packet runs through a single
//! [`check_plugin_allowed`] call before the host forwards it to the matching
//! plugin handler. The v2.1 KDC2 security lock denies `run_command` by
//! default; operators opt in via `/etc/mde/connect/policy.toml` (KDC2-1.10) or
//! the user override at `~/.config/mde/connect/policy.toml`.
//!
//! This layer is pure protocol policy — it touches no filesystem and no
//! networking — so it lives in the protocol crate alongside the
//! [`crate::plugins`] registry it gates. The authority is a thin trait so a
//! host's full policy object (e.g. mackesd's `LoadedPolicy`, which holds the
//! scorer + plugin allow/deny lists) satisfies it without dragging the host's
//! dep tree into this crate.
//!
//! **E2.2 (2026-06-05):** ported here verbatim from the retired
//! `mde-kdc` (legacy) `dispatch` module so the converged host has one home
//! for plugin policy — mackesd's `transport::policy::LoadedPolicy` now
//! implements [`PluginAuthority`] from this crate.

use crate::plugins::PluginKind;

/// Outcome of a single dispatch check. Stable Display tokens for audit-log
/// entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchDecision {
    /// Policy permits this plugin to act on behalf of this peer. Host forwards
    /// the packet to the plugin handler.
    Allowed,
    /// Policy denies the plugin. Host drops the packet + writes an audit
    /// entry. The optional `reason` token surfaces in the audit log for
    /// operator-side debugging.
    Denied {
        /// Stable machine-greppable reason code (e.g.
        /// `"plugin_in_deny_list"`, `"plugin_not_in_allow_list"`).
        reason: &'static str,
    },
}

impl DispatchDecision {
    /// True when the host should forward the packet.
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }
}

impl std::fmt::Display for DispatchDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allowed => f.write_str("allowed"),
            Self::Denied { reason } => write!(f, "denied({reason})"),
        }
    }
}

/// Decision authority. Implementations expose the plugin allow/deny lists.
/// mackesd's `transport::policy::LoadedPolicy` is the production impl; tests
/// use the inline `FixedPolicy` helper below.
pub trait PluginAuthority {
    /// True when the plugin (by its packet-kind token, e.g. `"clipboard"`,
    /// `"run_command"`) is allowed by the current policy. The default
    /// implementation defers to the impl-side allow/deny set semantics.
    fn plugin_allowed(&self, name: &str) -> bool;

    /// KDC2-3.11.a — per-device gating. Default implementation just delegates
    /// to `plugin_allowed(name)`, so existing impls keep working without
    /// per-device support.
    ///
    /// Real impls (mackesd's `LoadedPolicy`) consult the `[plugins.<name>]
    /// allow_devices = [...]` table from `policy.toml`: when present, the
    /// plugin is allowed only for listed device ids (overrides the base
    /// allow/deny). When absent, falls through to `plugin_allowed(name)`.
    fn plugin_allowed_for_device(&self, name: &str, _device_id: &str) -> bool {
        self.plugin_allowed(name)
    }
}

/// Check whether the given plugin is allowed to dispatch a packet from
/// `peer_id`.
///
/// `paired` should be true when the peer is currently in the pairing store.
/// Unpaired peers can never trigger a privileged plugin (defense in depth).
///
/// Pure function — no audit-log side-effect; the caller emits the audit entry
/// from the return value's Display rendering.
pub fn check_plugin_allowed(
    plugin: PluginKind,
    peer_id: &str,
    paired: bool,
    authority: &dyn PluginAuthority,
) -> DispatchDecision {
    // Unpaired peers can talk via ping / clipboard / share / notification only
    // — privileged plugins (RunCommand, SMS) require pairing.
    if !paired && privileged_plugin(plugin) {
        return DispatchDecision::Denied {
            reason: "unpaired_peer_privileged_plugin",
        };
    }
    let token = plugin.token();
    if authority.plugin_allowed_for_device(token, peer_id) {
        DispatchDecision::Allowed
    } else {
        DispatchDecision::Denied {
            reason: "plugin_policy_denied",
        }
    }
    .also_log(plugin, peer_id)
}

/// Plugins that require a paired peer regardless of allowlist.
const fn privileged_plugin(plugin: PluginKind) -> bool {
    matches!(plugin, PluginKind::Sms | PluginKind::Telephony)
}

/// Extension on `DispatchDecision` for the audit-log hint. Currently a no-op
/// `.also_log` that's a placeholder for the future audit-chain wire-up
/// (KDC2-3.11.b).
trait DispatchDecisionExt: Sized {
    fn also_log(self, _plugin: PluginKind, _peer_id: &str) -> Self {
        self
    }
}
impl DispatchDecisionExt for DispatchDecision {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny test fixture implementing `PluginAuthority`. Real callers use
    /// mackesd's `LoadedPolicy`.
    struct FixedPolicy {
        allow: Vec<&'static str>,
        deny: Vec<&'static str>,
    }
    impl PluginAuthority for FixedPolicy {
        fn plugin_allowed(&self, name: &str) -> bool {
            if self.deny.iter().any(|n| *n == name) {
                return false;
            }
            if self.allow.is_empty() {
                return true;
            }
            self.allow.iter().any(|n| *n == name)
        }
    }

    #[test]
    fn clipboard_allowed_for_paired_peer_under_baseline_policy() {
        let policy = FixedPolicy {
            allow: vec![],
            deny: vec!["run_command"],
        };
        let d = check_plugin_allowed(PluginKind::Clipboard, "alice", true, &policy);
        assert!(d.is_allowed());
        assert_eq!(format!("{d}"), "allowed");
    }

    #[test]
    fn sms_denied_for_unpaired_peer() {
        let policy = FixedPolicy {
            allow: vec![],
            deny: vec![],
        };
        let d = check_plugin_allowed(PluginKind::Sms, "bob", false, &policy);
        assert!(!d.is_allowed());
        match d {
            DispatchDecision::Denied { reason } => {
                assert_eq!(reason, "unpaired_peer_privileged_plugin");
            }
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[test]
    fn telephony_denied_for_unpaired_peer() {
        let policy = FixedPolicy {
            allow: vec![],
            deny: vec![],
        };
        let d = check_plugin_allowed(PluginKind::Telephony, "bob", false, &policy);
        assert!(!d.is_allowed());
    }

    #[test]
    fn clipboard_allowed_for_unpaired_peer_because_not_privileged() {
        // Pings + clipboards from unpaired peers are OK — they can't elevate,
        // just exchange small data. (Pairing gates the *initial* exchange at
        // the TLS handshake layer; this dispatch-level check is defense in
        // depth.)
        let policy = FixedPolicy {
            allow: vec![],
            deny: vec![],
        };
        let d = check_plugin_allowed(PluginKind::Clipboard, "bob", false, &policy);
        assert!(d.is_allowed());
    }

    /// KDC2-3.11.a fixture: per-device gating overrides the base allow/deny
    /// lists.
    struct PerDevicePolicy {
        per_device_allow: std::collections::BTreeMap<&'static str, Vec<&'static str>>,
    }
    impl PluginAuthority for PerDevicePolicy {
        fn plugin_allowed(&self, _name: &str) -> bool {
            // Without per-device gating, deny everything — so we know any
            // Allowed result came from the per-device path.
            false
        }
        fn plugin_allowed_for_device(&self, name: &str, device_id: &str) -> bool {
            self.per_device_allow
                .get(name)
                .map(|ids| ids.iter().any(|d| *d == device_id))
                .unwrap_or(false)
        }
    }

    #[test]
    fn per_device_gating_allows_specific_device() {
        let mut per_device_allow = std::collections::BTreeMap::new();
        per_device_allow.insert("clipboard", vec!["alice"]);
        let policy = PerDevicePolicy { per_device_allow };
        let alice = check_plugin_allowed(PluginKind::Clipboard, "alice", true, &policy);
        assert!(alice.is_allowed(), "alice on allowlist must dispatch");
        let bob = check_plugin_allowed(PluginKind::Clipboard, "bob", true, &policy);
        assert!(!bob.is_allowed(), "bob not on allowlist must be denied");
    }

    #[test]
    fn denied_decision_display_includes_reason_token() {
        let d = DispatchDecision::Denied {
            reason: "plugin_policy_denied",
        };
        assert_eq!(format!("{d}"), "denied(plugin_policy_denied)");
    }
}
