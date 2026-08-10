//! Root-only producer for one local Surface MOK import generation.

use anyhow::{ensure, Context};
use mackes_mesh_types::cloud::{cloud_request_digest, CloudArmSigner, CloudArmedToken};
use mackes_mesh_types::surface_hardware::{
    SurfaceActionHeader, SurfaceEnableRequest, SURFACE_HARDWARE_SCHEMA_VERSION,
};
use sha1::{Digest as _, Sha1};
use std::io::{BufRead as _, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use zeroize::{Zeroize, Zeroizing};

const CERTIFICATE: &str = "/usr/share/surface-secureboot/surface.cer";
const ACTIVATOR: &str = "/usr/libexec/mackesd/provision-surface-mok-import-credentials";
const INCOMING: &str = "/run/mcnf-surface-mok-import/incoming";
const MINT_LOCK: &str = "/run/mcnf-surface-mok-import/mint.lock";
const ENVELOPE_NAME: &str = "surface-mok-import.sealed";
const PASSPHRASE_NAME: &str = "surface-mok-import-passphrase";
const TTL_MS: i64 = 30_000;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(12);
const ACTIVATOR_TIMEOUT: Duration = Duration::from_secs(18);
const COMMAND_POLL: Duration = Duration::from_millis(25);
const COMMAND_TERMINATE_GRACE: Duration = Duration::from_secs(2);
const ACTIVATOR_TERMINATE_GRACE: Duration = Duration::from_secs(8);

#[derive(serde::Serialize)]
struct Permit<'a> {
    schema_version: u64,
    node: &'a str,
    request_id: &'a str,
    authorization_nonce: &'a str,
    expires_at_ms: u64,
    certificate_sha1: &'a str,
    password: &'a str,
}

pub fn run(node: &str) -> anyhow::Result<()> {
    ensure!(
        rustix::process::geteuid().is_root(),
        "Surface MOK minting requires root"
    );
    ensure!(
        node != "peer:unknown" && !node.is_empty(),
        "local node identity is unavailable"
    );
    let _mint_lock = acquire_mint_lock()?;

    let password = read_password()?;
    let certificate_sha1 = fixed_certificate_sha1()?;
    let signer =
        mackesd_core::ipc::action_auth::production_action_signer().map_err(anyhow::Error::msg)?;
    let was_active = service_is_active()?;
    let mut service_guard = ServiceRestoreGuard {
        was_active,
        committed: false,
    };

    // Stop before minting: a normal service stop may consume the whole token
    // TTL. The existing activator's later stop is then an immediate no-op.
    fixed_status(
        "/usr/bin/systemctl",
        &["--system", "stop", "mackesd-actions.service"],
    )
    .context("stopping the Surface action consumer before minting")?;

    let request_id = format!("surface-mok-{}", uuid::Uuid::new_v4());
    let nonce = uuid::Uuid::new_v4().to_string();
    let now = wall_now_ms()?;
    let password_text =
        std::str::from_utf8(password.as_slice()).context("password is not ASCII")?;
    let (request_body, plaintext) = build_bound_material(
        node,
        password_text,
        &certificate_sha1,
        &signer,
        now,
        &request_id,
        &nonce,
    )?;

    let passphrase = random_hex(32);
    let passphrase_text =
        std::str::from_utf8(passphrase.as_slice()).context("generated passphrase is not ASCII")?;
    let sealed = Zeroizing::new(
        mde_seal::seal_bytes(passphrase_text, plaintext.as_slice())
            .map_err(|_| anyhow::anyhow!("sealing Surface MOK permit failed"))?,
    );
    stage_encrypted_inputs(sealed.as_slice(), passphrase.as_slice())?;

    let activation_budget_ms = i64::try_from(ACTIVATOR_TIMEOUT.as_millis())
        .unwrap_or(i64::MAX)
        .saturating_add(1_000);
    ensure!(
        wall_now_ms()?.saturating_add(activation_budget_ms) < now.saturating_add(TTL_MS),
        "Surface MOK capability lacks enough lifetime for bounded activation"
    );
    fixed_status_bounded(ACTIVATOR, &["--activate-under-lock"], ACTIVATOR_TIMEOUT)
        .context("activating Surface MOK credentials")?;
    service_guard.committed = true;
    ensure!(
        wall_now_ms()? < now.saturating_add(TTL_MS),
        "Surface MOK capability expired before request publication"
    );
    let bus = mde_bus::persist::Persist::open(PathBuf::from("/run/mde-bus"))
        .context("opening the local Bus spool")?;
    let topic = format!("action/hardware/surface/{node}/enable");
    bus.write(
        &topic,
        mde_bus::hooks::config::Priority::Default,
        None,
        Some(&request_body),
    )
    .context("publishing the bound Surface enable request")?;
    println!("mint-surface-mok-import: submitted one request-bound local import");
    Ok(())
}

