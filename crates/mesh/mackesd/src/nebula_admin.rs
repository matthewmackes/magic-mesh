//! NET-INTROSPECT (PD-6/PD-7) — direct-vs-relay path classification via
//! Nebula's built-in debug SSH server.
//!
//! OSS Nebula has no HTTP admin API, but it ships a debug SSH server (the
//! `sshd:` config block — confirmed in our pinned 1.10.3 binary) whose
//! `list-hostmap -json` reports, per peer: the chosen remote UDP endpoint
//! (`currentRemote`), the relay state (`relay` / `currentRelaysToMe`), and the
//! peer name (`cert`). mackesd renders a **loopback-bound** `sshd:` block plus
//! a keypair, then SSHes in to classify each tunnel — replacing the honest
//! `path:"overlay"` placeholder the `transport_probe` reports.
//!
//! **Security.** The debug SSH listens on `127.0.0.1` only — never the overlay
//! or the underlay — so it adds no firewall-reachable surface; it is key-auth
//! (Ed25519, the §3 signature lock), host + client keys generated under the
//! nebula config dir with ssh-keygen's 0600.
//!
//! **Graceful degradation.** Every failure path (no `ssh-keygen`, no `ssh`
//! client, the debug SSH down, a parse miss) yields *no* classification, so
//! callers keep the honest "overlay" label — never a guess.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Loopback port the Nebula debug SSH server listens on.
pub const SSHD_PORT: u16 = 2476;
/// The authorized user mackesd connects to the debug SSH as.
pub const SSHD_USER: &str = "mackesd";
/// Filename (under the nebula config dir) of the debug SSH server host key.
pub const HOST_KEY_FILE: &str = "sshd_host_key";
/// Filename (under the nebula config dir) of mackesd's client key.
pub const CLIENT_KEY_FILE: &str = "sshd_client_key";

/// One peer tunnel's measured underlay path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelPath {
    /// Peer name — the Nebula cert CN, which equals `store::NodeRow.name`
    /// (the hostname at enrollment), so the latency cache joins on it.
    pub name: String,
    /// The chosen remote UDP endpoint (`"ip:port"`) when the tunnel is direct.
    pub endpoint: Option<String>,
    /// The relay peer this tunnel routes through, when relayed.
    pub relay_via: Option<String>,
}

impl TunnelPath {
    /// Path class: `"relay"` when routed through a relay, `"direct"` when a
    /// remote endpoint is chosen, else `"overlay"` (unknown — still
    /// handshaking, or no hostmap entry). Never guesses.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        if self.relay_via.is_some() {
            "relay"
        } else if self.endpoint.is_some() {
            "direct"
        } else {
            "overlay"
        }
    }
}

/// Render the loopback `sshd:` debug block (pure). `host_key_path` is the
/// nebula-side server host key; `authorized_pubkey` is mackesd's client public
/// key (an OpenSSH `authorized_keys` line). Bound to `127.0.0.1` only.
#[must_use]
pub fn render_sshd_block(host_key_path: &str, authorized_pubkey: &str, port: u16) -> String {
    format!(
        "\n# NET-INTROSPECT (PD-6/PD-7) — loopback-only debug SSH for direct/relay\n\
         # tunnel classification. 127.0.0.1 only: no overlay/underlay exposure.\n\
         sshd:\n\
         \x20 enabled: true\n\
         \x20 listen: 127.0.0.1:{port}\n\
         \x20 host_key: {host_key_path}\n\
         \x20 authorized_users:\n\
         \x20   - user: {SSHD_USER}\n\
         \x20     keys:\n\
         \x20       - \"{pk}\"\n",
        pk = authorized_pubkey.trim(),
    )
}

