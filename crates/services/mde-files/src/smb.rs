//! E10 — interactive SMB/Network browsing for the unified manager.
//!
//! Mirrors the shell `files.rs` Network pane: enumerate a host's Disk shares
//! with `smbclient -L <host> -N` (guest), then mount a chosen share over GVfs
//! (`gio mount smb://host/share`) so it browses through the Local browser at
//! the GVfs FUSE dir. The parser is pure + unit-tested; the browse/mount shell
//! out (best-effort, never panic) and are the SMB-server bench for the live
//! round-trip.

use std::process::Command;

/// Parse `smbclient -L` stdout into the Disk share names (skips IPC$/print$ and
/// the IPC/Printer types). Pure — unit-tested.
#[must_use]
pub fn parse_smb_shares(output: &str) -> Vec<String> {
    let mut shares = Vec::new();
    let mut in_table = false;
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("Sharename") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if t.is_empty() {
            break; // the share table ends at the first blank line
        }
        if t.starts_with("---") {
            continue;
        }
        let tokens: Vec<&str> = t.split_whitespace().collect();
        let Some(type_idx) = tokens
            .iter()
            .position(|tok| matches!(*tok, "Disk" | "IPC" | "Printer"))
        else {
            continue;
        };
        if type_idx == 0 {
            continue;
        }
        let name = tokens[..type_idx].join(" ");
        if tokens[type_idx] == "Disk" && name != "IPC$" && name != "print$" {
            shares.push(name);
        }
    }
    shares
}

/// List a host's Disk shares via `timeout <secs> smbclient -L <host> -N`.
/// Returns the share names, or a readable error. Synchronous (bounded by the
/// timeout); never panics.
pub fn smb_shares(host: &str, timeout_secs: u32) -> Result<Vec<String>, String> {
    let out = Command::new("timeout")
        .args([&timeout_secs.to_string(), "smbclient", "-L", host, "-N"])
        .output()
        .map_err(|e| format!("could not run smbclient: {e}"))?;
    let shares = parse_smb_shares(&String::from_utf8_lossy(&out.stdout));
    if !shares.is_empty() {
        return Ok(shares);
    }
    if out.status.code() == Some(124) {
        return Err(format!("Browsing '{host}' timed out."));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if out.status.code() == Some(127) || line.contains("not found") {
        return Err("smbclient is not installed.".to_string());
    }
    Err(if line.is_empty() {
        format!("No shares found on '{host}'.")
    } else {
        line.to_string()
    })
}

/// `smb://host/share` URI for a share.
#[must_use]
pub fn smb_uri(host: &str, share: &str) -> String {
    format!("smb://{host}/{share}")
}

/// Mount an SMB share over GVfs (`gio mount smb://host/share`), best-effort.
/// Returns the GVfs FUSE path the share will appear at once mounted.
pub fn mount_share(host: &str, share: &str) -> String {
    let _ = Command::new("gio")
        .args(["mount", &smb_uri(host, share)])
        .spawn();
    let gvfs = std::env::var("XDG_RUNTIME_DIR")
        .map(|r| format!("{r}/gvfs"))
        .unwrap_or_else(|_| "/run/user/gvfs".to_string());
    format!("{gvfs}/smb-share:server={host},share={share}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extracts_disk_shares_only() {
        let out = "\
\tSharename       Type      Comment
\t---------       ----      -------
\tdocs            Disk      Documents
\tmedia           Disk      Media library
\tIPC$            IPC       IPC Service
\tprint$          Printer   Printer Drivers
\tHP_LaserJet     Printer   the office printer

\tServer               Comment
";
        let shares = parse_smb_shares(out);
        assert_eq!(shares, vec!["docs".to_string(), "media".to_string()]);
    }

    #[test]
    fn parse_empty_or_garbage_yields_no_shares() {
        assert!(parse_smb_shares("").is_empty());
        assert!(parse_smb_shares("connection failed: NT_STATUS_...").is_empty());
    }

    #[test]
    fn smb_uri_and_mount_path_shapes() {
        assert_eq!(smb_uri("nas", "docs"), "smb://nas/docs");
    }
}
