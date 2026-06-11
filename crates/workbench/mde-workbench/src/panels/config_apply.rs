//! PLANES-7 — the **Config-apply** panel (Controller plane).
//!
//! Shows this node's *applied* fleet revision against the *newest* in the
//! replicated revision log, with a **Reconcile-now** action (W22), plus
//! the installed RPM version and the repo it came from (W28). Reads the
//! `magic_fleet::store` log directly (`<root>/fleet/revisions/<v>.yaml` +
//! `fleet/acks/<v>/<host>.json`) — the established panel pattern;
//! Reconcile-now shells `mackesd reconcile`.
//!
//! Build-now-defer-visual: the newest/applied projection is pure +
//! unit-tested; the on-Cosmic `/preview`, the last-Ansible-log tail, and
//! update-now-via-typed-job (W28's action half) are the deferred tail.

use std::path::{Path, PathBuf};

use iced::widget::{column, row, text};
use iced::{Element, Length, Task};
use serde::Deserialize;

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::{panel_container, status_badge, BadgeSeverity};
use crate::panels::fleet_settings::run_mackesd;

/// Minimal revision projection (the store's Revision, spec ignored).
#[derive(Debug, Clone, Deserialize)]
struct RevisionHead {
    version: u64,
    #[serde(default)]
    author: String,
    #[serde(default)]
    at: u64,
}

/// Minimal ack projection.
#[derive(Debug, Clone, Deserialize)]
struct AckRow {
    #[serde(default)]
    status: String,
}

/// `MDE_WORKGROUP_ROOT`-or-`/mnt/mesh-storage` (matches network_hosts/jobs).
#[must_use]
pub fn workgroup_root() -> PathBuf {
    std::env::var_os("MDE_WORKGROUP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/mesh-storage"))
}

/// The newest revision in the log: `(version, author, at)`, or `None`
/// when the log is empty/absent.
#[must_use]
pub fn newest_revision(root: &Path) -> Option<(u64, String, u64)> {
    let dir = root.join("fleet").join("revisions");
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|raw| serde_yaml::from_str::<RevisionHead>(&raw).ok())
        .max_by_key(|r| r.version)
        .map(|r| (r.version, r.author, r.at))
}

/// The highest revision `host` has an `applied` ack for, or `None`.
#[must_use]
pub fn applied_version(root: &Path, host: &str) -> Option<u64> {
    let acks_root = root.join("fleet").join("acks");
    let entries = std::fs::read_dir(acks_root).ok()?;
    entries
        .filter_map(std::result::Result::ok)
        .filter_map(|e| {
            let version: u64 = e.file_name().to_str()?.parse().ok()?;
            let ack_path = e.path().join(format!("{host}.json"));
            let raw = std::fs::read_to_string(ack_path).ok()?;
            let ack: AckRow = serde_json::from_str(&raw).ok()?;
            (ack.status == "applied").then_some(version)
        })
        .max()
}

/// The loaded config state.
#[derive(Debug, Clone, Default)]
pub struct ConfigState {
    pub newest: Option<u64>,
    pub newest_author: String,
    pub applied: Option<u64>,
    pub rpm_version: String,
    /// W28 — the dnf repo the installed RPM came from.
    pub repo_source: String,
    pub hostname: String,
}

impl ConfigState {
    /// Up-to-date iff applied == newest (and a newest exists).
    #[must_use]
    pub fn up_to_date(&self) -> bool {
        matches!((self.applied, self.newest), (Some(a), Some(n)) if a == n)
    }
}

/// The Config-apply panel.
#[derive(Debug, Clone, Default)]
pub struct ConfigApplyPanel {
    pub state: ConfigState,
    pub loaded: bool,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(ConfigState),
    ReconcileClicked,
    Reconciled(String),
    RefreshClicked,
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

fn rpm_version() -> String {
    std::process::Command::new("rpm")
        .args(["-q", "magic-mesh"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "not installed via RPM".into())
}

/// W28 — parse `dnf repoquery --installed --qf '%{from_repo}'` output
/// into the repo the package was installed from. dnf reports `@System`
/// or `<unknown>` (or nothing) when it can't attribute one — a local
/// `rpm -i` or an in-tree build — which we surface honestly rather than
/// inventing a source.
fn parse_from_repo(out: &str) -> Option<String> {
    let line = out.lines().map(str::trim).find(|l| !l.is_empty())?;
    match line {
        "@System" | "<unknown>" => None,
        repo => Some(repo.to_string()),
    }
}

/// W28 — which dnf repo served the installed `magic-mesh` RPM (the
/// PLANES-24 `file://` self-mirror, an upstream GitHub-Pages repo, or
/// honestly "unknown" when it wasn't installed from a configured repo).
fn repo_source() -> String {
    std::process::Command::new("dnf")
        .args([
            "repoquery",
            "--installed",
            "--qf",
            "%{from_repo}",
            "magic-mesh",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .as_deref()
        .and_then(parse_from_repo)
        .unwrap_or_else(|| "unknown (not from a configured repo)".into())
}

impl ConfigApplyPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let root = workgroup_root();
                let host = hostname();
                let (newest, newest_author) =
                    newest_revision(&root).map_or((None, String::new()), |(v, a, _)| (Some(v), a));
                Message::Loaded(ConfigState {
                    newest,
                    newest_author,
                    applied: applied_version(&root, &host),
                    rpm_version: rpm_version(),
                    repo_source: repo_source(),
                    hostname: host,
                })
            },
            crate::Message::ConfigApply,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(state) => {
                self.state = state;
                self.loaded = true;
                self.busy = false;
                self.status.clear();
                Task::none()
            }
            Message::ReconcileClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Reconciling to the elected baseline…".into();
                Task::perform(
                    async move {
                        match run_mackesd(&["reconcile".into()]).await {
                            Ok(_) => Message::Reconciled("reconcile complete".into()),
                            Err(e) => Message::Reconciled(format!("reconcile failed: {e}")),
                        }
                    },
                    crate::Message::ConfigApply,
                )
            }
            Message::Reconciled(msg) => {
                self.status = msg;
                self.busy = false;
                // Reload to reflect the new applied version.
                Self::load()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let s = &self.state;

        let fmt_v = |v: Option<u64>| v.map_or_else(|| "—".to_string(), |n| format!("rev {n}"));
        let (badge_label, severity) = if s.newest.is_none() {
            ("no revisions", BadgeSeverity::Neutral)
        } else if s.up_to_date() {
            ("up to date", BadgeSeverity::Success)
        } else {
            ("behind — reconcile", BadgeSeverity::Warning)
        };

        let reconcile = variant_button(
            "Reconcile now",
            ButtonVariant::Secondary,
            (!self.busy).then(|| crate::Message::ConfigApply(Message::ReconcileClicked)),
            palette,
        );
        let refresh = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::ConfigApply(Message::RefreshClicked)),
            palette,
        );

