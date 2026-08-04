//! Host-side collector hook implementations.

use crate::config::HostConfig;
use crate::http::{ClientResponse, ControllerClient, MAX_JSON_BODY_BYTES, MAX_WAV_BODY_BYTES};
use crate::protocol::{ApiStatus, CommandRequest, JobSpec, JobStatus, Operation};
use crate::rdp::RdpDriver;
use crate::receipt::{write_private_bytes, write_probe_receipt, write_reconnect_receipt};
use crate::wav::validate_browser_wav;
use crate::{hex_encode, random_bytes, utc_timestamp, wait_for_later_timestamp};
use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(20);
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CONSECUTIVE_STATUS_ERRORS: u8 = 3;
pub const RETRYABLE_UNTOUCHED_JOB_EXIT_CODE: i32 = 75;

#[derive(Debug)]
struct UntouchedRegisteredJob;

impl std::fmt::Display for UntouchedRegisteredJob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Browser controller job remained registered and unactivated")
    }
}

impl std::error::Error for UntouchedRegisteredJob {}

pub fn is_retryable_untouched_job(error: &anyhow::Error) -> bool {
    error.is::<UntouchedRegisteredJob>()
}

pub fn probe_failure_exit_code(error: &anyhow::Error) -> i32 {
    if is_retryable_untouched_job(error) {
        RETRYABLE_UNTOUCHED_JOB_EXIT_CODE
    } else {
        1
    }
}

struct RequiredEnvironment {
    domain: String,
    transport: String,
    source_commit: String,
    image_digest: String,
}

impl RequiredEnvironment {
    fn load() -> Result<Self> {
        let value = Self {
            domain: required_env("MCNF_BROWSER_VM_DOMAIN")?,
            transport: required_env("MCNF_BROWSER_VM_TRANSPORT")?,
            source_commit: required_env("MCNF_BROWSER_VM_SOURCE_COMMIT")?,
            image_digest: required_env("MCNF_BROWSER_VM_IMAGE_DIGEST")?,
        };
        ensure!(
            !value.domain.is_empty()
                && value.domain.len() <= 128
                && value
                    .domain
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')),
            "invalid Browser VM domain"
        );
        ensure!(
            value.transport == "rdp",
            "production browser control implements RDP only"
        );
        // JobSpec::validate owns the commit/digest shape below.
        Ok(value)
    }
}

struct ProbeArguments {
    operation: Operation,
    phase: String,
    tone_hz: u32,
    duration_seconds: u32,
    ready_receipt: PathBuf,
    start_signal: PathBuf,
    started_receipt: Option<PathBuf>,
    completed_receipt: PathBuf,
    release_signal: Option<PathBuf>,
    output_wav: Option<PathBuf>,
}

enum ProbePlan {
    Single(ProbeArguments),
    Duplex {
        playback: ProbeArguments,
        capture: ProbeArguments,
    },
}

impl ProbePlan {
    fn parse() -> Result<Self> {
        Self::parse_from(env::args_os().skip(1))
    }

    fn parse_from<I>(arguments: I) -> Result<Self>
    where
        I: IntoIterator<Item = std::ffi::OsString>,
    {
        let mut args = arguments.into_iter();
        let command = args
            .next()
            .context("probe operation is missing")?
            .to_str()
            .context("probe operation is not UTF-8")?
            .to_owned();
        let mut options = BTreeMap::new();
        while let Some(raw_key) = args.next() {
            let key = raw_key.to_str().context("probe option is not UTF-8")?;
            ensure!(
                key.starts_with("--") && key.len() > 2,
                "malformed probe option"
            );
            let value = args.next().context("probe option value is missing")?;
            let value = value.to_str().context("probe option value is not UTF-8")?;
            ensure!(
                options
                    .insert(key[2..].to_owned(), value.to_owned())
                    .is_none(),
                "duplicate probe option"
            );
        }
        let phase = take(&mut options, "phase")?;
        let plan = match command.as_str() {
            "playback" => Self::Single(ProbeArguments::from_options(
                Operation::Playback,
                phase,
                "",
                &mut options,
            )?),
            "capture" => Self::Single(ProbeArguments::from_options(
                Operation::Capture,
                phase,
                "",
                &mut options,
            )?),
            "duplex" => Self::Duplex {
                playback: ProbeArguments::from_options(
                    Operation::Playback,
                    phase.clone(),
                    "playback",
                    &mut options,
                )?,
                capture: ProbeArguments::from_options(
                    Operation::Capture,
                    phase,
                    "capture",
                    &mut options,
                )?,
            },
            _ => bail!("probe operation must be playback, capture, or duplex"),
        };
        ensure!(options.is_empty(), "unknown probe option");
        Ok(plan)
    }
}

