//! Sound panel — default-sink + default-source pickers backed
//! by `pactl` (PulseAudio / PipeWire-pulse compat layer).
//!
//! CB-1.4.b: replaces the v1.x `mackes/workbench/devices/sound.py`
//! GTK3 panel. Pulled the same `pactl` subprocess approach as
//! the Python panel — the alternative (`pipewire-rs` directly)
//! adds a substantial dep surface that v2.0.0's monolithic cut
//! is intentionally keeping small. `pactl` ships with
//! pulseaudio-utils which Fedora pre-installs on every desktop
//! variant.
//!
//! Volume slider + mute toggle are tracked separately in
//! "CB-1.4.b follow-up: per-sink volume + mute" — the
//! acceptance criterion for this task is "picker shows every
//! active sink + changes propagate to PipeWire immediately",
//! which the pickers alone satisfy.

use iced::widget::{checkbox, column, pick_list, row, slider, text};
use iced::{Element, Length, Task};

use crate::controls::{variant_button, ButtonVariant};
use tokio::process::Command;

/// pactl's `list short sinks` / `list short sources` output is
/// tab-separated columns: `index<TAB>name<TAB>driver<TAB>spec<TAB>state`.
/// We only need the `name` column — that's the identifier
/// `set-default-sink` / `set-default-source` accept.
const NAME_COL: usize = 1;

#[derive(Debug, Clone, Default)]
pub struct SoundPanel {
    /// Whether `pactl info` succeeded on the last load. Drives
    /// the empty-state body when PA/PipeWire isn't reachable.
    pub pactl_available: bool,
    pub sinks: Vec<String>,
    pub sources: Vec<String>,
    pub default_sink: String,
    pub default_source: String,
    /// CB-1.4.b follow-up — default-sink volume as a percent
    /// (0–150 range; PA allows >100% for boost).
    pub volume_pct: u32,
    /// CB-1.4.b follow-up — default-sink mute state.
    pub muted: bool,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        pactl_available: bool,
        sinks: Vec<String>,
        sources: Vec<String>,
        default_sink: String,
        default_source: String,
        volume_pct: u32,
        muted: bool,
    },
    Error(String),
    SinkSelected(String),
    SourceSelected(String),
    SinkApplied,
    SourceApplied,
    /// CB-1.4.b follow-up — user dragged the volume slider.
    /// Slider values are 0–150 (matching the PA 0–150%
    /// canonical range; alsa/PW pass higher values through
    /// but our slider caps at the soft ceiling).
    VolumeChanged(u32),
    /// CB-1.4.b follow-up — user toggled the mute checkbox.
    MuteToggled(bool),
    /// CB-1.4.b follow-up — set-sink-volume / set-sink-mute
    /// completed (either success or failure; the panel just
    /// clears busy and refreshes).
    VolumeApplied,
}

