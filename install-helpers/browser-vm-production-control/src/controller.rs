//! Guest controller and fail-closed Browser event state machine.

use crate::auth::{verify_request_signature, ReplayCache};
use crate::config::ControllerConfig;
use crate::http::{
    authenticate_response, read_request, write_response, HttpRequest, HttpResponse,
    MAX_JSON_BODY_BYTES, MAX_WAV_BODY_BYTES,
};
use crate::page;
use crate::protocol::{
    validate_job_id, ApiStatus, BrowserEvent, CommandRequest, JobSpec, JobStatus, Operation,
};
use crate::wav::{validate_browser_wav, CHANNELS, SAMPLE_RATE};
use crate::{hex_encode, random_bytes};
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::time::{Duration, Instant};
use zeroize::Zeroize;

const JOB_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_PLAYBACK_OVERRUN_MS: u64 = 5_000;
const MAX_CAPTURE_OVERRUN_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    Registered,
    PageLoaded,
    PlaybackArmed,
    PlaybackStarted,
    PlaybackCompleted,
    CaptureReady,
    CaptureStarted,
    CaptureWavReceived,
    CaptureCompleted,
    Released,
    Failed,
}

impl Stage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::PageLoaded => "page_loaded",
            Self::PlaybackArmed => "playback_armed",
            Self::PlaybackStarted => "playback_started",
            Self::PlaybackCompleted => "playback_completed",
            Self::CaptureReady => "capture_ready",
            Self::CaptureStarted => "capture_started",
            Self::CaptureWavReceived => "capture_wav_received",
            Self::CaptureCompleted => "capture_completed",
            Self::Released => "released",
            Self::Failed => "failed",
        }
    }
}

struct Job {
    spec: JobSpec,
    created: Instant,
    page_claimed: bool,
    stage: Stage,
    user_gesture_observed: bool,
    channels: u8,
    browser_api: &'static str,
    measured_started: Option<Instant>,
    wav: Option<Vec<u8>>,
    release_requested: bool,
}

impl Job {
    fn new(spec: JobSpec) -> Self {
        Self {
            spec,
            created: Instant::now(),
            page_claimed: false,
            stage: Stage::Registered,
            user_gesture_observed: false,
            channels: 0,
            browser_api: "unavailable",
            measured_started: None,
            wav: None,
            release_requested: false,
        }
    }

    fn status(&self) -> JobStatus {
        JobStatus {
            schema_version: 1,
            job_id: self.spec.job_id.clone(),
            state: self.stage.as_str().to_owned(),
            user_gesture_observed: self.user_gesture_observed,
            browser_api: self.browser_api.to_owned(),
            channels: self.channels,
        }
    }

    fn claim_page_transport(&mut self) -> Result<()> {
        // Chromium may speculatively fetch a typed loopback URL and then issue
        // the committed navigation. Both requests are transport delivery of
        // the same unguessable job, not separate Browser activations. Permit a
        // re-fetch only until the first page_loaded event advances the strict
        // state machine; every later GET remains fail-closed.
        ensure!(
            self.stage == Stage::Registered,
            "probe page is no longer available"
        );
        self.page_claimed = true;
        Ok(())
    }

    fn common_gesture(
        &mut self,
        is_trusted: bool,
        user_activation: bool,
        audio_context_state: &str,
        sample_rate: u32,
        channels: u8,
    ) -> Result<()> {
        ensure!(is_trusted, "browser event was not trusted");
        ensure!(
            user_activation,
            "browser event lacked active user activation"
        );
        ensure!(
            audio_context_state == "running",
            "WebAudio context is not running"
        );
        ensure!(sample_rate == SAMPLE_RATE, "WebAudio context is not 48 kHz");
        ensure!(
            channels == u8::try_from(CHANNELS)?,
            "WebAudio graph is not stereo"
        );
        self.user_gesture_observed = true;
        self.channels = channels;
        Ok(())
    }

