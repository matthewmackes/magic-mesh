//! NetworkManager/ModemManager observation plumbing for mesh-status.
//!
//! The command boundary is intentionally injectable so parsing and the
//! privacy invariant are testable without NetworkManager, ModemManager, or
//! root.  Only provider, link name, state, and bounded link facts are read;
//! connection/profile fields are never requested or serialized.

use std::process::Command;

use mackes_mesh_types::network_status::{
    NetworkProvider, ProviderLinkObservation, ProviderLinkState, MAX_PROVIDER_LINKS,
};
use serde_json::{Map, Value};

/// Maximum stdout retained from any status command.  Audio provider tools can
/// enumerate arbitrary device graphs; the world-readable status boundary must
/// remain small even when a provider misbehaves.
pub const MAX_STATUS_COMMAND_BYTES: usize = 64 * 1024;

/// Honest availability of an observed audio component.  `Available` means
/// that the component returned bounded evidence, not that playback works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioAvailability {
    /// A provider returned usable evidence for this component.
    Available,
    /// The provider/command was absent, failed, or returned no evidence.
    Unavailable,
}

/// One bounded, credential-free audio component observation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AudioComponentObservation {
    /// Whether this component produced evidence.
    pub availability: AudioAvailability,
    /// Number of provider records observed, when a record count is meaningful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_items: Option<u16>,
}

/// Result of the PulseAudio-compatible control-surface probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PulseAudioCompatibility {
    /// `pactl info` proved a PulseAudio-compatible server, including
    /// PipeWire's `pipewire-pulse` server.
    Compatible,
    /// The command returned evidence, but it did not identify a compatible
    /// PulseAudio server.
    Unknown,
    /// No usable `pactl` evidence was available.
    Unavailable,
}

/// PulseAudio compatibility observation with an explicit non-health result.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PulseAudioObservation {
    /// Whether `pactl` returned bounded evidence.
    pub availability: AudioAvailability,
    /// Compatibility classification proved by the returned server identity.
    pub compatibility: PulseAudioCompatibility,
    /// Number of non-empty lines retained as bounded evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_items: Option<u16>,
}

impl AudioComponentObservation {
    fn unavailable() -> Self {
        Self {
            availability: AudioAvailability::Unavailable,
            observed_items: None,
        }
    }

    fn available(observed_items: Option<u16>) -> Self {
        Self {
            availability: AudioAvailability::Available,
            observed_items,
        }
    }
}

/// The daemon's additive `network.audio` publication.  No usernames, device
/// labels, profile names, or command output are retained; only typed bounded
/// availability and counts cross the world-readable mesh-status boundary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AudioObservation {
    /// Overall observation availability; this is not a health claim.
    pub availability: AudioAvailability,
    /// PulseAudio protocol compatibility (including PipeWire's Pulse server).
    pub pulse_audio_compatibility: PulseAudioObservation,
    /// PipeWire graph evidence.
    pub pipewire_graph: AudioComponentObservation,
    /// WirePlumber policy evidence.
    pub wireplumber_policy: AudioComponentObservation,
    /// ALSA/UCM card-discovery evidence.
    pub alsa_ucm_discovery: AudioComponentObservation,
    /// Playback-device enumeration evidence.
    pub playback: AudioComponentObservation,
    /// Capture-device enumeration evidence.
    pub capture: AudioComponentObservation,
    /// Recovery/provider availability evidence.  No automatic healthy value is
    /// emitted when the recovery provider is absent.
    pub recovery: AudioComponentObservation,
}

/// A command runner for read-only, credential-free audio provider probes.
pub trait AudioCommandRunner {
    /// Run a fixed-argument command and return bounded stdout only on success.
    fn run_audio(&self, program: &str, args: &[&str]) -> Option<String>;
}

/// A minimal command runner used by the collector and its tests.
pub trait NetworkCommandRunner {
    /// Run a command with fixed arguments and return stdout only on success.
    fn run(&self, program: &str, args: &[&str]) -> Option<String>;
}