impl ProbeArguments {
    fn from_options(
        operation: Operation,
        phase: String,
        prefix: &str,
        options: &mut BTreeMap<String, String>,
    ) -> Result<Self> {
        let tone_hz = take(options, &option_name(prefix, "tone-hz"))?
            .parse::<u32>()
            .context("tone-hz is invalid")?;
        let duration_seconds = take(options, &option_name(prefix, "duration-seconds"))?
            .parse::<u32>()
            .context("duration-seconds is invalid")?;
        let ready_receipt =
            absolute_option(&take(options, &option_name(prefix, "ready-receipt"))?)?;
        let start_signal = absolute_option(&take(options, &option_name(prefix, "start-signal"))?)?;
        let completed_receipt =
            absolute_option(&take(options, &option_name(prefix, "completed-receipt"))?)?;
        let started_receipt = options
            .remove(&option_name(prefix, "started-receipt"))
            .map(|value| absolute_option(&value))
            .transpose()?;
        let release_signal = options
            .remove(&option_name(prefix, "release-signal"))
            .map(|value| absolute_option(&value))
            .transpose()?;
        let output_wav = options
            .remove(&option_name(prefix, "output-wav"))
            .map(|value| absolute_option(&value))
            .transpose()?;
        match operation {
            Operation::Playback => ensure!(
                started_receipt.is_some() && release_signal.is_none() && output_wav.is_none(),
                "playback hook options do not match collector contract"
            ),
            Operation::Capture => ensure!(
                started_receipt.is_none() && release_signal.is_some() && output_wav.is_some(),
                "capture hook options do not match collector contract"
            ),
        }
        Ok(Self {
            operation,
            phase,
            tone_hz,
            duration_seconds,
            ready_receipt,
            start_signal,
            started_receipt,
            completed_receipt,
            release_signal,
            output_wav,
        })
    }
}

fn option_name(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}-{name}")
    }
}

fn take(options: &mut BTreeMap<String, String>, name: &str) -> Result<String> {
    options
        .remove(name)
        .with_context(|| format!("required probe option --{name} is missing"))
}

fn absolute_option(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    ensure!(
        path.is_absolute(),
        "collector control path must be absolute"
    );
    Ok(path)
}

fn required_env(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("required collector environment {name} is missing"))
}

struct ProbeControl {
    client: ControllerClient,
    job_id: String,
}

impl ProbeControl {
    fn register(config: &HostConfig, spec: &JobSpec) -> Result<Self> {
        let secret = config.controller_secret()?;
        let client = ControllerClient::new(config.controller_host, config.controller_port, *secret);
        let response = client.json("POST", "/v1/jobs", spec)?;
        ensure_status(&response, 201)?;
        let accepted: ApiStatus = serde_json::from_slice(&response.body)
            .context("parse controller registration response")?;
        ensure!(
            accepted.schema_version == 1 && accepted.status == "registered",
            "controller did not register the Browser job"
        );
        Ok(Self {
            client,
            job_id: spec.job_id.clone(),
        })
    }

    fn status(&self) -> Result<JobStatus> {
        let path = format!("/v1/jobs/{}", self.job_id);
        let response = self.client.empty("GET", &path, MAX_JSON_BODY_BYTES)?;
        ensure_status(&response, 200)?;
        let status: JobStatus =
            serde_json::from_slice(&response.body).context("parse controller job status")?;
        ensure!(
            status.schema_version == 1 && status.job_id == self.job_id,
            "controller returned another job identity"
        );
        Ok(status)
    }

    fn wav(&self) -> Result<Vec<u8>> {
        let path = format!("/v1/jobs/{}/wav", self.job_id);
        let response = self.client.empty("GET", &path, MAX_WAV_BODY_BYTES)?;
        ensure_status(&response, 200)?;
        ensure!(
            response.header("content-type") == Some("audio/wav"),
            "controller returned a non-WAV capture"
        );
        Ok(response.body)
    }

    fn release(&self) -> Result<()> {
        let path = format!("/v1/jobs/{}/command", self.job_id);
        let response = self.client.json(
            "POST",
            &path,
            &CommandRequest {
                schema_version: 1,
                command: "release".to_owned(),
            },
        )?;
        ensure_status(&response, 200)
    }

