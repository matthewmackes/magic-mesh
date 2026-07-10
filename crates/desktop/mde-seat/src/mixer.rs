//! The audio-mixer client seam — the DAW-authentic mixer's data source (lock 4).
//!
//! [`PwGraph`] (E12-16) is the real client: it reads the `PipeWire` graph via
//! `pw-dump`'s JSON, folds every audio node into channel [`MixerStrip`]s + a
//! master sink strip, classifies each strip's [`StripOrigin`] (host session ·
//! local VM · mesh-remote), and drives volume/mute through `wpctl`. When
//! `PipeWire` is absent (a headless CI host with no `pw-dump`), the client answers
//! a typed [`SeatError::Unavailable`] naming `PipeWire` — the honest "no mixer
//! here" state the System surface renders (§7), never fake strips at fake levels.
//! [`UnboundMixer`] is the same honest answer as an explicit no-backend seam.
//!
//! The graph client's I/O is a narrow [`PwRunner`] seam (dump + volume/mute
//! writes), so the whole fold — nodes → strips, origin classification, unit-gain
//! defaults — is pure and unit-tested headless against hand-built `pw-dump` JSON.
//! Prefer the native `pipewire` crate one day; today its bindgen/libpipewire deps
//! don't resolve under the airgapped 1.94 farm pin, so the `pw-dump` / `wpctl`
//! JSON path is the client's I/O (a typed runner, not raw product shell).

use crate::error::{Backend, SeatError};

/// Where a mixer strip's audio comes from — the lock-4 span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StripOrigin {
    /// The local host session (musicd / voice / a host app).
    HostSession,
    /// A local VM session, by its VM name.
    LocalVm(String),
    /// A mesh-remote peer's audio stream, by peer node id.
    MeshRemote(String),
}

/// One mixer channel strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixerStrip {
    /// A stable id for the strip (the `PipeWire` node id, once bound).
    pub id: String,
    /// Operator-facing name (application / VM / peer label).
    pub name: String,
    /// Where the audio originates.
    pub origin: StripOrigin,
    /// Volume 0–100.
    pub volume: u8,
    /// Muted.
    pub muted: bool,
}

/// The whole mixer state: the master strip plus every channel strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixerStatus {
    /// The master output strip.
    pub master: MixerStrip,
    /// Every channel strip (host / VM / mesh-remote).
    pub strips: Vec<MixerStrip>,
}

/// The mixer seam. Production impl ([`PwGraph`]) drives the `PipeWire` graph;
/// [`UnboundMixer`] is the honest no-backend answer.
pub trait MixerClient: Send {
    /// Read the whole mixer state.
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when no `PipeWire` daemon / `pw-dump` is present,
    /// or the graph has no audio sink to act as master.
    fn status(&self) -> Result<MixerStatus, SeatError>;

    /// Set a strip's volume (0–100).
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when `PipeWire` is absent; [`SeatError::Backend`]
    /// on a control failure.
    fn set_volume(&self, strip_id: &str, volume: u8) -> Result<(), SeatError>;

    /// Set a strip's mute.
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when `PipeWire` is absent; [`SeatError::Backend`]
    /// on a control failure.
    fn set_muted(&self, strip_id: &str, muted: bool) -> Result<(), SeatError>;
}

// ── The PipeWire property contract ──────────────────────────────────────────
//
// The classifier keys off standard PipeWire node props plus two MDE-private ones
// the VM / mesh audio bridges stamp on the streams they proxy. Reading a graph
// never invents an origin: an untagged stream is honestly a HostSession strip.

/// `media.class` of a playback application stream (musicd / voice / a VM / a
/// remote proxy) — these become channel strips.
const CLASS_STREAM_OUTPUT: &str = "Stream/Output/Audio";
/// `media.class` of an output sink — the master strip candidate.
const CLASS_SINK: &str = "Audio/Sink";
/// MDE-private prop the mesh audio bridge stamps on a remote peer's proxied
/// stream: the peer node-id. Its presence ⇒ [`StripOrigin::MeshRemote`].
const PROP_MESH_PEER: &str = "mde.mesh.peer";
/// MDE-private prop the VM audio bridge stamps on a guest's stream: the VM name.
/// Its presence ⇒ [`StripOrigin::LocalVm`].
const PROP_VM_NAME: &str = "mde.vm.name";