/// Production command runner.  Commands are narrowly shaped below; no shell
/// is involved and no connection/profile data is requested.
pub struct SystemNetworkCommandRunner;

impl NetworkCommandRunner for SystemNetworkCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Option<String> {
        let output = Command::new(program).args(args).output().ok()?;
        output.status.success().then(|| {
            let end = output.stdout.len().min(MAX_STATUS_COMMAND_BYTES);
            String::from_utf8_lossy(&output.stdout[..end]).into_owned()
        })
    }
}

impl<T: NetworkCommandRunner + ?Sized> AudioCommandRunner for T {
    fn run_audio(&self, program: &str, args: &[&str]) -> Option<String> {
        self.run(program, args)
            .map(|stdout| bounded_text(&stdout).to_string())
    }
}

fn bounded_record_count(stdout: &str) -> Option<u16> {
    let count = stdout.lines().filter(|line| !line.trim().is_empty()).count();
    u16::try_from(count.min(usize::from(u16::MAX))).ok()
}

fn bounded_text(text: &str) -> &str {
    let end = text.len().min(MAX_STATUS_COMMAND_BYTES);
    let end = (0..=end)
        .rev()
        .find(|candidate| text.is_char_boundary(*candidate))
        .unwrap_or(0);
    &text[..end]
}

fn component_from_output(stdout: Option<String>) -> AudioComponentObservation {
    match stdout.filter(|text| !text.trim().is_empty()) {
        Some(text) => {
            AudioComponentObservation::available(bounded_record_count(bounded_text(&text)))
        }
        None => AudioComponentObservation::unavailable(),
    }
}

/// Collect read-only audio observations from the conventional Linux audio
/// providers.  The commands intentionally enumerate no names or profiles:
/// only their bounded presence and record counts are published.
#[must_use]
pub fn collect_audio_observation<R: AudioCommandRunner>(runner: &R) -> AudioObservation {
    let pulse = runner.run_audio("pactl", &["info"]);
    let pipewire = runner.run_audio("pw-cli", &["ls", "Node"]);
    let wireplumber = runner.run_audio("wpctl", &["status"]);
    let alsa_ucm = runner.run_audio("alsaucm", &["listcards"]);
    let playback = runner.run_audio("aplay", &["-l"]);
    let capture = runner.run_audio("arecord", &["-l"]);

    let pulse_audio_compatibility = match pulse.filter(|text| !text.trim().is_empty()) {
        Some(text) => {
            let bounded = bounded_text(&text);
            let compatibility = if bounded.to_ascii_lowercase().contains("pulseaudio")
                || bounded.to_ascii_lowercase().contains("pipewire")
            {
                PulseAudioCompatibility::Compatible
            } else {
                PulseAudioCompatibility::Unknown
            };
            PulseAudioObservation {
                availability: AudioAvailability::Available,
                compatibility,
                observed_items: bounded_record_count(bounded),
            }
        }
        None => PulseAudioObservation {
            availability: AudioAvailability::Unavailable,
            compatibility: PulseAudioCompatibility::Unavailable,
            observed_items: None,
        },
    };
    let pipewire_graph = component_from_output(pipewire);
    let wireplumber_policy = component_from_output(wireplumber);
    let alsa_ucm_discovery = component_from_output(alsa_ucm);
    let playback = component_from_output(playback);
    let capture = component_from_output(capture);

    let component_count = [
        &pipewire_graph,
        &wireplumber_policy,
        &alsa_ucm_discovery,
        &playback,
        &capture,
    ]
    .iter()
    .filter(|component| component.availability == AudioAvailability::Available)
    .count();
    let available_count = component_count
        + usize::from(
            pulse_audio_compatibility.availability == AudioAvailability::Available,
        );

    // Recovery is deliberately a conservative provider-availability signal:
    // it is available only when at least one provider produced evidence.  It
    // does not claim that a recovery action succeeded or that audio is healthy.
    let recovery = if available_count > 0 {
        AudioComponentObservation::available(Some(available_count as u16))
    } else {
        AudioComponentObservation::unavailable()
    };

    AudioObservation {
        availability: if available_count > 0 {
            AudioAvailability::Available
        } else {
            AudioAvailability::Unavailable
        },
        pulse_audio_compatibility,
        pipewire_graph,
        wireplumber_policy,
        alsa_ucm_discovery,
        playback,
        capture,
        recovery,
    }
}