    fn delete(&self) -> Result<()> {
        let path = format!("/v1/jobs/{}", self.job_id);
        let response = self.client.empty("DELETE", &path, MAX_JSON_BODY_BYTES)?;
        ensure_status(&response, 200)
    }
}

fn ensure_status(response: &ClientResponse, expected: u16) -> Result<()> {
    ensure!(
        response.status == expected,
        "Browser controller rejected the bounded operation (HTTP {})",
        response.status
    );
    Ok(())
}

pub fn run_probe() -> Result<()> {
    let environment = RequiredEnvironment::load()?;
    let plan = ProbePlan::parse()?;
    let config = HostConfig::load()?;
    let mut driver = RdpDriver::connect(&config).context("connect Browser RDP probe session")?;
    let result = match plan {
        ProbePlan::Single(arguments) => {
            run_probe_job(&environment, &config, &arguments, &mut driver)
        }
        ProbePlan::Duplex { playback, capture } => {
            run_probe_job(&environment, &config, &playback, &mut driver)
                .and_then(|()| driver.settle_between_browser_jobs())
                .and_then(|()| run_probe_job(&environment, &config, &capture, &mut driver))
        }
    };
    let shutdown = driver.shutdown();
    result?;
    shutdown?;
    Ok(())
}

fn run_probe_job(
    environment: &RequiredEnvironment,
    config: &HostConfig,
    arguments: &ProbeArguments,
    driver: &mut RdpDriver,
) -> Result<()> {
    let spec = JobSpec {
        schema_version: 1,
        job_id: hex_encode(&random_bytes::<32>()?),
        operation: arguments.operation,
        phase: arguments.phase.clone(),
        tone_hz: arguments.tone_hz,
        duration_seconds: arguments.duration_seconds,
        source_commit: environment.source_commit.clone(),
        image_digest: environment.image_digest.clone(),
        transport: environment.transport.clone(),
    };
    spec.validate()?;
    let control = ProbeControl::register(config, &spec)?;
    let result = run_registered_probe(config, &spec, arguments, &control, driver);
    let retry_registered = result.is_err()
        && control
            .status()
            .is_ok_and(|status| is_unactivated_registered_job(&status));
    let deletion = control.delete();
    match result {
        Ok(()) => {
            deletion?;
            Ok(())
        }
        Err(_) if retry_registered => {
            // Only this typed outcome permits the collector to launch a fresh
            // hook process and one-time job. Deletion must succeed first.
            deletion.context("delete untouched Browser probe job before retry")?;
            Err(UntouchedRegisteredJob.into())
        }
        Err(error) => {
            let _ignored = deletion;
            Err(error)
        }
    }
}

fn is_unactivated_registered_job(status: &JobStatus) -> bool {
    status.schema_version == 1
        && status.state == "registered"
        && !status.user_gesture_observed
        && status.browser_api == "unavailable"
        && status.channels == 0
}

fn run_registered_probe(
    config: &HostConfig,
    spec: &JobSpec,
    arguments: &ProbeArguments,
    control: &ProbeControl,
    driver: &mut RdpDriver,
) -> Result<()> {
    let url = format!("{}/probe/{}", config.browser_origin(), spec.job_id);
    driver.navigate(&url)?;
    wait_for_state(control, driver, "page_loaded", false, "unavailable")?;

    // First real RDP click arms WebAudio or invokes getUserMedia. Only the
    // controller's browser-origin callback can promote the ready receipt.
    driver.click(config.control_button_x, config.control_button_y)?;
    let (ready_state, browser_api) = match spec.operation {
        Operation::Playback => ("playback_armed", "WebAudio"),
        Operation::Capture => ("capture_ready", "getUserMedia+WebAudio"),
    };
    wait_for_state(control, driver, ready_state, true, browser_api)?;
    write_probe_receipt(&arguments.ready_receipt, spec, "ready")?;

    wait_for_signal(&arguments.start_signal, driver)?;
    // The measured start is a second trusted RDP click, not an API command.
    driver.click(config.control_button_x, config.control_button_y)?;
    match spec.operation {
        Operation::Playback => {
            wait_for_state(control, driver, "playback_started", true, browser_api)?;
            write_probe_receipt(
                arguments
                    .started_receipt
                    .as_deref()
                    .context("playback started receipt is missing")?,
                spec,
                "started",
            )?;
            wait_for_state(control, driver, "playback_completed", true, browser_api)?;
            write_probe_receipt(&arguments.completed_receipt, spec, "completed")?;
        }
        Operation::Capture => {
            wait_for_state(control, driver, "capture_started", true, browser_api)?;
            wait_for_state(control, driver, "capture_completed", true, browser_api)?;
            let wav = control.wav()?;
            validate_browser_wav(&wav, spec.duration_seconds)?;
            write_private_bytes(
                arguments
                    .output_wav
                    .as_deref()
                    .context("capture output path is missing")?,
                &wav,
            )?;
            write_probe_receipt(&arguments.completed_receipt, spec, "completed")?;
            wait_for_signal(
                arguments
                    .release_signal
                    .as_deref()
                    .context("capture release signal is missing")?,
                driver,
            )?;
            control.release()?;
            wait_for_state(control, driver, "released", true, browser_api)?;
        }
    }
    Ok(())
}

