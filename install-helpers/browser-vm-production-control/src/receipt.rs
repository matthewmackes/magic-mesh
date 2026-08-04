//! Atomic private receipts accepted by `collect-browser-vm-live-audio.sh`.

use crate::protocol::{JobSpec, Operation};
use crate::{hex_encode, random_bytes, utc_timestamp};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

#[derive(Serialize)]
struct ProbeReceipt<'a> {
    schema_version: u8,
    kind: &'static str,
    operation: &'a str,
    state: &'a str,
    phase: &'a str,
    expected_tone_hz: u32,
    profile: &'static str,
    source_commit: &'a str,
    image_digest: &'a str,
    transport: &'a str,
    control_channel: &'static str,
    browser_api: &'static str,
    user_gesture_observed: bool,
    capture_point: &'static str,
    channels: u8,
    recorded_at: String,
}

#[derive(Serialize)]
struct ReconnectReceipt<'a> {
    schema_version: u8,
    kind: &'static str,
    domain: &'a str,
    profile: &'static str,
    source_commit: &'a str,
    image_digest: &'a str,
    transport: &'a str,
    status: &'static str,
    disconnect_observed_at: &'a str,
    reconnect_observed_at: &'a str,
}

pub fn write_probe_receipt(path: &Path, spec: &JobSpec, state: &str) -> Result<()> {
    ensure!(
        matches!(state, "ready" | "started" | "completed"),
        "invalid probe receipt state"
    );
    if spec.operation == Operation::Capture {
        ensure!(
            state != "started",
            "collector does not admit a capture started receipt"
        );
    }
    let (browser_api, capture_point) = match spec.operation {
        Operation::Playback => ("WebAudio", "guest-browser-webaudio-output"),
        Operation::Capture => ("getUserMedia+WebAudio", "guest-browser-vm-capture-input"),
    };
    let receipt = ProbeReceipt {
        schema_version: 1,
        kind: "browser_vm_guest_audio_probe_receipt",
        operation: spec.operation.as_str(),
        state,
        phase: &spec.phase,
        expected_tone_hz: spec.tone_hz,
        profile: "browser-vm-chromium",
        source_commit: &spec.source_commit,
        image_digest: &spec.image_digest,
        transport: &spec.transport,
        control_channel: "rdp-webaudio",
        browser_api,
        user_gesture_observed: true,
        capture_point,
        channels: 2,
        recorded_at: utc_timestamp()?,
    };
    write_private_json(path, &receipt)
}

pub fn write_reconnect_receipt(
    path: &Path,
    domain: &str,
    source_commit: &str,
    image_digest: &str,
    transport: &str,
    disconnect_observed_at: &str,
    reconnect_observed_at: &str,
) -> Result<()> {
    let receipt = ReconnectReceipt {
        schema_version: 1,
        kind: "browser_vm_transport_reconnect_receipt",
        domain,
        profile: "browser-vm-chromium",
        source_commit,
        image_digest,
        transport,
        status: "observed",
        disconnect_observed_at,
        reconnect_observed_at,
    };
    write_private_json(path, &receipt)
}

pub fn write_private_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(!bytes.is_empty(), "private output may not be empty");
    let parent = validate_destination(path)?;
    let file_name = path
        .file_name()
        .context("private output has no file name")?
        .to_string_lossy();
    let temporary = parent.join(format!(
        ".{file_name}.tmp.{}",
        hex_encode(&random_bytes::<16>()?)
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("create private output {}", temporary.display()))?;
        file.write_all(bytes).context("write private output")?;
        file.sync_all().context("sync private output")?;
        drop(file);
        // hard_link is the portable no-replace publication primitive on the
        // same filesystem: it fails if an attacker made the target meanwhile.
        fs::hard_link(&temporary, path)
            .with_context(|| format!("publish private output {}", path.display()))?;
        fs::remove_file(&temporary).context("remove private output staging link")?;
        Ok(())
    })();
    if result.is_err() {
        let _ignored = fs::remove_file(&temporary);
    }
    result
}

fn write_private_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec(value).context("encode private receipt")?;
    bytes.push(b'\n');
    write_private_bytes(path, &bytes)
}

fn validate_destination(path: &Path) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "private output path must be absolute");
    ensure!(
        fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "private output target already exists"
    );
    let parent = path.parent().context("private output has no parent")?;
    let canonical = fs::canonicalize(parent)
        .with_context(|| format!("canonicalize private output parent {}", parent.display()))?;
    ensure!(
        canonical == parent,
        "private output parent contains a symlink"
    );
    let metadata = fs::metadata(parent).context("inspect private output parent")?;
    ensure!(
        metadata.is_dir(),
        "private output parent is not a directory"
    );
    let owner = fs::metadata("/proc/self")
        .context("inspect effective process owner")?
        .uid();
    ensure!(
        metadata.uid() == owner,
        "private output parent has the wrong owner"
    );
    ensure!(
        metadata.mode() & 0o022 == 0,
        "private output parent is group/world writable"
    );
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::write_private_bytes;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or_default();
        let path =
            std::env::temp_dir().join(format!("mcnf-receipt-test-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).ok();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).ok();
        path
    }

    #[test]
    fn output_is_private_and_no_replace() {
        let dir = fixture();
        let path = dir.join("receipt.json");
        assert!(write_private_bytes(&path, b"{}\n").is_ok());
        let metadata = fs::metadata(&path).ok();
        assert_eq!(
            metadata.as_ref().map(|value| value.mode() & 0o777),
            Some(0o600)
        );
        assert!(write_private_bytes(&path, b"changed\n").is_err());
        assert_eq!(fs::read(&path).ok().as_deref(), Some(&b"{}\n"[..]));
        fs::remove_dir_all(dir).ok();
    }
}
