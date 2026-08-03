//! Typed RDP audio capability and the bounded host PipeWire sink.
//!
//! The RDPSND protocol processor is supplied by `ironrdp-rdpsnd` when the
//! `live-connect` feature is enabled.  This module deliberately advertises one
//! bounded PCM format only: that lets the sink be configured before the RDP
//! handshake and makes a server format mismatch fail closed instead of
//! sending samples to a sink with the wrong interpretation.

use std::fmt;

/// The exact reason a connection cannot claim an audio path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdpAudioUnsupportedReason {
    /// No usable `pw-cat`/`pw-playback` process could be connected to stdin.
    NoHostPlaybackSink,
    /// RDPSND selected a server format that is not the one advertised here.
    NoSharedFormat,
    /// The PipeWire process rejected or lost the PCM stream.
    SinkWriteFailed,
    /// The live-connect feature is not compiled into this build.
    LiveConnectDisabled,
}

impl RdpAudioUnsupportedReason {
    /// Stable, credential-free operator diagnostic.
    #[must_use]
    pub const fn diagnostic(self) -> &'static str {
        match self {
            Self::NoHostPlaybackSink => {
                "no usable host PipeWire playback sink (pw-cat/pw-playback)"
            }
            Self::NoSharedFormat => "RDPSND did not select the advertised shared PCM format",
            Self::SinkWriteFailed => "host PipeWire playback sink rejected or lost PCM input",
            Self::LiveConnectDisabled => "mde-vdi-rdp was built without the live-connect feature",
        }
    }
}

/// The one PCM format this client advertises to RDPSND.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RdpPcmFormat {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Interleaved channel count.
    pub channels: u16,
    /// Bits per sample.
    pub bits_per_sample: u16,
}

impl RdpPcmFormat {
    #[cfg(feature = "live-connect")]
    pub(crate) const STEREO_S16_48K: Self = Self {
        sample_rate: 48_000,
        channels: 2,
        bits_per_sample: 16,
    };

    /// Bytes per interleaved sample frame.
    #[must_use]
    pub fn bytes_per_frame(self) -> u32 {
        u32::from(self.channels) * (u32::from(self.bits_per_sample) / 8)
    }
}

/// Bounded, non-sensitive audio counters.  Values saturate rather than
/// wrapping so a long-lived session cannot turn diagnostics into nonsense.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RdpAudioStats {
    /// RDPSND WAVE/WAVE2 messages observed by the handler.
    pub waves_received: u64,
    /// Bytes copied from accepted RDP wave messages into the bounded queue.
    pub pcm_bytes_queued: u64,
    /// Bytes accepted by the PipeWire writer thread.
    pub pcm_bytes_written: u64,
    /// Wave messages whose server format did not match the advertised PCM.
    pub format_mismatches: u64,
    /// Wave messages dropped because a bounded queue was full or a message was
    /// larger than the per-message ceiling.
    pub waves_dropped: u64,
    /// Number of sink write/process failures.
    pub sink_failures: u64,
    /// Whether at least one wave was proven to use the advertised format.
    pub shared_format_selected: bool,
}

/// What this RDP connection can honestly claim about audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdpAudioCapability {
    /// No usable audio endpoint or no compatible RDPSND format exists.
    Unsupported {
        /// The bounded reason for refusing an audio claim.
        reason: RdpAudioUnsupportedReason,
    },
    /// The RDPSND endpoint is attached and the host sink is wired, but no
    /// accepted PCM wave has been observed yet.  This is endpoint evidence,
    /// not live playback proof.
    EndpointWired {
        /// The only format offered to the server.
        format: RdpPcmFormat,
    },
    /// PCM has passed RDPSND format validation and was delivered to the
    /// PipeWire playback process.  A speaker/mixer capture is still required
    /// before an external acceptance verifier may claim audible playback.
    PcmStreaming {
        /// The validated format delivered to the sink.
        format: RdpPcmFormat,
    },
}

