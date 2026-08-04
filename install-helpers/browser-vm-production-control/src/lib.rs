//! Production control hooks for Browser VM live audio qualification.
//!
//! The host hooks use `mde-vdi-rdp` for every browser navigation and click. A
//! small guest service only serves a loopback page and relays authenticated job
//! state; it cannot synthesize browser receipts or audio. The page itself must
//! observe trusted user activation before it opens WebAudio or getUserMedia.

pub mod auth;
pub mod config;
pub mod controller;
#[cfg(feature = "host-control")]
pub mod hook;
pub mod http;
pub mod page;
pub mod protocol;
#[cfg(feature = "host-control")]
pub mod rdp;
pub mod receipt;
pub mod wav;

use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;

/// Fixed host-side configuration path used when the collector clears the hook
/// environment. Tests may opt into another absolute path with the documented
/// environment variable.
pub const DEFAULT_HOST_CONFIG: &str = "/etc/mcnf/browser-vm-production-control.json";

/// Fixed guest-controller configuration path.
pub const DEFAULT_CONTROLLER_CONFIG: &str =
    "/etc/mcnf/browser-vm-guest-audio-probe-controller.json";

/// Read cryptographically random bytes directly from the kernel RNG.
pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut file = File::open("/dev/urandom").context("open /dev/urandom")?;
    let mut bytes = [0_u8; N];
    file.read_exact(&mut bytes)
        .context("read kernel randomness")?;
    Ok(bytes)
}

/// Encode bytes as lowercase hexadecimal without bringing secret-bearing data
/// through a formatter that might be logged accidentally.
#[must_use]
pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

/// Parse fixed-length lowercase or uppercase hexadecimal.
pub fn hex_decode<const N: usize>(value: &str) -> Result<[u8; N]> {
    if value.len() != N * 2 || !value.is_ascii() {
        bail!("expected {} hexadecimal characters", N * 2);
    }
    let mut out = [0_u8; N];
    let bytes = value.as_bytes();
    for index in 0..N {
        let high = hex_nibble(bytes[index * 2])?;
        let low = hex_nibble(bytes[index * 2 + 1])?;
        out[index] = (high << 4) | low;
    }
    Ok(out)
}

fn hex_nibble(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => bail!("invalid hexadecimal character"),
    }
}

/// Current Unix timestamp in whole seconds.
pub fn unix_seconds() -> Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch")?;
    i64::try_from(duration.as_secs()).context("Unix timestamp exceeds i64")
}

/// Collector-compatible UTC timestamp with whole-second precision.
pub fn utc_timestamp() -> Result<String> {
    let now = OffsetDateTime::now_utc();
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    ))
}

/// Wait for the next formatted UTC second. The collector's receipt schema has
/// whole-second timestamps and requires disconnect strictly before reconnect.
pub fn wait_for_later_timestamp(previous: &str, deadline: Duration) -> Result<String> {
    let started = std::time::Instant::now();
    loop {
        let current = utc_timestamp()?;
        if current.as_str() > previous {
            return Ok(current);
        }
        if started.elapsed() >= deadline {
            bail!("UTC clock did not advance before the deadline");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    use super::{hex_decode, hex_encode, utc_timestamp};

    #[test]
    fn hex_round_trip_is_exact() {
        let value = [0_u8, 1, 15, 16, 127, 128, 254, 255];
        let encoded = hex_encode(&value);
        assert_eq!(encoded, "00010f107f80feff");
        assert_eq!(hex_decode::<8>(&encoded).ok(), Some(value));
    }

    #[test]
    fn collector_timestamp_has_whole_second_shape() {
        let stamp = utc_timestamp().unwrap_or_default();
        assert_eq!(stamp.len(), 20);
        assert!(stamp.ends_with('Z'));
        assert_eq!(stamp.as_bytes().get(10), Some(&b'T'));
    }
}
