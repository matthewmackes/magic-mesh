//! `Join` CLI verb handler (one-command peer/lighthouse join).
//!
//! Extracted verbatim from `bin/mackesd.rs` (arch-1). Behaviour is unchanged;
//! only the location moved. The invite-redeem branch + the lighthouse etcd/
//! CA-backup provisioning helpers are join-exclusive and kept private here.
use crate::*;

const SETUP_ETCD: &str = "/usr/libexec/mackesd/setup-etcd";
const SETUP_SYNCTHING: &str = "/usr/libexec/mackesd/setup-syncthing";
const WORKSTATION_SETUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
const SETUP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
const SETUP_TERMINATE_GRACE: std::time::Duration = std::time::Duration::from_secs(1);
const MAX_SETUP_DIAGNOSTIC_BYTES: usize = 8 * 1024;
const MAX_WORKSTATION_LIGHTHOUSE_ANCHORS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
enum SetupCommandFailure {
    Spawn(String),
    Wait(String),
    TimedOut {
        seconds: u64,
    },
    Exited {
        code: Option<i32>,
        diagnostic: String,
    },
}

impl std::fmt::Display for SetupCommandFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "could not start: {error}"),
            Self::Wait(error) => write!(f, "could not observe completion: {error}"),
            Self::TimedOut { seconds } => {
                write!(f, "exceeded the bounded {seconds}s execution window")
            }
            Self::Exited { code, diagnostic } => {
                let code = code.map_or_else(|| "by signal".to_string(), |value| value.to_string());
                write!(f, "exited {code}: {diagnostic}")
            }
        }
    }
}

trait SetupCommandRunner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        timeout: std::time::Duration,
    ) -> Result<(), SetupCommandFailure>;
}

struct SystemSetupCommandRunner;

