//! PD-5 / Q8 — the shared connection launcher.
//!
//! One launch engine for every reach-this-peer gesture: the Peers
//! directory (PD-5) and the Remote Access panel (SVC-1) both consume
//! it — no duplicated spawn code. SSH opens cosmic-term running
//! `ssh $USER@host` (the L7 lock); RDP/VNC shell to remmina.

/// A connection protocol the launcher can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Ssh,
    Rdp,
    Vnc,
}

impl Protocol {
    #[must_use]
    pub fn scheme(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Rdp => "rdp",
            Self::Vnc => "vnc",
        }
    }
    #[must_use]
    pub fn default_port(self) -> u16 {
        match self {
            Self::Ssh => 22,
            Self::Rdp => 3389,
            Self::Vnc => 5900,
        }
    }
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Ssh => "SSH",
            Self::Rdp => "RDP",
            Self::Vnc => "VNC",
        }
    }
}

/// Launch a connection to `host`. SSH → cosmic-term + `ssh $USER@host`
/// (PEERS L7); RDP/VNC → `remmina -c <scheme>://host:port`. `true`
/// when the launcher binary spawned (the window detaches — spawn IS
/// the success signal).
pub async fn launch(host: &str, protocol: Protocol) -> bool {
    use tokio::process::Command;
    if protocol == Protocol::Ssh {
        let user = std::env::var("USER").unwrap_or_else(|_| "root".into());
        let target = format!("{user}@{host}");
        return spawn_ok(Command::new("cosmic-term").args(["--", "ssh", &target])).await;
    }
    let url = format!(
        "{}://{}:{}",
        protocol.scheme(),
        host,
        protocol.default_port()
    );
    spawn_ok(Command::new("remmina").args(["-c", &url])).await
}

async fn spawn_ok(cmd: &mut tokio::process::Command) -> bool {
    match cmd.spawn() {
        Ok(mut child) => {
            let _ = child.try_wait();
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_table_is_locked() {
        assert_eq!(Protocol::Ssh.default_port(), 22);
        assert_eq!(Protocol::Rdp.default_port(), 3389);
        assert_eq!(Protocol::Vnc.default_port(), 5900);
        assert_eq!(Protocol::Ssh.label(), "SSH");
        assert_eq!(Protocol::Vnc.scheme(), "vnc");
    }
}