    fn apply_event(&mut self, event: BrowserEvent) -> Result<()> {
        match event {
            BrowserEvent::PageLoaded => {
                ensure!(
                    self.page_claimed && self.stage == Stage::Registered,
                    "unexpected page load"
                );
                self.stage = Stage::PageLoaded;
            }
            BrowserEvent::PlaybackArmed {
                is_trusted,
                user_activation,
                audio_context_state,
                sample_rate,
                channels,
            } => {
                ensure!(
                    self.spec.operation == Operation::Playback,
                    "operation mismatch"
                );
                ensure!(
                    self.stage == Stage::PageLoaded,
                    "playback arm is out of order"
                );
                self.common_gesture(
                    is_trusted,
                    user_activation,
                    &audio_context_state,
                    sample_rate,
                    channels,
                )?;
                self.browser_api = "WebAudio";
                self.stage = Stage::PlaybackArmed;
            }
            BrowserEvent::PlaybackStarted {
                is_trusted,
                user_activation,
                audio_context_state,
                sample_rate,
                channels,
            } => {
                ensure!(
                    self.stage == Stage::PlaybackArmed,
                    "playback start is out of order"
                );
                self.common_gesture(
                    is_trusted,
                    user_activation,
                    &audio_context_state,
                    sample_rate,
                    channels,
                )?;
                self.measured_started = Some(Instant::now());
                self.stage = Stage::PlaybackStarted;
            }
            BrowserEvent::PlaybackCompleted {
                oscillator_ended,
                elapsed_ms,
            } => {
                ensure!(
                    self.stage == Stage::PlaybackStarted,
                    "playback completion is out of order"
                );
                ensure!(oscillator_ended, "WebAudio oscillator did not report ended");
                let minimum = u64::from(self.spec.duration_seconds) * 1_000;
                ensure!(
                    elapsed_ms >= minimum.saturating_sub(250)
                        && elapsed_ms <= minimum + MAX_PLAYBACK_OVERRUN_MS,
                    "browser playback duration is outside bounds"
                );
                let observed = self
                    .measured_started
                    .context("playback start observation is missing")?
                    .elapsed();
                ensure!(
                    observed >= Duration::from_millis(minimum.saturating_sub(350)),
                    "controller did not observe a full playback interval"
                );
                self.stage = Stage::PlaybackCompleted;
            }
            BrowserEvent::CaptureReady {
                is_trusted,
                user_activation,
                audio_context_state,
                media_track_kind,
                media_track_state,
                sample_rate,
                channels,
            } => {
                ensure!(
                    self.spec.operation == Operation::Capture,
                    "operation mismatch"
                );
                ensure!(
                    self.stage == Stage::PageLoaded,
                    "capture ready is out of order"
                );
                ensure!(
                    media_track_kind == "audio",
                    "getUserMedia did not return audio"
                );
                ensure!(
                    media_track_state == "live",
                    "getUserMedia track is not live"
                );
                self.common_gesture(
                    is_trusted,
                    user_activation,
                    &audio_context_state,
                    sample_rate,
                    channels,
                )?;
                self.browser_api = "getUserMedia+WebAudio";
                self.stage = Stage::CaptureReady;
            }
            BrowserEvent::CaptureStarted {
                is_trusted,
                user_activation,
                audio_context_state,
                sample_rate,
                channels,
            } => {
                ensure!(
                    self.stage == Stage::CaptureReady,
                    "capture start is out of order"
                );
                self.common_gesture(
                    is_trusted,
                    user_activation,
                    &audio_context_state,
                    sample_rate,
                    channels,
                )?;
                self.measured_started = Some(Instant::now());
                self.stage = Stage::CaptureStarted;
            }
            BrowserEvent::CaptureCompleted {
                frames,
                sample_rate,
                channels,
                elapsed_ms,
            } => {
                ensure!(
                    self.stage == Stage::CaptureWavReceived,
                    "capture completion arrived without a validated browser WAV"
                );
                let expected_frames = SAMPLE_RATE
                    .checked_mul(self.spec.duration_seconds)
                    .context("capture frame count overflow")?;
                ensure!(
                    frames == expected_frames,
                    "browser reported the wrong frame count"
                );
                ensure!(
                    sample_rate == SAMPLE_RATE,
                    "capture completion sample rate changed"
                );
                ensure!(
                    channels == u8::try_from(CHANNELS)?,
                    "capture completion is not stereo"
                );
                let minimum = u64::from(self.spec.duration_seconds) * 1_000;
                ensure!(
                    elapsed_ms >= minimum.saturating_sub(250)
                        && elapsed_ms <= minimum + MAX_CAPTURE_OVERRUN_MS,
                    "browser capture duration is outside bounds"
                );
                self.stage = Stage::CaptureCompleted;
            }
            BrowserEvent::Released => {
                ensure!(
                    self.spec.operation == Operation::Capture
                        && self.stage == Stage::CaptureCompleted
                        && self.release_requested,
                    "microphone release is out of order"
                );
                self.stage = Stage::Released;
            }
            BrowserEvent::Failed { reason_code } => {
                ensure!(
                    !reason_code.is_empty()
                        && reason_code.len() <= 80
                        && reason_code
                            .bytes()
                            .all(|value| value.is_ascii_alphanumeric()
                                || matches!(value, b'-' | b'_')),
                    "browser failure reason is malformed"
                );
                self.stage = Stage::Failed;
                self.wav = None;
            }
        }
        Ok(())
    }

