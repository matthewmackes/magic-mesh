//! Root-owned host configuration and guest-controller configuration.

use crate::{hex_decode, DEFAULT_CONTROLLER_CONFIG, DEFAULT_HOST_CONFIG};
use anyhow::{bail, ensure, Context, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::env;
use std::fs;
use std::net::IpAddr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

const HOST_CONFIG_ENV: &str = "MCNF_BROWSER_VM_CONTROL_CONFIG";
const CONTROLLER_CONFIG_ENV: &str = "MCNF_BROWSER_VM_CONTROLLER_CONFIG";

/// Host hook configuration. IP literals are deliberate: Browser evidence must
/// not follow mutable DNS or a proxy selected by the ambient environment.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    pub schema_version: u8,
    pub rdp_host: IpAddr,
    pub rdp_port: u16,
    pub rdp_username: String,
    pub rdp_password_file: PathBuf,
    pub controller_host: IpAddr,
    pub controller_port: u16,
    pub controller_secret_file: PathBuf,
    pub desktop_width: u16,
    pub desktop_height: u16,
    pub control_button_x: u16,
    pub control_button_y: u16,
}

/// Guest service configuration. Only the exact hypervisor-side host address may
/// use the authenticated API; browser endpoints additionally require a loopback
/// peer and a one-time 256-bit job id.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerConfig {
    pub schema_version: u8,
    pub listen_address: IpAddr,
    pub listen_port: u16,
    pub allowed_host_address: IpAddr,
    pub controller_secret_file: PathBuf,
    #[serde(default = "default_max_jobs")]
    pub max_jobs: usize,
}

const fn default_max_jobs() -> usize {
    4
}

impl HostConfig {
    /// Load the fixed production config, or an explicit absolute test config.
    pub fn load() -> Result<Self> {
        let path = configured_path(HOST_CONFIG_ENV, DEFAULT_HOST_CONFIG)?;
        let value: Self = read_owned_json(&path, false)?;
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<()> {
        ensure!(self.schema_version == 1, "unsupported host config schema");
        ensure!(self.rdp_port != 0, "RDP port may not be zero");
        ensure!(self.controller_port != 0, "controller port may not be zero");
        ensure!(
            !self.rdp_host.is_unspecified() && !self.rdp_host.is_loopback(),
            "RDP host must be one exact non-loopback guest address"
        );
        ensure!(
            !self.controller_host.is_unspecified() && !self.controller_host.is_loopback(),
            "controller host must be the guest address, not wildcard/loopback"
        );
        ensure!(
            self.controller_host == self.rdp_host,
            "RDP and Browser controller must resolve to the same guest address"
        );
        ensure!(
            !self.rdp_username.is_empty()
                && self.rdp_username.len() <= 128
                && !self.rdp_username.chars().any(char::is_control),
            "invalid RDP username"
        );
        ensure!(
            self.desktop_width >= 200
                && self.desktop_width <= 8192
                && self.desktop_width % 2 == 0
                && self.desktop_height >= 200
                && self.desktop_height <= 8192,
            "desktop dimensions are outside the RDP contract"
        );
        ensure!(
            self.control_button_x < self.desktop_width
                && self.control_button_y < self.desktop_height,
            "control button lies outside the RDP desktop"
        );
        require_absolute(&self.rdp_password_file, "RDP password file")?;
        require_absolute(&self.controller_secret_file, "controller secret file")?;
        Ok(())
    }

    /// Read the RDP password from its private file. It is never accepted through
    /// argv, collector environment, controller traffic, or process output.
    pub fn rdp_password(&self) -> Result<Zeroizing<String>> {
        read_private_text(&self.rdp_password_file, 4096, "RDP password")
    }

    /// Read the 256-bit host/controller authentication key.
    pub fn controller_secret(&self) -> Result<Zeroizing<[u8; 32]>> {
        read_secret(&self.controller_secret_file)
    }

    /// Loopback URL typed into Chromium over RDP. `127.0.0.1` is a potentially
    /// trustworthy browser origin, so getUserMedia remains standards-compliant.
    #[must_use]
    pub fn browser_origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.controller_port)
    }
}