impl RdpAudioCapability {
    /// Conservative compile-time baseline. The live connection's
    /// `PreparedAudio` probe is authoritative when `live-connect` is enabled.
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(feature = "live-connect")]
        {
            return Self::Unsupported {
                reason: RdpAudioUnsupportedReason::NoHostPlaybackSink,
            };
        }

        #[cfg(not(feature = "live-connect"))]
        Self::Unsupported {
            reason: RdpAudioUnsupportedReason::LiveConnectDisabled,
        }
    }

    /// Whether this value alone proves audible host playback.  Endpoint wiring
    /// and PCM delivery are intentionally distinct from a capture-based live
    /// proof, so this remains fail-closed for all in-process capabilities.
    #[must_use]
    pub const fn can_claim_acceptance(self) -> bool {
        false
    }

    /// Return the single bounded diagnostic, if this is unsupported.
    #[must_use]
    pub const fn unsupported_reason(self) -> Option<RdpAudioUnsupportedReason> {
        match self {
            Self::Unsupported { reason } => Some(reason),
            Self::EndpointWired { .. } | Self::PcmStreaming { .. } => None,
        }
    }

    /// Compact operator-facing diagnostic without endpoint data or payloads.
    #[must_use]
    pub fn diagnostic(self) -> String {
        match self.unsupported_reason() {
            Some(reason) => reason.diagnostic().to_owned(),
            None => match self {
                Self::EndpointWired { .. } => {
                    "RDPSND endpoint wired; awaiting validated PCM".to_owned()
                }
                Self::PcmStreaming { .. } => {
                    "validated PCM delivered to host PipeWire playback process".to_owned()
                }
                Self::Unsupported { reason } => reason.diagnostic().to_owned(),
            },
        }
    }
}

impl fmt::Display for RdpAudioUnsupportedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.diagnostic())
    }
}

#[cfg(feature = "live-connect")]
mod live {
    use super::{RdpAudioCapability, RdpAudioStats, RdpAudioUnsupportedReason, RdpPcmFormat};
    use ironrdp_rdpsnd::client::RdpsndClientHandler;
    use ironrdp_rdpsnd::pdu::{self, AudioFormat, PitchPdu, VolumePdu};
    use std::borrow::Cow;
    use std::collections::VecDeque;
    use std::io::Write;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{self, SyncSender, TrySendError};
    use std::sync::{Arc, Mutex};
    use std::thread;

    const MAX_PENDING_WAVES: usize = 8;
    const MAX_WAVE_BYTES: usize = 1_048_576;
    const MAX_AUDIO_SINK_QUEUE: usize = 8;

    #[derive(Debug)]
    pub(crate) struct PendingWave {
        pub(crate) format_no: usize,
        pub(crate) data: Vec<u8>,
    }

    #[derive(Debug, Default)]
    struct AudioState {
        stats: RdpAudioStats,
        pending: VecDeque<PendingWave>,
        sink: Option<SyncSender<Vec<u8>>>,
    }

    type SharedAudioState = Arc<Mutex<AudioState>>;

    fn lock(state: &SharedAudioState) -> std::sync::MutexGuard<'_, AudioState> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A prepared audio endpoint. The sender is bounded; the RDP pump never
    /// blocks on a slow host sink and drops excess waves with a counter.
    pub(crate) struct PreparedAudio {
        state: SharedAudioState,
        pub(crate) handler: Option<PipeWireRdpsndHandler>,
        pub(crate) initial_capability: RdpAudioCapability,
        _writer: Option<thread::JoinHandle<()>>,
    }

    impl PreparedAudio {
        pub(crate) fn new() -> Self {
            let state = Arc::new(Mutex::new(AudioState::default()));
            match spawn_pipewire_sink(RdpPcmFormat::STEREO_S16_48K, Arc::clone(&state)) {
                Ok((sender, writer)) => {
                    lock(&state).sink = Some(sender);
                    Self {
                        state: Arc::clone(&state),
                        handler: Some(PipeWireRdpsndHandler::new(Arc::clone(&state))),
                        initial_capability: RdpAudioCapability::EndpointWired {
                            format: RdpPcmFormat::STEREO_S16_48K,
                        },
                        _writer: Some(writer),
                    }
                }
                Err(reason) => Self {
                    state,
                    handler: None,
                    initial_capability: RdpAudioCapability::Unsupported { reason },
                    _writer: None,
                },
            }
        }

        pub(crate) fn stats(&self) -> RdpAudioStats {
            lock(&self.state).stats
        }

        pub(crate) fn take_pending(&self) -> Vec<PendingWave> {
            lock(&self.state).pending.drain(..).collect()
        }

        pub(crate) fn reject_shared_format(&self) {
            let mut state = lock(&self.state);
            state.stats.format_mismatches = state.stats.format_mismatches.saturating_add(1);
        }