fn wait_for_state(
    control: &ProbeControl,
    driver: &mut RdpDriver,
    expected: &str,
    gesture: bool,
    browser_api: &str,
) -> Result<()> {
    let started = Instant::now();
    let mut last_poll = Instant::now() - Duration::from_secs(1);
    let mut last_state = "unknown".to_owned();
    let mut consecutive_status_errors = 0_u8;
    while started.elapsed() < CONTROL_TIMEOUT {
        let _frame = driver.pump_once()?;
        if last_poll.elapsed() >= Duration::from_millis(100) {
            let status = match control.status() {
                Ok(status) => {
                    consecutive_status_errors = 0;
                    status
                }
                Err(error) => {
                    consecutive_status_errors += 1;
                    if consecutive_status_errors > MAX_CONSECUTIVE_STATUS_ERRORS {
                        return Err(error).context(
                            "Browser controller status remained unavailable after bounded retries",
                        );
                    }
                    // Chromium may leave a speculative loopback connection in
                    // front of the controller's host-status request. The
                    // controller closes that idle connection at its bounded
                    // read timeout. Keep pumping RDP and retry the authenticated
                    // status request; no failed or unauthenticated response is
                    // accepted, and CONTROL_TIMEOUT still bounds the operation.
                    last_poll = Instant::now();
                    continue;
                }
            };
            last_state.clone_from(&status.state);
            if status.state == "failed" {
                bail!("Browser page failed closed before {expected}");
            }
            if status.state == expected {
                ensure!(
                    status.user_gesture_observed == gesture,
                    "controller gesture state does not match operation"
                );
                ensure!(
                    status.browser_api == browser_api,
                    "controller Browser API mismatch"
                );
                ensure!(
                    (!gesture && status.channels == 0) || (gesture && status.channels == 2),
                    "controller channel state does not match operation"
                );
                return Ok(());
            }
            last_poll = Instant::now();
        }
    }
    bail!("Browser controller did not reach {expected} (last state {last_state})")
}

fn wait_for_signal(path: &Path, driver: &mut RdpDriver) -> Result<()> {
    let started = Instant::now();
    while started.elapsed() < SIGNAL_TIMEOUT {
        let _frame = driver.pump_once()?;
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_file() && !metadata.file_type().is_symlink(),
                    "collector signal is not a regular file"
                );
                let owner = fs::metadata("/proc/self")
                    .context("inspect hook owner")?
                    .uid();
                ensure!(metadata.uid() == owner, "collector signal owner mismatch");
                ensure!(
                    metadata.mode() & 0o077 == 0,
                    "collector signal is not private"
                );
                ensure!(metadata.len() <= 16, "collector signal is oversized");
                ensure!(
                    fs::read(path).context("read collector signal")? == b"start\n",
                    "collector signal content is invalid"
                );
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect collector signal"),
        }
    }
    bail!("collector signal did not arrive before the bounded deadline")
}

pub fn run_reconnect() -> Result<()> {
    let environment = RequiredEnvironment::load()?;
    let receipt = absolute_option(&required_env("MCNF_BROWSER_VM_RECONNECT_RECEIPT")?)?;
    let config = HostConfig::load()?;
    let mut driver = RdpDriver::connect(&config)?;
    driver.request_full_refresh()?;
    let baseline = driver.wait_for_browser_frame(Duration::from_secs(20))?;
    driver.disconnect()?;
    let disconnect_at = utc_timestamp()?;
    let identity = driver.reconnect_and_observe(&baseline)?;
    ensure!(
        identity >= crate::rdp::MIN_RECONNECT_IDENTITY_PER_MILLE,
        "reconnect identity is below floor"
    );
    let observed = utc_timestamp()?;
    let reconnect_at = if observed > disconnect_at {
        observed
    } else {
        wait_for_later_timestamp(&disconnect_at, Duration::from_secs(2))?
    };
    write_reconnect_receipt(
        &receipt,
        &environment.domain,
        &environment.source_commit,
        &environment.image_digest,
        &environment.transport,
        &disconnect_at,
        &reconnect_at,
    )?;
    driver.shutdown()
}