    fn accept_wav(&mut self, bytes: Vec<u8>) -> Result<()> {
        ensure!(
            self.stage == Stage::CaptureStarted,
            "browser WAV is out of order"
        );
        let elapsed = self
            .measured_started
            .context("capture start observation is missing")?
            .elapsed();
        let minimum = Duration::from_millis(
            u64::from(self.spec.duration_seconds)
                .saturating_mul(1_000)
                .saturating_sub(350),
        );
        ensure!(
            elapsed >= minimum,
            "browser WAV arrived before a real capture interval"
        );
        validate_browser_wav(&bytes, self.spec.duration_seconds)?;
        self.wav = Some(bytes);
        self.stage = Stage::CaptureWavReceived;
        Ok(())
    }
}

pub struct Controller {
    config: ControllerConfig,
    secret: [u8; 32],
    replay: ReplayCache,
    jobs: BTreeMap<String, Job>,
}

impl Controller {
    pub fn new(config: ControllerConfig) -> Result<Self> {
        let secret = config.controller_secret()?;
        Ok(Self {
            config,
            secret: *secret,
            replay: ReplayCache::default(),
            jobs: BTreeMap::new(),
        })
    }

    pub fn serve(mut self) -> Result<()> {
        let address = SocketAddr::new(self.config.listen_address, self.config.listen_port);
        let listener = TcpListener::bind(address)
            .with_context(|| format!("bind Browser probe controller at {address}"))?;
        eprintln!("browser-vm-guest-audio-probe-controller: ready on {address}");
        for accepted in listener.incoming() {
            let mut stream = match accepted {
                Ok(stream) => stream,
                Err(error) => {
                    eprintln!("browser-vm-guest-audio-probe-controller: accept failed: {error}");
                    continue;
                }
            };
            let peer = match stream.peer_addr() {
                Ok(peer) => peer,
                Err(_) => continue,
            };
            let request = match read_request(&mut stream, MAX_WAV_BODY_BYTES) {
                Ok(request) => request,
                Err(_) => {
                    let response = HttpResponse::error(400, "request-rejected");
                    let _ignored = write_response(&mut stream, &response);
                    continue;
                }
            };
            let response = self.handle(peer.ip(), &request);
            let _ignored = write_response(&mut stream, &response);
        }
        Ok(())
    }

    fn handle(&mut self, peer: IpAddr, request: &HttpRequest) -> HttpResponse {
        self.jobs.retain(|_, job| job.created.elapsed() <= JOB_TTL);
        if request.path.starts_with("/v1/") || request.path == "/v1/jobs" {
            return self.handle_host(peer, request);
        }
        if request.path.starts_with("/probe/") {
            return self
                .handle_browser(peer, request)
                .unwrap_or_else(|_| HttpResponse::error(400, "browser-request-rejected"));
        }
        HttpResponse::error(404, "not-found")
    }

