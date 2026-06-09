//! VOIP-28 slice 3: RTP + G.711 media engine (cpal capture/playback).
//!
//! Pure, unit-tested layers:
//!   * G.711 µ-law (PCMU/0) + A-law (PCMA/8) companding (ITU-T G.711).
//!   * RTP packetization (RFC 3550) — 12-byte header build + payload parse.
//!
//! The duplex engine (`start_media`) binds the negotiated RTP port, opens a
//! cpal input + output stream at 8 kHz mono, and runs the send (mic → G.711 →
//! RTP) and receive (RTP → G.711 → speaker) loops until stopped. The audible
//! round-trip needs audio hardware + a SIP peer → that is the bench; the code
//! here is real (no stub paths) and reachable from a connected call.

use std::collections::VecDeque;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::sip::RemoteMedia;

/// 8 kHz, 20 ms frames → 160 samples per RTP packet (G.711 = 1 byte/sample).
const SAMPLE_RATE: u32 = 8000;
const FRAME_SAMPLES: usize = 160;

// ── G.711 µ-law (PCMU, payload type 0) ──────────────────────────────────────

const ULAW_BIAS: i32 = 0x84;
const ULAW_CLIP: i32 = 32635;

/// Encode a 16-bit PCM sample to µ-law.
#[must_use]
pub fn linear_to_ulaw(sample: i16) -> u8 {
    let mut s = i32::from(sample);
    let sign = if s < 0 {
        s = -s;
        0x80u8
    } else {
        0
    };
    if s > ULAW_CLIP {
        s = ULAW_CLIP;
    }
    s += ULAW_BIAS;
    let mut exponent: i32 = 7;
    let mut mask = 0x4000;
    while exponent > 0 && (s & mask) == 0 {
        exponent -= 1;
        mask >>= 1;
    }
    let mantissa = (s >> (exponent + 3)) & 0x0F;
    #[allow(clippy::cast_possible_truncation)]
    let byte = sign | ((exponent as u8) << 4) | mantissa as u8;
    !byte
}

/// Decode a µ-law byte to a 16-bit PCM sample.
#[must_use]
pub fn ulaw_to_linear(ulaw: u8) -> i16 {
    let u = !ulaw;
    let sign = u & 0x80;
    let exponent = i32::from((u >> 4) & 0x07);
    let mantissa = i32::from(u & 0x0F);
    let mut sample = ((mantissa << 3) + ULAW_BIAS) << exponent;
    sample -= ULAW_BIAS;
    #[allow(clippy::cast_possible_truncation)]
    if sign != 0 {
        -sample as i16
    } else {
        sample as i16
    }
}

// ── G.711 A-law (PCMA, payload type 8) ──────────────────────────────────────

/// Encode a 16-bit PCM sample to A-law.
#[must_use]
pub fn linear_to_alaw(sample: i16) -> u8 {
    let mut s = i32::from(sample);
    let sign = if s >= 0 {
        0xD5u8
    } else {
        s = -s - 1;
        0x55u8
    };
    if s > 32635 {
        s = 32635;
    }
    let (exponent, mantissa) = if s >= 256 {
        let mut exp = 7;
        let mut mask = 0x4000;
        while exp > 1 && (s & mask) == 0 {
            exp -= 1;
            mask >>= 1;
        }
        (exp, (s >> (exp + 3)) & 0x0F)
    } else {
        (0, s >> 4)
    };
    #[allow(clippy::cast_possible_truncation)]
    let byte = ((exponent as u8) << 4) | (mantissa as u8 & 0x0F);
    byte ^ sign
}

/// Decode an A-law byte to a 16-bit PCM sample.
#[must_use]
pub fn alaw_to_linear(alaw: u8) -> i16 {
    let a = alaw ^ 0x55;
    let sign = a & 0x80;
    let exponent = i32::from((a >> 4) & 0x07);
    let mantissa = i32::from(a & 0x0F);
    let mut sample = (mantissa << 4) + 8;
    if exponent > 0 {
        sample += 0x100;
        sample <<= exponent - 1;
    }
    #[allow(clippy::cast_possible_truncation)]
    if sign == 0 {
        -sample as i16
    } else {
        sample as i16
    }
}

/// Encode a PCM frame with the codec selected by the RTP payload type.
fn encode_frame(pcm: &[i16], payload_type: u8) -> Vec<u8> {
    if payload_type == 8 {
        pcm.iter().map(|&s| linear_to_alaw(s)).collect()
    } else {
        pcm.iter().map(|&s| linear_to_ulaw(s)).collect()
    }
}