/// Add the typed audio observation to an existing mesh-status `network`
/// object.  Existing network facts are preserved and missing providers are
/// represented as `unavailable`, never as fabricated healthy values.
pub fn merge_audio_observation(network: &mut Map<String, Value>, observation: &AudioObservation) {
    if let Ok(value) = serde_json::to_value(observation) {
        network.insert("audio".to_string(), value);
    }
}

fn split_terse_fields(line: &str) -> Vec<String> {
    let sentinel = '\u{0}';
    line.replace("\\:", &sentinel.to_string())
        .split(':')
        .map(|field| field.replace(sentinel, ":"))
        .collect()
}

fn provider_for_nmcli_type(kind: &str) -> Option<NetworkProvider> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "wifi" | "wlan" => Some(NetworkProvider::Wifi),
        "ethernet" | "802-3-ethernet" => Some(NetworkProvider::Ethernet),
        "gsm" | "cdma" | "wwan" => Some(NetworkProvider::Cellular),
        _ => None,
    }
}

fn state_from_nmcli(raw: &str) -> ProviderLinkState {
    let state = raw.trim().to_ascii_lowercase();
    if state.contains("disconnected") || state.contains("deactivating") {
        ProviderLinkState::Disconnected
    } else if state.starts_with("100") || state.contains("connected") {
        ProviderLinkState::Connected
    } else if state.contains("connecting") || state.contains("config") {
        ProviderLinkState::Connecting
    } else if state.contains("unavailable") || state.contains("unmanaged") {
        ProviderLinkState::Unavailable
    } else {
        ProviderLinkState::Unknown
    }
}

/// Parse `nmcli -t -f DEVICE,TYPE,STATE device status`.
///
/// The command intentionally omits `CONNECTION`, `SSID`, and every profile
/// field.  Unknown device types and malformed rows are ignored.
#[must_use]
pub fn parse_nmcli_device_status(stdout: &str) -> Vec<ProviderLinkObservation> {
    let mut observations = Vec::new();
    for line in stdout.lines() {
        let fields = split_terse_fields(line);
        if fields.len() < 3 {
            continue;
        }
        let Some(provider) = provider_for_nmcli_type(&fields[1]) else {
            continue;
        };
        let interface = fields[0].trim();
        let observation =
            ProviderLinkObservation::new(provider, interface, state_from_nmcli(&fields[2]));
        if !observation.has_safe_interface_identifier() {
            continue;
        }
        observations.push(observation);
        if observations.len() == MAX_PROVIDER_LINKS {
            break;
        }
    }
    observations
}

fn state_from_modemmanager(raw: &str) -> ProviderLinkState {
    match raw.trim().to_ascii_lowercase().as_str() {
        "connected" | "registered" => ProviderLinkState::Connected,
        "connecting" | "registered-home" | "registered-roaming" => ProviderLinkState::Connecting,
        "failed" | "unknown" => ProviderLinkState::Unavailable,
        "disabled" | "locked" => ProviderLinkState::Disconnected,
        _ => ProviderLinkState::Unknown,
    }
}