    fn handle_host(&mut self, peer: IpAddr, request: &HttpRequest) -> HttpResponse {
        if peer != self.config.allowed_host_address {
            return HttpResponse::error(403, "host-peer-rejected");
        }
        let nonce = match self.authenticate_host(request) {
            Ok(nonce) => nonce,
            Err(_) => return HttpResponse::error(401, "authentication-rejected"),
        };
        let mut response = self
            .route_host(request)
            .unwrap_or_else(|_| HttpResponse::error(422, "host-request-rejected"));
        if authenticate_response(&mut response, &self.secret, &nonce).is_err() {
            return HttpResponse::error(500, "response-authentication-failed");
        }
        response
    }

    fn authenticate_host(&mut self, request: &HttpRequest) -> Result<String> {
        let timestamp = request
            .header("x-mcnf-time")
            .context("authenticated timestamp is missing")?
            .parse::<i64>()
            .context("authenticated timestamp is invalid")?;
        let nonce = request
            .header("x-mcnf-nonce")
            .context("authenticated nonce is missing")?;
        let signature = request
            .header("x-mcnf-signature")
            .context("authenticated signature is missing")?;
        verify_request_signature(
            &self.secret,
            &request.method,
            &request.path,
            timestamp,
            nonce,
            &request.body,
            signature,
        )?;
        self.replay.admit(timestamp, nonce)?;
        Ok(nonce.to_owned())
    }

    fn route_host(&mut self, request: &HttpRequest) -> Result<HttpResponse> {
        if request.path == "/v1/jobs" {
            ensure!(request.method == "POST", "job collection only accepts POST");
            require_json(request)?;
            ensure!(
                request.body.len() <= MAX_JSON_BODY_BYTES,
                "job body is too large"
            );
            let spec: JobSpec = serde_json::from_slice(&request.body).context("parse job spec")?;
            spec.validate()?;
            ensure!(
                self.jobs.len() < self.config.max_jobs,
                "controller job capacity reached"
            );
            ensure!(
                !self.jobs.contains_key(&spec.job_id),
                "job id already exists"
            );
            self.jobs.insert(spec.job_id.clone(), Job::new(spec));
            return HttpResponse::json(
                201,
                &ApiStatus {
                    schema_version: 1,
                    status: "registered".to_owned(),
                },
            );
        }

        let fields = request.path.split('/').collect::<Vec<_>>();
        ensure!(
            fields.len() >= 4 && fields[1] == "v1" && fields[2] == "jobs",
            "bad job API path"
        );
        let job_id = fields[3];
        validate_job_id(job_id)?;
        if fields.len() == 4 {
            return match request.method.as_str() {
                "GET" => {
                    ensure!(request.body.is_empty(), "GET body is forbidden");
                    let job = self.jobs.get(job_id).context("unknown job")?;
                    HttpResponse::json(200, &job.status())
                }
                "DELETE" => {
                    ensure!(request.body.is_empty(), "DELETE body is forbidden");
                    ensure!(self.jobs.remove(job_id).is_some(), "unknown job");
                    HttpResponse::json(
                        200,
                        &ApiStatus {
                            schema_version: 1,
                            status: "deleted".to_owned(),
                        },
                    )
                }
                _ => bail!("unsupported job method"),
            };
        }
        ensure!(fields.len() == 5, "bad job action path");
        match (request.method.as_str(), fields[4]) {
            ("POST", "command") => {
                require_json(request)?;
                let command: CommandRequest =
                    serde_json::from_slice(&request.body).context("parse job command")?;
                ensure!(command.schema_version == 1, "unsupported command schema");
                ensure!(command.command == "release", "only release is admitted");
                let job = self.jobs.get_mut(job_id).context("unknown job")?;
                ensure!(
                    job.spec.operation == Operation::Capture
                        && job.stage == Stage::CaptureCompleted,
                    "release command is out of order"
                );
                job.release_requested = true;
                HttpResponse::json(
                    200,
                    &ApiStatus {
                        schema_version: 1,
                        status: "release_requested".to_owned(),
                    },
                )
            }
            ("GET", "wav") => {
                ensure!(request.body.is_empty(), "WAV GET body is forbidden");
                let job = self.jobs.get(job_id).context("unknown job")?;
                ensure!(
                    matches!(job.stage, Stage::CaptureCompleted | Stage::Released),
                    "capture is not complete"
                );
                let wav = job
                    .wav
                    .as_ref()
                    .context("validated browser WAV is missing")?;
                Ok(HttpResponse::bytes(200, "audio/wav", wav.clone()))
            }
            _ => bail!("unsupported job action"),
        }
    }