        pub(crate) fn deliver_pcm(&self, data: Vec<u8>) {
            let sender = lock(&self.state).sink.clone();
            let Some(sender) = sender else {
                let mut state = lock(&self.state);
                state.stats.sink_failures = state.stats.sink_failures.saturating_add(1);
                return;
            };
            let len = u64::try_from(data.len()).unwrap_or(u64::MAX);
            match sender.try_send(data) {
                Ok(()) => {
                    let mut state = lock(&self.state);
                    state.stats.pcm_bytes_queued = state.stats.pcm_bytes_queued.saturating_add(len);
                    state.stats.shared_format_selected = true;
                }
                Err(TrySendError::Full(_)) => {
                    let mut state = lock(&self.state);
                    state.stats.waves_dropped = state.stats.waves_dropped.saturating_add(1);
                }
                Err(TrySendError::Disconnected(_)) => {
                    let mut state = lock(&self.state);
                    state.stats.sink_failures = state.stats.sink_failures.saturating_add(1);
                }
            }
        }

        pub(crate) fn capability(&self) -> RdpAudioCapability {
            let state = lock(&self.state);
            if state.sink.is_none() {
                return RdpAudioCapability::Unsupported {
                    reason: RdpAudioUnsupportedReason::NoHostPlaybackSink,
                };
            }
            if state.stats.format_mismatches > 0 {
                return RdpAudioCapability::Unsupported {
                    reason: RdpAudioUnsupportedReason::NoSharedFormat,
                };
            }
            if state.stats.sink_failures > 0 {
                return RdpAudioCapability::Unsupported {
                    reason: RdpAudioUnsupportedReason::SinkWriteFailed,
                };
            }
            if state.stats.pcm_bytes_written > 0 {
                RdpAudioCapability::PcmStreaming {
                    format: RdpPcmFormat::STEREO_S16_48K,
                }
            } else {
                RdpAudioCapability::EndpointWired {
                    format: RdpPcmFormat::STEREO_S16_48K,
                }
            }
        }
    }

    impl Drop for PreparedAudio {
        fn drop(&mut self) {
            lock(&self.state).sink.take();
            // The writer has a bounded receive loop and owns the child. Do not
            // join from the RDP teardown path: a broken PipeWire FIFO must not
            // turn disconnect into an unbounded wait.
            let _ = self._writer.take();
        }
    }

    #[derive(Debug)]
    pub(crate) struct PipeWireRdpsndHandler {
        state: SharedAudioState,
        formats: Vec<AudioFormat>,
    }

    impl PipeWireRdpsndHandler {
        fn new(state: SharedAudioState) -> Self {
            Self {
                state,
                formats: vec![supported_format()],
            }
        }
    }

    pub(crate) fn supported_format() -> AudioFormat {
        AudioFormat {
            format: pdu::WaveFormat::PCM,
            n_channels: RdpPcmFormat::STEREO_S16_48K.channels,
            n_samples_per_sec: RdpPcmFormat::STEREO_S16_48K.sample_rate,
            n_avg_bytes_per_sec: 192_000,
            n_block_align: 4,
            bits_per_sample: RdpPcmFormat::STEREO_S16_48K.bits_per_sample,
            data: None,
        }
    }

    impl RdpsndClientHandler for PipeWireRdpsndHandler {
        fn get_formats(&self) -> &[AudioFormat] {
            &self.formats
        }

        fn wave(&mut self, format_no: usize, _ts: u32, data: Cow<'_, [u8]>) {
            let mut state = lock(&self.state);
            state.stats.waves_received = state.stats.waves_received.saturating_add(1);
            if data.len() > MAX_WAVE_BYTES || state.pending.len() >= MAX_PENDING_WAVES {
                state.stats.waves_dropped = state.stats.waves_dropped.saturating_add(1);
                return;
            }
            state.pending.push_back(PendingWave {
                format_no,
                data: data.into_owned(),
            });
        }

        fn set_volume(&mut self, _volume: VolumePdu) {}

        fn set_pitch(&mut self, _pitch: PitchPdu) {}

        fn close(&mut self) {}
    }

