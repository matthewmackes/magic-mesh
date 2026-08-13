//! Credential-free audio readiness provider for WL-UX-011.
//!
//! PipeWire owns the graph, WirePlumber owns policy, and the kernel owns the
//! physical sound-card inventory.  This projection publishes only a typed
//! readiness and bounded counts; command output, device labels, usernames,
//! profiles, and routes never cross the provider boundary.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OBSERVATION_BYTES: usize = 64 * 1024;
const MAX_AUDIO_OBJECTS: usize = 256;

/// Truthful readiness of the node's audio provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioReadiness {
    Ready,
    Disconnected,
    Disabled,
    Unknown,
}

/// Bounded, credential-free audio-provider projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSnapshot {
    pub schema_version: u16,
    pub node_id: String,
    pub observed_unix_ms: u64,
    pub readiness: AudioReadiness,
    pub kernel_cards: u16,
    pub pipewire_audio_nodes: u16,
    pub reason: String,
}

fn parse_pipewire_nodes(raw: &str) -> Option<usize> {
    if raw.len() > MAX_OBSERVATION_BYTES || raw.contains('\0') {
        return None;
    }
    let count = raw
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.starts_with("media.class")
                && line
                    .split_once('=')
                    .is_some_and(|(_, value)| value.trim().trim_matches('"').starts_with("Audio/"))
        })
        .count();
    (count <= MAX_AUDIO_OBJECTS).then_some(count)
}

fn parse_wireplumber_status(raw: &str) -> Option<()> {
    if raw.is_empty()
        || raw.len() > MAX_OBSERVATION_BYTES
        || raw.contains('\0')
        || !raw.lines().any(|line| line.trim() == "Audio")
        || !raw.lines().any(|line| line.contains("Sinks:"))
        || !raw.lines().any(|line| line.contains("Sources:"))
    {
        return None;
    }
    Some(())
}

