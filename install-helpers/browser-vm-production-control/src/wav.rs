//! Validation for browser-produced PCM. Production code never generates audio;
//! it accepts only a bounded stereo WAV uploaded by the active Chromium page.

use anyhow::{ensure, Context, Result};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;
pub const BITS_PER_SAMPLE: u16 = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WavInfo {
    pub frames: u32,
    pub peak_absolute_sample: u16,
}

pub fn validate_browser_wav(bytes: &[u8], duration_seconds: u32) -> Result<WavInfo> {
    ensure!(
        bytes.len() >= 44,
        "browser WAV is shorter than its canonical header"
    );
    ensure!(&bytes[0..4] == b"RIFF", "browser WAV is not RIFF");
    ensure!(&bytes[8..12] == b"WAVE", "browser WAV is not WAVE");
    ensure!(
        &bytes[12..16] == b"fmt ",
        "browser WAV omits canonical fmt chunk"
    );
    ensure!(
        read_u32(bytes, 16)? == 16,
        "browser WAV fmt chunk is not PCM16"
    );
    ensure!(read_u16(bytes, 20)? == 1, "browser WAV is not linear PCM");
    ensure!(
        read_u16(bytes, 22)? == CHANNELS,
        "browser WAV is not stereo"
    );
    ensure!(
        read_u32(bytes, 24)? == SAMPLE_RATE,
        "browser WAV is not 48 kHz"
    );
    ensure!(
        read_u32(bytes, 28)? == SAMPLE_RATE * u32::from(CHANNELS) * 2,
        "browser WAV byte rate is inconsistent"
    );
    ensure!(
        read_u16(bytes, 32)? == CHANNELS * 2,
        "browser WAV block alignment is invalid"
    );
    ensure!(
        read_u16(bytes, 34)? == BITS_PER_SAMPLE,
        "browser WAV is not signed 16-bit PCM"
    );
    ensure!(
        &bytes[36..40] == b"data",
        "browser WAV omits canonical data chunk"
    );
    let data_bytes = usize::try_from(read_u32(bytes, 40)?).context("WAV data length overflow")?;
    ensure!(
        bytes.len() == 44 + data_bytes,
        "browser WAV length does not match its data chunk"
    );
    ensure!(
        data_bytes % 4 == 0,
        "browser WAV ends inside a stereo frame"
    );
    let expected_frames = SAMPLE_RATE
        .checked_mul(duration_seconds)
        .context("expected capture frame count overflow")?;
    let frames = u32::try_from(data_bytes / 4).context("browser WAV frame count overflow")?;
    ensure!(
        frames == expected_frames,
        "browser WAV has the wrong capture duration"
    );

    let mut peak = 0_u16;
    for sample in bytes[44..].chunks_exact(2) {
        let value = i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs();
        peak = peak.max(value);
    }
    ensure!(
        peak >= 32,
        "browser WAV contains no observed non-silent microphone samples"
    );
    Ok(WavInfo {
        frames,
        peak_absolute_sample: peak,
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let slice = bytes
        .get(offset..offset + 2)
        .context("truncated WAV u16 field")?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let slice = bytes
        .get(offset..offset + 4)
        .context("truncated WAV u32 field")?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
mod tests {
    use super::{validate_browser_wav, CHANNELS, SAMPLE_RATE};

    fn fixture(frames: u32, sample: i16) -> Vec<u8> {
        let data_len = frames * u32::from(CHANNELS) * 2;
        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&CHANNELS.to_le_bytes());
        bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        bytes.extend_from_slice(&(SAMPLE_RATE * u32::from(CHANNELS) * 2).to_le_bytes());
        bytes.extend_from_slice(&(CHANNELS * 2).to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for _ in 0..frames * u32::from(CHANNELS) {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn accepts_exact_non_silent_browser_pcm_shape() {
        let bytes = fixture(SAMPLE_RATE * 2, 128);
        let info = validate_browser_wav(&bytes, 2).ok();
        assert_eq!(info.map(|value| value.frames), Some(SAMPLE_RATE * 2));
    }

    #[test]
    fn rejects_silence_and_wrong_duration() {
        assert!(validate_browser_wav(&fixture(SAMPLE_RATE * 2, 0), 2).is_err());
        assert!(validate_browser_wav(&fixture(SAMPLE_RATE, 128), 2).is_err());
    }
}