/// Parse the deliberately narrow ModemManager key/value projection.
///
/// Only `modem.generic.state` and a sanitized `modem.generic.device` are
/// accepted.  APN, SIM, operator, bearer, and credential keys are ignored.
#[must_use]
pub fn parse_mmcli_status(stdout: &str) -> Option<ProviderLinkObservation> {
    let mut state = None;
    let mut interface = String::new();
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "modem.generic.state" => state = Some(state_from_modemmanager(value)),
            "modem.generic.device" => {
                let value = value.trim();
                let candidate = ProviderLinkObservation::new(
                    NetworkProvider::Cellular,
                    value,
                    ProviderLinkState::Unknown,
                );
                if candidate.has_safe_interface_identifier() && value.starts_with("/dev/") {
                    interface = candidate.interface;
                }
            }
            _ => {}
        }
    }
    state.map(|status| ProviderLinkObservation::new(NetworkProvider::Cellular, interface, status))
}

/// Collect safe NetworkManager and ModemManager provider/link observations.
///
/// Missing tools, permissions, or malformed provider output yield an empty or
/// partial result; mesh overlay, DNS, route, and lighthouse facts are not
/// queried or modified by this function.
#[must_use]
pub fn collect_provider_links<R: NetworkCommandRunner>(runner: &R) -> Vec<ProviderLinkObservation> {
    let mut observations = runner
        .run(
            "nmcli",
            &["-t", "-f", "DEVICE,TYPE,STATE", "device", "status"],
        )
        .map(|stdout| parse_nmcli_device_status(&stdout))
        .unwrap_or_default();

    if let Some(stdout) = runner.run(
        "mmcli",
        &[
            "--modem=0",
            "--output-keyvalue",
            "--output-fields=modem.generic.state,modem.generic.device",
        ],
    ) {
        if let Some(modem) = parse_mmcli_status(&stdout) {
            if !observations
                .iter()
                .any(|item| item.provider == NetworkProvider::Cellular)
            {
                observations.push(modem);
            }
        }
    }
    observations.truncate(MAX_PROVIDER_LINKS);
    observations
}