/// Classify a node's [`StripOrigin`] from its `pw-dump` props object.
///
/// The MDE-private tags win first (`mde.mesh.peer` → mesh-remote, `mde.vm.name` →
/// local VM); everything else is the honest host session default.
fn classify_origin(props: &serde_json::Value) -> StripOrigin {
    if let Some(peer) = props
        .get(PROP_MESH_PEER)
        .and_then(serde_json::Value::as_str)
    {
        return StripOrigin::MeshRemote(peer.to_owned());
    }
    if let Some(vm) = props.get(PROP_VM_NAME).and_then(serde_json::Value::as_str) {
        return StripOrigin::LocalVm(vm.to_owned());
    }
    StripOrigin::HostSession
}

/// The operator-facing strip name: description → application name → node name →
/// the node id as a last resort (never blank).
fn node_name(props: &serde_json::Value, id: u64) -> String {
    for key in ["node.description", "application.name", "node.name"] {
        if let Some(v) = props.get(key).and_then(serde_json::Value::as_str) {
            if !v.is_empty() {
                return v.to_owned();
            }
        }
    }
    format!("node {id}")
}

/// Read a node's `(volume, muted)` from its `Props` param.
///
/// `channelVolumes` (the per-channel linear gains) is averaged; a bare `volume`
/// scalar is the fallback. An unset volume is **unity** — `PipeWire`'s own default
/// for an untouched node — so a missing value reads 100, never an invented level.
/// `mute` defaults to `PipeWire`'s default of `false`.
#[allow(clippy::cast_precision_loss)] // a channel count is tiny; f64 is exact
fn node_volume(pw_props: &serde_json::Value) -> (u8, bool) {
    let muted = pw_props
        .get("mute")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let linear = pw_props
        .get("channelVolumes")
        .and_then(serde_json::Value::as_array)
        .and_then(|chans| {
            let sum: f64 = chans.iter().filter_map(serde_json::Value::as_f64).sum();
            let n = chans.iter().filter(|c| c.is_number()).count();
            (n > 0).then(|| sum / n as f64)
        })
        .or_else(|| pw_props.get("volume").and_then(serde_json::Value::as_f64))
        .unwrap_or(1.0);
    (linear_to_percent(linear), muted)
}

/// Map a `PipeWire` linear gain (0.0 = silent, 1.0 = unity) to the model's 0–100.
///
/// The mapping is linear — `wpctl set-volume` takes the same linear fraction, so
/// read and write stay consistent. A perceptual/cubic fader curve is a *rendering*
/// choice for the shell's strip UI (E12-15), not the seat model's job. Gains above
/// unity (software boost) clamp to 100 since the model tops out there.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0..=100 below
fn linear_to_percent(linear: f64) -> u8 {
    let pct = (linear * 100.0).round();
    if pct <= 0.0 {
        0
    } else if pct >= 100.0 {
        100
    } else {
        pct as u8 // safe: 0 < pct < 100
    }
}