    fn handle_browser(&mut self, peer: IpAddr, request: &HttpRequest) -> Result<HttpResponse> {
        ensure!(
            peer.is_loopback(),
            "browser endpoint requires a loopback peer"
        );
        let expected_host = format!("127.0.0.1:{}", self.config.listen_port);
        ensure!(
            request.header("host") == Some(expected_host.as_str()),
            "browser Host is not loopback"
        );
        let fields = request.path.split('/').collect::<Vec<_>>();
        ensure!(
            fields.len() >= 3 && fields[1] == "probe",
            "bad browser path"
        );
        let job_id = fields[2];
        validate_job_id(job_id)?;
        let origin = self.config.browser_origin();
        if fields.len() == 3 {
            ensure!(
                request.method == "GET" && request.body.is_empty(),
                "probe page requires GET"
            );
            let job = self.jobs.get_mut(job_id).context("unknown job")?;
            job.claim_page_transport()?;
            let nonce = hex_encode(&random_bytes::<18>()?);
            let rendered = page::render(&job.spec, &nonce);
            let mut response = HttpResponse::bytes(200, "text/html; charset=utf-8", rendered.html);
            response.add_header("Cache-Control", "no-store, max-age=0".to_owned())?;
            response.add_header("Pragma", "no-cache".to_owned())?;
            response.add_header("Content-Security-Policy", rendered.csp)?;
            response.add_header(
                "Permissions-Policy",
                "microphone=(self), autoplay=(self)".to_owned(),
            )?;
            response.add_header("Cross-Origin-Resource-Policy", "same-origin".to_owned())?;
            return Ok(response);
        }
        ensure!(fields.len() == 4, "bad browser action path");
        validate_browser_fetch_headers(request, &origin)?;
        match (request.method.as_str(), fields[3]) {
            ("POST", "event") => {
                require_json(request)?;
                ensure!(
                    request.body.len() <= MAX_JSON_BODY_BYTES,
                    "browser event is too large"
                );
                let event: BrowserEvent =
                    serde_json::from_slice(&request.body).context("parse browser event")?;
                let job = self.jobs.get_mut(job_id).context("unknown job")?;
                job.apply_event(event)?;
                HttpResponse::json(
                    200,
                    &ApiStatus {
                        schema_version: 1,
                        status: "accepted".to_owned(),
                    },
                )
            }
            ("POST", "wav") => {
                ensure!(
                    request.header("content-type") == Some("audio/wav"),
                    "WAV content type rejected"
                );
                let job = self.jobs.get_mut(job_id).context("unknown job")?;
                job.accept_wav(request.body.clone())?;
                HttpResponse::json(
                    200,
                    &ApiStatus {
                        schema_version: 1,
                        status: "wav_accepted".to_owned(),
                    },
                )
            }
            ("GET", "command") => {
                ensure!(request.body.is_empty(), "command GET body is forbidden");
                let job = self.jobs.get(job_id).context("unknown job")?;
                ensure!(
                    job.spec.operation == Operation::Capture,
                    "playback has no browser command"
                );
                let command = if job.release_requested {
                    "release"
                } else {
                    "wait"
                };
                #[derive(Serialize)]
                struct BrowserCommand<'a> {
                    schema_version: u8,
                    command: &'a str,
                }
                HttpResponse::json(
                    200,
                    &BrowserCommand {
                        schema_version: 1,
                        command,
                    },
                )
            }
            _ => bail!("unsupported browser action"),
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.secret.zeroize();
        for job in self.jobs.values_mut() {
            if let Some(wav) = &mut job.wav {
                wav.zeroize();
            }
        }
    }
}

fn require_json(request: &HttpRequest) -> Result<()> {
    ensure!(
        request.header("content-type") == Some("application/json"),
        "request is not exact application/json"
    );
    Ok(())
}