impl ControllerConfig {
    pub fn load() -> Result<Self> {
        let path = configured_path(CONTROLLER_CONFIG_ENV, DEFAULT_CONTROLLER_CONFIG)?;
        let value: Self = read_owned_json(&path, false)?;
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported controller config schema"
        );
        ensure!(self.listen_port != 0, "controller port may not be zero");
        ensure!(
            !self.allowed_host_address.is_unspecified() && !self.allowed_host_address.is_loopback(),
            "allowed host address must be one exact non-loopback address"
        );
        ensure!(
            (1..=16).contains(&self.max_jobs),
            "max_jobs must be between 1 and 16"
        );
        require_absolute(&self.controller_secret_file, "controller secret file")?;
        Ok(())
    }

    pub fn controller_secret(&self) -> Result<Zeroizing<[u8; 32]>> {
        read_secret(&self.controller_secret_file)
    }

    #[must_use]
    pub fn browser_origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.listen_port)
    }
}

fn configured_path(variable: &str, default: &str) -> Result<PathBuf> {
    let value = env::var_os(variable).map_or_else(|| PathBuf::from(default), PathBuf::from);
    require_absolute(&value, "configuration path")?;
    Ok(value)
}

fn require_absolute(path: &Path, label: &str) -> Result<()> {
    ensure!(path.is_absolute(), "{label} must be an absolute path");
    ensure!(
        !path.as_os_str().is_empty(),
        "{label} may not be an empty path"
    );
    Ok(())
}

fn read_owned_json<T: DeserializeOwned>(path: &Path, secret: bool) -> Result<T> {
    let bytes = read_owned_file(path, if secret { 0o077 } else { 0o022 }, 64 * 1024)?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn read_owned_file(path: &Path, forbidden_mode: u32, maximum: usize) -> Result<Vec<u8>> {
    require_absolute(path, "private file")?;
    let link_metadata =
        fs::symlink_metadata(path).with_context(|| format!("inspect {}", path.display()))?;
    ensure!(
        !link_metadata.file_type().is_symlink() && link_metadata.is_file(),
        "{} must be a non-symlink regular file",
        path.display()
    );
    let owner = fs::metadata("/proc/self")
        .context("inspect effective process owner")?
        .uid();
    ensure!(
        link_metadata.uid() == owner,
        "{} is not owned by the service account",
        path.display()
    );
    ensure!(
        link_metadata.mode() & forbidden_mode == 0,
        "{} has unsafe permissions",
        path.display()
    );
    let length = usize::try_from(link_metadata.len()).context("private file length overflow")?;
    ensure!(
        length > 0 && length <= maximum,
        "{} has an invalid bounded size",
        path.display()
    );
    fs::read(path).with_context(|| format!("read {}", path.display()))
}

fn read_private_text(path: &Path, maximum: usize, label: &str) -> Result<Zeroizing<String>> {
    let bytes = read_owned_file(path, 0o077, maximum)?;
    let mut value = String::from_utf8(bytes).with_context(|| format!("{label} is not UTF-8"))?;
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    if value.is_empty() || value.contains(['\0', '\n', '\r']) {
        bail!("{label} must contain exactly one non-empty line");
    }
    Ok(Zeroizing::new(value))
}

fn read_secret(path: &Path) -> Result<Zeroizing<[u8; 32]>> {
    let text = read_private_text(path, 256, "controller secret")?;
    let secret = hex_decode::<32>(&text).context("controller secret must be 32-byte hex")?;
    Ok(Zeroizing::new(secret))
}

#[cfg(test)]
mod tests {
    use super::{ControllerConfig, HostConfig};
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::PathBuf;

    #[test]
    fn host_config_rejects_button_outside_desktop() {
        let value = HostConfig {
            schema_version: 1,
            rdp_host: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2)),
            rdp_port: 3389,
            rdp_username: "mm".to_owned(),
            rdp_password_file: PathBuf::from("/private/password"),
            controller_host: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2)),
            controller_port: 38443,
            controller_secret_file: PathBuf::from("/private/secret"),
            desktop_width: 1920,
            desktop_height: 1080,
            control_button_x: 1920,
            control_button_y: 540,
        };
        assert!(value.validate().is_err());
    }

    #[test]
    fn controller_rejects_loopback_as_host_api_peer() {
        let value = ControllerConfig {
            schema_version: 1,
            listen_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            listen_port: 38443,
            allowed_host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            controller_secret_file: PathBuf::from("/private/secret"),
            max_jobs: 4,
        };
        assert!(value.validate().is_err());
    }
}
