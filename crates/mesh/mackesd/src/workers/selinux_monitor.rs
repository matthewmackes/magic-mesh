//! SELinux AVC monitor (SEC / NOTIFY-SRC). A node's notification Alert Center
//! was missing SELinux denials entirely: nothing detected AVCs or published
//! them to a bus alert lane, so they could never surface (locally or, via the
//! NOTIFY-DIST-2 mirror, mesh-wide).
//!
//! `auditd` captures AVC denials to `/var/log/audit/audit.log` (NOT the kernel
//! journal when auditd runs), so we scrape them with `ausearch --checkpoint`
//! (the canonical incremental query) and publish each *distinct* denial —
//! deduped by `comm + source-type + target-type + class + perms` within a window
//! (AVCs repeat noisily) — to the security lane `fleet/sec/selinux/<host>`. The
//! `chat` worker folds `fleet/sec*` into the Security alert timeline of the ONE
//! notification interface and federates it mesh-wide over the replicated chat log
//! (NOTIFY-CHAT); every node's Chat surface then shows it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::workers::{ShutdownToken, Worker};

/// Poll cadence — AVCs are not latency-critical; cheap `ausearch` incremental.
pub const DEFAULT_TICK: Duration = Duration::from_secs(30);
/// One alert per distinct denial signature per this window (AVCs repeat).
pub const ALERT_WINDOW_MS: u64 = 60 * 60 * 1000;
/// ausearch checkpoint file (incremental cursor across ticks + restarts).
const CHECKPOINT_PATH: &str = "/var/lib/mackesd/selinux-avc.ckpt";

/// One parsed AVC denial, reduced to the fields that matter for an alert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvcDenial {
    /// The denied process command (`comm=`).
    pub comm: String,
    /// The denied permission set (the `{ ... }` block).
    pub perms: String,
    /// Source SELinux *type* (from `scontext=user:role:TYPE:level`).
    pub scontext_type: String,
    /// Target SELinux *type* (from `tcontext=`).
    pub tcontext_type: String,
    /// Object class (`tclass=`).
    pub tclass: String,
    /// `permissive=1` — logged but not enforced (info, not a real block).
    pub permissive: bool,
}

impl AvcDenial {
    /// Stable dedup signature — ignores pid/timestamp/inode noise so a
    /// repeating denial alerts once per window.
    #[must_use]
    pub fn signature(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.comm, self.perms, self.scontext_type, self.tcontext_type, self.tclass
        )
    }

    /// Human-readable one-liner for the alert summary.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} denied {{{}}} on {} ({} → {}){}",
            self.comm,
            self.perms.trim(),
            self.tclass,
            self.scontext_type,
            self.tcontext_type,
            if self.permissive { " [permissive]" } else { "" }
        )
    }
}

/// `user:role:TYPE:level` → `TYPE` (the discriminating part of a context).
fn ctx_type(ctx: &str) -> String {
    ctx.split(':').nth(2).unwrap_or(ctx).to_string()
}

/// Parse one ausearch/journal line into an [`AvcDenial`]. `None` unless it is an
/// AVC *denial*. Works on both raw audit lines and `ausearch -i` output (both
/// carry `avc: denied { perms } ... comm=.. scontext=.. tcontext=.. tclass=..`).
#[must_use]
pub fn parse_avc_line(line: &str) -> Option<AvcDenial> {
    if !line.contains("avc:") || !line.contains("denied") {
        return None;
    }
    let perms = line
        .split_once('{')
        .and_then(|(_, r)| r.split_once('}'))
        .map(|(p, _)| p.trim().to_string())?;
    // `key=value` or `key="value"`.
    let field = |key: &str| -> Option<String> {
        line.split(&format!("{key}=")).nth(1).map(|rest| {
            let v = rest.trim_start();
            if let Some(stripped) = v.strip_prefix('"') {
                stripped.split('"').next().unwrap_or("").to_string()
            } else {
                v.split_whitespace().next().unwrap_or("").to_string()
            }
        })
    };
    Some(AvcDenial {
        comm: field("comm").unwrap_or_else(|| "?".into()),
        perms,
        scontext_type: ctx_type(&field("scontext").unwrap_or_default()),
        tcontext_type: ctx_type(&field("tcontext").unwrap_or_default()),
        tclass: field("tclass").unwrap_or_else(|| "?".into()),
        permissive: field("permissive").as_deref() == Some("1"),
    })
}

/// Worker: scrape new AVC denials and publish distinct ones to the security lane.
pub struct SelinuxMonitorWorker {
    host: String,
    tick: Duration,
    checkpoint: PathBuf,
    /// signature → last-alerted epoch ms (the per-window throttle).
    alerted: Mutex<HashMap<String, i64>>,
}

