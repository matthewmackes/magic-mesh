//! Surface MOK import credential boundary.
//!
//! A MOK-manager password must never ride the world-readable Bus, argv, the
//! environment, or a caller-selected hash file. Production accepts only an
//! `mde-seal` envelope and its passphrase from two fixed systemd credential
//! names. The decrypted permit is bound to one already-authorized Surface
//! request, one node, one certificate fingerprint, and a <=30-second expiry.
//! `mokutil` receives the password twice over a private stdin pipe; no password
//! or derived hash is materialized on disk.
//!
//! The credential provisioner must place the binary `mde_seal::seal_bytes`
//! result in `surface-mok-import.sealed` and the independent sealing passphrase
//! in `surface-mok-import-passphrase` (normally both via systemd encrypted
//! credentials). The authenticated plaintext is exactly:
//! `{"schema_version":1,"node":"...","request_id":"...",`
//! `"authorization_nonce":"...","expires_at_ms":...,`
//! `"certificate_sha1":"AA:...","password":"8-16 alphanumerics"}`.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::{parse_sha1_fingerprint, MOK_KEY_PATH};

pub(super) const MOK_IMPORT_ENVELOPE_CREDENTIAL: &str = "surface-mok-import.sealed";
pub(super) const MOK_IMPORT_PASSPHRASE_CREDENTIAL: &str = "surface-mok-import-passphrase";

const PERMIT_SCHEMA_VERSION: u64 = 1;
const MAX_PERMIT_TTL_MS: u64 = 30_000;
const MAX_CREDENTIAL_BYTES: usize = 64 * 1024;
const MAX_PASSPHRASE_BYTES: usize = 4 * 1024;
const MAX_CAPTURE_BYTES: usize = 64 * 1024;
const READ_BUFFER_BYTES: usize = 8 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone)]
pub(super) struct MokImportBinding {
    pub(super) node: String,
    pub(super) request_id: String,
    pub(super) authorization_nonce: String,
    pub(super) authorization_expires_at_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMokImportPermit {
    schema_version: u64,
    node: String,
    request_id: String,
    authorization_nonce: String,
    expires_at_ms: u64,
    certificate_sha1: String,
    password: String,
}

impl Drop for RawMokImportPermit {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

/// Decrypted permit. Debug is deliberately omitted so password material cannot
/// accidentally enter structured logs.
pub(super) struct MokImportPermit {
    password: Zeroizing<String>,
}

impl MokImportPermit {
    pub(super) fn password(&self) -> &str {
        self.password.as_str()
    }
}

pub(super) fn load_systemd_permit(
    binding: &MokImportBinding,
    certificate_sha1: &[u8; 20],
    now_ms: u64,
) -> Result<MokImportPermit, String> {
    if !rustix::process::geteuid().is_root() {
        return Err("Surface MOK import requires the root service process".to_string());
    }
    let directory = std::env::var_os("CREDENTIALS_DIRECTORY")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "Surface MOK import systemd credentials are unavailable".to_string())?;
    load_permit_from_paths(
        &directory.join(MOK_IMPORT_ENVELOPE_CREDENTIAL),
        &directory.join(MOK_IMPORT_PASSPHRASE_CREDENTIAL),
        binding,
        certificate_sha1,
        now_ms,
    )
}

fn load_permit_from_paths(
    envelope_path: &Path,
    passphrase_path: &Path,
    binding: &MokImportBinding,
    certificate_sha1: &[u8; 20],
    now_ms: u64,
) -> Result<MokImportPermit, String> {
    let envelope = Zeroizing::new(
        read_bounded_regular(envelope_path, MAX_CREDENTIAL_BYTES)
            .map_err(|error| format!("read sealed Surface MOK permit: {error}"))?,
    );
    let mut passphrase_bytes = Zeroizing::new(
        read_bounded_regular(passphrase_path, MAX_PASSPHRASE_BYTES)
            .map_err(|error| format!("read Surface MOK permit passphrase: {error}"))?,
    );
    while matches!(passphrase_bytes.last(), Some(b'\n' | b'\r')) {
        passphrase_bytes.pop();
    }
    let passphrase = std::str::from_utf8(passphrase_bytes.as_slice())
        .map_err(|_| "Surface MOK permit passphrase is not UTF-8".to_string())?;
    if passphrase.is_empty() {
        return Err("Surface MOK permit passphrase is empty".to_string());
    }
    let plaintext = Zeroizing::new(
        mde_seal::unseal_bytes(passphrase, envelope.as_slice())
            .map_err(|_| "sealed Surface MOK permit failed authentication".to_string())?,
    );
    let permit: RawMokImportPermit = serde_json::from_slice(plaintext.as_slice())
        .map_err(|_| "sealed Surface MOK permit is malformed".to_string())?;
    validate_permit(permit, binding, certificate_sha1, now_ms)
}

fn validate_permit(
    mut permit: RawMokImportPermit,
    binding: &MokImportBinding,
    certificate_sha1: &[u8; 20],
    now_ms: u64,
) -> Result<MokImportPermit, String> {
    if permit.schema_version != PERMIT_SCHEMA_VERSION {
        return Err(format!(
            "Surface MOK permit requires schema_version {PERMIT_SCHEMA_VERSION}"
        ));
    }
    if permit.node != binding.node
        || permit.request_id != binding.request_id
        || permit.authorization_nonce != binding.authorization_nonce
    {
        return Err("Surface MOK permit is not bound to this authorized request".to_string());
    }
    if permit.expires_at_ms <= now_ms
        || permit.expires_at_ms > now_ms.saturating_add(MAX_PERMIT_TTL_MS)
        || permit.expires_at_ms > binding.authorization_expires_at_ms
    {
        return Err("Surface MOK permit is expired or exceeds the 30-second lifetime".to_string());
    }
    let permit_fingerprint = parse_sha1_fingerprint(&permit.certificate_sha1)
        .ok_or_else(|| "Surface MOK permit certificate fingerprint is malformed".to_string())?;
    if &permit_fingerprint != certificate_sha1 {
        return Err("Surface MOK permit targets a different certificate".to_string());
    }
    if !(8..=16).contains(&permit.password.len())
        || !permit
            .password
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err("Surface MOK permit password must be 8-16 ASCII letters or digits".to_string());
    }
    Ok(MokImportPermit {
        password: Zeroizing::new(std::mem::take(&mut permit.password)),
    })
}