struct CapturedSetupStream {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_setup_stream(mut stream: impl std::io::Read) -> std::io::Result<CapturedSetupStream> {
    let mut bytes = Vec::with_capacity(MAX_SETUP_DIAGNOSTIC_BYTES);
    let mut buffer = [0_u8; 4096];
    let mut truncated = false;
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let keep = read.min(MAX_SETUP_DIAGNOSTIC_BYTES.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    Ok(CapturedSetupStream { bytes, truncated })
}

fn redact_setup_diagnostic(raw: &str) -> String {
    let mut redacted = Vec::new();
    for word in raw.split_whitespace() {
        let token_like = (word.contains("mesh:") && word.contains('#'))
            || word.contains("MDEINV1-")
            || word.contains("mde-invite:");
        redacted.push(if token_like { "<redacted-token>" } else { word });
    }
    redacted.join(" ")
}

fn materialize_setup_stream(captured: CapturedSetupStream) -> String {
    let mut text = redact_setup_diagnostic(&String::from_utf8_lossy(&captured.bytes));
    if captured.truncated {
        text.push_str(" [output truncated]");
    }
    text
}

fn signal_setup_process_group(pid: u32, signal: &str) {
    #[cfg(unix)]
    {
        let process_group = format!("-{pid}");
        let mut kill = std::process::Command::new("/usr/bin/kill");
        kill.args([signal, "--", &process_group])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        mackesd_core::lifecycle_child_env::strip_lifecycle_child_env(&mut kill);
        let _ = kill.status();
    }
    #[cfg(not(unix))]
    let _ = (pid, signal);
}

fn terminate_setup_child(child: &mut std::process::Child) {
    signal_setup_process_group(child.id(), "-TERM");
    let deadline = std::time::Instant::now() + SETUP_TERMINATE_GRACE;
    while std::time::Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(SETUP_POLL_INTERVAL);
    }
    // The shell may exit before a dnf/systemctl descendant. Signal the group
    // even after the direct child is reaped so no helper subprocess escapes.
    signal_setup_process_group(child.id(), "-KILL");
    let _ = child.kill();
    let _ = child.wait();
}

fn join_setup_stream(
    name: &str,
    reader: std::thread::JoinHandle<std::io::Result<CapturedSetupStream>>,
) -> Result<String, SetupCommandFailure> {
    let captured = reader
        .join()
        .map_err(|_| SetupCommandFailure::Wait(format!("{name} reader panicked")))?
        .map_err(|error| SetupCommandFailure::Wait(format!("reading {name}: {error}")))?;
    Ok(materialize_setup_stream(captured))
}

impl SetupCommandRunner for SystemSetupCommandRunner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        timeout: std::time::Duration,
    ) -> Result<(), SetupCommandFailure> {
        let mut command = std::process::Command::new(program);
        command
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        mackesd_core::lifecycle_child_env::strip_lifecycle_child_env(&mut command);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            // A helper may be waiting on dnf/systemctl. Give it a fresh process
            // group so the timeout reaps descendants too, not just the shell.
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|error| SetupCommandFailure::Spawn(error.to_string()))?;
        let stdout = child.stdout.take().ok_or_else(|| {
            terminate_setup_child(&mut child);
            SetupCommandFailure::Wait("stdout pipe unavailable".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            terminate_setup_child(&mut child);
            SetupCommandFailure::Wait("stderr pipe unavailable".to_string())
        })?;
        let stdout_reader = std::thread::spawn(move || read_setup_stream(stdout));
        let stderr_reader = std::thread::spawn(move || read_setup_stream(stderr));

        let started = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= timeout => {
                    terminate_setup_child(&mut child);
                    let _ = join_setup_stream("stdout", stdout_reader);
                    let _ = join_setup_stream("stderr", stderr_reader);
                    return Err(SetupCommandFailure::TimedOut {
                        seconds: timeout.as_secs(),
                    });
                }
                Ok(None) => std::thread::sleep(SETUP_POLL_INTERVAL),
                Err(error) => {
                    terminate_setup_child(&mut child);
                    let _ = join_setup_stream("stdout", stdout_reader);
                    let _ = join_setup_stream("stderr", stderr_reader);
                    return Err(SetupCommandFailure::Wait(error.to_string()));
                }
            }
        };
        let stdout = join_setup_stream("stdout", stdout_reader)?;
        let stderr = join_setup_stream("stderr", stderr_reader)?;
        if status.success() {
            return Ok(());
        }
        let diagnostic = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        Err(SetupCommandFailure::Exited {
            code: status.code(),
            diagnostic: if diagnostic.trim().is_empty() {
                "no diagnostic output".to_string()
            } else {
                diagnostic
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkstationSetupPlan {
    anchors_csv: String,
    anchor_count: usize,
    overlay_ip: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WorkstationSetupState {
    Ready,
    Degraded { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JoinRoleResolution {
    effective: mde_role::Role,
    pin_required: bool,
}

fn parse_requested_join_role(role: Option<&str>) -> anyhow::Result<Option<mde_role::Role>> {
    role.map(|value| {
        value
            .parse()
            .map_err(|_| anyhow::anyhow!("unknown role value — expected lighthouse|workstation"))
    })
    .transpose()
}

fn resolve_join_role(
    pinned: Option<mde_role::Role>,
    requested: Option<mde_role::Role>,
) -> Result<JoinRoleResolution, String> {
    match (pinned, requested) {
        (Some(effective), Some(requested)) if effective != requested => Err(format!(
            "role is already pinned as `{effective}`; refusing conflicting `--role {requested}`. Re-role the node explicitly before joining"
        )),
        (Some(effective), _) => Ok(JoinRoleResolution {
            effective,
            pin_required: false,
        }),
        (None, requested) => Ok(JoinRoleResolution {
            effective: requested.unwrap_or(mde_role::Role::Workstation),
            pin_required: true,
        }),
    }
}

fn load_join_role(requested: Option<mde_role::Role>) -> anyhow::Result<JoinRoleResolution> {
    let pinned = match mde_role::load() {
        Ok(existing) => Some(existing),
        Err(mde_role::LoadError::NotPinned) => None,
        Err(error) => anyhow::bail!("reading role: {error}"),
    };
    resolve_join_role(pinned, requested).map_err(anyhow::Error::msg)
}

fn enroll_tui_command() -> std::process::Command {
    let mut command = std::process::Command::new("mde-enroll");
    mackesd_core::lifecycle_child_env::strip_lifecycle_child_env(&mut command);
    command
}

fn persist_join_role(resolution: JoinRoleResolution) -> anyhow::Result<()> {
    if resolution.pin_required {
        mde_role::pin(resolution.effective)
            .map_err(|error| anyhow::anyhow!("pinning role: {error}"))?;
        println!("role pinned: {}", resolution.effective.as_str());
    } else {
        println!("role already pinned: {}", resolution.effective);
    }
    Ok(())
}

fn parse_ipv4_cidr(cidr: &str) -> Result<(u32, u32), String> {
    let (network, prefix) = cidr
        .split_once('/')
        .ok_or_else(|| format!("mesh CIDR `{cidr}` has no prefix"))?;
    let network = network
        .parse::<std::net::Ipv4Addr>()
        .map_err(|_| format!("mesh CIDR `{cidr}` is not IPv4"))?;
    let prefix = prefix
        .parse::<u32>()
        .map_err(|_| format!("mesh CIDR `{cidr}` has an invalid prefix"))?;
    if prefix > 32 {
        return Err(format!("mesh CIDR `{cidr}` has an invalid prefix"));
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    Ok((u32::from(network) & mask, mask))
}

fn workstation_setup_plan(
    bundle: &mackesd_core::ca::bundle::NebulaBundle,
) -> Result<WorkstationSetupPlan, String> {
    if bundle.lighthouses.is_empty() {
        return Err("the signed lighthouse overlay roster is empty".to_string());
    }
    if bundle.lighthouses.len() > MAX_WORKSTATION_LIGHTHOUSE_ANCHORS {
        return Err(format!(
            "the signed lighthouse overlay roster has {} entries (maximum {MAX_WORKSTATION_LIGHTHOUSE_ANCHORS})",
            bundle.lighthouses.len()
        ));
    }
    let (network, mask) = parse_ipv4_cidr(&bundle.mesh_cidr)?;
    let overlay_ip = bundle
        .overlay_ip
        .parse::<std::net::Ipv4Addr>()
        .map_err(|_| "the signed Workstation overlay address is not IPv4".to_string())?;
    if u32::from(overlay_ip) & mask != network {
        return Err("the signed Workstation overlay address is outside the mesh CIDR".to_string());
    }

    let mut anchors = std::collections::BTreeSet::new();
    for (index, lighthouse) in bundle.lighthouses.iter().enumerate() {
        let anchor = lighthouse
            .overlay_ip
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| {
                format!(
                    "signed lighthouse roster entry {} has a non-IPv4 overlay address",
                    index + 1
                )
            })?;
        if u32::from(anchor) & mask != network {
            return Err(format!(
                "signed lighthouse roster entry {} is outside the mesh CIDR",
                index + 1
            ));
        }
        if anchor == overlay_ip {
            return Err(format!(
                "signed lighthouse roster entry {} reuses the Workstation overlay address",
                index + 1
            ));
        }
        anchors.insert(anchor);
    }
    let anchors_csv = anchors
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    Ok(WorkstationSetupPlan {
        anchors_csv,
        anchor_count: anchors.len(),
        overlay_ip: overlay_ip.to_string(),
    })
}

fn setup_workstation_substrate<R: SetupCommandRunner>(
    bundle: &mackesd_core::ca::bundle::NebulaBundle,
    runner: &R,
) -> anyhow::Result<WorkstationSetupState> {
    let plan = workstation_setup_plan(bundle).map_err(|reason| {
        anyhow::anyhow!(
            "workstation fleet activation failed closed after network enrollment: {reason}; no setup helper was run. Recovery requires a corrected signed roster and a newly minted enrollment token after the signer is repaired"
        )
    })?;
    let etcd_args = vec![
        "--client-only".to_string(),
        "--anchors".to_string(),
        plan.anchors_csv.clone(),
    ];
    runner
        .run(SETUP_ETCD, &etcd_args, WORKSTATION_SETUP_TIMEOUT)
        .map_err(|failure| {
            anyhow::anyhow!(
                "workstation fleet activation failed closed after network enrollment: client-only etcd setup {failure}; Syncthing was not attempted. Recover without re-enrollment: fix the packaged-helper or systemd error, then rerun `{SETUP_ETCD} --client-only --anchors {}` followed by `{SETUP_SYNCTHING} --listen {}`; both helpers are idempotent and receive no enrollment token",
                plan.anchors_csv,
                plan.overlay_ip,
            )
        })?;
    println!(
        "workstation fleet: etcd client configured from {} signed lighthouse anchor(s)",
        plan.anchor_count
    );

    let syncthing_args = vec!["--listen".to_string(), plan.overlay_ip.clone()];
    match runner.run(SETUP_SYNCTHING, &syncthing_args, WORKSTATION_SETUP_TIMEOUT) {
        Ok(()) => Ok(WorkstationSetupState::Ready),
        Err(failure) => Ok(WorkstationSetupState::Degraded {
            reason: format!(
                "Syncthing setup {failure}; overlay and etcd coordination remain configured, and the single-use enrollment token is consumed. Recover without re-enrollment by fixing the packaged-helper or systemd error and rerunning `{SETUP_SYNCTHING} --listen {}`; the helper is idempotent and receives no enrollment token",
                plan.overlay_ip,
            ),
        }),
    }
}

fn finish_network_enrollment<R, E>(
    role: mde_role::Role,
    bundle: &mackesd_core::ca::bundle::NebulaBundle,
    runner: &R,
    mut enable_service: E,
) -> anyhow::Result<Option<WorkstationSetupState>>
where
    R: SetupCommandRunner,
    E: FnMut(&str),
{
    // Once network enrollment succeeds, its bearer cannot be reused. Bring up
    // the control daemon and health watchdog before optional role-specific
    // helpers so a helper failure cannot strand a signed node without its
    // essential recovery services.
    enable_service("mackesd.service");
    enable_service("mesh-health.timer");

    if role != mde_role::Role::Workstation {
        return Ok(None);
    }

    setup_workstation_substrate(bundle, runner)
        .map(Some)
        .map_err(|error| {
            anyhow::anyhow!(
                "network enrollment succeeded and the single-use enrollment token was consumed; do not retry that token. Activation of mackesd and mesh-health was attempted before Workstation setup, but Workstation activation is incomplete: {error}"
            )
        })
}

/// OW-4 — redeem a wizard-minted `MDEINV1-…` invite (or its `mde-invite:` QR
/// twin) on the join side. Validates the presented code — mesh-scope + TTL
/// offline, then the bearer ledger — and maps it to the same v3 CSR the
/// lighthouse signs (`invite::redeem`). The MDEINV1 envelope is endpoint-less
/// by design (a code is presented over many transports and stays QR-short), so
/// the live network-enroll leg (CSR → signed bundle → overlay IP) is
/// integration-gated with a typed error rather than faked: a code alone cannot
/// contact a lighthouse. The operator completes a live join with the
/// endpoint-bearing v3 token from `mackesd found`.
fn cmd_join_invite(
    raw_token: &str,
    requested_role: Option<mde_role::Role>,
    _name: Option<String>,
    workgroup_root: Option<PathBuf>,
) -> anyhow::Result<()> {
    use mackesd_core::onboard::invite;

    let root = workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
    let node_id = default_node_id();

    // Decode up front to learn the invite's declared mesh: a box already on a
    // mesh must present an invite FOR that mesh (cross-mesh codes refused),
    // while a fresh box ADOPTS the mesh the invite names.
    let decoded = invite::Invite::decode(raw_token)
        .ok_or_else(|| anyhow::anyhow!("invite refused: {}", invite::RedeemError::Malformed))?;
    let founded = mackesd_core::ca::bundle::read_bundle(&mackesd_core::ca::bundle::bundle_path(
        &root, &node_id,
    ))
    .is_ok();
    let expected_mesh = if founded {
        invite::resolve_mesh_id(&root, &node_id)
    } else {
        decoded.mesh_id
    };

    // Resolve a pre-existing pin before the invite ledger is mutated. A
    // conflicting explicit role is refused while the invite remains usable.
    let role = load_join_role(requested_role)?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));

    // Validate: mesh-scope + TTL + the bearer ledger. Expired / foreign /
    // tampered codes are refused here with a typed error, never a panic.
    let redeemed = invite::validate_for_redeem(&root, raw_token, now_ms, &expected_mesh)
        .map_err(|e| anyhow::anyhow!("invite refused ({}): {e}", e.reason()))?;

    // Validation records the one-time invite redemption. Pin only after it
    // succeeds; invalid invites must not mutate the local role.
    persist_join_role(role)?;

    // Validated — but the envelope has no `/enroll` endpoint, so the live enroll
    // leg needs the lighthouse address the invite cannot supply. Gate it
    // honestly rather than fake an endpoint: the redemption mapping is proven by
    // unit tests to yield the same v3 CSR inputs, and the 2-box network leg is
    // integration-gated.
    anyhow::bail!(
        "invite for mesh `{}` validated (live + ledger-recorded) — its redemption \
         maps to the same v3 CSR the lighthouse signs, but an MDEINV1 code is \
         endpoint-less; the live enroll leg needs the lighthouse `/enroll` endpoint. \
         Complete a network join now with the endpoint-bearing token from \
         `mackesd found` (mesh:<id>@<ip>:<port>#<bearer>?fp=<sha256>). [OW-4]",
        redeemed.mesh_id,
    );
}

/// ONBOARD-4 — the `join` verb. One-command peer join: pin role +
/// fingerprint-pinned network-enroll + materialize /etc/nebula.
pub fn run(
    token: Option<String>,
    role: Option<&str>,
    name: Option<String>,
    workgroup_root: Option<PathBuf>,
) -> anyhow::Result<()> {
    use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecyclePlanV1};
    let root = workgroup_root
        .clone()
        .unwrap_or_else(mackesd_core::default_qnm_shared_root);
    let node_id = default_node_id();
    let generation = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().max(1))
        .unwrap_or(1);
    let lifecycle_plan = LifecyclePlanV1 {
        schema_version: 1,
        request_id: format!("join-{node_id}-{generation}"),
        target_id: node_id,
        intent: LifecycleIntentKind::Onboard,
        generation,
        // Token redemption, role persistence, network enrollment, and setup
        // are one atomic enrollment boundary from this CLI's perspective.
        steps: vec!["mesh".into()],
    };
    let mut authority =
        mackesd_core::lifecycle_authority::LifecycleAuthority::begin(&root, lifecycle_plan)
            .map_err(|error| {
                anyhow::anyhow!("cannot acquire lifecycle authority for join: {error:?}")
            })?;
    let result = authority.run_next(|_| {
        run_inner(token, role, name, workgroup_root.clone()).map_err(|error| error.to_string())
    });
    if let Err(error) = result {
        let _ = authority.finish();
        return Err(anyhow::anyhow!(
            "join lifecycle authority recorded failure: {error:?}"
        ));
    }
    authority
        .finish()
        .map_err(|error| anyhow::anyhow!("cannot release lifecycle authority: {error:?}"))
}

fn run_inner(
    token: Option<String>,
    role: Option<&str>,
    name: Option<String>,
    workgroup_root: Option<PathBuf>,
) -> anyhow::Result<()> {
    // No token → hand off to the enrollment TUI (ONBOARD-5, `mde-enroll`).
    let token = token.or_else(|| {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            None
        } else {
            let mut raw = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin().lock(), &mut raw).ok()?;
            let raw = raw.trim().to_string();
            (!raw.is_empty()).then_some(raw)
        }
    });
    let Some(raw_token) = token else {
        let launched = enroll_tui_command().status();
        return match launched {
            Ok(s) if s.success() => Ok(()),
            _ => Err(anyhow::anyhow!(
                "no token given and the `mde-enroll` TUI isn't on PATH. \
                 Pass the token from `mackesd found`:\n  mackesd join '<token>'"
            )),
        };
    };

    let requested_role = parse_requested_join_role(role)?;

    // OW-4 — a wizard-minted `MDEINV1-…` invite (or its `mde-invite:` QR twin) is
    // a DIFFERENT token type than the v3 `mesh:<id>@<ip>:<port>#<bearer>` join
    // token, so `parse_join_token` would reject it. Redeem it on this branch:
    // validate mesh-scope + TTL + the bearer ledger, then gate the endpoint-
    // needing live leg (the envelope is endpoint-less by design).
    if mackesd_core::onboard::invite::looks_like_invite(&raw_token) {
        return cmd_join_invite(&raw_token, requested_role, name, workgroup_root);
    }

    let token = mackesd_core::nebula_enroll::parse_join_token(&raw_token).ok_or_else(|| {
        anyhow::anyhow!("invalid join token (expected mesh:<id>@<ip>:<port>#<bearer>?fp=<sha256>)")
    })?;
    let root = workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
    let node_id = default_node_id();
    let display_name = name.unwrap_or_else(|| {
        node_id
            .strip_prefix("peer:")
            .unwrap_or(&node_id)
            .to_string()
    });

    // The on-disk pin is authoritative during rejoin. Role omission defaults
    // to Workstation only for a genuinely fresh node; an explicit conflict is
    // rejected before either legacy or network enrollment can consume a token.
    let role = load_join_role(requested_role)?;
    persist_join_role(role)?;
    let effective_role = role.effective;

    if token.fp.is_none() {
        // No fingerprint → legacy co-located QNM-Shared flow (the network
        // path requires the pinned fp). Honest fallback, not an error.
        println!("token has no fingerprint — using the co-located QNM-Shared enroll flow");
        let outcome = mackesd_core::nebula_enroll::enroll_with_token(
            &root,
            &node_id,
            &display_name,
            &raw_token,
        )
        .map_err(|e| anyhow::anyhow!("enroll: {e}"))?;
        println!(
            "enrolled into `{}` as {} (waited {:?})",
            outcome.mesh_id, outcome.overlay_ip, outcome.waited
        );
        return Ok(());
    }

    // Network enroll (the MESH-1 fix) — runs on a small async runtime.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building async runtime for network enroll")?;
    let config_dir = std::path::PathBuf::from("/etc/nebula");
    let bundle = runtime.block_on(mackesd_core::nebula_enroll_client::network_enroll(
        &root,
        &config_dir,
        &node_id,
        &display_name,
        token,
    ))?;
    // The bearer has completed its single use. Drop the only remaining encoded
    // copy before any setup subprocess is spawned; helpers receive overlay IPs
    // only, and no token can reach argv, stdout, or diagnostics.
    drop(raw_token);

    // Bring the peer fully live + boot-durable (ONBOARD-9): the overlay, the
    // worker daemon, and the health watchdog — not just nebula. A `join` now
    // leaves a node that survives reboot and self-recovers, instead of one the
    // operator must `systemctl restart mackesd` by hand.
    enable_now_service("nebula.service");

    // CONNECT-4 — if this peer joined as a Lighthouse, it's an ingress node too.
    provision_caddy_if_lighthouse(effective_role);

    // LIGHTHOUSE-10 — an ADDITIONAL lighthouse (the 2nd–5th) persists its own
    // public underlay address so its heartbeat publishes it to the directory and
    // every node's enroll roster includes it (full redundancy). Auto-detect the
    // primary public IPv4 (override later with `mackesd set-external-addr`).
    if effective_role == mde_role::Role::Lighthouse {
        match detect_primary_ipv4() {
            Ok(ip) => {
                if let Err(e) =
                    mackesd_core::lighthouse_addr::write_external_addr(&format!("{ip}:4242"))
                {
                    eprintln!(
                        "join: could not persist external-addr ({e}) — set it with `mackesd set-external-addr`"
                    );
                }
            }
            Err(e) => eprintln!(
                "join: could not auto-detect public IP ({e}) — run `mackesd set-external-addr <ip:4242>` so this lighthouse is reachable"
            ),
        }
        // HA / turn-key — a new lighthouse auto-joins the etcd quorum as a voter
        // (no manual `etcdctl member add`). Best-effort: failure logs an
        // actionable message and the enrolled lighthouse still comes up.
        lighthouse_join_etcd(&bundle, &display_name);

        // MIG-3 — a joined lighthouse inherits the mesh CA (same mesh,
        // same signing key as the founder), so it will hold ca.key and
        // the backup worker would otherwise loud-warn SEC-7/ENT-11
        // "UNBACKED-UP" every boot. Provision the sealed CA-backup
        // passphrase credential now (generated-on-joiner, host-bound via
        // systemd-creds — never transmitted off this box) + write the
        // LoadCredentialEncrypted drop-in so the upcoming mackesd restart
        // picks it up. Best-effort: a miss logs an actionable line but
        // never aborts the join.
        provision_ca_backup_passphrase_if_lighthouse(effective_role);
    }

    // SETUP-7 — capture the joined facts (mesh-id + lighthouse roster from the
    // signed bundle) for idempotent re-convergence.
    let roster: Vec<String> = bundle
        .lighthouses
        .iter()
        .map(|lh| lh.overlay_ip.clone())
        .collect();
    emit_site_yml_best_effort(effective_role.as_str(), &bundle.mesh_id, roster);

    // A Workstation is not a fleet member merely because Nebula has a signed
    // identity. Materialize its etcd client endpoints from the signer-provided
    // overlay roster, then stand up the Syncthing file plane. Both packaged
    // helpers are idempotent; execution is timeout-bounded and never receives
    // the enrollment token. Coordination fails closed. A file-plane miss is
    // reported as degraded while the usable overlay/control plane comes up.
    let workstation_setup = finish_network_enrollment(
        effective_role,
        &bundle,
        &SystemSetupCommandRunner,
        enable_now_service,
    )?;

    println!(
        "joined `{}` as {} (overlay {})",
        bundle.mesh_id, node_id, bundle.overlay_ip
    );
    println!("services: nebula + mackesd + mesh-health enabled (boot-durable) and running");
    match workstation_setup {
        Some(WorkstationSetupState::Ready) => {
            println!("workstation fleet: etcd client + Syncthing configured (boot-durable)");
        }
        Some(WorkstationSetupState::Degraded { reason }) => {
            eprintln!("workstation fleet: DEGRADED — {reason}");
        }
        None => {}
    }
    Ok(())
}

