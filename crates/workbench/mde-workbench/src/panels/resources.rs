//! Maintain → Resources panel — lightweight CPU / RAM / disk
//! view. Three numbers, three percentages. The Python panel
//! refreshed every 1.5 s via a GLib timer; the Iced port
//! exposes a Refresh button + lets the user trigger updates
//! on demand (the workbench doesn't need continuous polling
//! for a panel the user only opens occasionally).
//!
//! CB-1.7 partial: replaces the v1.x
//! `mackes/workbench/maintain/resources.py`. Reads /proc
//! directly — no psutil-equivalent dep needed.

use iced::widget::{column, progress_bar, row, text};
use iced::{Element, Length, Task};
use mde_theme::Palette;

use crate::controls::{variant_button, ButtonVariant};

#[derive(Debug, Clone, Default)]
pub struct ResourcesPanel {
    pub cpu_percent: f32,
    pub mem_used_mib: u64,
    pub mem_total_mib: u64,
    pub disk_used_gib: u64,
    pub disk_total_gib: u64,
    pub status: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Sample),
    Error(String),
    RefreshClicked,
}

#[derive(Debug, Clone, Default)]
pub struct Sample {
    pub cpu_percent: f32,
    pub mem_used_mib: u64,
    pub mem_total_mib: u64,
    pub disk_used_gib: u64,
    pub disk_total_gib: u64,
}