fn validate_browser_fetch_headers(request: &HttpRequest, origin: &str) -> Result<()> {
    if request.method == "POST" {
        ensure!(
            request.header("origin") == Some(origin),
            "browser Origin is not the loopback probe"
        );
    } else {
        let referer = request
            .header("referer")
            .context("browser command request omitted its probe referrer")?;
        ensure!(
            referer.starts_with(&format!("{origin}/probe/")),
            "browser command referrer is not the loopback probe"
        );
    }
    ensure!(
        request.header("sec-fetch-site") == Some("same-origin"),
        "browser request is not same-origin"
    );
    ensure!(
        request.header("sec-fetch-mode") == Some("cors"),
        "browser request is not a fetch CORS-mode request"
    );
    let user_agent = request
        .header("user-agent")
        .context("browser User-Agent is missing")?;
    ensure!(
        user_agent.contains("Chrome/") || user_agent.contains("Chromium/"),
        "request did not identify Chromium"
    );
    Ok(())
}

/// Load production configuration and serve until systemd stops the process.
pub fn run() -> Result<()> {
    let config = ControllerConfig::load()?;
    Controller::new(config)?.serve()
}

#[cfg(test)]
mod tests {
    use super::{Job, Stage};
    use crate::protocol::{BrowserEvent, JobSpec, Operation};

    fn spec(operation: Operation) -> JobSpec {
        JobSpec {
            schema_version: 1,
            job_id: "a".repeat(64),
            operation,
            phase: "before-recovery".to_owned(),
            tone_hz: 719,
            duration_seconds: if operation == Operation::Playback {
                8
            } else {
                2
            },
            source_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            image_digest: format!("sha256:{}", "b".repeat(64)),
            transport: "rdp".to_owned(),
        }
    }

    #[test]
    fn untrusted_browser_gesture_cannot_arm_playback() {
        let mut job = Job::new(spec(Operation::Playback));
        job.page_claimed = true;
        assert!(job.apply_event(BrowserEvent::PageLoaded).is_ok());
        let event = BrowserEvent::PlaybackArmed {
            is_trusted: false,
            user_activation: true,
            audio_context_state: "running".to_owned(),
            sample_rate: 48_000,
            channels: 2,
        };
        assert!(job.apply_event(event).is_err());
        assert_eq!(job.stage, Stage::PageLoaded);
        assert!(!job.user_gesture_observed);
    }

    #[test]
    fn speculative_refetch_closes_at_the_first_browser_event() {
        let mut job = Job::new(spec(Operation::Capture));
        assert!(job.claim_page_transport().is_ok());
        assert!(job.claim_page_transport().is_ok());
        assert!(job.apply_event(BrowserEvent::PageLoaded).is_ok());
        assert!(job.claim_page_transport().is_err());
        assert_eq!(job.stage, Stage::PageLoaded);
    }

    #[test]
    fn capture_cannot_complete_without_browser_wav() {
        let mut job = Job::new(spec(Operation::Capture));
        job.page_claimed = true;
        assert!(job.apply_event(BrowserEvent::PageLoaded).is_ok());
        assert!(job
            .apply_event(BrowserEvent::CaptureReady {
                is_trusted: true,
                user_activation: true,
                audio_context_state: "running".to_owned(),
                media_track_kind: "audio".to_owned(),
                media_track_state: "live".to_owned(),
                sample_rate: 48_000,
                channels: 2,
            })
            .is_ok());
        assert!(job
            .apply_event(BrowserEvent::CaptureStarted {
                is_trusted: true,
                user_activation: true,
                audio_context_state: "running".to_owned(),
                sample_rate: 48_000,
                channels: 2,
            })
            .is_ok());
        assert!(job
            .apply_event(BrowserEvent::CaptureCompleted {
                frames: 96_000,
                sample_rate: 48_000,
                channels: 2,
                elapsed_ms: 2_000,
            })
            .is_err());
        assert_eq!(job.stage, Stage::CaptureStarted);
    }

    #[test]
    fn browser_failure_never_promotes_a_receipt_state() {
        let mut job = Job::new(spec(Operation::Playback));
        job.page_claimed = true;
        assert!(job.apply_event(BrowserEvent::PageLoaded).is_ok());
        assert!(job
            .apply_event(BrowserEvent::Failed {
                reason_code: "permission-denied".to_owned(),
            })
            .is_ok());
        assert_eq!(job.stage, Stage::Failed);
        assert!(!job.user_gesture_observed);
    }
}