/// Fold a `pw-dump` JSON document into the [`MixerStatus`].
///
/// Every `PipeWire:Interface:Node` with a playback-stream `media.class` becomes a
/// channel strip (origin-classified); the first audio sink (by node id) is the
/// master. Pure + tolerant: a node missing props/params folds to unity-gain
/// defaults, and unrelated objects are ignored.
///
/// # Errors
/// [`SeatError::Unavailable`] when the graph carries no audio sink — an honest
/// "no output device" state, never a fabricated master.
pub fn fold_graph(dump: &serde_json::Value) -> Result<MixerStatus, SeatError> {
    let nodes = dump.as_array().map(Vec::as_slice).unwrap_or_default();

    let mut master: Option<(u64, MixerStrip)> = None;
    let mut strips = Vec::new();

    for obj in nodes {
        if obj.get("type").and_then(serde_json::Value::as_str) != Some("PipeWire:Interface:Node") {
            continue;
        }
        let Some(id) = obj.get("id").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let info = &obj["info"];
        let props = &info["props"];
        let class = props.get("media.class").and_then(serde_json::Value::as_str);
        // The first Props param carries volume/mute for both sinks and streams.
        let pw_props = info
            .get("params")
            .and_then(|p| p.get("Props"))
            .and_then(serde_json::Value::as_array)
            .and_then(|arr| arr.first())
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let (volume, muted) = node_volume(&pw_props);

        match class {
            Some(CLASS_SINK) => {
                let strip = MixerStrip {
                    id: id.to_string(),
                    name: node_name(props, id),
                    origin: StripOrigin::HostSession,
                    volume,
                    muted,
                };
                // Lowest node id wins the master seat — a stable pick across dumps.
                if master.as_ref().is_none_or(|(cur, _)| id < *cur) {
                    master = Some((id, strip));
                }
            }
            Some(CLASS_STREAM_OUTPUT) => strips.push(MixerStrip {
                id: id.to_string(),
                name: node_name(props, id),
                origin: classify_origin(props),
                volume,
                muted,
            }),
            _ => {}
        }
    }

    let Some((_, master)) = master else {
        return Err(SeatError::Unavailable {
            backend: Backend::PipeWire,
            reason: "the PipeWire graph has no audio sink".to_owned(),
        });
    };

    // Stable render order: by name, then node id (numeric-safe via zero-pad-free
    // parse fallback to string compare).
    strips.sort_by(|a, b| {
        a.name.cmp(&b.name).then_with(|| {
            a.id.parse::<u64>()
                .unwrap_or(0)
                .cmp(&b.id.parse::<u64>().unwrap_or(0))
        })
    });
    Ok(MixerStatus { master, strips })
}

/// The `PipeWire` graph I/O seam: read the graph (`pw-dump`) and write a node's
/// volume / mute (`wpctl`). Production impl is [`PwCli`]; tests inject a fake.
pub trait PwRunner: Send {
    /// The `pw-dump` JSON document (the whole graph as a JSON array).
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when `pw-dump` is absent (no `PipeWire` here);
    /// [`SeatError::Backend`] when it runs but fails.
    fn dump(&self) -> Result<serde_json::Value, SeatError>;

    /// Set a node's linear volume fraction (0.0 = silent, 1.0 = unity).
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when `wpctl` is absent; [`SeatError::Backend`] on
    /// a control failure.
    fn set_volume(&self, node_id: u32, fraction: f64) -> Result<(), SeatError>;

    /// Set a node's mute.
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when `wpctl` is absent; [`SeatError::Backend`] on
    /// a control failure.
    fn set_mute(&self, node_id: u32, muted: bool) -> Result<(), SeatError>;
}

/// The production runner: `pw-dump` for reads, `wpctl` for volume/mute writes.
///
/// Both tools ship with the baked-in `PipeWire` + `WirePlumber` (lock 2); a host
/// without them (headless CI) surfaces as [`SeatError::Unavailable`].
#[derive(Debug, Clone, Copy, Default)]
pub struct PwCli;

impl PwCli {
    /// Classify a failed `Command::output()` — a missing binary is the honest
    /// "no `PipeWire` here" ([`SeatError::Unavailable`]); anything else is a failure.
    fn spawn_error(ctx: &str, e: &std::io::Error) -> SeatError {
        if e.kind() == std::io::ErrorKind::NotFound {
            SeatError::Unavailable {
                backend: Backend::PipeWire,
                reason: format!("{ctx} not found — PipeWire tooling absent"),
            }
        } else {
            SeatError::Backend {
                backend: Backend::PipeWire,
                reason: format!("{ctx}: {e}"),
            }
        }
    }