    fn spawn_pipewire_sink(
        format: RdpPcmFormat,
        state: SharedAudioState,
    ) -> Result<(SyncSender<Vec<u8>>, thread::JoinHandle<()>), RdpAudioUnsupportedReason> {
        let rate = format.sample_rate.to_string();
        let channels = format.channels.to_string();
        let candidates = ["pw-cat", "pw-playback"];
        for program in candidates {
            let mut command = Command::new(program);
            if program == "pw-cat" {
                command.arg("--playback");
            }
            let child = command
                .args([
                    "--raw",
                    "--format",
                    "s16",
                    "--rate",
                    rate.as_str(),
                    "--channels",
                    channels.as_str(),
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            let Ok(mut child) = child else {
                continue;
            };
            if child.try_wait().ok().flatten().is_some() {
                continue;
            }
            let Some(stdin) = child.stdin.take() else {
                let _ = child.kill();
                continue;
            };
            let (sender, receiver) = mpsc::sync_channel(MAX_AUDIO_SINK_QUEUE);
            let writer = thread::Builder::new()
                .name("rdp-pipewire-audio".to_owned())
                .spawn(move || write_pipewire_audio(child, stdin, receiver, state))
                .map_err(|_| RdpAudioUnsupportedReason::NoHostPlaybackSink)?;
            return Ok((sender, writer));
        }
        Err(RdpAudioUnsupportedReason::NoHostPlaybackSink)
    }

    fn write_pipewire_audio(
        mut child: Child,
        mut stdin: impl Write,
        receiver: mpsc::Receiver<Vec<u8>>,
        state: SharedAudioState,
    ) {
        while let Ok(data) = receiver.recv() {
            if stdin.write_all(&data).is_err() {
                let mut shared = lock(&state);
                shared.stats.sink_failures = shared.stats.sink_failures.saturating_add(1);
                break;
            }
            let mut shared = lock(&state);
            shared.stats.pcm_bytes_written = shared
                .stats
                .pcm_bytes_written
                .saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(test)]
    mod tests {
        use super::{
            supported_format, AudioState, PipeWireRdpsndHandler, RdpPcmFormat, MAX_PENDING_WAVES,
            MAX_WAVE_BYTES,
        };
        use ironrdp_rdpsnd::client::RdpsndClientHandler;
        use std::borrow::Cow;
        use std::sync::{Arc, Mutex};

        #[test]
        fn handler_bounds_pending_wave_count_and_size() {
            let state = Arc::new(Mutex::new(AudioState::default()));
            let mut handler = PipeWireRdpsndHandler::new(Arc::clone(&state));
            for _ in 0..MAX_PENDING_WAVES {
                handler.wave(0, 0, Cow::Owned(vec![0; 4]));
            }
            handler.wave(0, 0, Cow::Owned(vec![0; 4]));
            handler.wave(0, 0, Cow::Owned(vec![0; MAX_WAVE_BYTES + 1]));

            let state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(state.pending.len(), MAX_PENDING_WAVES);
            assert_eq!(state.stats.waves_received, (MAX_PENDING_WAVES + 2) as u64);
            assert_eq!(state.stats.waves_dropped, 2);
        }

        #[test]
        fn advertised_format_is_fixed_pcm_for_the_sink() {
            let format = supported_format();
            assert_eq!(format.format, ironrdp_rdpsnd::pdu::WaveFormat::PCM);
            assert_eq!(
                format.n_samples_per_sec,
                RdpPcmFormat::STEREO_S16_48K.sample_rate
            );
            assert_eq!(format.n_channels, RdpPcmFormat::STEREO_S16_48K.channels);
            assert_eq!(
                format.bits_per_sample,
                RdpPcmFormat::STEREO_S16_48K.bits_per_sample
            );
            assert_eq!(format.n_block_align, 4);
            assert_eq!(format.n_avg_bytes_per_sec, 192_000);
        }
    }
}

#[cfg(feature = "live-connect")]
pub(crate) use live::{supported_format, PendingWave, PipeWireRdpsndHandler, PreparedAudio};

#[cfg(test)]
mod tests {
    use super::{RdpAudioCapability, RdpAudioStats, RdpAudioUnsupportedReason, RdpPcmFormat};

    #[test]
    fn capability_distinguishes_endpoint_from_pcm_delivery() {
        let format = RdpPcmFormat {
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 16,
        };
        assert!(!RdpAudioCapability::EndpointWired { format }.can_claim_acceptance());
        assert!(!RdpAudioCapability::PcmStreaming { format }.can_claim_acceptance());
        assert!(RdpAudioCapability::EndpointWired { format }
            .diagnostic()
            .contains("awaiting validated PCM"));
    }

    #[test]
    fn unsupported_reasons_are_bounded_and_credential_free() {
        let reasons = [
            RdpAudioUnsupportedReason::NoHostPlaybackSink,
            RdpAudioUnsupportedReason::NoSharedFormat,
            RdpAudioUnsupportedReason::SinkWriteFailed,
        ];
        for reason in reasons {
            let diagnostic = reason.diagnostic();
            assert!(!diagnostic.contains("password"));
            assert!(!diagnostic.contains("username"));
            assert!(!diagnostic.contains("host:"));
        }
    }

    #[test]
    fn stats_default_to_zero() {
        assert_eq!(
            RdpAudioStats::default(),
            RdpAudioStats {
                waves_received: 0,
                pcm_bytes_queued: 0,
                pcm_bytes_written: 0,
                format_mismatches: 0,
                waves_dropped: 0,
                sink_failures: 0,
                shared_format_selected: false,
            }
        );
    }
}
