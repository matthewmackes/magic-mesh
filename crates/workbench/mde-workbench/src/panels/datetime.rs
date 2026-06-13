//! System → Date & Time panel — timedatectl wrapper.
//!
//! CB-1.9.a: replaces the v1.x
//! `mackes/workbench/system/datetime.py`. The Python panel ran
//! `timedatectl status` + `list-timezones` synchronously
//! in `__init__` (which the 11.9 reliability sweep then routed
//! through `mackes.workbench._async`); the Iced port keeps
//! the same subprocess shape but builds the asynchrony into
//! `Task::perform` directly. The worklist sketched a
//! `dev.mackes.MDE.System.DateTime` zbus surface as an
//! alternative — rejected for the same reason every other
//! CB-1.x panel rejected new mded subcommands: timedatectl
//! is the canonical Linux interface to systemd-timedated, and
//! polkit already gates the privileged actions. Adding a
//! daemon-side wrapper would only add latency.
//!
//! Set-time-manually is intentionally not exposed (matches
//! the Python panel rationale — almost always wrong on a
//! networked machine, falls through to shell access if
//! someone really needs it).

use cosmic::iced::widget::{checkbox, column, pick_list, row, text};
use cosmic::iced::{Length, Task};
use tokio::process::Command;

#[derive(Debug, Clone, Default)]
pub struct DateTimePanel {
    pub timezones: Vec<String>,
    pub timezone: String,
    pub ntp_active: bool,
    pub rtc_in_utc: bool,
    /// `timedatectl` not on PATH (e.g. inside a container that
    /// stripped systemd binaries). Drives the empty-state body.
    pub timedatectl_available: bool,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        timedatectl_available: bool,
        timezones: Vec<String>,
        timezone: String,
        ntp_active: bool,
        rtc_in_utc: bool,
    },
    Error(String),
    TimezoneSelected(String),
    NtpToggled(bool),
    Applied,
}

impl DateTimePanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let status_raw = run_timedatectl(&["status"]).await;
                let timedatectl_available = !status_raw.is_empty();
                if !timedatectl_available {
                    return Message::Loaded {
                        timedatectl_available,
                        timezones: Vec::new(),
                        timezone: String::new(),
                        ntp_active: false,
                        rtc_in_utc: false,
                    };
                }
                let parsed = parse_status(&status_raw);
                let timezones_raw = run_timedatectl(&["list-timezones"]).await;
                Message::Loaded {
                    timedatectl_available,
                    timezones: parse_timezones(&timezones_raw),
                    timezone: parsed.timezone,
                    ntp_active: parsed.ntp_active,
                    rtc_in_utc: parsed.rtc_in_utc,
                }
            },
            crate::Message::DateTime,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                timedatectl_available,
                timezones,
                timezone,
                ntp_active,
                rtc_in_utc,
            } => {
                self.timedatectl_available = timedatectl_available;
                self.timezones = timezones;
                self.timezone = if self.timezones.iter().any(|tz| *tz == timezone) {
                    timezone
                } else {
                    self.timezones.first().cloned().unwrap_or_default()
                };
                self.ntp_active = ntp_active;
                self.rtc_in_utc = rtc_in_utc;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::TimezoneSelected(tz) => {
                if self.busy {
                    return Task::none();
                }
                self.timezone = tz.clone();
                self.busy = true;
                self.status = format!("Setting timezone to {tz}…");
                Task::perform(
                    async move {
                        let _ = run_timedatectl(&["set-timezone", &tz]).await;
                        Message::Applied
                    },
                    crate::Message::DateTime,
                )
            }
            Message::NtpToggled(v) => {
                if self.busy {
                    return Task::none();
                }
                self.ntp_active = v;
                self.busy = true;
                self.status = format!("Setting NTP {}…", if v { "on" } else { "off" });
                let arg = if v { "true" } else { "false" };
                Task::perform(
                    async move {
                        let _ = run_timedatectl(&["set-ntp", arg]).await;
                        Message::Applied
                    },
                    crate::Message::DateTime,
                )
            }
            Message::Applied => {
                self.status = "Applied.".into();
                self.busy = false;
                Task::none()
            }
        }
    }

    pub fn view(&self) -> cosmic::Element<'_, crate::Message> {
        if !self.timedatectl_available {
            return column![
                text("timedatectl unavailable").size(18),
                text(
                    "MDE talks to systemd-timedated through timedatectl. \
                     Install the systemd binaries (or run on a host system) \
                     and reopen this panel.",
                )
                .size(13),
            ]
            .spacing(8)
            .width(Length::Fill)
            .into();
        }

        let tz_pick: pick_list::PickList<'_, String, _, _, crate::Message, cosmic::Theme> =
            pick_list(
                self.timezones.clone(),
                current_or_none(&self.timezones, &self.timezone),
                |v| crate::Message::DateTime(Message::TimezoneSelected(v)),
            );

        column![
            row![text("Timezone").width(Length::Fixed(180.0)), tz_pick,].spacing(12),
            checkbox(self.ntp_active)
                .label("Sync time over NTP")
                .on_toggle(|v| crate::Message::DateTime(Message::NtpToggled(v))),
            text(format!(
                "RTC mode: {}",
                if self.rtc_in_utc {
                    "UTC (recommended)"
                } else {
                    "local time"
                }
            ))
            .size(13),
            row![text(&self.status).size(13)].spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

fn current_or_none(list: &[String], value: &str) -> Option<String> {
    list.iter().find(|v| *v == value).cloned()
}

/// Parsed `timedatectl status` payload — only the fields the
/// panel surfaces.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ParsedStatus {
    pub timezone: String,
    pub ntp_active: bool,
    pub rtc_in_utc: bool,
}