/// Add provider observations to an existing mesh-status `network` object.
///
/// This is additive: all existing overlay, DNS, route, gateway, cipher, and
/// lighthouse keys remain untouched.  The caller owns the final atomic
/// snapshot write; this helper only materializes the typed `interfaces` key.
pub fn merge_provider_links(
    network: &mut Map<String, Value>,
    observations: &[ProviderLinkObservation],
) {
    let bounded = observations
        .iter()
        .filter(|observation| observation.has_safe_interface_identifier())
        .take(MAX_PROVIDER_LINKS);
    let interfaces = bounded
        .filter_map(|observation| serde_json::to_value(observation).ok())
        .collect::<Vec<_>>();
    network.insert("interfaces".to_string(), Value::Array(interfaces));
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeRunner {
        nmcli: Option<&'static str>,
        mmcli: Option<&'static str>,
    }

    struct QueryShapeRunner;

    impl NetworkCommandRunner for QueryShapeRunner {
        fn run(&self, program: &str, args: &[&str]) -> Option<String> {
            let joined = args.join(" ").to_ascii_lowercase();
            for forbidden in ["connection", "profile", "ssid", "apn", "password", "psk"] {
                assert!(
                    !joined.contains(forbidden),
                    "queried forbidden field {forbidden}"
                );
            }
            match program {
                "nmcli" => {
                    assert_eq!(args, ["-t", "-f", "DEVICE,TYPE,STATE", "device", "status"]);
                    Some("wlan0:wifi:connected\n".to_string())
                }
                "mmcli" => {
                    assert_eq!(
                        args,
                        [
                            "--modem=0",
                            "--output-keyvalue",
                            "--output-fields=modem.generic.state,modem.generic.device",
                        ]
                    );
                    Some(
                        "modem.generic.state=disconnected\nmodem.generic.device=/dev/cdc-wdm0\n"
                            .to_string(),
                    )
                }
                _ => panic!("unexpected provider program {program}"),
            }
        }
    }

    impl NetworkCommandRunner for FakeRunner {
        fn run(&self, program: &str, _args: &[&str]) -> Option<String> {
            match program {
                "nmcli" => self.nmcli.map(str::to_owned),
                "mmcli" => self.mmcli.map(str::to_owned),
                _ => None,
            }
        }
    }

    #[test]
    fn nmcli_parser_keeps_only_typed_link_facts() {
        let observations = parse_nmcli_device_status(
            "wlan0:wifi:connected:Secret SSID\neth0:ethernet:disconnected:Office\nwwan0:gsm:connecting:carrier\nlo:loopback:connected:\n",
        );
        assert_eq!(observations.len(), 3);
        assert_eq!(observations[0].provider, NetworkProvider::Wifi);
        assert_eq!(observations[0].status, ProviderLinkState::Connected);
        assert!(observations[0].up);
        assert_eq!(observations[1].status, ProviderLinkState::Disconnected);
        assert!(!observations[1].up);
        assert_eq!(observations[2].provider, NetworkProvider::Cellular);
        let wire = serde_json::to_string(&observations).unwrap();
        for secret in ["Secret SSID", "Office", "carrier", "password", "psk", "apn"] {
            assert!(!wire
                .to_ascii_lowercase()
                .contains(&secret.to_ascii_lowercase()));
        }
    }

    #[test]
    fn parsers_and_merge_reject_credential_shaped_identifiers() {
        let observations = parse_nmcli_device_status(
            "wlan0:wifi:connected\noffice wifi:wifi:connected\nuser@example.com:ethernet:connected\npassword=hunter2:gsm:connected\n",
        );
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].interface, "wlan0");

        let modem = parse_mmcli_status(
            "modem.generic.state=connected\nmodem.generic.device=/dev/mapper/private\n",
        )
        .unwrap();
        assert!(modem.interface.is_empty());

        let malformed = ProviderLinkObservation::new(
            NetworkProvider::Wifi,
            "secret profile",
            ProviderLinkState::Connected,
        );
        let mut network = Map::new();
        merge_provider_links(&mut network, &[malformed]);
        assert!(network["interfaces"].as_array().unwrap().is_empty());
    }

    #[test]
    fn modem_parser_drops_apn_and_sim_fields() {
        let observation = parse_mmcli_status(
            "modem.generic.state=connected\nmodem.generic.device=/dev/cdc-wdm0\nmodem.3gpp.operator-name=private\nmodem.generic.access-technologies=lte\nmodem.3gpp.profile-apn=secret.apn\n",
        )
        .unwrap();
        assert_eq!(observation.provider, NetworkProvider::Cellular);
        assert_eq!(observation.interface, "/dev/cdc-wdm0");
        let wire = serde_json::to_string(&observation).unwrap();
        assert!(!wire.contains("secret.apn"));
        assert!(!wire.contains("operator"));
    }

    #[test]
    fn collection_and_merge_are_additive_and_bounded() {
        let runner = FakeRunner {
            nmcli: Some("eth0:ethernet:connected:profile\n"),
            mmcli: Some("modem.generic.state=connected\nmodem.generic.device=/dev/cdc-wdm0\n"),
        };
        let observations = collect_provider_links(&runner);
        assert_eq!(observations.len(), 2);
        let mut network = Map::new();
        network.insert("overlay_if".into(), Value::String("nebula1".into()));
        network.insert("routes".into(), Value::Array(Vec::new()));
        network.insert("lighthouse_ips".into(), serde_json::json!(["10.42.0.1"]));
        merge_provider_links(&mut network, &observations);
        assert_eq!(network["overlay_if"], "nebula1");
        assert_eq!(network["lighthouse_ips"], serde_json::json!(["10.42.0.1"]));
        assert_eq!(network["interfaces"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn missing_provider_tools_degrade_to_empty_without_fabrication() {
        let observations = collect_provider_links(&FakeRunner {
            nmcli: None,
            mmcli: None,
        });
        assert!(observations.is_empty());
    }

    #[test]
    fn collection_queries_only_bounded_noncredential_fields() {
        let observations = collect_provider_links(&QueryShapeRunner);
        assert_eq!(observations.len(), 2);
    }

    struct AudioRunner {
        values: std::collections::BTreeMap<&'static str, Option<&'static str>>,
    }

    impl AudioCommandRunner for AudioRunner {
        fn run_audio(&self, program: &str, args: &[&str]) -> Option<String> {
            assert!(
                args.iter().all(|arg| {
                    !arg.contains("profile")
                        && !arg.contains("password")
                        && !arg.contains("secret")
                }),
                "audio probe requested credential-shaped arguments"
            );
            self.values
                .get(program)
                .copied()
                .flatten()
                .map(str::to_owned)
        }
    }

    #[test]
    fn audio_publication_has_typed_provider_fields_and_is_additive() {
        let runner = AudioRunner {
            values: [
                ("pactl", Some("Server Name: PulseAudio (on PipeWire)\n")),
                ("pw-cli", Some("id 42, type PipeWire:Interface:Node\nid 43\n")),
                ("wpctl", Some("Audio\n ├─ Devices:\n")),
                ("alsaucm", Some("hw:0\nhw:1\n")),
                ("aplay", Some("card 0: HDMI\ncard 1: Analog\n")),
                ("arecord", Some("card 1: Analog\n")),
            ]
            .into_iter()
            .collect(),
        };
        let observation = collect_audio_observation(&runner);
        assert_eq!(observation.availability, AudioAvailability::Available);
        assert_eq!(
            observation.pulse_audio_compatibility.availability,
            AudioAvailability::Available
        );
        assert_eq!(
            observation.pulse_audio_compatibility.compatibility,
            PulseAudioCompatibility::Compatible
        );
        assert_eq!(observation.pipewire_graph.observed_items, Some(2));
        assert_eq!(observation.playback.observed_items, Some(2));
        assert_eq!(observation.capture.observed_items, Some(1));
        assert_eq!(observation.recovery.observed_items, Some(6));

        let mut network = Map::new();
        network.insert("overlay_if".into(), Value::String("nebula1".into()));
        merge_audio_observation(&mut network, &observation);
        assert_eq!(network["overlay_if"], "nebula1");
        assert_eq!(network["audio"]["pipewire_graph"]["availability"], "available");
        assert_eq!(network["audio"]["recovery"]["observed_items"], 6);
    }

    #[test]
    fn missing_audio_providers_are_unavailable_without_fabricated_health() {
        let observation = collect_audio_observation(&AudioRunner {
            values: [
                ("pactl", None),
                ("pw-cli", None),
                ("wpctl", None),
                ("alsaucm", None),
                ("aplay", None),
                ("arecord", None),
            ]
            .into_iter()
            .collect(),
        });
        assert_eq!(observation.availability, AudioAvailability::Unavailable);
        assert_eq!(observation.recovery.availability, AudioAvailability::Unavailable);
        let wire = serde_json::to_value(&observation).unwrap();
        assert_eq!(wire["playback"]["availability"], "unavailable");
        assert_eq!(wire["capture"]["availability"], "unavailable");
        assert!(wire["playback"].get("observed_items").is_none());
        assert!(wire["capture"].get("observed_items").is_none());
    }

    #[test]
    fn audio_command_output_is_bounded_before_publication() {
        let large = Box::leak("x\n".repeat(MAX_STATUS_COMMAND_BYTES * 2).into_boxed_str());
        let runner = AudioRunner {
            values: [
                ("pactl", Some("Server Name: PulseAudio\n")),
                ("pw-cli", Some("node\n")),
                ("wpctl", Some("policy\n")),
                ("alsaucm", Some("card\n")),
                ("aplay", Some(large)),
                ("arecord", None),
            ]
            .into_iter()
            .collect(),
        };
        let observation = collect_audio_observation(&runner);
        assert_eq!(observation.playback.availability, AudioAvailability::Available);
        assert!(observation.playback.observed_items.is_some());
        let wire = serde_json::to_vec(&observation).unwrap();
        assert!(wire.len() < MAX_STATUS_COMMAND_BYTES);
    }
}