    /// Run `wpctl <args>`; a non-zero exit or missing binary is typed.
    fn wpctl(args: &[String]) -> Result<(), SeatError> {
        let out = std::process::Command::new("wpctl")
            .args(args)
            .output()
            .map_err(|e| Self::spawn_error("wpctl", &e))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(SeatError::Backend {
                backend: Backend::PipeWire,
                reason: format!(
                    "wpctl {}: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            })
        }
    }
}

impl PwRunner for PwCli {
    fn dump(&self) -> Result<serde_json::Value, SeatError> {
        let out = std::process::Command::new("pw-dump")
            .output()
            .map_err(|e| Self::spawn_error("pw-dump", &e))?;
        if !out.status.success() {
            return Err(SeatError::Backend {
                backend: Backend::PipeWire,
                reason: format!("pw-dump: {}", String::from_utf8_lossy(&out.stderr).trim()),
            });
        }
        serde_json::from_slice(&out.stdout).map_err(|e| SeatError::Protocol {
            backend: Backend::PipeWire,
            reason: format!("pw-dump JSON: {e}"),
        })
    }

    fn set_volume(&self, node_id: u32, fraction: f64) -> Result<(), SeatError> {
        // wpctl takes the linear fraction directly (matches linear_to_percent).
        Self::wpctl(&[
            "set-volume".to_owned(),
            node_id.to_string(),
            format!("{:.4}", fraction.clamp(0.0, 1.0)),
        ])
    }

    fn set_mute(&self, node_id: u32, muted: bool) -> Result<(), SeatError> {
        Self::wpctl(&[
            "set-mute".to_owned(),
            node_id.to_string(),
            if muted { "1" } else { "0" }.to_owned(),
        ])
    }
}

/// The real `PipeWire` mixer client (E12-16). Reads the graph through a
/// [`PwRunner`] and drives volume/mute on the live graph.
pub struct PwGraph {
    runner: Box<dyn PwRunner>,
}

impl PwGraph {
    /// A client over the real host `PipeWire` (`pw-dump` + `wpctl`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            runner: Box::new(PwCli),
        }
    }

    /// A client over an injected runner — the headless test / mirror seam.
    #[must_use]
    pub fn with_runner(runner: Box<dyn PwRunner>) -> Self {
        Self { runner }
    }

    /// Parse a strip id (a node-id string this client minted) back to the
    /// `PipeWire` node id `wpctl` needs.
    fn node_id(strip_id: &str) -> Result<u32, SeatError> {
        strip_id.parse::<u32>().map_err(|_| SeatError::Protocol {
            backend: Backend::PipeWire,
            reason: format!("strip id {strip_id:?} is not a PipeWire node id"),
        })
    }
}

impl Default for PwGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl MixerClient for PwGraph {
    fn status(&self) -> Result<MixerStatus, SeatError> {
        fold_graph(&self.runner.dump()?)
    }

    fn set_volume(&self, strip_id: &str, volume: u8) -> Result<(), SeatError> {
        let id = Self::node_id(strip_id)?;
        self.runner
            .set_volume(id, f64::from(volume.min(100)) / 100.0)
    }

    fn set_muted(&self, strip_id: &str, muted: bool) -> Result<(), SeatError> {
        let id = Self::node_id(strip_id)?;
        self.runner.set_mute(id, muted)
    }
}

/// The not-yet-bound mixer client: a typed [`SeatError::Unavailable`] for every
/// call — an explicit no-backend seam (the same honest answer [`PwGraph`] gives on
/// a host without `PipeWire`).
///
/// The Mixer section shows "audio graph not available" rather than fake faders.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnboundMixer;

impl UnboundMixer {
    const REASON: &'static str = "no PipeWire graph on this host";

    fn unavailable() -> SeatError {
        SeatError::Unavailable {
            backend: Backend::PipeWire,
            reason: Self::REASON.to_owned(),
        }
    }
}

impl MixerClient for UnboundMixer {
    fn status(&self) -> Result<MixerStatus, SeatError> {
        Err(Self::unavailable())
    }

    fn set_volume(&self, _strip_id: &str, _volume: u8) -> Result<(), SeatError> {
        Err(Self::unavailable())
    }