fn classify(
    pipewire: Option<&str>,
    wireplumber: Option<&str>,
    kernel_cards: Option<Vec<String>>,
) -> (AudioReadiness, usize, usize, &'static str) {
    let Some(pipewire_nodes) = pipewire.and_then(parse_pipewire_nodes) else {
        return (
            AudioReadiness::Unknown,
            0,
            0,
            "PipeWire graph unavailable or malformed",
        );
    };
    if wireplumber.and_then(parse_wireplumber_status).is_none() {
        return (
            AudioReadiness::Unknown,
            0,
            0,
            "WirePlumber policy unavailable or malformed",
        );
    }
    let Some(mut cards) = kernel_cards else {
        return (
            AudioReadiness::Unknown,
            0,
            0,
            "kernel audio inventory unavailable",
        );
    };
    if cards.len() > MAX_AUDIO_OBJECTS
        || cards.iter().any(|card| {
            !card.strip_prefix("card").is_some_and(|index| {
                !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
    {
        return (
            AudioReadiness::Unknown,
            0,
            0,
            "kernel audio inventory malformed",
        );
    }
    cards.sort_unstable();
    if cards.windows(2).any(|pair| pair[0] == pair[1]) {
        return (
            AudioReadiness::Unknown,
            0,
            0,
            "kernel audio inventory contains duplicate identities",
        );
    }
    if cards.is_empty() && pipewire_nodes > 0 {
        return (
            AudioReadiness::Unknown,
            0,
            0,
            "PipeWire graph contradicts kernel audio inventory",
        );
    }
    if cards.is_empty() {
        return (
            AudioReadiness::Disabled,
            0,
            0,
            "no kernel audio hardware is exposed",
        );
    }
    if pipewire_nodes == 0 {
        return (
            AudioReadiness::Disconnected,
            cards.len(),
            0,
            "audio hardware is present without a PipeWire audio node",
        );
    }
    (
        AudioReadiness::Ready,
        cards.len(),
        pipewire_nodes,
        "PipeWire, WirePlumber, and kernel audio facts agree",
    )
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let uid_output = std::process::Command::new("id")
        .args(["-u", "mm"])
        .output()
        .ok()?;
    if !uid_output.status.success() {
        return None;
    }
    let uid = String::from_utf8(uid_output.stdout).ok()?;
    let uid = uid.trim();
    if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let runtime = format!("XDG_RUNTIME_DIR=/run/user/{uid}");
    let mut command = std::process::Command::new("runuser");
    command.args(["-u", "mm", "--", "env", runtime.as_str(), program]);
    command.args(args);
    let output = super::proc::output_with_timeout(command, COMMAND_TIMEOUT).ok()?;
    if !output.status.success() || output.stdout.len() > MAX_OBSERVATION_BYTES {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn kernel_cards(root: &Path) -> std::io::Result<Vec<String>> {
    let mut cards = std::fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("card"))
        .collect::<Vec<_>>();
    cards.sort_unstable();
    if cards.len() > MAX_AUDIO_OBJECTS {
        cards.truncate(MAX_AUDIO_OBJECTS + 1);
    }
    Ok(cards)
}

/// Gather current state without reading routes, profiles, labels, or secrets.
#[must_use]
pub fn gather(node_id: &str) -> AudioSnapshot {
    let pipewire = run("pw-cli", &["ls", "Node"]);
    let wireplumber = run("wpctl", &["status"]);
    let cards = kernel_cards(Path::new("/sys/class/sound")).ok();
    let (readiness, card_count, node_count, reason) =
        classify(pipewire.as_deref(), wireplumber.as_deref(), cards);
    AudioSnapshot {
        schema_version: 1,
        node_id: node_id.to_owned(),
        observed_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        readiness,
        kernel_cards: card_count.try_into().unwrap_or(u16::MAX),
        pipewire_audio_nodes: node_count.try_into().unwrap_or(u16::MAX),
        reason: reason.to_owned(),
    }
}

#[must_use]
pub fn snapshot_path(workgroup_root: &Path, node_id: &str) -> PathBuf {
    workgroup_root
        .join("audio-provider")
        .join(format!("{node_id}.json"))
}

/// Publish one current observation. This grants no mutation authority.
pub fn publish_system(workgroup_root: &Path, node_id: &str) -> std::io::Result<PathBuf> {
    if node_id.is_empty()
        || node_id.len() > 128
        || !node_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(std::io::Error::other(
            "invalid audio-provider node identity",
        ));
    }
    let path = snapshot_path(workgroup_root, node_id);
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("audio snapshot has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".{node_id}.json.tmp"));
    let bytes = serde_json::to_vec_pretty(&gather(node_id)).map_err(std::io::Error::other)?;
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WP: &str = "PipeWire 'pipewire-0'\nAudio\n ├─ Sinks:\n └─ Sources:\n";

    #[test]
    fn hostile_audio_observations_fail_unknown_without_leaking_provider_data() {
        let oversized = "x".repeat(MAX_OBSERVATION_BYTES + 1);
        let cases = [
            classify(
                Some("media.class = \"Audio/Sink\"\n"),
                Some(WP),
                Some(vec![]),
            ),
            classify(
                Some("media.class = \"Audio/Sink\"\n"),
                Some("secret route"),
                Some(vec!["card0".into()]),
            ),
            classify(Some(&oversized), Some(WP), Some(vec!["card0".into()])),
            classify(
                Some("media.class = \"Audio/Sink\"\n"),
                Some(WP),
                Some(vec!["card0".into(), "card0".into()]),
            ),
            classify(
                Some("media.class = \"Audio/Sink\"\n"),
                Some(WP),
                Some(vec!["card-substituted".into()]),
            ),
        ];
        assert!(cases.iter().all(|case| case.0 == AudioReadiness::Unknown));
        for case in cases {
            assert_eq!(case.1, 0);
            assert_eq!(case.2, 0);
            assert!(!case.3.contains("secret"));
        }
    }

    #[test]
    fn audio_readiness_distinguishes_ready_disconnected_and_disabled() {
        let ready = classify(
            Some("media.class = \"Audio/Sink\"\nmedia.class = \"Video/Source\"\n"),
            Some(WP),
            Some(vec!["card0".into()]),
        );
        assert_eq!(ready.0, AudioReadiness::Ready);
        assert_eq!((ready.1, ready.2), (1, 1));

        let disconnected = classify(Some(""), Some(WP), Some(vec!["card0".into()]));
        assert_eq!(disconnected.0, AudioReadiness::Disconnected);

        let disabled = classify(Some(""), Some(WP), Some(vec![]));
        assert_eq!(disabled.0, AudioReadiness::Disabled);
    }
}