/// Execute the one fixed mutation. mokutil's interactive reader accepts a
/// non-TTY stdin and asks for the password twice. Feeding that pipe avoids both
/// `--hash-file` and any secret-bearing argv/environment value.
pub(super) fn import_fixed_certificate(password: &str) -> Result<(), String> {
    let mut command = Command::new("/usr/bin/mokutil");
    command
        .args(mokutil_import_args())
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn fixed mokutil import: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| abort_child(&mut child, "mokutil stdout pipe was unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| abort_child(&mut child, "mokutil stderr pipe was unavailable"))?;
    let stdout_reader = spawn_reader(stdout, "surface-mok-stdout")
        .map_err(|error| abort_child(&mut child, &format!("spawn stdout reader: {error}")))?;
    let stderr_reader = match spawn_reader(stderr, "surface-mok-stderr") {
        Ok(reader) => reader,
        Err(error) => {
            abort_and_join(&mut child, stdout_reader);
            return Err(format!("spawn stderr reader: {error}"));
        }
    };

    let mut input = Zeroizing::new(Vec::with_capacity(password.len() * 2 + 2));
    input.extend_from_slice(password.as_bytes());
    input.push(b'\n');
    input.extend_from_slice(password.as_bytes());
    input.push(b'\n');
    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| "mokutil stdin pipe was unavailable".to_string())
        .and_then(|mut stdin| {
            stdin
                .write_all(input.as_slice())
                .map_err(|error| format!("write private mokutil stdin: {error}"))
        });
    if let Err(error) = write_result {
        abort_and_join(&mut child, stdout_reader);
        let _ = stderr_reader.join();
        return Err(error);
    }

    let status = wait_bounded(&mut child, COMMAND_TIMEOUT);
    let stdout_result = join_reader(stdout_reader);
    let stderr_result = join_reader(stderr_reader);
    let stdout = stdout_result?;
    let stderr = stderr_result?;
    let status = status?;
    if !status.success() {
        let detail = std::str::from_utf8(&stderr)
            .ok()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("mokutil import failed without diagnostic output");
        return Err(detail.to_string());
    }
    // A successful import normally prints prompts to stdout. Never surface it:
    // it is irrelevant to the typed result and retaining less subprocess data
    // reduces the chance of future tool output reaching logs.
    drop(stdout);
    Ok(())
}