/// HA / turn-key — a freshly-joined lighthouse auto-joins the etcd quorum as a
/// voter via the native member API ([`mackesd_core::substrate::etcd_membership`]),
/// then starts its local etcd via `setup-etcd --join --initial-cluster`. The
/// anchors are the EXISTING lighthouses from the signed bundle. Best-effort with a
/// short retry for the just-brought-up overlay handshake; on failure it prints the
/// exact manual command and returns — the lighthouse is enrolled either way.
fn lighthouse_join_etcd(bundle: &mackesd_core::ca::bundle::NebulaBundle, self_name: &str) {
    use mackesd_core::substrate::etcd_membership;
    let self_overlay = bundle.overlay_ip.clone();
    let anchor_overlay = bundle
        .lighthouses
        .iter()
        .map(|lh| lh.overlay_ip.clone())
        .find(|ip| ip != &self_overlay);
    let Some(anchor_overlay) = anchor_overlay else {
        eprintln!(
            "join: no existing lighthouse anchor in the bundle — skipping etcd auto-join \
             (a founding lighthouse bootstraps etcd with `setup-etcd --init`)"
        );
        return;
    };
    let anchors: Vec<String> = bundle
        .lighthouses
        .iter()
        .filter(|lh| lh.overlay_ip != self_overlay)
        .map(|lh| etcd_membership::client_url(&lh.overlay_ip))
        .collect();
    let mut last = String::new();
    for attempt in 1..=5 {
        match etcd_membership::add_self_as_voter_blocking(&anchors, self_name, &self_overlay) {
            Some(Ok(csv)) => {
                let mut setup = std::process::Command::new("/usr/libexec/mackesd/setup-etcd");
                setup.args([
                    "--join",
                    &anchor_overlay,
                    "--listen",
                    &self_overlay,
                    "--initial-cluster",
                    &csv,
                ]);
                mackesd_core::lifecycle_child_env::strip_lifecycle_child_env(&mut setup);
                let st = setup.status();
                match st {
                    Ok(s) if s.success() => {
                        println!(
                            "etcd: joined the quorum as a voter (member added + local etcd started)"
                        );
                    }
                    _ => eprintln!(
                        "etcd: member added but `setup-etcd --join` failed — start the local \
                         member by hand: /usr/libexec/mackesd/setup-etcd --join {anchor_overlay} \
                         --listen {self_overlay}"
                    ),
                }
                return;
            }
            Some(Err(e)) => last = e,
            None => last = "bridge runtime unavailable".to_string(),
        }
        if attempt < 5 {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    }
    eprintln!(
        "join: etcd auto-join did not complete ({last}) — the lighthouse is enrolled; add it to \
         the quorum once the overlay is up: /usr/libexec/mackesd/setup-etcd --join {anchor_overlay} \
         --listen {self_overlay}"
    );
}

/// MIG-3 — on a joined Lighthouse, ensure a sealed CA-backup passphrase
/// credential exists so the box boots without the SEC-7/ENT-11
/// "UNBACKED-UP" warning. The passphrase is GENERATED locally + sealed
/// host-bound via systemd-creds (TPM/host key) — it never leaves this
/// box and is never logged (only its presence/length). No-op for
/// non-lighthouse roles + idempotent (never rotates an existing cred).
///
/// The OFF-FLEET / off-site CA-backup push is intentionally NOT touched
/// here — that remains an operator-run step. This only clears the
/// "no backup passphrase credential" boot error.
///
/// Best-effort + idempotent: a miss logs an actionable line but never
/// aborts the join (the lighthouse still joins; the worker keeps
/// warning until the operator provisions it by hand per the unit
/// comment).
fn provision_ca_backup_passphrase_if_lighthouse(role: mde_role::Role) {
    use mackesd_core::ca::backup_provision::{ProvisionOutcome, provision};
    match provision(role) {
        Ok(ProvisionOutcome::Provisioned { sealed_bytes }) => {
            // Log presence/length only — NEVER the passphrase value.
            println!(
                "MIG-3: sealed CA-backup passphrase provisioned ({sealed_bytes}-byte credential) — CA no longer UNBACKED-UP"
            );
            // The drop-in is new; reload so the upcoming mackesd.service
            // (re)start surfaces $CREDENTIALS_DIRECTORY/backup-passphrase.
            let mut reload = std::process::Command::new("systemctl");
            reload.arg("daemon-reload");
            mackesd_core::lifecycle_child_env::strip_lifecycle_child_env(&mut reload);
            let _ = reload.status();
        }
        Ok(ProvisionOutcome::AlreadyPresent) => {
            println!("MIG-3: CA-backup passphrase credential already present — left untouched");
        }
        Ok(ProvisionOutcome::NotLighthouse) => {}
        Err(e) => eprintln!(
            "MIG-3: could not provision the CA-backup passphrase ({e}) — this lighthouse will \
             warn SEC-7/ENT-11 until you provision it by hand (see the EFF-15 comment in the \
             mackesd.service unit)"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Invocation {
        program: String,
        args: Vec<String>,
        timeout: std::time::Duration,
    }

    struct FakeRunner {
        outcomes: RefCell<VecDeque<Result<(), SetupCommandFailure>>>,
        invocations: RefCell<Vec<Invocation>>,
    }

    impl FakeRunner {
        fn new(outcomes: Vec<Result<(), SetupCommandFailure>>) -> Self {
            Self {
                outcomes: RefCell::new(outcomes.into()),
                invocations: RefCell::new(Vec::new()),
            }
        }
    }

    impl SetupCommandRunner for FakeRunner {
        fn run(
            &self,
            program: &str,
            args: &[String],
            timeout: std::time::Duration,
        ) -> Result<(), SetupCommandFailure> {
            self.invocations.borrow_mut().push(Invocation {
                program: program.to_string(),
                args: args.to_vec(),
                timeout,
            });
            self.outcomes
                .borrow_mut()
                .pop_front()
                .expect("fake runner needs one outcome per invocation")
        }
    }

    fn bundle(lighthouses: &[(&str, &str)]) -> mackesd_core::ca::bundle::NebulaBundle {
        mackesd_core::ca::bundle::NebulaBundle {
            mesh_id: "surface-mesh".to_string(),
            epoch: 7,
            ca_cert_pem: "public-ca".to_string(),
            peer_cert_pem: "public-peer".to_string(),
            overlay_ip: "10.42.0.79".to_string(),
            mesh_cidr: "10.42.0.0/16".to_string(),
            lighthouses: lighthouses
                .iter()
                .map(
                    |(node_id, overlay_ip)| mackesd_core::ca::bundle::LighthouseEntry {
                        node_id: (*node_id).to_string(),
                        overlay_ip: (*overlay_ip).to_string(),
                        external_addr: "203.0.113.10:4242".to_string(),
                        relay_tls: None,
                    },
                )
                .collect(),
            relay_trust_authority: None,
            created_at: 1_786_000_000,
        }
    }

    #[test]
    fn omitted_role_preserves_a_pinned_lighthouse_and_runs_no_workstation_helper() {
        let resolved = resolve_join_role(Some(mde_role::Role::Lighthouse), None)
            .expect("the existing pin is authoritative");
        assert_eq!(resolved.effective, mde_role::Role::Lighthouse);
        assert!(!resolved.pin_required);

        let runner = FakeRunner::new(vec![]);
        let mut services = Vec::new();
        let state = finish_network_enrollment(
            resolved.effective,
            &bundle(&[("peer:lh1", "10.42.0.1")]),
            &runner,
            |service| services.push(service.to_string()),
        )
        .expect("Lighthouse finalization");
        assert_eq!(state, None);
        assert!(runner.invocations.borrow().is_empty());
        assert_eq!(
            services,
            vec![
                "mackesd.service".to_string(),
                "mesh-health.timer".to_string()
            ]
        );
    }

    #[test]
    fn conflicting_explicit_role_is_refused_before_enrollment() {
        let error = resolve_join_role(
            Some(mde_role::Role::Lighthouse),
            Some(mde_role::Role::Workstation),
        )
        .expect_err("an explicit role cannot override the pin");
        assert!(error.contains("already pinned as `lighthouse`"), "{error}");
        assert!(error.contains("refusing conflicting"), "{error}");
    }

    #[test]
    fn omitted_role_defaults_to_workstation_only_when_unpinned() {
        assert_eq!(
            resolve_join_role(None, None).expect("fresh-node default"),
            JoinRoleResolution {
                effective: mde_role::Role::Workstation,
                pin_required: true,
            }
        );
    }

    #[test]
    fn invalid_role_diagnostic_never_echoes_token_shaped_input() {
        let token_shaped = "mesh:test@192.0.2.1:4243#bearer-value?fp=abcd";
        let error = parse_requested_join_role(Some(token_shaped))
            .expect_err("invalid role")
            .to_string();
        assert!(error.contains("unknown role value"), "{error}");
        assert!(!error.contains("bearer-value"), "{error}");
        assert!(!error.contains("mesh:"), "{error}");
    }

    #[test]
    fn helper_failure_after_enrollment_keeps_essential_activation_and_reports_recovery() {
        let runner = FakeRunner::new(vec![Err(SetupCommandFailure::Exited {
            code: Some(1),
            diagnostic: "etcd unit failed".to_string(),
        })]);
        let mut services = Vec::new();
        let error = finish_network_enrollment(
            mde_role::Role::Workstation,
            &bundle(&[("peer:lh1", "10.42.0.1")]),
            &runner,
            |service| services.push(service.to_string()),
        )
        .expect_err("required etcd setup fails")
        .to_string();

        assert_eq!(
            services,
            vec![
                "mackesd.service".to_string(),
                "mesh-health.timer".to_string()
            ]
        );
        assert_eq!(runner.invocations.borrow().len(), 1);
        assert!(error.contains("network enrollment succeeded"), "{error}");
        assert!(
            error.contains("single-use enrollment token was consumed"),
            "{error}"
        );
        assert!(error.contains("do not retry that token"), "{error}");
        assert!(error.contains("Recover without re-enrollment"), "{error}");
        assert!(error.contains(SETUP_ETCD), "{error}");
        assert!(error.contains(SETUP_SYNCTHING), "{error}");
        assert!(
            !error.contains("mesh:"),
            "diagnostic exposed a token shape: {error}"
        );
    }

    #[test]
    fn workstation_setup_uses_signed_roster_then_syncthing() {
        let runner = FakeRunner::new(vec![Ok(()), Ok(())]);
        let state = setup_workstation_substrate(
            &bundle(&[
                ("peer:lh3", "10.42.0.3"),
                ("peer:lh1", "10.42.0.1"),
                ("peer:lh3-duplicate", "10.42.0.3"),
            ]),
            &runner,
        )
        .expect("setup");
        assert_eq!(state, WorkstationSetupState::Ready);
        assert_eq!(
            *runner.invocations.borrow(),
            vec![
                Invocation {
                    program: SETUP_ETCD.to_string(),
                    args: vec![
                        "--client-only".to_string(),
                        "--anchors".to_string(),
                        "10.42.0.1,10.42.0.3".to_string(),
                    ],
                    timeout: WORKSTATION_SETUP_TIMEOUT,
                },
                Invocation {
                    program: SETUP_SYNCTHING.to_string(),
                    args: vec!["--listen".to_string(), "10.42.0.79".to_string()],
                    timeout: WORKSTATION_SETUP_TIMEOUT,
                },
            ]
        );
    }

    #[test]
    fn malformed_or_empty_signed_roster_fails_before_subprocesses() {
        for candidate in [
            bundle(&[]),
            bundle(&[("peer:bad", "not-an-ip")]),
            bundle(&[("peer:foreign", "10.43.0.1")]),
            bundle(&[("peer:collision", "10.42.0.79")]),
        ] {
            let runner = FakeRunner::new(vec![]);
            let error = setup_workstation_substrate(&candidate, &runner)
                .expect_err("malformed roster must fail closed")
                .to_string();
            assert!(error.contains("failed closed"), "{error}");
            assert!(runner.invocations.borrow().is_empty());
        }
    }

    #[test]
    fn etcd_failure_is_fail_closed_and_skips_syncthing() {
        let runner = FakeRunner::new(vec![Err(SetupCommandFailure::TimedOut { seconds: 300 })]);
        let error = setup_workstation_substrate(&bundle(&[("peer:lh1", "10.42.0.1")]), &runner)
            .expect_err("etcd is required")
            .to_string();
        assert!(error.contains("failed closed"), "{error}");
        assert!(error.contains("Syncthing was not attempted"), "{error}");
        assert!(error.contains("bounded 300s"), "{error}");
        assert!(error.contains("Recover without re-enrollment"), "{error}");
        assert_eq!(runner.invocations.borrow().len(), 1);
        assert_eq!(runner.invocations.borrow()[0].program, SETUP_ETCD);
    }

    #[test]
    fn syncthing_failure_reports_degraded_but_keeps_coordination() {
        let runner = FakeRunner::new(vec![
            Ok(()),
            Err(SetupCommandFailure::Exited {
                code: Some(1),
                diagnostic: "syncthing package unavailable".to_string(),
            }),
        ]);
        let state = setup_workstation_substrate(&bundle(&[("peer:lh1", "10.42.0.1")]), &runner)
            .expect("etcd succeeded");
        let WorkstationSetupState::Degraded { reason } = state else {
            panic!("expected degraded file plane");
        };
        assert!(reason.contains("syncthing package unavailable"));
        assert!(reason.contains("etcd coordination remain configured"));
        assert!(reason.contains("single-use enrollment token is consumed"));
        assert!(reason.contains("Recover without re-enrollment"));
        assert_eq!(runner.invocations.borrow().len(), 2);
    }

    #[test]
    fn workstation_setup_is_repeatable_with_identical_idempotent_commands() {
        let runner = FakeRunner::new(vec![Ok(()), Ok(()), Ok(()), Ok(())]);
        let signed = bundle(&[("peer:lh1", "10.42.0.1"), ("peer:lh2", "10.42.0.2")]);
        assert_eq!(
            setup_workstation_substrate(&signed, &runner).unwrap(),
            WorkstationSetupState::Ready
        );
        assert_eq!(
            setup_workstation_substrate(&signed, &runner).unwrap(),
            WorkstationSetupState::Ready
        );
        let calls = runner.invocations.borrow();
        assert_eq!(&calls[..2], &calls[2..]);
    }

    #[test]
    fn production_runner_enforces_timeout_and_reaps_the_child() {
        let started = std::time::Instant::now();
        let result = SystemSetupCommandRunner.run(
            "/usr/bin/sleep",
            &["30".to_string()],
            std::time::Duration::from_millis(100),
        );
        assert!(
            matches!(result, Err(SetupCommandFailure::TimedOut { .. })),
            "{result:?}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "timeout did not bound execution"
        );
    }

    #[test]
    fn system_setup_runner_strips_bootstrap_dest_env() {
        std::env::set_var("MACKESD_BOOTSTRAP_SSH_KEY", "/tmp/must-not-leak");
        std::env::set_var("MACKESD_BOOTSTRAP_KNOWN_HOSTS", "/tmp/must-not-leak-hosts");
        std::env::set_var("JOIN_TOKEN", "must-not-leak-token");
        let result = SystemSetupCommandRunner.run(
            "sh",
            &[
                "-c".to_string(),
                "test -z \"$MACKESD_BOOTSTRAP_SSH_KEY$MACKESD_BOOTSTRAP_KNOWN_HOSTS$JOIN_TOKEN\""
                    .to_string(),
            ],
            std::time::Duration::from_secs(2),
        );
        std::env::remove_var("MACKESD_BOOTSTRAP_SSH_KEY");
        std::env::remove_var("MACKESD_BOOTSTRAP_KNOWN_HOSTS");
        std::env::remove_var("JOIN_TOKEN");
        assert!(
            result.is_ok(),
            "setup helper inherited dest env: {result:?}"
        );
    }

    #[test]
    fn enroll_tui_strips_bootstrap_dest_env() {
        let mut command = std::process::Command::new("sh");
        command.args([
            "-c",
            "printf %s \"$MACKESD_BOOTSTRAP_SSH_KEY$MACKESD_BOOTSTRAP_KNOWN_HOSTS$JOIN_TOKEN\"",
        ]);
        command.env("MACKESD_BOOTSTRAP_SSH_KEY", "/tmp/must-not-leak");
        command.env("MACKESD_BOOTSTRAP_KNOWN_HOSTS", "/tmp/must-not-leak-hosts");
        command.env("JOIN_TOKEN", "must-not-leak-token");
        mackesd_core::lifecycle_child_env::strip_lifecycle_child_env(&mut command);
        let output = command.output().expect("run stripped child");
        assert!(output.status.success());
        assert!(
            output.stdout.is_empty(),
            "enroll TUI inherited dest env: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[test]
    fn helper_diagnostics_redact_token_shaped_values() {
        let diagnostic = redact_setup_diagnostic(
            "helper failed token=mesh:test@192.0.2.1:4243#bearer-value?fp=abcd; retry",
        );
        assert!(!diagnostic.contains("bearer-value"));
        assert!(diagnostic.contains("<redacted-token>"));
    }
}