    fn set_muted(&self, _strip_id: &str, _muted: bool) -> Result<(), SeatError> {
        Err(Self::unavailable())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn unbound_mixer_is_honestly_unavailable_not_fake_strips() {
        let m = UnboundMixer;
        let e = m.status().expect_err("must not fabricate strips");
        assert_eq!(e.backend(), Backend::PipeWire);
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
        assert!(matches!(
            m.set_volume("42", 60),
            Err(SeatError::Unavailable { .. })
        ));
        assert!(matches!(
            m.set_muted("42", true),
            Err(SeatError::Unavailable { .. })
        ));
    }

    #[test]
    fn strip_origin_models_the_full_lock4_span() {
        // The three origins the mixer must cover exist in the model now.
        let origins = [
            StripOrigin::HostSession,
            StripOrigin::LocalVm("win10".into()),
            StripOrigin::MeshRemote("nyc3".into()),
        ];
        assert_eq!(origins.len(), 3);
    }

    // ── pure-fold coverage ──────────────────────────────────────────────────

    /// One `pw-dump` node: `type`/`id`/`media.class`, optional MDE tags, and an
    /// optional `channelVolumes` + `mute` in the first Props param.
    fn node(
        id: u64,
        class: &str,
        name: &str,
        extra_props: &serde_json::Value,
        pw_props: &serde_json::Value,
    ) -> serde_json::Value {
        let mut props = serde_json::json!({
            "media.class": class,
            "node.description": name,
        });
        if let (Some(p), Some(e)) = (props.as_object_mut(), extra_props.as_object()) {
            for (k, v) in e {
                p.insert(k.clone(), v.clone());
            }
        }
        serde_json::json!({
            "id": id,
            "type": "PipeWire:Interface:Node",
            "info": { "props": props, "params": { "Props": [pw_props] } },
        })
    }

    fn vol(chans: &[f64], mute: bool) -> serde_json::Value {
        serde_json::json!({ "channelVolumes": chans, "mute": mute })
    }

    /// A realistic graph: a sink, a musicd host stream, a VM stream, a mesh-remote
    /// stream, plus a non-node object and a source that must be ignored.
    fn graph() -> serde_json::Value {
        serde_json::json!([
            node(
                33,
                CLASS_SINK,
                "Built-in Speakers",
                &serde_json::json!({}),
                &vol(&[0.8, 0.8], false)
            ),
            node(
                40,
                CLASS_STREAM_OUTPUT,
                "musicd",
                &serde_json::json!({}),
                &vol(&[0.5, 0.5], false)
            ),
            node(
                41,
                CLASS_STREAM_OUTPUT,
                "win10 guest",
                &serde_json::json!({ PROP_VM_NAME: "win10" }),
                &vol(&[1.0], true)
            ),
            node(
                42,
                CLASS_STREAM_OUTPUT,
                "peer audio",
                &serde_json::json!({ PROP_MESH_PEER: "nyc3" }),
                &vol(&[0.25, 0.25], false)
            ),
            node(
                50,
                "Audio/Source",
                "Built-in Mic",
                &serde_json::json!({}),
                &vol(&[1.0], false)
            ),
            serde_json::json!({ "id": 99, "type": "PipeWire:Interface:Port" }),
        ])
    }

    #[test]
    fn folds_sink_to_master_and_output_streams_to_classified_strips() {
        let status = fold_graph(&graph()).expect("a graph with a sink folds");

        assert_eq!(status.master.id, "33");
        assert_eq!(status.master.name, "Built-in Speakers");
        assert_eq!(status.master.volume, 80);
        assert!(!status.master.muted);
        assert_eq!(status.master.origin, StripOrigin::HostSession);

        // Only the three Stream/Output nodes are strips (no source, no port).
        assert_eq!(status.strips.len(), 3);
        // Sorted by name: "musicd" < "peer audio" < "win10 guest".
        assert_eq!(status.strips[0].name, "musicd");
        assert_eq!(status.strips[0].origin, StripOrigin::HostSession);
        assert_eq!(status.strips[0].volume, 50);

        assert_eq!(
            status.strips[1].origin,
            StripOrigin::MeshRemote("nyc3".into())
        );
        assert_eq!(status.strips[1].volume, 25);

        assert_eq!(
            status.strips[2].origin,
            StripOrigin::LocalVm("win10".into())
        );
        assert!(status.strips[2].muted, "the muted VM stream folds muted");
        assert_eq!(status.strips[2].volume, 100);
    }

    #[test]
    fn a_graph_without_a_sink_is_honestly_unavailable_not_a_fake_master() {
        let no_sink = serde_json::json!([node(
            40,
            CLASS_STREAM_OUTPUT,
            "musicd",
            &serde_json::json!({}),
            &vol(&[0.5], false)
        )]);
        let e = fold_graph(&no_sink).expect_err("no sink ⇒ no master");
        assert_eq!(e.backend(), Backend::PipeWire);
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
    }

    #[test]
    fn lowest_node_id_wins_the_master_seat_stably() {
        let two_sinks = serde_json::json!([
            node(
                70,
                CLASS_SINK,
                "HDMI",
                &serde_json::json!({}),
                &vol(&[1.0], false)
            ),
            node(
                33,
                CLASS_SINK,
                "Speakers",
                &serde_json::json!({}),
                &vol(&[0.6], false)
            ),
        ]);
        let status = fold_graph(&two_sinks).expect("folds");
        assert_eq!(status.master.id, "33", "lowest id is the stable master");
        assert_eq!(status.master.volume, 60);
    }

    #[test]
    fn unset_volume_reads_unity_not_an_invented_level() {
        // No channelVolumes, no volume, no mute ⇒ PipeWire's unset default: unity,
        // unmuted — the honest reading, not a fabricated silence.
        let (volume, muted) = node_volume(&serde_json::Value::Null);
        assert_eq!(volume, 100);
        assert!(!muted);
    }

    #[test]
    fn linear_gain_maps_and_clamps_to_the_0_100_model() {
        assert_eq!(linear_to_percent(0.0), 0);
        assert_eq!(linear_to_percent(0.5), 50);
        assert_eq!(linear_to_percent(1.0), 100);
        assert_eq!(linear_to_percent(1.5), 100, "software boost clamps to 100");
        assert_eq!(linear_to_percent(-0.2), 0, "a negative gain floors at 0");
    }

    // ── the graph client over an injected runner ────────────────────────────

    /// A fake runner over shared recorders, so a test can move it into the client
    /// and still read the writes it recorded.
    #[derive(Clone, Default)]
    struct FakeRunner {
        dump: serde_json::Value,
        volumes: Arc<Mutex<Vec<(u32, f64)>>>,
        mutes: Arc<Mutex<Vec<(u32, bool)>>>,
    }

    impl PwRunner for FakeRunner {
        fn dump(&self) -> Result<serde_json::Value, SeatError> {
            Ok(self.dump.clone())
        }
        fn set_volume(&self, node_id: u32, fraction: f64) -> Result<(), SeatError> {
            self.volumes.lock().expect("lock").push((node_id, fraction));
            Ok(())
        }
        fn set_mute(&self, node_id: u32, muted: bool) -> Result<(), SeatError> {
            self.mutes.lock().expect("lock").push((node_id, muted));
            Ok(())
        }
    }

    #[test]
    fn pwgraph_status_folds_the_injected_dump() {
        let graph = PwGraph::with_runner(Box::new(FakeRunner {
            dump: graph(),
            ..Default::default()
        }));
        let status = graph.status().expect("folds the fake graph");
        assert_eq!(status.master.id, "33");
        assert_eq!(status.strips.len(), 3);
    }

    #[test]
    fn pwgraph_writes_reach_the_runner_with_the_right_node_and_fraction() {
        let runner = FakeRunner::default();
        let recorder = runner.clone(); // shares the same Arc recorders
        let graph = PwGraph::with_runner(Box::new(runner));

        // 75% ⇒ the linear 0.75 fraction wpctl gets; node id parsed from the strip.
        graph.set_volume("41", 75).expect("volume write");
        graph.set_muted("41", true).expect("mute write");
        assert_eq!(*recorder.volumes.lock().expect("lock"), vec![(41, 0.75)]);
        assert_eq!(*recorder.mutes.lock().expect("lock"), vec![(41, true)]);

        // A non-numeric strip id is refused typed, never silently dropped.
        assert!(matches!(
            graph.set_volume("master", 10),
            Err(SeatError::Protocol { .. })
        ));
    }

    #[test]
    fn the_real_client_on_this_host_answers_typed_never_panics() {
        // With or without PipeWire tooling, PwGraph::new().status() must return a
        // real MixerStatus or a typed SeatError tagged PipeWire — the §7 contract.
        match PwGraph::new().status() {
            Ok(status) => {
                let _ = status.strips.len();
            }
            Err(e) => assert_eq!(e.backend(), Backend::PipeWire),
        }
    }
}