fn mokutil_import_args() -> [&'static str; 2] {
    ["--import", MOK_KEY_PATH]
}

pub(super) fn read_bounded_regular(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    #[cfg(unix)]
    let file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};
        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into()
    };
    #[cfg(not(unix))]
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "credential is not a regular file",
        ));
    }
    if metadata.len() > max_bytes as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "credential exceeds its byte limit",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize + 1);
    file.take(max_bytes as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        bytes.zeroize();
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "credential exceeds its byte limit",
        ));
    }
    Ok(bytes)
}

fn spawn_reader<R>(reader: R, name: &'static str) -> io::Result<JoinHandle<io::Result<Vec<u8>>>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || drain_reader(reader))
}

fn drain_reader<R: Read>(mut reader: R) -> io::Result<Vec<u8>> {
    let mut captured = Vec::with_capacity(READ_BUFFER_BYTES);
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(captured);
        }
        let remaining = MAX_CAPTURE_BYTES.saturating_sub(captured.len());
        if remaining != 0 {
            captured.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }
}

fn join_reader(reader: JoinHandle<io::Result<Vec<u8>>>) -> Result<Vec<u8>, String> {
    reader
        .join()
        .map_err(|_| "mokutil output reader panicked".to_string())?
        .map_err(|error| format!("read mokutil output: {error}"))
}

fn wait_bounded(child: &mut Child, timeout: Duration) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "fixed mokutil import exceeded {}s timeout",
                    timeout.as_secs()
                ));
            }
            Ok(None) => thread::sleep(POLL),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("wait for fixed mokutil import: {error}"));
            }
        }
    }
}

fn abort_child(child: &mut Child, detail: &str) -> String {
    let _ = child.kill();
    let _ = child.wait();
    detail.to_string()
}