impl ResourcesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                match sample_resources().await {
                    Ok(s) => Message::Loaded(s),
                    Err(e) => Message::Error(e),
                }
            },
            crate::Message::Resources,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(s) => {
                self.cpu_percent = s.cpu_percent;
                self.mem_used_mib = s.mem_used_mib;
                self.mem_total_mib = s.mem_total_mib;
                self.disk_used_gib = s.disk_used_gib;
                self.disk_total_gib = s.disk_total_gib;
                self.status = "Sampled.".into();
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                Task::none()
            }
            Message::RefreshClicked => Self::load(),
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let mem_frac = safe_fraction(self.mem_used_mib, self.mem_total_mib);
        let disk_frac = safe_fraction(self.disk_used_gib, self.disk_total_gib);
        // UX-7.a — refresh routed through the shared button variant.
        let refresh_btn = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            Some(crate::Message::Resources(Message::RefreshClicked)),
            Palette::dark(),
        );

        column![
            row![
                text("CPU").width(Length::Fixed(80.0)),
                progress_bar(0.0..=100.0, self.cpu_percent),
                text(format!("{:.1}%", self.cpu_percent)).width(Length::Fixed(80.0)),
            ]
            .spacing(12),
            row![
                text("Memory").width(Length::Fixed(80.0)),
                progress_bar(0.0..=1.0, mem_frac),
                text(format!(
                    "{} / {} MiB",
                    self.mem_used_mib, self.mem_total_mib,
                )),
            ]
            .spacing(12),
            row![
                text("Disk").width(Length::Fixed(80.0)),
                progress_bar(0.0..=1.0, disk_frac),
                text(format!(
                    "{} / {} GiB",
                    self.disk_used_gib, self.disk_total_gib,
                )),
            ]
            .spacing(12),
            row![refresh_btn, text(&self.status).size(13)].spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Compute a safe `used / total` fraction in `[0.0, 1.0]`.
/// Returns 0.0 when `total` is zero (matches the v1.x panel's
/// guard against a freshly-spawned panel reading /proc before
/// the kernel filled the second-sample buffer).
#[must_use]
pub fn safe_fraction(used: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    (used as f32 / total as f32).clamp(0.0, 1.0)
}

/// Sample CPU + memory + disk in one shot. Errors are
/// human-readable strings so the panel can surface them in
/// the status row.
pub async fn sample_resources() -> Result<Sample, String> {
    let cpu_percent = cpu_percent_one_shot().await;
    let (mem_used_mib, mem_total_mib) = meminfo_used_total().await?;
    let (disk_used_gib, disk_total_gib) = disk_used_total().await?;
    Ok(Sample {
        cpu_percent,
        mem_used_mib,
        mem_total_mib,
        disk_used_gib,
        disk_total_gib,
    })
}

/// Read `/proc/stat` for the aggregate `cpu` line. Returns
/// `(idle, total)` jiffies — caller diffs two samples to get
/// a percentage. Treats parse failures as `(0, 0)` so the
/// caller's percentage falls to 0% rather than crashing.
async fn read_proc_stat_idle_total() -> (u64, u64) {
    let Ok(raw) = tokio::fs::read_to_string("/proc/stat").await else {
        return (0, 0);
    };
    parse_proc_stat_idle_total(&raw).unwrap_or((0, 0))
}

/// Pure parser for /proc/stat's `cpu ...` first line. Returns
/// the `(idle, total)` jiffy pair on success.
#[must_use]
pub fn parse_proc_stat_idle_total(raw: &str) -> Option<(u64, u64)> {
    let first = raw.lines().next()?;
    let mut parts = first.split_whitespace();
    if parts.next()? != "cpu" {
        return None;
    }
    let vals: Vec<u64> = parts.filter_map(|s| s.parse::<u64>().ok()).collect();
    if vals.is_empty() {
        return None;
    }
    let total: u64 = vals.iter().sum();
    // idle is the 4th counter (cpu user nice system idle ...);
    // be defensive about short reads.
    let idle = vals.get(3).copied().unwrap_or(0);
    Some((idle, total))
}

/// Compute a one-shot CPU percentage by taking two /proc/stat
/// samples 200ms apart. Matches the v1.x panel's idle-delta
/// approach but in one async call.
pub async fn cpu_percent_one_shot() -> f32 {
    let (idle_a, total_a) = read_proc_stat_idle_total().await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let (idle_b, total_b) = read_proc_stat_idle_total().await;
    let dt = total_b.saturating_sub(total_a);
    let di = idle_b.saturating_sub(idle_a);
    if dt == 0 {
        return 0.0;
    }
    let busy_frac = 1.0 - (di as f32 / dt as f32);
    (busy_frac * 100.0).clamp(0.0, 100.0)
}

/// Read `/proc/meminfo` for `MemTotal` and `MemAvailable`
/// (in kB). Returns `(used_MiB, total_MiB)`. `used` is
/// `total - available`, matching what every Linux UI shows
/// users by default.
async fn meminfo_used_total() -> Result<(u64, u64), String> {
    let raw = tokio::fs::read_to_string("/proc/meminfo")
        .await
        .map_err(|e| format!("reading /proc/meminfo: {e}"))?;
    parse_meminfo_used_total(&raw)
        .ok_or_else(|| "couldn't parse /proc/meminfo (MemTotal/MemAvailable absent)".to_string())
}

/// Pure parser for /proc/meminfo. Looks for MemTotal +
/// MemAvailable lines, returns `(used_MiB, total_MiB)` after
/// converting kB → MiB (divide by 1024).
#[must_use]
pub fn parse_meminfo_used_total(raw: &str) -> Option<(u64, u64)> {
    let mut total_kb: Option<u64> = None;
    let mut avail_kb: Option<u64> = None;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail_kb = parse_kb(rest);
        }
        if total_kb.is_some() && avail_kb.is_some() {
            break;
        }
    }
    let total = total_kb?;
    let avail = avail_kb?;
    let used_kb = total.saturating_sub(avail);
    Some((used_kb / 1024, total / 1024))
}

fn parse_kb(s: &str) -> Option<u64> {
    s.trim()
        .split_whitespace()
        .next()
        .and_then(|n| n.parse::<u64>().ok())
}