#[cfg(test)]
mod tests {
    use super::{
        is_retryable_untouched_job, is_unactivated_registered_job, probe_failure_exit_code,
        ProbeArguments, ProbePlan, UntouchedRegisteredJob, RETRYABLE_UNTOUCHED_JOB_EXIT_CODE,
    };
    use crate::protocol::{JobStatus, Operation};
    use std::ffi::OsString;

    #[test]
    fn operation_enum_remains_collector_spelling() {
        assert_eq!(Operation::Playback.as_str(), "playback");
        assert_eq!(Operation::Capture.as_str(), "capture");
        let _type_guard: Option<ProbeArguments> = None;
        let _argv_guard: Vec<OsString> = Vec::new();
    }

    #[test]
    fn duplex_arguments_map_both_jobs_without_reconnecting() {
        let arguments = [
            "duplex",
            "--phase",
            "before-recovery",
            "--playback-tone-hz",
            "523",
            "--playback-duration-seconds",
            "8",
            "--playback-ready-receipt",
            "/run/playback-ready",
            "--playback-start-signal",
            "/run/playback-start",
            "--playback-started-receipt",
            "/run/playback-started",
            "--playback-completed-receipt",
            "/run/playback-completed",
            "--capture-tone-hz",
            "719",
            "--capture-duration-seconds",
            "2",
            "--capture-ready-receipt",
            "/run/capture-ready",
            "--capture-start-signal",
            "/run/capture-start",
            "--capture-completed-receipt",
            "/run/capture-completed",
            "--capture-release-signal",
            "/run/capture-release",
            "--capture-output-wav",
            "/run/capture.wav",
        ]
        .into_iter()
        .map(OsString::from);

        let ProbePlan::Duplex { playback, capture } =
            ProbePlan::parse_from(arguments).expect("valid duplex contract")
        else {
            panic!("duplex command did not produce a duplex plan");
        };
        assert_eq!(playback.operation, Operation::Playback);
        assert_eq!(playback.phase, "before-recovery");
        assert_eq!(playback.tone_hz, 523);
        assert_eq!(playback.duration_seconds, 8);
        assert_eq!(
            playback.started_receipt.as_deref(),
            Some(std::path::Path::new("/run/playback-started"))
        );
        assert_eq!(capture.operation, Operation::Capture);
        assert_eq!(capture.phase, "before-recovery");
        assert_eq!(capture.tone_hz, 719);
        assert_eq!(capture.duration_seconds, 2);
        assert_eq!(
            capture.release_signal.as_deref(),
            Some(std::path::Path::new("/run/capture-release"))
        );
        assert_eq!(
            capture.output_wav.as_deref(),
            Some(std::path::Path::new("/run/capture.wav"))
        );
    }

    #[test]
    fn reattach_is_limited_to_an_unactivated_registered_job() {
        let mut status = JobStatus {
            schema_version: 1,
            job_id: "a".repeat(64),
            state: "registered".to_owned(),
            user_gesture_observed: false,
            browser_api: "unavailable".to_owned(),
            channels: 0,
        };
        assert!(is_unactivated_registered_job(&status));

        status.user_gesture_observed = true;
        assert!(!is_unactivated_registered_job(&status));
        status.user_gesture_observed = false;
        status.state = "page_loaded".to_owned();
        assert!(!is_unactivated_registered_job(&status));

        let retryable = anyhow::Error::new(UntouchedRegisteredJob);
        assert!(is_retryable_untouched_job(&retryable));
        assert!(!is_retryable_untouched_job(&anyhow::anyhow!("other")));
        assert_eq!(RETRYABLE_UNTOUCHED_JOB_EXIT_CODE, 75);
        assert_eq!(probe_failure_exit_code(&retryable), 75);
        assert_eq!(probe_failure_exit_code(&anyhow::anyhow!("other")), 1);
    }
}