fn abort_and_join<R>(child: &mut Child, reader: JoinHandle<R>) {
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000_000;
    const FP_TEXT: &str = "01:23:45:67:89:AB:CD:EF:10:32:54:76:98:BA:DC:FE:11:22:33:44";

    fn raw(node: &str, request_id: &str) -> RawMokImportPermit {
        RawMokImportPermit {
            schema_version: PERMIT_SCHEMA_VERSION,
            node: node.to_string(),
            request_id: request_id.to_string(),
            authorization_nonce: "auth-nonce-1".to_string(),
            expires_at_ms: NOW + MAX_PERMIT_TTL_MS,
            certificate_sha1: FP_TEXT.to_string(),
            password: "MokPass42".to_string(),
        }
    }

    fn write_sealed(directory: &Path, permit: &RawMokImportPermit) -> (PathBuf, PathBuf, [u8; 20]) {
        let passphrase = "surface-test-seal-passphrase";
        let plaintext = serde_json::to_vec(permit).expect("serialize permit");
        let sealed = mde_seal::seal_bytes(passphrase, &plaintext).expect("seal permit");
        let envelope = directory.join(MOK_IMPORT_ENVELOPE_CREDENTIAL);
        let key = directory.join(MOK_IMPORT_PASSPHRASE_CREDENTIAL);
        std::fs::write(&envelope, sealed).expect("write sealed permit");
        std::fs::write(&key, format!("{passphrase}\n")).expect("write passphrase");
        (
            envelope,
            key,
            parse_sha1_fingerprint(FP_TEXT).expect("fixture fingerprint"),
        )
    }

    #[test]
    fn sealed_permit_is_exact_request_certificate_and_time_bound() {
        let directory = tempfile::tempdir().expect("tempdir");
        let (envelope, key, fingerprint) = write_sealed(directory.path(), &raw("Surface", "r1"));
        let binding = MokImportBinding {
            node: "Surface".to_string(),
            request_id: "r1".to_string(),
            authorization_nonce: "auth-nonce-1".to_string(),
            authorization_expires_at_ms: NOW + MAX_PERMIT_TTL_MS,
        };
        let permit = load_permit_from_paths(&envelope, &key, &binding, &fingerprint, NOW)
            .expect("bound permit");
        assert_eq!(permit.password(), "MokPass42");

        for hostile in [
            MokImportBinding {
                node: "other".to_string(),
                request_id: "r1".to_string(),
                authorization_nonce: "auth-nonce-1".to_string(),
                authorization_expires_at_ms: NOW + MAX_PERMIT_TTL_MS,
            },
            MokImportBinding {
                node: "Surface".to_string(),
                request_id: "other".to_string(),
                authorization_nonce: "auth-nonce-1".to_string(),
                authorization_expires_at_ms: NOW + MAX_PERMIT_TTL_MS,
            },
            MokImportBinding {
                node: "Surface".to_string(),
                request_id: "r1".to_string(),
                authorization_nonce: "other-nonce".to_string(),
                authorization_expires_at_ms: NOW + MAX_PERMIT_TTL_MS,
            },
            MokImportBinding {
                node: "Surface".to_string(),
                request_id: "r1".to_string(),
                authorization_nonce: "auth-nonce-1".to_string(),
                authorization_expires_at_ms: NOW + 1,
            },
        ] {
            assert!(load_permit_from_paths(&envelope, &key, &hostile, &fingerprint, NOW).is_err());
        }
        let mut other_fingerprint = fingerprint;
        other_fingerprint[0] ^= 0xff;
        assert!(
            load_permit_from_paths(&envelope, &key, &binding, &other_fingerprint, NOW).is_err()
        );
        assert!(load_permit_from_paths(
            &envelope,
            &key,
            &binding,
            &fingerprint,
            NOW + MAX_PERMIT_TTL_MS
        )
        .is_err());
    }

    #[test]
    fn sealed_permit_rejects_tamper_unknown_fields_and_unsafe_passwords() {
        let directory = tempfile::tempdir().expect("tempdir");
        let binding = MokImportBinding {
            node: "Surface".to_string(),
            request_id: "r1".to_string(),
            authorization_nonce: "auth-nonce-1".to_string(),
            authorization_expires_at_ms: NOW + MAX_PERMIT_TTL_MS,
        };

        let (envelope, key, fingerprint) = write_sealed(directory.path(), &raw("Surface", "r1"));
        let mut tampered = std::fs::read(&envelope).expect("read envelope");
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        std::fs::write(&envelope, tampered).expect("write tamper");
        assert!(load_permit_from_paths(&envelope, &key, &binding, &fingerprint, NOW).is_err());

        let mut unsafe_password = raw("Surface", "r1");
        unsafe_password.password = "password\n--root-pw".to_string();
        let (envelope, key, fingerprint) = write_sealed(directory.path(), &unsafe_password);
        assert!(load_permit_from_paths(&envelope, &key, &binding, &fingerprint, NOW).is_err());

        let passphrase = "surface-test-seal-passphrase";
        let unknown = format!(
            r#"{{"schema_version":1,"node":"Surface","request_id":"r1","authorization_nonce":"auth-nonce-1","expires_at_ms":{},"certificate_sha1":"{}","password":"MokPass42","extra":true}}"#,
            NOW + MAX_PERMIT_TTL_MS,
            FP_TEXT
        );
        let sealed = mde_seal::seal_bytes(passphrase, unknown.as_bytes()).expect("seal unknown");
        std::fs::write(&envelope, sealed).expect("write unknown permit");
        std::fs::write(&key, passphrase).expect("write key");
        assert!(load_permit_from_paths(&envelope, &key, &binding, &fingerprint, NOW).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn credential_reader_rejects_final_symlink_and_oversize() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("tempdir");
        let target = directory.path().join("target");
        let link = directory.path().join("link");
        std::fs::write(&target, b"secret").expect("write target");
        symlink(&target, &link).expect("symlink");
        assert!(read_bounded_regular(&link, 32).is_err());

        let oversized = directory.path().join("oversized");
        std::fs::write(&oversized, vec![b'x'; 33]).expect("write oversized");
        assert!(read_bounded_regular(&oversized, 32).is_err());
    }

    #[test]
    fn mokutil_import_argv_is_fixed_and_contains_no_secret() {
        // Keep this assertion adjacent to the execution path: the only runtime
        // argv values are the closed verb and fixed package certificate.
        assert_eq!(mokutil_import_args(), ["--import", MOK_KEY_PATH]);
        assert!(mokutil_import_args()
            .iter()
            .all(|argument| !argument.contains("MokPass42")));
    }
}