fn build_bound_material(
    node: &str,
    password: &str,
    certificate_sha1: &str,
    signer: &CloudArmSigner,
    now: i64,
    request_id: &str,
    nonce: &str,
) -> anyhow::Result<(String, Zeroizing<Vec<u8>>)> {
    let expires = now
        .checked_add(TTL_MS)
        .context("capability expiry overflow")?;
    let mut request = SurfaceEnableRequest {
        header: SurfaceActionHeader {
            schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
            node: node.to_owned(),
            request_id: request_id.to_owned(),
            issued_at_ms: u64::try_from(now).context("negative system clock")?,
            armed_token: None,
        },
        arm_token: None,
    };
    let unsigned = serde_json::to_string(&request).context("serializing Surface request")?;
    let token = CloudArmedToken::mint(
        signer,
        &nonce,
        expires,
        "surface-enable",
        node,
        node,
        &cloud_request_digest(&unsigned).map_err(anyhow::Error::msg)?,
    );
    request.header.armed_token = Some(token.encode());
    let request_body =
        serde_json::to_string(&request).context("serializing armed Surface request")?;

    let permit = Permit {
        schema_version: 1,
        node,
        request_id: &request_id,
        authorization_nonce: &nonce,
        expires_at_ms: u64::try_from(expires).context("negative permit expiry")?,
        certificate_sha1: &certificate_sha1,
        password,
    };
    let plaintext =
        Zeroizing::new(serde_json::to_vec(&permit).context("serializing sealed permit")?);
    Ok((request_body, plaintext))
}

fn read_password() -> anyhow::Result<Zeroizing<Vec<u8>>> {
    let mut bytes = Zeroizing::new(Vec::new());
    std::io::stdin()
        .lock()
        .take(18)
        .read_until(b'\n', &mut bytes)?;
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    ensure!(
        (8..=16).contains(&bytes.len()) && bytes.iter().all(u8::is_ascii_alphanumeric),
        "Surface MOK password must be 8-16 ASCII letters or digits"
    );
    Ok(bytes)
}

fn fixed_certificate_sha1() -> anyhow::Result<String> {
    let path = Path::new(CERTIFICATE);
    let meta = std::fs::symlink_metadata(path).context("inspecting fixed Surface certificate")?;
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink() && (1..65_536).contains(&meta.len()),
        "fixed Surface certificate is not a bounded regular file"
    );
    let der = std::fs::read(path).context("reading fixed Surface certificate")?;
    Ok(Sha1::digest(&der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":"))
}

fn acquire_mint_lock() -> anyhow::Result<std::fs::File> {
    use fs2::FileExt as _;
    use std::os::unix::fs::PermissionsExt as _;

    let root = Path::new(INCOMING)
        .parent()
        .context("Surface MOK runtime root is invalid")?;
    std::fs::create_dir_all(root)?;
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(MINT_LOCK)
        .context("opening the fixed Surface MOK mint lock")?;
    std::fs::set_permissions(MINT_LOCK, std::fs::Permissions::from_mode(0o600))?;
    file.lock_exclusive()
        .context("locking the Surface MOK mint boundary")?;
    Ok(file)
}