        let rows = column![
            text("Fleet configuration").size(20),
            row![
                text("Newest revision:").size(14),
                text(fmt_v(s.newest)).size(14)
            ]
            .spacing(8),
            row![
                text("Applied here:").size(14),
                text(fmt_v(s.applied)).size(14)
            ]
            .spacing(8),
            status_badge(badge_label, severity, palette),
            row![
                text("Author:").size(13),
                text(s.newest_author.clone()).size(13)
            ]
            .spacing(8),
            row![text("RPM:").size(13), text(s.rpm_version.clone()).size(13)].spacing(8),
            row![
                text("Repo source:").size(13),
                text(s.repo_source.clone()).size(13)
            ]
            .spacing(8),
            row![reconcile, refresh].spacing(12),
            text(self.status.clone()).size(13),
        ]
        .spacing(10);

        panel_container(rows.width(Length::Fill).into(), density)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_rev(root: &Path, version: u64, author: &str) {
        let dir = root.join("fleet").join("revisions");
        std::fs::create_dir_all(&dir).unwrap();
        let yaml = format!("version: {version}\nauthor: {author}\nat: 100\nspec: {{}}\n");
        std::fs::write(dir.join(format!("{version:020}.yaml")), yaml).unwrap();
    }

    fn write_ack(root: &Path, version: u64, host: &str, status: &str) {
        let dir = root
            .join("fleet")
            .join("acks")
            .join(format!("{version:020}"));
        std::fs::create_dir_all(&dir).unwrap();
        let json = format!(r#"{{"peer":"{host}","status":"{status}","at":1}}"#);
        std::fs::write(dir.join(format!("{host}.json")), json).unwrap();
    }

    #[test]
    fn newest_revision_picks_the_max_version() {
        let tmp = tempfile::tempdir().unwrap();
        write_rev(tmp.path(), 1, "alice");
        write_rev(tmp.path(), 3, "bob");
        write_rev(tmp.path(), 2, "carol");
        let (v, author, _) = newest_revision(tmp.path()).unwrap();
        assert_eq!(v, 3);
        assert_eq!(author, "bob");
    }

    #[test]
    fn applied_version_is_highest_applied_ack_for_host() {
        let tmp = tempfile::tempdir().unwrap();
        write_ack(tmp.path(), 1, "pine", "applied");
        write_ack(tmp.path(), 2, "pine", "applied");
        write_ack(tmp.path(), 3, "pine", "failed"); // not counted
        write_ack(tmp.path(), 2, "oak", "applied"); // other host ignored
        assert_eq!(applied_version(tmp.path(), "pine"), Some(2));
        assert_eq!(applied_version(tmp.path(), "stranger"), None);
    }

    #[test]
    fn up_to_date_only_when_applied_equals_newest() {
        let mut s = ConfigState {
            newest: Some(3),
            applied: Some(3),
            ..Default::default()
        };
        assert!(s.up_to_date());
        s.applied = Some(2);
        assert!(!s.up_to_date());
        s.applied = None;
        assert!(!s.up_to_date());
    }

    #[test]
    fn from_repo_parse_handles_real_repos_and_unattributed_installs() {
        // A configured repo (the PLANES-24 self-mirror or upstream).
        assert_eq!(
            parse_from_repo("mackes-mirror-magic-mesh\n"),
            Some("mackes-mirror-magic-mesh".to_string())
        );
        // dnf's "no attributable repo" sentinels → honest None.
        assert_eq!(parse_from_repo("@System\n"), None);
        assert_eq!(parse_from_repo("<unknown>"), None);
        // Empty output (package absent / dnf error) → None.
        assert_eq!(parse_from_repo(""), None);
        assert_eq!(parse_from_repo("   \n"), None);
    }

    #[test]
    fn empty_log_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(newest_revision(tmp.path()).is_none());
        assert!(applied_version(tmp.path(), "pine").is_none());
    }
}