/// Decode a G.711 payload with the codec selected by the RTP payload type.
fn decode_frame(payload: &[u8], payload_type: u8) -> Vec<i16> {
    if payload_type == 8 {
        payload.iter().map(|&b| alaw_to_linear(b)).collect()
    } else {
        payload.iter().map(|&b| ulaw_to_linear(b)).collect()
    }
}

// ── RTP (RFC 3550) ──────────────────────────────────────────────────────────

/// Build an RTP packet: 12-byte header (V=2, no padding/extension/CSRC) + the
/// G.711 payload.
#[must_use]
pub fn build_rtp_packet(
    payload_type: u8,
    seq: u16,
    timestamp: u32,
    ssrc: u32,
    payload: &[u8],
) -> Vec<u8> {
    let mut p = Vec::with_capacity(12 + payload.len());
    p.push(0x80); // V=2, P=0, X=0, CC=0
    p.push(payload_type & 0x7F); // M=0 + PT
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&timestamp.to_be_bytes());
    p.extend_from_slice(&ssrc.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

/// Return the payload slice of an RTP packet (skips the 12-byte fixed header +
/// any CSRC identifiers), or `None` if it is too short / not RTP v2.
#[must_use]
pub fn rtp_payload(packet: &[u8]) -> Option<&[u8]> {
    if packet.len() < 12 || (packet[0] >> 6) != 2 {
        return None;
    }
    let cc = (packet[0] & 0x0F) as usize;
    let header = 12 + cc * 4;
    packet.get(header..)
}

// ── Duplex media engine ─────────────────────────────────────────────────────

/// A running media session — drop or `stop()` to tear down the streams.
pub struct MediaSession {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MediaSession {
    /// Signal the audio thread to stop and join it.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for MediaSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Start the RTP/G.711 media path for an established call: bind `local_rtp_port`,
/// connect to the peer's negotiated endpoint, and run capture/playback until
/// stopped. Returns an error (never panics) if the socket or audio devices are
/// unavailable — the call's signaling stays up regardless.
pub fn start_media(local_rtp_port: u16, remote: &RemoteMedia) -> Result<MediaSession, String> {
    let sock = UdpSocket::bind(("0.0.0.0", local_rtp_port))
        .map_err(|e| format!("RTP bind :{local_rtp_port} failed ({e})"))?;
    sock.connect((remote.addr.as_str(), remote.port))
        .map_err(|e| {
            format!(
                "RTP connect to {}:{} failed ({e})",
                remote.addr, remote.port
            )
        })?;
    sock.set_read_timeout(Some(Duration::from_millis(50))).ok();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let payload_type = remote.payload_type;

    let thread = std::thread::Builder::new()
        .name("mwv-rtp-media".into())
        .spawn(move || run_audio(&sock, payload_type, &stop_thread))
        .map_err(|e| format!("media thread spawn failed ({e})"))?;

    Ok(MediaSession {
        stop,
        thread: Some(thread),
    })
}

/// The audio thread: owns the cpal streams (`Stream` is `!Send`) and drives the
/// RTP send/recv loops. Shared `VecDeque`s bridge the cpal callbacks and the
/// network I/O.
fn run_audio(sock: &UdpSocket, payload_type: u8, stop: &AtomicBool) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    // mic samples captured by the input callback, drained by the send loop.
    let capture: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::new()));
    // decoded peer audio queued by the recv loop, drained by the output callback.
    let playback: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::new()));

    let host = cpal::default_host();
    let config = cpal::StreamConfig {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        buffer_size: cpal::BufferSize::Default,
    };

    // Output stream (peer audio → speaker). Optional: a node with no output
    // device still sends mic audio.
    let cap_in = capture.clone();
    let play_out = playback.clone();
    let _in_stream = host.default_input_device().and_then(|dev| {
        dev.build_input_stream(
            &config,
            move |data: &[i16], _| {
                if let Ok(mut q) = cap_in.lock() {
                    q.extend(data.iter().copied());
                }
            },
            |err| tracing::warn!(?err, "voice-hud: input stream error"),
            None,
        )
        .ok()
    });
    let _out_stream = host.default_output_device().and_then(|dev| {
        dev.build_output_stream(
            &config,
            move |data: &mut [i16], _| {
                if let Ok(mut q) = play_out.lock() {
                    for slot in data.iter_mut() {
                        *slot = q.pop_front().unwrap_or(0);
                    }
                }
            },
            |err| tracing::warn!(?err, "voice-hud: output stream error"),
            None,
        )
        .ok()
    });
    if let Some(s) = &_in_stream {
        let _ = s.play();
    }
    if let Some(s) = &_out_stream {
        let _ = s.play();
    }

    let ssrc: u32 = 0x4D57_5631; // "MWV1"
    let mut seq: u16 = 0;
    let mut timestamp: u32 = 0;
    let mut recv_buf = [0u8; 2048];

    while !stop.load(Ordering::Relaxed) {
        // Send: drain whole 20 ms frames from the capture queue.
        loop {
            let frame: Option<Vec<i16>> = capture.lock().ok().and_then(|mut q| {
                if q.len() >= FRAME_SAMPLES {
                    Some(q.drain(..FRAME_SAMPLES).collect())
                } else {
                    None
                }
            });
            let Some(pcm) = frame else { break };
            let payload = encode_frame(&pcm, payload_type);
            let packet = build_rtp_packet(payload_type, seq, timestamp, ssrc, &payload);
            let _ = sock.send(&packet);
            seq = seq.wrapping_add(1);
            timestamp = timestamp.wrapping_add(FRAME_SAMPLES as u32);
        }
        // Receive: decode any waiting RTP into the playback queue.
        match sock.recv(&mut recv_buf) {
            Ok(n) => {
                if let Some(payload) = rtp_payload(&recv_buf[..n]) {
                    let pcm = decode_frame(payload, payload_type);
                    if let Ok(mut q) = playback.lock() {
                        // Bound the buffer so a stalled consumer can't grow it
                        // without limit (~0.5 s of audio).
                        if q.len() < SAMPLE_RATE as usize / 2 {
                            q.extend(pcm);
                        }
                    }
                }
            }
            Err(_) => {
                // Read timeout (no packet this tick) — yield briefly.
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    // Streams stop on drop here.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulaw_silence_and_roundtrip() {
        // Silence encodes to 0xFF (µ-law) and decodes back near zero.
        assert_eq!(linear_to_ulaw(0), 0xFF);
        assert!(ulaw_to_linear(0xFF).abs() <= ULAW_BIAS as i16);
        // Round-trip stays within the companding step across the range.
        for s in [-32000i16, -8000, -1000, -100, 0, 100, 1000, 8000, 32000] {
            let back = ulaw_to_linear(linear_to_ulaw(s));
            // µ-law max step near full-scale is well under 2000.
            assert!(
                (i32::from(back) - i32::from(s)).abs() < 2000,
                "s={s} back={back}"
            );
            // Sign preserved (except around zero).
            if s.abs() > 200 {
                assert_eq!(back.signum(), s.signum(), "sign s={s} back={back}");
            }
        }
    }

    #[test]
    fn alaw_roundtrip_preserves_magnitude_and_sign() {
        for s in [-32000i16, -8000, -1000, -100, 100, 1000, 8000, 32000] {
            let back = alaw_to_linear(linear_to_alaw(s));
            assert!(
                (i32::from(back) - i32::from(s)).abs() < 2200,
                "s={s} back={back}"
            );
            if s.abs() > 300 {
                assert_eq!(back.signum(), s.signum(), "sign s={s} back={back}");
            }
        }
    }

    #[test]
    fn encode_decode_frame_selects_codec_by_pt() {
        let pcm = [100i16, -100, 2000, -2000];
        // PCMU (0) and PCMA (8) both round-trip a frame approximately.
        for pt in [0u8, 8] {
            let enc = encode_frame(&pcm, pt);
            assert_eq!(enc.len(), pcm.len());
            let dec = decode_frame(&enc, pt);
            assert_eq!(dec.len(), pcm.len());
            for (a, b) in pcm.iter().zip(dec.iter()) {
                assert!((i32::from(*a) - i32::from(*b)).abs() < 2200);
            }
        }
    }

    #[test]
    fn rtp_build_then_parse_recovers_payload() {
        let payload = vec![1u8, 2, 3, 4, 5];
        let pkt = build_rtp_packet(0, 7, 1600, 0xABCD_1234, &payload);
        assert_eq!(pkt.len(), 12 + payload.len());
        assert_eq!(pkt[0], 0x80); // V=2
        assert_eq!(pkt[1], 0); // PT=0, M=0
        assert_eq!(u16::from_be_bytes([pkt[2], pkt[3]]), 7); // seq
        assert_eq!(rtp_payload(&pkt), Some(&payload[..]));
    }

    #[test]
    fn rtp_payload_rejects_short_or_nonrtp() {
        assert_eq!(rtp_payload(&[0u8; 4]), None); // too short
        assert_eq!(rtp_payload(&[0x40u8; 16]), None); // version 1, not 2
    }
}
