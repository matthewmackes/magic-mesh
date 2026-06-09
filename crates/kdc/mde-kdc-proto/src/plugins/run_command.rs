//! KDC2-2.19 run_command plugin — `kdeconnect.runcommand` body.
//!
//! Remote command execution. **Deny-by-default** per the v2.1
//! KDC2 security-review lock — the system policy.toml ships
//! with `[plugins].deny = ["runcommand"]`, and the dispatch
//! check (`mde_kdc::dispatch::check_plugin_allowed`) refuses
//! the packet before the body's handler runs.
//!
//! Operators who want remote command execution opt in
//! explicitly via the user policy override at
//! `~/.config/mde/connect/policy.toml`. Per-device allow lists
//! land in the KDC2-3.11.a follow-up.
//!
//! Wire body matches upstream's `kdeconnect.runcommand`:
//!
//! ```text
//! { "id": <ms>, "type": "kdeconnect.runcommand",
//!   "body": { "key": "<cmd-id>", "name": "Open browser",
//!             "command": "xdg-open https://example.com" } }
//! ```

use serde::{Deserialize, Serialize};

use crate::wire::Packet;

/// `kdeconnect.runcommand` body. All three fields camelCase on
/// the wire to match upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCommandBody {
    /// Stable identifier the sender uses to reference the
    /// configured command. Receivers MAY use this to
    /// disambiguate identical `command` strings across the
    /// device's command list.
    pub key: String,
    /// Human-readable name shown in the sender's UI.
    pub name: String,
    /// Shell command line the sender wants executed.
    /// Receivers MUST refuse to execute unless policy allows
    /// the sender — the policy gate is the only protection
    /// here.
    pub command: String,
}

/// Build a `kdeconnect.runcommand` packet. Used by tests + the
/// future operator-facing `mde-kdc run-on-peer <peer> <key>`
/// CLI.
#[must_use]
pub fn run_command_packet(id_ms: i64, key: String, name: String, command: String) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.runcommand".to_string(),
        body: serde_json::to_value(RunCommandBody { key, name, command })
            .expect("RunCommandBody is always JSON-serializable"),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::from_packet_body;

    fn sample() -> RunCommandBody {
        RunCommandBody {
            key: "open-browser".to_string(),
            name: "Open browser".to_string(),
            command: "xdg-open https://example.com".to_string(),
        }
    }

    #[test]
    fn run_command_serializes_with_camel_case_keys() {
        let p = run_command_packet(1, sample().key, sample().name, sample().command);
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains(r#""key":"open-browser""#));
        assert!(s.contains(r#""name":"Open browser""#));
        assert!(s.contains(r#""command":"xdg-open https://example.com""#));
    }

    #[test]
    fn run_command_packet_kind_matches_plugin_token() {
        let p = run_command_packet(1, "k".into(), "n".into(), "c".into());
        assert_eq!(p.kind, "kdeconnect.runcommand");
        assert_eq!(p.kind, crate::plugins::PluginKind::RunCommand.packet_kind());
    }

    #[test]
    fn run_command_body_round_trips_via_wire() {
        let body = sample();
        let p = run_command_packet(
            42,
            body.key.clone(),
            body.name.clone(),
            body.command.clone(),
        );
        let wire = serde_json::to_string(&p).unwrap();
        let decoded: Packet = serde_json::from_str(&wire).unwrap();
        let back: RunCommandBody = from_packet_body(&decoded).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn run_command_token_does_not_include_dot_subkey() {
        // Upstream's runcommand plugin uses just `runcommand`
        // (not `runcommand.run` or similar). Locking the token
        // so a future variant addition doesn't accidentally
        // change the on-wire string.
        assert_eq!(crate::plugins::PluginKind::RunCommand.token(), "runcommand");
    }
}