fn stage_encrypted_inputs(envelope: &[u8], passphrase: &[u8]) -> anyhow::Result<()> {
    let incoming = Path::new(INCOMING);
    ensure!(
        !incoming.exists(),
        "a prior Surface MOK credential generation is still staged"
    );
    let root = incoming
        .parent()
        .context("Surface MOK incoming path has no parent")?;
    let generation = uuid::Uuid::new_v4().to_string();
    let temp = root.join(format!(".incoming.{generation}"));
    std::fs::create_dir(&temp)?;
    std::fs::set_permissions(&temp, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    let generation_path = temp.join(".generation");
    std::fs::write(&generation_path, generation.as_bytes())?;
    std::fs::set_permissions(
        &generation_path,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )?;
    let envelope_path = temp.join(ENVELOPE_NAME);
    let passphrase_path = temp.join(PASSPHRASE_NAME);
    let staged = (|| -> anyhow::Result<()> {
        encrypt_credential(ENVELOPE_NAME, envelope, &envelope_path)?;
        encrypt_credential(PASSPHRASE_NAME, passphrase, &passphrase_path)?;
        for path in [&envelope_path, &passphrase_path] {
            std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        }
        publish_generation_directory(&temp, incoming)
    })();
    if staged.is_err() && temp.exists() {
        let _ = std::fs::remove_dir_all(&temp);
    }
    staged
}

fn publish_generation_directory(temp: &Path, incoming: &Path) -> anyhow::Result<()> {
    ensure!(
        !incoming.exists(),
        "a prior Surface MOK credential generation is still staged"
    );
    std::fs::rename(temp, incoming).context("atomically publishing Surface MOK generation")
}

fn encrypt_credential(name: &str, plaintext: &[u8], output: &Path) -> anyhow::Result<()> {
    let mut child = Command::new("/usr/bin/systemd-creds")
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin")
        .arg("encrypt")
        .arg(format!("--name={name}"))
        .arg("-")
        .arg(output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let write_result = child
        .stdin
        .take()
        .context("systemd-creds stdin unavailable")?
        .write_all(plaintext);
    if let Err(error) = write_result {
        kill_and_reap(&mut child);
        return Err(error).context("writing private systemd-creds stdin");
    }
    let status = wait_bounded(&mut child, COMMAND_TIMEOUT, COMMAND_TERMINATE_GRACE)?;
    ensure!(
        status.success(),
        "systemd-creds encryption failed for fixed credential {name}"
    );
    Ok(())
}

fn fixed_status(program: &str, args: &[&str]) -> anyhow::Result<()> {
    fixed_status_bounded(program, args, COMMAND_TIMEOUT)
}

fn service_is_active() -> anyhow::Result<bool> {
    let status = run_status_bounded(
        "/usr/bin/systemctl",
        &[
            "--system",
            "is-active",
            "--quiet",
            "mackesd-actions.service",
        ],
        COMMAND_TIMEOUT,
        COMMAND_TERMINATE_GRACE,
    )?;
    match status.code() {
        Some(0) => Ok(true),
        Some(3) => Ok(false),
        _ => anyhow::bail!("cannot determine prior Surface action service state"),
    }
}

struct ServiceRestoreGuard {
    was_active: bool,
    committed: bool,
}

impl Drop for ServiceRestoreGuard {
    fn drop(&mut self) {
        if self.was_active && !self.committed {
            let _ = fixed_status_bounded(
                "/usr/bin/systemctl",
                &["--system", "start", "--no-block", "mackesd-actions.service"],
                Duration::from_secs(3),
            );
        }
    }
}

fn fixed_status_bounded(program: &str, args: &[&str], timeout: Duration) -> anyhow::Result<()> {
    let grace = if program == ACTIVATOR {
        ACTIVATOR_TERMINATE_GRACE
    } else {
        COMMAND_TERMINATE_GRACE
    };
    let status = run_status_bounded(program, args, timeout, grace)?;
    ensure!(status.success(), "fixed privileged helper failed");
    Ok(())
}

fn run_status_bounded(
    program: &str,
    args: &[&str],
    timeout: Duration,
    terminate_grace: Duration,
) -> anyhow::Result<std::process::ExitStatus> {
    let mut child = Command::new(program)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    wait_bounded(&mut child, timeout, terminate_grace)
}

fn wait_bounded(
    child: &mut std::process::Child,
    timeout: Duration,
    terminate_grace: Duration,
) -> anyhow::Result<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            terminate_and_reap(child, terminate_grace);
            anyhow::bail!("fixed privileged helper exceeded its bounded runtime");
        }
        thread::sleep(COMMAND_POLL);
    }
}