impl SelinuxMonitorWorker {
    /// New worker for `host` (the short hostname tagged into the topic + body).
    #[must_use]
    pub fn new(host: String) -> Self {
        Self {
            host,
            tick: DEFAULT_TICK,
            checkpoint: PathBuf::from(CHECKPOINT_PATH),
            alerted: Mutex::new(HashMap::new()),
        }
    }

    /// New denials since the last checkpoint (empty when `ausearch` is absent or
    /// nothing new — exit 10).
    fn read_new_avc(&self) -> Vec<AvcDenial> {
        if Command::new("ausearch").arg("--version").output().is_err() {
            return vec![];
        }
        if let Some(parent) = self.checkpoint.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let out = Command::new("ausearch")
            .args(["-m", "AVC,USER_AVC", "-i", "--checkpoint"])
            .arg(&self.checkpoint)
            .output();
        let Ok(o) = out else {
            return vec![];
        };
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(parse_avc_line)
            .collect()
    }

    fn tick_once(&self) {
        let now = now_ms();
        for d in self.read_new_avc() {
            let sig = d.signature();
            let recent = self
                .alerted
                .lock()
                .expect("alerted mutex")
                .get(&sig)
                .copied()
                .is_some_and(|t| (now - t) as u64 <= ALERT_WINDOW_MS);
            if recent {
                continue;
            }
            publish_selinux_alert(&self.host, &d);
            self.alerted.lock().expect("alerted mutex").insert(sig, now);
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Build the security-lane alert body for an AVC denial. Pure + testable; the
/// panel reads `severity`/`alert`/`summary`/`host`. Enforced denials are
/// `warning` (they block something); permissive ones are `info`.
#[must_use]
pub fn selinux_alert_body(host: &str, d: &AvcDenial) -> String {
    let severity = if d.permissive { "info" } else { "warning" };
    let q = |s: &str| s.replace('"', "'");
    format!(
        r#"{{"host":"{}","severity":"{}","alert":"SELinux denial","summary":"{}","comm":"{}","tclass":"{}","permissive":{}}}"#,
        q(host),
        severity,
        q(&d.summary()),
        q(&d.comm),
        q(&d.tclass),
        d.permissive
    )
}

fn publish_selinux_alert(host: &str, d: &AvcDenial) {
    let topic = format!("fleet/sec/selinux/{host}");
    let body = selinux_alert_body(host, d);
    let mut cmd = Command::new("mde-bus");
    cmd.args(["publish", &topic, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

#[async_trait::async_trait]
impl Worker for SelinuxMonitorWorker {
    fn name(&self) -> &'static str {
        "selinux_monitor"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.tick) => self.tick_once(),
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"type=AVC msg=audit(1718560000.123:456): avc:  denied  { read write } for  pid=1234 comm="firefox" name="foo" dev="dm-0" ino=99 scontext=unconfined_u:unconfined_r:mozilla_t:s0 tcontext=system_u:object_r:user_home_t:s0 tclass=file permissive=0"#;

    #[test]
    fn parses_a_denial() {
        let d = parse_avc_line(SAMPLE).expect("parse");
        assert_eq!(d.comm, "firefox");
        assert_eq!(d.perms, "read write");
        assert_eq!(d.scontext_type, "mozilla_t");
        assert_eq!(d.tcontext_type, "user_home_t");
        assert_eq!(d.tclass, "file");
        assert!(!d.permissive);
    }

    #[test]
    fn non_denial_lines_are_ignored() {
        assert!(parse_avc_line("kernel: usb 1-1: new high-speed USB device").is_none());
        assert!(parse_avc_line("avc:  granted  { read }").is_none());
    }

    #[test]
    fn signature_dedups_pid_noise_but_splits_on_perms() {
        let a = parse_avc_line(SAMPLE).unwrap();
        let b = parse_avc_line(&SAMPLE.replace("pid=1234", "pid=9999")).unwrap();
        assert_eq!(
            a.signature(),
            b.signature(),
            "pid must not change signature"
        );
        let c = parse_avc_line(&SAMPLE.replace("{ read write }", "{ open }")).unwrap();
        assert_ne!(
            a.signature(),
            c.signature(),
            "different perms = different alert"
        );
    }

    #[test]
    fn alert_body_is_valid_json_with_severity() {
        let d = parse_avc_line(SAMPLE).unwrap();
        let body = selinux_alert_body("UNIT-EAGLE", &d);
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(v["severity"], "warning");
        assert_eq!(v["host"], "UNIT-EAGLE");
        assert!(v["summary"].as_str().unwrap().contains("firefox"));
        // permissive denial → info
        let mut p = d.clone();
        p.permissive = true;
        let pv: serde_json::Value = serde_json::from_str(&selinux_alert_body("h", &p)).unwrap();
        assert_eq!(pv["severity"], "info");
    }
}
