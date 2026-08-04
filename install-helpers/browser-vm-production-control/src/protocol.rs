//! Strict wire and state contracts shared by host hook and guest controller.

use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Playback,
    Capture,
}

impl Operation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Playback => "playback",
            Self::Capture => "capture",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobSpec {
    pub schema_version: u8,
    pub job_id: String,
    pub operation: Operation,
    pub phase: String,
    pub tone_hz: u32,
    pub duration_seconds: u32,
    pub source_commit: String,
    pub image_digest: String,
    pub transport: String,
}

impl JobSpec {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "unsupported job schema");
        validate_job_id(&self.job_id)?;
        ensure!(
            matches!(self.phase.as_str(), "before-recovery" | "after-recovery"),
            "invalid probe phase"
        );
        ensure!(
            (20..=20_000).contains(&self.tone_hz),
            "tone lies outside the admitted audible range"
        );
        match self.operation {
            Operation::Playback => ensure!(
                self.duration_seconds == 8,
                "playback duration must match collector contract"
            ),
            Operation::Capture => ensure!(
                self.duration_seconds == 2,
                "capture duration must match collector contract"
            ),
        }
        ensure!(
            self.source_commit.len() == 40
                && self
                    .source_commit
                    .bytes()
                    .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
                && !self.source_commit.bytes().all(|value| value == b'0'),
            "source commit is not a full non-null lowercase SHA"
        );
        ensure!(
            self.image_digest.len() == 71
                && self.image_digest.starts_with("sha256:")
                && self.image_digest[7..]
                    .bytes()
                    .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
                && !self.image_digest[7..].bytes().all(|value| value == b'0'),
            "image digest is not a full non-null lowercase SHA-256"
        );
        ensure!(self.transport == "rdp", "this controller admits RDP only");
        Ok(())
    }
}

pub fn validate_job_id(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("job id must be 256-bit lowercase hex");
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrowserEvent {
    PageLoaded,
    PlaybackArmed {
        is_trusted: bool,
        user_activation: bool,
        audio_context_state: String,
        sample_rate: u32,
        channels: u8,
    },
    PlaybackStarted {
        is_trusted: bool,
        user_activation: bool,
        audio_context_state: String,
        sample_rate: u32,
        channels: u8,
    },
    PlaybackCompleted {
        oscillator_ended: bool,
        elapsed_ms: u64,
    },
    CaptureReady {
        is_trusted: bool,
        user_activation: bool,
        audio_context_state: String,
        media_track_kind: String,
        media_track_state: String,
        sample_rate: u32,
        channels: u8,
    },
    CaptureStarted {
        is_trusted: bool,
        user_activation: bool,
        audio_context_state: String,
        sample_rate: u32,
        channels: u8,
    },
    CaptureCompleted {
        frames: u32,
        sample_rate: u32,
        channels: u8,
        elapsed_ms: u64,
    },
    Released,
    Failed {
        reason_code: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobStatus {
    pub schema_version: u8,
    pub job_id: String,
    pub state: String,
    pub user_gesture_observed: bool,
    pub browser_api: String,
    pub channels: u8,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApiStatus {
    pub schema_version: u8,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRequest {
    pub schema_version: u8,
    pub command: String,
}

#[cfg(test)]
mod tests {
    use super::{JobSpec, Operation};

    fn valid() -> JobSpec {
        JobSpec {
            schema_version: 1,
            job_id: "a".repeat(64),
            operation: Operation::Playback,
            phase: "before-recovery".to_owned(),
            tone_hz: 523,
            duration_seconds: 8,
            source_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            image_digest: format!("sha256:{}", "a".repeat(64)),
            transport: "rdp".to_owned(),
        }
    }

    #[test]
    fn exact_collector_job_is_admitted() {
        assert!(valid().validate().is_ok());
    }

    #[test]
    fn sunshine_cannot_be_mislabeled_as_rdp_control() {
        let mut value = valid();
        value.transport = "sunshine".to_owned();
        assert!(value.validate().is_err());
    }
}