fn kill_and_reap(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn terminate_and_reap(child: &mut std::process::Child, grace: Duration) {
    let _ = Command::new("/usr/bin/kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(COMMAND_POLL);
    }
    kill_and_reap(child);
}

fn random_hex(bytes: usize) -> Zeroizing<Vec<u8>> {
    use rand::RngCore as _;
    let mut raw = Zeroizing::new(vec![0_u8; bytes]);
    rand::rngs::OsRng.fill_bytes(&mut raw);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = Zeroizing::new(vec![0_u8; bytes.saturating_mul(2)]);
    for (index, byte) in raw.iter().copied().enumerate() {
        encoded[index * 2] = HEX[usize::from(byte >> 4)];
        encoded[index * 2 + 1] = HEX[usize::from(byte & 0x0f)];
    }
    raw.zeroize();
    encoded
}

fn wall_now_ms() -> anyhow::Result<i64> {
    let duration = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    i64::try_from(duration.as_millis()).context("system clock is beyond the capability range")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_construction_binds_request_token_and_sealed_permit() {
        let signer = CloudArmSigner::new(b"surface-mok-test-key".to_vec()).unwrap();
        let now = 1_700_000_000_000_i64;
        let request_id = "surface-mok-request-1";
        let nonce = "surface-mok-nonce-0123456789abcdef";
        let fingerprint = "01:23:45:67:89:AB:CD:EF:10:32:54:76:98:BA:DC:FE:11:22:33:44";
        let (body, permit) = build_bound_material(
            "peer:surface",
            "Secret123",
            fingerprint,
            &signer,
            now,
            request_id,
            nonce,
        )
        .unwrap();

        let request: SurfaceEnableRequest = serde_json::from_str(&body).unwrap();
        let token = CloudArmedToken::parse(request.header.armed_token.as_deref().unwrap()).unwrap();
        let permit: serde_json::Value = serde_json::from_slice(permit.as_slice()).unwrap();
        assert_eq!(request.header.node, "peer:surface");
        assert_eq!(request.header.request_id, request_id);
        assert_eq!(token.node, "peer:surface");
        assert_eq!(token.target, "peer:surface");
        assert_eq!(token.nonce, nonce);
        assert_eq!(token.expires_at_ms, now + TTL_MS);
        assert_eq!(permit["node"], token.node);
        assert_eq!(permit["request_id"], request_id);
        assert_eq!(permit["authorization_nonce"], token.nonce);
        assert_eq!(permit["expires_at_ms"], token.expires_at_ms);
        assert_eq!(permit["certificate_sha1"], fingerprint);
        assert_eq!(token.request_sha256, cloud_request_digest(&body).unwrap());
        assert!(signer.verify_payload(&token.signing_payload(), &token.signature));
    }

    #[test]
    fn generated_sealing_passphrase_is_bounded_hex() {
        let passphrase = random_hex(32);
        assert_eq!(passphrase.len(), 64);
        assert!(passphrase.iter().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn generation_directory_publish_is_atomic_and_refuses_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let incoming = root.path().join("incoming");
        let first = root.path().join(".incoming.first");
        std::fs::create_dir(&first).unwrap();
        std::fs::write(first.join(".generation"), b"first").unwrap();
        publish_generation_directory(&first, &incoming).unwrap();
        assert_eq!(
            std::fs::read(incoming.join(".generation")).unwrap(),
            b"first"
        );

        let second = root.path().join(".incoming.second");
        std::fs::create_dir(&second).unwrap();
        std::fs::write(second.join(".generation"), b"second").unwrap();
        assert!(publish_generation_directory(&second, &incoming).is_err());
        assert_eq!(
            std::fs::read(incoming.join(".generation")).unwrap(),
            b"first"
        );
        assert_eq!(
            std::fs::read(second.join(".generation")).unwrap(),
            b"second"
        );
    }

    #[test]
    fn bounded_wait_kills_and_reaps_a_stuck_child() {
        let mut child = Command::new("/usr/bin/sleep").arg("5").spawn().unwrap();
        let error = wait_bounded(
            &mut child,
            Duration::from_millis(25),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(error.to_string().contains("bounded runtime"));
        assert!(
            child.try_wait().unwrap().is_some(),
            "timed-out child was not reaped"
        );
    }
}