/// Parse the multi-line key-value output of
/// `timedatectl status`. The exact line wording varies by
/// systemd version; the parser is forgiving — it greps on
/// keyword fragments rather than exact prefixes.
#[must_use]
pub fn parse_status(raw: &str) -> ParsedStatus {
    let mut out = ParsedStatus::default();
    for line in raw.lines() {
        let lc = line.to_ascii_lowercase();
        if let Some(idx) = line.find(':') {
            let value = line[idx + 1..].trim();
            if lc.contains("time zone") {
                // Format: "Asia/Tokyo (JST, +0900)"
                if let Some(space) = value.find(' ') {
                    out.timezone = value[..space].to_string();
                } else {
                    out.timezone = value.to_string();
                }
            } else if lc.contains("ntp service") || lc.contains("system clock synchronized") {
                let v = value.to_ascii_lowercase();
                if v.contains("active") || v.contains("yes") {
                    out.ntp_active = true;
                }
            } else if lc.contains("rtc in local tz") {
                let v = value.to_ascii_lowercase();
                out.rtc_in_utc = !(v.contains("yes") || v.contains("true"));
            }
        }
    }
    out
}

/// Parse `timedatectl list-timezones` output — one timezone
/// per line. Empty / whitespace-only lines are skipped.
#[must_use]
pub fn parse_timezones(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Shell out to `timedatectl` with the given args; empty string
/// on any error (binary missing, decode failure, non-zero exit
/// — the caller uses empty as the "unavailable" signal).
pub async fn run_timedatectl(args: &[&str]) -> String {
    let Ok(output) = Command::new("timedatectl").args(args).output().await else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8(output.stdout).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS_TYPICAL: &str = "\
               Local time: Mon 2024-05-06 12:34:56 JST
           Universal time: Mon 2024-05-06 03:34:56 UTC
                 RTC time: Mon 2024-05-06 03:34:56
                Time zone: Asia/Tokyo (JST, +0900)
System clock synchronized: yes
              NTP service: active
          RTC in local TZ: no
";

    #[test]
    fn parse_status_extracts_timezone_ntp_and_rtc_mode() {
        let parsed = parse_status(STATUS_TYPICAL);
        assert_eq!(parsed.timezone, "Asia/Tokyo");
        assert!(parsed.ntp_active);
        assert!(parsed.rtc_in_utc);
    }

    #[test]
    fn parse_status_handles_rtc_in_local_tz_yes() {
        let raw = "RTC in local TZ: yes\n";
        assert!(!parse_status(raw).rtc_in_utc);
    }

    #[test]
    fn parse_status_defaults_unknown_fields() {
        let parsed = parse_status("(nothing useful)");
        assert!(parsed.timezone.is_empty());
        assert!(!parsed.ntp_active);
        assert!(!parsed.rtc_in_utc);
    }

    #[test]
    fn parse_timezones_extracts_one_per_line() {
        let raw = "America/New_York\nEurope/London\n\n   \nAsia/Tokyo\n";
        assert_eq!(
            parse_timezones(raw),
            vec!["America/New_York", "Europe/London", "Asia/Tokyo"]
        );
    }

    #[test]
    fn parse_timezones_empty_on_empty_input() {
        assert!(parse_timezones("").is_empty());
    }

    #[test]
    fn loaded_with_unknown_timezone_falls_back_to_first_listed() {
        let mut panel = DateTimePanel::new();
        let _ = panel.update(Message::Loaded {
            timedatectl_available: true,
            timezones: vec!["A".into(), "B".into()],
            timezone: "vanished".into(),
            ntp_active: false,
            rtc_in_utc: true,
        });
        assert_eq!(panel.timezone, "A");
    }

    #[test]
    fn loaded_with_known_timezone_preserves_selection() {
        let mut panel = DateTimePanel::new();
        let _ = panel.update(Message::Loaded {
            timedatectl_available: true,
            timezones: vec!["A".into(), "B".into()],
            timezone: "B".into(),
            ntp_active: true,
            rtc_in_utc: true,
        });
        assert_eq!(panel.timezone, "B");
        assert!(panel.ntp_active);
    }

    #[test]
    fn loaded_timedatectl_unavailable_clears_state() {
        let mut panel = DateTimePanel::new();
        let _ = panel.update(Message::Loaded {
            timedatectl_available: false,
            timezones: Vec::new(),
            timezone: String::new(),
            ntp_active: false,
            rtc_in_utc: false,
        });
        assert!(!panel.timedatectl_available);
    }

    #[test]
    fn timezone_selected_while_busy_is_noop() {
        let mut panel = DateTimePanel::new();
        panel.busy = true;
        panel.timezone = "A".into();
        let _ = panel.update(Message::TimezoneSelected("B".into()));
        assert_eq!(panel.timezone, "A");
    }

    #[test]
    fn ntp_toggle_records_intent_and_sets_busy() {
        let mut panel = DateTimePanel::new();
        let _ = panel.update(Message::NtpToggled(true));
        assert!(panel.ntp_active);
        assert!(panel.busy);
        assert!(panel.status.contains("on"));
    }

    #[test]
    fn applied_clears_busy_and_records_status() {
        let mut panel = DateTimePanel::new();
        panel.busy = true;
        panel.status = "Setting…".into();
        let _ = panel.update(Message::Applied);
        assert!(!panel.busy);
        assert_eq!(panel.status, "Applied.");
    }

    #[test]
    fn error_message_clears_busy_and_stores_msg() {
        let mut panel = DateTimePanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Error("polkit denied".into()));
        assert_eq!(panel.status, "polkit denied");
        assert!(!panel.busy);
    }
}