impl SoundPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let pactl_available = !run_pactl(&["info"]).await.is_empty();
                if !pactl_available {
                    return Message::Loaded {
                        pactl_available,
                        sinks: Vec::new(),
                        sources: Vec::new(),
                        default_sink: String::new(),
                        default_source: String::new(),
                        volume_pct: 0,
                        muted: false,
                    };
                }
                let sinks_raw = run_pactl(&["list", "short", "sinks"]).await;
                let sources_raw = run_pactl(&["list", "short", "sources"]).await;
                let default_sink = run_pactl(&["get-default-sink"]).await;
                let default_source = run_pactl(&["get-default-source"]).await;
                let vol_raw = run_pactl(&["get-sink-volume", "@DEFAULT_SINK@"]).await;
                let mute_raw = run_pactl(&["get-sink-mute", "@DEFAULT_SINK@"]).await;
                Message::Loaded {
                    pactl_available,
                    sinks: parse_pactl_short(&sinks_raw, false),
                    sources: parse_pactl_short(&sources_raw, true),
                    default_sink: default_sink.trim().to_string(),
                    default_source: default_source.trim().to_string(),
                    volume_pct: parse_volume_percent(&vol_raw),
                    muted: parse_mute(&mute_raw),
                }
            },
            crate::Message::Sound,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                pactl_available,
                sinks,
                sources,
                default_sink,
                default_source,
                volume_pct,
                muted,
            } => {
                self.pactl_available = pactl_available;
                self.sinks = sinks;
                self.sources = sources;
                self.default_sink = pick_existing(&self.sinks, &default_sink);
                self.default_source = pick_existing(&self.sources, &default_source);
                self.volume_pct = volume_pct.min(150);
                self.muted = muted;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::SinkSelected(name) => {
                if self.busy {
                    return Task::none();
                }
                self.default_sink = name.clone();
                self.busy = true;
                self.status = "Applying…".into();
                Task::perform(
                    async move {
                        let _ = run_pactl(&["set-default-sink", &name]).await;
                        Message::SinkApplied
                    },
                    crate::Message::Sound,
                )
            }
            Message::SourceSelected(name) => {
                if self.busy {
                    return Task::none();
                }
                self.default_source = name.clone();
                self.busy = true;
                self.status = "Applying…".into();
                Task::perform(
                    async move {
                        let _ = run_pactl(&["set-default-source", &name]).await;
                        Message::SourceApplied
                    },
                    crate::Message::Sound,
                )
            }
            Message::SinkApplied | Message::SourceApplied | Message::VolumeApplied => {
                self.status = "Applied.".into();
                self.busy = false;
                Task::none()
            }
            Message::VolumeChanged(v) => {
                if self.busy {
                    return Task::none();
                }
                let pct = v.min(150);
                self.volume_pct = pct;
                self.busy = true;
                self.status = format!("Setting volume to {pct}%…");
                Task::perform(
                    async move {
                        let _ =
                            run_pactl(&["set-sink-volume", "@DEFAULT_SINK@", &format!("{pct}%")])
                                .await;
                        Message::VolumeApplied
                    },
                    crate::Message::Sound,
                )
            }
            Message::MuteToggled(muted) => {
                if self.busy {
                    return Task::none();
                }
                self.muted = muted;
                self.busy = true;
                self.status = if muted { "Muting…" } else { "Unmuting…" }.into();
                let flag = if muted { "1" } else { "0" };
                Task::perform(
                    async move {
                        let _ = run_pactl(&["set-sink-mute", "@DEFAULT_SINK@", flag]).await;
                        Message::VolumeApplied
                    },
                    crate::Message::Sound,
                )
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        if !self.pactl_available {
            return column![
                text("Audio routing unavailable").size(18),
                text(
                    "MDE talks to PulseAudio / PipeWire through `pactl`. \
                     Install the pulseaudio-utils package and reopen this \
                     panel to pick a default sink + source.",
                )
                .size(13),
            ]
            .spacing(8)
            .width(Length::Fill)
            .into();
        }

        // UX-7.a — refresh routed through the shared button
        // variant so busy/disabled state is consistent across
        // panels.
        let refresh_btn = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::SoundRefresh),
            crate::live_theme::palette(),
        );

        let sinks = self.sinks.clone();
        let sources = self.sources.clone();
        let sink_pick: pick_list::PickList<'_, String, _, _, crate::Message> = pick_list(
            sinks,
            current_or_none(&self.sinks, &self.default_sink),
            |v| crate::Message::Sound(Message::SinkSelected(v)),
        );
        let source_pick: pick_list::PickList<'_, String, _, _, crate::Message> = pick_list(
            sources,
            current_or_none(&self.sources, &self.default_source),
            |v| crate::Message::Sound(Message::SourceSelected(v)),
        );

        let volume_slider = slider(0.0..=150.0, self.volume_pct as f32, |v| {
            crate::Message::Sound(Message::VolumeChanged(v as u32))
        })
        .step(1.0_f32);
        let mute_checkbox = checkbox(self.muted)
            .label("Muted")
            .on_toggle(|v| crate::Message::Sound(Message::MuteToggled(v)));

        column![
            row![text("Default sink").width(Length::Fixed(180.0)), sink_pick,].spacing(12),
            row![
                text("Volume").width(Length::Fixed(180.0)),
                volume_slider,
                text(format!("{}%", self.volume_pct)).width(Length::Fixed(60.0)),
                mute_checkbox,
            ]
            .spacing(12),
            row![
                text("Default source").width(Length::Fixed(180.0)),
                source_pick,
            ]
            .spacing(12),
            row![refresh_btn, text(&self.status).size(13)].spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

fn current_or_none(list: &[String], value: &str) -> Option<String> {
    list.iter().find(|v| *v == value).cloned()
}

/// Pick the first table entry, falling back to empty when the
/// table itself is empty. Used so the default-sink/source picker
/// lands on something selectable when the system default
/// references an unplugged device that pactl no longer lists.
fn pick_existing(table: &[String], wanted: &str) -> String {
    if table.iter().any(|n| n == wanted) {
        wanted.to_string()
    } else {
        table.first().cloned().unwrap_or_default()
    }
}

/// Parse `pactl list short sinks` / `... sources` output into
/// a Vec of identifier names. When `filter_monitors` is set,
/// source names ending in `.monitor` are skipped (those are
/// pactl's loopback-of-output captures, never user-meaningful
/// recording inputs).
#[must_use]
pub fn parse_pactl_short(raw: &str, filter_monitors: bool) -> Vec<String> {
    raw.lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            cols.get(NAME_COL).map(|s| s.trim().to_string())
        })
        .filter(|n| !n.is_empty())
        .filter(|n| !filter_monitors || !n.ends_with(".monitor"))
        .collect()
}