/// Read `$HOME` disk used + total via `statvfs`. The tokio
/// runtime doesn't have a direct statvfs binding; we shell
/// out to `df` for portability + parse the human columns.
async fn disk_used_total() -> Result<(u64, u64), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let Ok(output) = tokio::process::Command::new("df")
        .args(["-B1G", "--output=size,used", &home])
        .output()
        .await
    else {
        return Err("df not found".to_string());
    };
    if !output.status.success() {
        return Err(format!(
            "df failed: {}",
            String::from_utf8(output.stderr).unwrap_or_default()
        ));
    }
    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    parse_df_size_used(&stdout).ok_or_else(|| "couldn't parse df output".to_string())
}

/// Pure parser for `df -B1G --output=size,used <path>` output.
/// First line is the header, second has the numbers. Returns
/// `(used_GiB, total_GiB)`.
#[must_use]
pub fn parse_df_size_used(raw: &str) -> Option<(u64, u64)> {
    let mut lines = raw.lines();
    lines.next()?; // header
    let data = lines.next()?;
    let mut parts = data.split_whitespace();
    let total: u64 = parts.next()?.trim_end_matches('G').parse().ok()?;
    let used: u64 = parts.next()?.trim_end_matches('G').parse().ok()?;
    Some((used, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_fraction_handles_zero_total() {
        assert_eq!(safe_fraction(5, 0), 0.0);
    }

    #[test]
    fn safe_fraction_clamps_to_one() {
        assert_eq!(safe_fraction(20, 10), 1.0);
        assert_eq!(safe_fraction(0, 10), 0.0);
        assert!((safe_fraction(5, 10) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_proc_stat_extracts_idle_and_total() {
        // user nice system idle iowait irq softirq steal
        let raw = "cpu  10 0 5 100 0 0 0 0\nintr 1\n";
        let (idle, total) = parse_proc_stat_idle_total(raw).unwrap();
        assert_eq!(idle, 100);
        assert_eq!(total, 115);
    }

    #[test]
    fn parse_proc_stat_rejects_missing_cpu_line() {
        assert!(parse_proc_stat_idle_total("").is_none());
        assert!(parse_proc_stat_idle_total("intr 1\n").is_none());
    }

    #[test]
    fn parse_meminfo_computes_used_from_total_minus_available() {
        let raw = "\
MemTotal:        8000000 kB
MemFree:         2000000 kB
MemAvailable:    4000000 kB
Buffers:               0 kB
";
        let (used_mib, total_mib) = parse_meminfo_used_total(raw).unwrap();
        // total = 8000000 kB → 7812 MiB
        // used  = (8000000 - 4000000) = 4000000 kB → 3906 MiB
        assert_eq!(total_mib, 7812);
        assert_eq!(used_mib, 3906);
    }

    #[test]
    fn parse_meminfo_none_when_keys_missing() {
        assert!(parse_meminfo_used_total("MemTotal:  100 kB\n").is_none());
    }

    #[test]
    fn parse_df_extracts_used_and_total_from_block_output() {
        let raw = "1G-blocks  Used\n  500G    120G\n";
        let (used, total) = parse_df_size_used(raw).unwrap();
        assert_eq!(used, 120);
        assert_eq!(total, 500);
    }

    #[test]
    fn parse_df_none_on_empty_or_malformed() {
        assert!(parse_df_size_used("").is_none());
        assert!(parse_df_size_used("header only\n").is_none());
    }

    #[test]
    fn loaded_records_sample_and_marks_status() {
        let mut panel = ResourcesPanel::new();
        let _ = panel.update(Message::Loaded(Sample {
            cpu_percent: 42.0,
            mem_used_mib: 1000,
            mem_total_mib: 4000,
            disk_used_gib: 100,
            disk_total_gib: 500,
        }));
        assert!((panel.cpu_percent - 42.0).abs() < f32::EPSILON);
        assert_eq!(panel.mem_used_mib, 1000);
        assert_eq!(panel.disk_total_gib, 500);
        assert!(panel.status.contains("Sample"));
    }

    #[test]
    fn error_message_carries_to_status() {
        let mut panel = ResourcesPanel::new();
        let _ = panel.update(Message::Error("df not found".into()));
        assert_eq!(panel.status, "df not found");
    }
}