/// Ensure the debug-SSH host key + mackesd client key exist under `config_dir`
/// (generating Ed25519 keypairs with `ssh-keygen` on first call; idempotent),
/// returning mackesd's client **public** key line for `authorized_users`.
///
/// # Errors
/// When `ssh-keygen` is absent/fails or the generated pubkey can't be read —
/// the caller treats this as "no debug SSH this run" and degrades honestly.
pub fn ensure_sshd_keys(config_dir: &Path) -> Result<String, String> {
    let host_key = config_dir.join(HOST_KEY_FILE);
    let client_key = config_dir.join(CLIENT_KEY_FILE);
    keygen_if_absent(&host_key, "nebula-sshd-host")?;
    keygen_if_absent(&client_key, "mackesd-nebula-admin")?;
    let pub_path = config_dir.join(format!("{CLIENT_KEY_FILE}.pub"));
    std::fs::read_to_string(&pub_path)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("read {}: {e}", pub_path.display()))
}

fn keygen_if_absent(key_path: &Path, comment: &str) -> Result<(), String> {
    if key_path.exists() {
        return Ok(());
    }
    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let out = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-q", "-C", comment, "-f"])
        .arg(key_path)
        .output()
        .map_err(|e| format!("ssh-keygen spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ssh-keygen {}: {}",
            key_path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Ensure keys + render the `sshd:` block for `materialize_config` to append.
/// Returns an empty string (no block) on any failure — honest degradation, the
/// node simply runs without the debug SSH and classification stays "overlay".
#[must_use]
pub fn ensure_and_render_sshd(config_dir: &Path) -> String {
    match ensure_sshd_keys(config_dir) {
        Ok(pubkey) => {
            let host_key = config_dir.join(HOST_KEY_FILE);
            render_sshd_block(&host_key.to_string_lossy(), &pubkey, SSHD_PORT)
        }
        Err(e) => {
            tracing::warn!(error = %e, "nebula_admin: debug SSH keys unavailable; relay/direct classification disabled");
            String::new()
        }
    }
}

/// Query the live tunnel paths from the local Nebula debug SSH server, keyed by
/// peer name. Empty on any failure (no client key, no `ssh`, sshd down, parse
/// miss) — callers keep the honest "overlay" label.
#[must_use]
pub fn query_tunnels(config_dir: &Path) -> HashMap<String, TunnelPath> {
    let client_key = config_dir.join(CLIENT_KEY_FILE);
    if !client_key.exists() {
        return HashMap::new();
    }
    let dest = format!("{SSHD_USER}@127.0.0.1");
    let out = Command::new("ssh")
        .args([
            "-i",
            &client_key.to_string_lossy(),
            "-p",
            &SSHD_PORT.to_string(),
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "ConnectTimeout=2",
            "-o",
            "LogLevel=ERROR",
            &dest,
            "list-hostmap -json",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => parse_hostmap(&String::from_utf8_lossy(&o.stdout)),
        _ => HashMap::new(),
    }
}

/// The default nebula config dir (matches `nebula_supervisor`'s
/// `/etc/nebula`); the latency worker has no config-dir handle of its own.
#[must_use]
pub fn query_tunnels_default() -> HashMap<String, TunnelPath> {
    query_tunnels(Path::new("/etc/nebula"))
}

/// Parse `nebula <debug-ssh> list-hostmap -json` (the 1.10.3 `ControlHostInfo`
/// array) into per-peer paths, keyed by the peer name from the cert. Pure +
/// defensive: tolerates absent/empty `currentRemote`, the cert name living at
/// either `cert.details.name` (v1) or `cert.name` (v2), and relay state in
/// either `currentRelaysToMe` or `relay`. Entries without a name are skipped.
#[must_use]
pub fn parse_hostmap(json: &str) -> HashMap<String, TunnelPath> {
    let mut out = HashMap::new();
    let Ok(serde_json::Value::Array(entries)) = serde_json::from_str::<serde_json::Value>(json)
    else {
        return out;
    };
    for e in &entries {
        let Some(name) = cert_name(e) else { continue };
        let endpoint = e
            .get("currentRemote")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let relay_via = first_relay(e);
        out.insert(
            name.clone(),
            TunnelPath {
                name,
                endpoint,
                relay_via,
            },
        );
    }
    out
}

/// The peer name from a hostmap entry's cert — `cert.details.name` (cert v1) or
/// `cert.name` (cert v2).
fn cert_name(entry: &serde_json::Value) -> Option<String> {
    let cert = entry.get("cert")?;
    cert.get("details")
        .and_then(|d| d.get("name"))
        .and_then(|n| n.as_str())
        .or_else(|| cert.get("name").and_then(|n| n.as_str()))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The relay peer this tunnel routes through, if any — `currentRelaysToMe`
/// (relays carrying our traffic to this peer) preferred, then `relay`.
fn first_relay(entry: &serde_json::Value) -> Option<String> {
    for key in ["currentRelaysToMe", "relay"] {
        if let Some(arr) = entry.get(key).and_then(|v| v.as_array()) {
            if let Some(first) = arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .find(|s| !s.is_empty())
            {
                return Some(first.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_sshd_block_is_loopback_keyauth() {
        let block = render_sshd_block(
            "/etc/nebula/sshd_host_key",
            "ssh-ed25519 AAAA... mackesd",
            2476,
        );
        assert!(block.contains("sshd:"));
        assert!(block.contains("enabled: true"));
        // NET-4: must bind loopback ONLY — never the overlay/underlay.
        assert!(block.contains("listen: 127.0.0.1:2476"));
        assert!(!block.contains("0.0.0.0"));
        assert!(block.contains("host_key: /etc/nebula/sshd_host_key"));
        assert!(block.contains("user: mackesd"));
        assert!(block.contains("ssh-ed25519 AAAA... mackesd"));
    }

    #[test]
    fn parse_hostmap_classifies_direct_and_relay() {
        // Representative nebula 1.10.3 `list-hostmap -json` shape.
        let json = r#"[
          {"vpnAddrs":["10.42.0.5"],"localIndex":111,"remoteIndex":222,
           "remoteAddrs":["203.0.113.5:4242"],"currentRemote":"203.0.113.5:4242",
           "cert":{"details":{"name":"anvil"}},"relay":[]},
          {"vpnAddrs":["10.42.0.6"],"localIndex":333,"remoteIndex":444,
           "remoteAddrs":[],"currentRemote":"",
           "cert":{"details":{"name":"forge"}},"currentRelaysToMe":["10.42.0.1"]}
        ]"#;
        let m = parse_hostmap(json);
        assert_eq!(m.len(), 2);

        let anvil = &m["anvil"];
        assert_eq!(anvil.endpoint.as_deref(), Some("203.0.113.5:4242"));
        assert_eq!(anvil.relay_via, None);
        assert_eq!(anvil.kind(), "direct");

        let forge = &m["forge"];
        assert_eq!(forge.endpoint, None);
        assert_eq!(forge.relay_via.as_deref(), Some("10.42.0.1"));
        assert_eq!(forge.kind(), "relay");
    }

    #[test]
    fn parse_hostmap_handles_cert_v2_name_and_skips_nameless() {
        let json = r#"[
          {"currentRemote":"198.51.100.9:4242","cert":{"name":"oak"}},
          {"currentRemote":"198.51.100.10:4242","cert":{"details":{}}}
        ]"#;
        let m = parse_hostmap(json);
        assert_eq!(m.len(), 1, "the nameless entry is skipped");
        assert_eq!(m["oak"].kind(), "direct");
    }

    #[test]
    fn parse_hostmap_unknown_when_no_remote_no_relay() {
        let json = r#"[{"currentRemote":"","cert":{"details":{"name":"pine"}}}]"#;
        let m = parse_hostmap(json);
        assert_eq!(m["pine"].kind(), "overlay");
    }

    #[test]
    fn parse_hostmap_rejects_garbage() {
        assert!(parse_hostmap("not json").is_empty());
        assert!(parse_hostmap("{}").is_empty());
    }

    #[test]
    fn tunnel_path_kind_precedence() {
        // Relay wins over a stale endpoint reading.
        let both = TunnelPath {
            name: "x".into(),
            endpoint: Some("1.2.3.4:4242".into()),
            relay_via: Some("10.42.0.1".into()),
        };
        assert_eq!(both.kind(), "relay");
    }
}