/// Shell out to `pactl` with the given args, returning stdout
/// as a string. Returns `""` on any error (binary missing, bus
/// unreachable, non-zero exit) so callers can use empty as the
/// "unavailable" signal without bubbling Result through the
/// reducer.
pub async fn run_pactl(args: &[&str]) -> String {
    let Ok(output) = Command::new("pactl").args(args).output().await else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8(output.stdout).unwrap_or_default()
}

/// Parse the first percent value out of `pactl get-sink-volume`
/// output. The typical shape is multi-line, e.g.
/// `Volume: front-left: 65536 / 100% / 0.00 dB, front-right:
/// 65536 / 100% / 0.00 dB`. We surface the first `<n>%` we
/// encounter. Returns 0 on any failure (empty input, no
/// percent token, non-numeric) so the slider lands at zero
/// rather than crashing.
#[must_use]
pub fn parse_volume_percent(raw: &str) -> u32 {
    for token in raw.split(|c: char| c.is_whitespace() || c == ',' || c == '/') {
        if let Some(num) = token.strip_suffix('%') {
            if let Ok(n) = num.parse::<u32>() {
                return n;
            }
        }
    }
    0
}

/// Parse `pactl get-sink-mute` output. Typical shape: `Mute:
/// yes` or `Mute: no`. Returns true only for the `yes` case;
/// any parse failure (or `no`) returns false so the checkbox
/// defaults to "not muted".
#[must_use]
pub fn parse_mute(raw: &str) -> bool {
    raw.trim().to_ascii_lowercase().contains(": yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pactl_short_extracts_name_column() {
        let raw = "0\talsa_output.pci-0000_00_1f.3.analog-stereo\tPipeWire\tfloat32le 2ch 48000Hz\tSUSPENDED\n\
                   1\tbluez_output.E4_67_BA_45_8D_06.1\tPipeWire\ts16le 2ch 44100Hz\tRUNNING";
        let names = parse_pactl_short(raw, false);
        assert_eq!(
            names,
            vec![
                "alsa_output.pci-0000_00_1f.3.analog-stereo".to_string(),
                "bluez_output.E4_67_BA_45_8D_06.1".to_string(),
            ]
        );
    }

    #[test]
    fn parse_pactl_short_filter_monitors_drops_loopback_sources() {
        let raw = "0\talsa_input.pci-0000_00_1f.3.analog-stereo\tPipeWire\tspec\tIDLE\n\
                   1\talsa_output.pci-0000_00_1f.3.analog-stereo.monitor\tPipeWire\tspec\tIDLE";
        let names = parse_pactl_short(raw, true);
        assert_eq!(
            names,
            vec!["alsa_input.pci-0000_00_1f.3.analog-stereo".to_string()]
        );
    }

    #[test]
    fn parse_pactl_short_keeps_monitors_when_filter_off() {
        let raw = "0\talsa_output.pci-0000_00_1f.3.analog-stereo.monitor\tPipeWire\tspec\tIDLE\n";
        let names = parse_pactl_short(raw, false);
        assert_eq!(
            names,
            vec!["alsa_output.pci-0000_00_1f.3.analog-stereo.monitor".to_string()]
        );
    }

    #[test]
    fn parse_pactl_short_drops_blank_and_malformed_lines() {
        let raw = "\n0\tonly-one-col\n\t\t\t\n1\tvalid_name\tPipeWire\tspec\tIDLE";
        let names = parse_pactl_short(raw, false);
        assert_eq!(
            names,
            vec!["only-one-col".to_string(), "valid_name".to_string()]
        );
    }

    #[test]
    fn parse_pactl_short_empty_on_empty_input() {
        assert!(parse_pactl_short("", false).is_empty());
    }

    #[test]
    fn pick_existing_falls_back_to_first_when_wanted_absent() {
        let table = vec!["a".to_string(), "b".to_string()];
        assert_eq!(pick_existing(&table, "z"), "a");
        assert_eq!(pick_existing(&table, "b"), "b");
        assert_eq!(pick_existing(&[], "z"), "");
    }

    #[test]
    fn loaded_pactl_unavailable_clears_busy_and_lists() {
        let mut panel = SoundPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Loaded {
            pactl_available: false,
            sinks: Vec::new(),
            sources: Vec::new(),
            default_sink: String::new(),
            default_source: String::new(),
            volume_pct: 0,
            muted: false,
        });
        assert!(!panel.pactl_available);
        assert!(!panel.busy);
        assert!(panel.sinks.is_empty());
    }

    #[test]
    fn loaded_with_unknown_default_sink_falls_back_to_first_listed() {
        let mut panel = SoundPanel::new();
        let _ = panel.update(Message::Loaded {
            pactl_available: true,
            sinks: vec!["alpha".into(), "beta".into()],
            sources: vec!["mic".into()],
            default_sink: "vanished".into(),
            default_source: "mic".into(),
            volume_pct: 65,
            muted: false,
        });
        assert_eq!(panel.default_sink, "alpha");
        assert_eq!(panel.default_source, "mic");
        assert_eq!(panel.volume_pct, 65);
        assert!(!panel.muted);
    }

    #[test]
    fn loaded_with_known_default_sink_preserves_selection() {
        let mut panel = SoundPanel::new();
        let _ = panel.update(Message::Loaded {
            pactl_available: true,
            sinks: vec!["alpha".into(), "beta".into()],
            sources: vec![],
            default_sink: "beta".into(),
            default_source: String::new(),
            volume_pct: 100,
            muted: true,
        });
        assert_eq!(panel.default_sink, "beta");
        assert!(panel.muted);
        assert_eq!(panel.volume_pct, 100);
    }

    #[test]
    fn parse_volume_percent_extracts_first_percent_value() {
        let raw = "Volume: front-left: 65536 /  65% / -10.50 dB,   front-right: 65536 /  65% / -10.50 dB\n           balance 0.00\n";
        assert_eq!(parse_volume_percent(raw), 65);
    }

    #[test]
    fn parse_volume_percent_handles_100_and_boost() {
        assert_eq!(
            parse_volume_percent("Volume: 1: 65536 / 100% / 0.00 dB"),
            100
        );
        assert_eq!(
            parse_volume_percent("Volume: 1: 98304 / 150% / +8.00 dB"),
            150
        );
    }

    #[test]
    fn parse_volume_percent_zero_on_garbage() {
        assert_eq!(parse_volume_percent(""), 0);
        assert_eq!(parse_volume_percent("no percent here"), 0);
        assert_eq!(parse_volume_percent("nope%"), 0);
    }

    #[test]
    fn parse_mute_yes_means_muted() {
        assert!(parse_mute("Mute: yes\n"));
        assert!(parse_mute("  mute: YES "));
    }

    #[test]
    fn parse_mute_no_or_garbage_means_not_muted() {
        assert!(!parse_mute("Mute: no"));
        assert!(!parse_mute(""));
        assert!(!parse_mute("nothing relevant"));
    }

    #[test]
    fn volume_changed_clamps_to_150_and_sets_busy() {
        let mut panel = SoundPanel::new();
        let _ = panel.update(Message::VolumeChanged(200));
        assert_eq!(panel.volume_pct, 150);
        assert!(panel.busy);
        assert!(panel.status.contains("Setting volume"));
    }

    #[test]
    fn mute_toggled_updates_state_and_status() {
        let mut panel = SoundPanel::new();
        let _ = panel.update(Message::MuteToggled(true));
        assert!(panel.muted);
        assert!(panel.status.contains("Muting"));
        let _ = panel.update(Message::VolumeApplied);
        let mut panel2 = SoundPanel::new();
        panel2.muted = true;
        let _ = panel2.update(Message::MuteToggled(false));
        assert!(!panel2.muted);
        assert!(panel2.status.contains("Unmuting"));
    }

    #[test]
    fn volume_applied_clears_busy() {
        let mut panel = SoundPanel::new();
        panel.busy = true;
        panel.status = "Setting volume…".into();
        let _ = panel.update(Message::VolumeApplied);
        assert!(!panel.busy);
        assert_eq!(panel.status, "Applied.");
    }

    #[test]
    fn sink_selected_while_busy_is_noop() {
        let mut panel = SoundPanel::new();
        panel.busy = true;
        panel.default_sink = "alpha".into();
        let _ = panel.update(Message::SinkSelected("beta".into()));
        assert_eq!(panel.default_sink, "alpha");
    }

    #[test]
    fn applied_messages_clear_busy_and_record_status() {
        let mut panel = SoundPanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(Message::SinkApplied);
        assert!(!panel.busy);
        assert_eq!(panel.status, "Applied.");
    }

    #[test]
    fn error_message_clears_busy_and_stores_msg() {
        let mut panel = SoundPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Error("pactl bus closed".into()));
        assert_eq!(panel.status, "pactl bus closed");
        assert!(!panel.busy);
    }
}
