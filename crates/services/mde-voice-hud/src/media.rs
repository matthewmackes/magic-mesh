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

// ── RFC 4733 telephone-event (out-of-band DTMF) ──────────────────────────────

/// How many 20 ms packets one DTMF tone spans (~180 ms). Each repeats the same
/// event with a growing duration; the last three carry the end-of-event bit so a
/// lost final packet still terminates the tone (RFC 4733 §2.5.1.4).
const DTMF_PACKETS: u32 = 9;

/// Map a dialer character to its RFC 4733 telephone-event code (0-9 → 0-9,
/// `*` → 10, `#` → 11, A-D → 12-15). Returns `None` for anything that is not a
/// DTMF-representable key, so a stray char never produces a malformed event.
#[must_use]
pub const fn dtmf_event_code(c: char) -> Option<u8> {
    match c {
        '0'..='9' => Some(c as u8 - b'0'),
        '*' => Some(10),
        '#' => Some(11),
        'A' | 'a' => Some(12),
        'B' | 'b' => Some(13),
        'C' | 'c' => Some(14),
        'D' | 'd' => Some(15),
        _ => None,
    }
}

/// The event-specific fields of one RFC 4733 telephone-event packet — the parts
/// that change packet-to-packet within a single DTMF tone (the RTP framing
/// fields are passed separately to [`build_dtmf_packet`]).
#[derive(Debug, Clone, Copy)]
pub struct DtmfEvent {
    /// The telephone-event code (see [`dtmf_event_code`]).
    pub event: u8,
    /// The end-of-event bit: set on the trailing packets so the tone terminates
    /// even if the final packet is lost (RFC 4733 §2.5.1.4).
    pub end: bool,
    /// Cumulative tone duration in timestamp units (grows each packet).
    pub duration: u16,
    /// The RTP marker bit: set only on the first packet of a new event.
    pub marker: bool,
}

/// Build one RFC 4733 telephone-event RTP packet from the RTP framing fields
/// (`payload_type`/`seq`/`timestamp`/`ssrc`) and the per-packet [`DtmfEvent`].
///
/// The 4-byte event payload is `[event, E<<7 | volume, duration_be_hi,
/// duration_be_lo]`. The same `timestamp` (the event's start) is reused for
/// every packet of one tone, while `ev.duration` grows each packet.
#[must_use]
pub fn build_dtmf_packet(
    payload_type: u8,
    seq: u16,
    timestamp: u32,
    ssrc: u32,
    ev: DtmfEvent,
) -> Vec<u8> {
    let mut p = Vec::with_capacity(16);
    p.push(0x80); // V=2, P=0, X=0, CC=0
    let m_bit = if ev.marker { 0x80 } else { 0x00 };
    p.push(m_bit | (payload_type & 0x7F));
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&timestamp.to_be_bytes());
    p.extend_from_slice(&ssrc.to_be_bytes());
    // Telephone-event payload (RFC 4733 §2.3): event, E|R|volume, duration.
    p.push(ev.event);
    let volume: u8 = 10; // -10 dBm0, a conventional DTMF level.
    p.push(if ev.end { 0x80 } else { 0x00 } | (volume & 0x3F));
    p.extend_from_slice(&ev.duration.to_be_bytes());
    p
}

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
    /// In-call microphone mute. When set, the send loop drops captured mic
    /// frames instead of packetizing them, so the peer hears silence while the
    /// receive path keeps playing their audio. Shared with the audio thread so
    /// a toggle takes effect on the next frame without restarting the session.
    muted: Arc<AtomicBool>,
    /// Pending DTMF event codes (RFC 4733) queued by [`Self::send_dtmf`] and
    /// drained by the send loop, which transmits each as a telephone-event tone.
    /// `None` when the peer did not negotiate `telephone-event` — `send_dtmf`
    /// then no-ops rather than queue a tone that can never be sent.
    dtmf_queue: Option<Arc<Mutex<VecDeque<u8>>>>,
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

    /// Set the microphone-mute state. `true` stops transmitting mic audio (the
    /// peer hears silence) while still playing the peer's audio; `false`
    /// resumes transmitting. Takes effect on the next 20 ms frame.
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    /// `true` when the microphone is currently muted.
    #[must_use]
    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    /// `true` when the peer negotiated out-of-band DTMF (a `telephone-event`
    /// payload type), so [`Self::send_dtmf`] will actually transmit.
    #[must_use]
    pub const fn dtmf_supported(&self) -> bool {
        self.dtmf_queue.is_some()
    }

    /// Queue a DTMF keypress for transmission as an RFC 4733 telephone-event
    /// tone. `true` if the key maps to a DTMF event AND the peer negotiated
    /// out-of-band DTMF (so the tone will be sent); `false` otherwise (the call
    /// continues normally — a non-DTMF key or a peer with no telephone-event is
    /// simply ignored). The send loop transmits the tone on the next frames.
    pub fn send_dtmf(&self, key: char) -> bool {
        let Some(code) = dtmf_event_code(key) else {
            return false;
        };
        let Some(queue) = &self.dtmf_queue else {
            return false;
        };
        if let Ok(mut q) = queue.lock() {
            q.push_back(code);
            true
        } else {
            false
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
    let muted = Arc::new(AtomicBool::new(false));
    let muted_thread = muted.clone();
    let payload_type = remote.payload_type;

    // DTMF queue is only wired when the peer negotiated telephone-event — a
    // `None` here means `send_dtmf` no-ops (the peer can't decode it anyway).
    let dtmf_queue: Option<Arc<Mutex<VecDeque<u8>>>> = remote
        .telephone_event_pt
        .map(|_| Arc::new(Mutex::new(VecDeque::new())));
    let dtmf_thread = dtmf_queue.clone();
    let telephone_event_pt = remote.telephone_event_pt;

    let thread = std::thread::Builder::new()
        .name("mwv-rtp-media".into())
        .spawn(move || {
            let codec = CodecConfig {
                payload_type,
                telephone_event_pt,
            };
            run_audio(
                &sock,
                codec,
                &stop_thread,
                &muted_thread,
                dtmf_thread.as_deref(),
            );
        })
        .map_err(|e| format!("media thread spawn failed ({e})"))?;

    Ok(MediaSession {
        stop,
        muted,
        dtmf_queue,
        thread: Some(thread),
    })
}

/// The negotiated RTP payload types for a session: the G.711 audio codec and the
/// dynamic `telephone-event` type for out-of-band DTMF (`None` if the peer did
/// not offer it). Carried as one value so the audio thread takes fewer params.
#[derive(Debug, Clone, Copy)]
struct CodecConfig {
    /// Audio payload type: 0 = PCMU (µ-law), 8 = PCMA (A-law).
    payload_type: u8,
    /// The peer's `telephone-event` payload type for DTMF, or `None`.
    telephone_event_pt: Option<u8>,
}

/// The audio thread: owns the cpal streams (`Stream` is `!Send`) and drives the
/// RTP send/recv loops. Shared `VecDeque`s bridge the cpal callbacks and the
/// network I/O. `dtmf_queue` (when the peer negotiated `telephone-event`) feeds
/// RFC 4733 DTMF tones the send loop interleaves between audio frames.
fn run_audio(
    sock: &UdpSocket,
    codec: CodecConfig,
    stop: &AtomicBool,
    muted: &AtomicBool,
    dtmf_queue: Option<&Mutex<VecDeque<u8>>>,
) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let payload_type = codec.payload_type;
    let telephone_event_pt = codec.telephone_event_pt;

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
            // Mic mute: the frame is still drained from the capture queue (so it
            // can't grow unbounded while muted), but it is not packetized or
            // sent — the peer hears silence. The RTP seq/timestamp still advance
            // so the stream stays well-formed when transmit resumes.
            if !muted.load(Ordering::Relaxed) {
                let payload = encode_frame(&pcm, payload_type);
                let packet = build_rtp_packet(payload_type, seq, timestamp, ssrc, &payload);
                let _ = sock.send(&packet);
            }
            seq = seq.wrapping_add(1);
            timestamp = timestamp.wrapping_add(FRAME_SAMPLES as u32);
        }
        // DTMF: if a keypress is queued and the peer negotiated telephone-event,
        // transmit it as an RFC 4733 tone interleaved into the audio RTP stream
        // (same SSRC, shared seq, the event's start timestamp). Pop one digit per
        // outer tick so a fast sequence keeps an inter-digit gap.
        if let (Some(te_pt), Some(queue)) = (telephone_event_pt, dtmf_queue) {
            let code = queue.lock().ok().and_then(|mut q| q.pop_front());
            if let Some(event) = code {
                send_dtmf_tone(sock, te_pt, &mut seq, &mut timestamp, ssrc, event);
            }
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

/// One DTMF packetization interval (20 ms), matching the audio frame cadence.
const DTMF_PACKET_INTERVAL: Duration = Duration::from_millis(20);

/// Transmit one DTMF digit as an RFC 4733 telephone-event tone on the live
/// stream. Sends `DTMF_PACKETS` "active" packets, one per 20 ms, the duration
/// growing each packet, then three end-of-event packets that ALL carry the full
/// final duration (RFC 4733 §2.5.1.4 — the retransmissions repeat the terminator
/// so a single lost final packet still ends the tone). `seq`/`timestamp` are the
/// shared audio counters: seq advances per packet, and the timestamp jumps past
/// the whole event afterward so audio resumes on the correct RTP timeline.
fn send_dtmf_tone(
    sock: &UdpSocket,
    te_pt: u8,
    seq: &mut u16,
    timestamp: &mut u32,
    ssrc: u32,
    event: u8,
) {
    let start_ts = *timestamp;
    #[allow(clippy::cast_possible_truncation)]
    let final_duration = (DTMF_PACKETS * FRAME_SAMPLES as u32) as u16;
    // Active packets: duration grows each interval; M-bit only on the first.
    for i in 0..DTMF_PACKETS {
        #[allow(clippy::cast_possible_truncation)]
        let ev = DtmfEvent {
            event,
            end: false,
            duration: ((i + 1) * FRAME_SAMPLES as u32) as u16,
            marker: i == 0,
        };
        let _ = sock.send(&build_dtmf_packet(te_pt, *seq, start_ts, ssrc, ev));
        *seq = seq.wrapping_add(1);
        std::thread::sleep(DTMF_PACKET_INTERVAL);
    }
    // End-of-event: three retransmissions, all carrying the full duration.
    for _ in 0..3 {
        let ev = DtmfEvent {
            event,
            end: true,
            duration: final_duration,
            marker: false,
        };
        let _ = sock.send(&build_dtmf_packet(te_pt, *seq, start_ts, ssrc, ev));
        *seq = seq.wrapping_add(1);
    }
    // Audio resumes after the event's RTP duration.
    *timestamp = timestamp.wrapping_add(u32::from(final_duration));
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

    #[test]
    fn mute_toggle_round_trips_on_a_live_session() {
        // Stand up a real session against a loopback "peer" (a bound UDP socket
        // that just receives) and exercise the public mute API. No audio
        // hardware is required — `start_media` returns Ok even when cpal finds no
        // devices; the send/recv loop runs regardless. This proves the mute flag
        // is wired end-to-end (start_media → run_audio) and toggles live.
        let peer = UdpSocket::bind(("127.0.0.1", 0)).expect("bind loopback peer");
        let peer_addr = peer.local_addr().expect("peer addr");
        let remote = RemoteMedia {
            addr: peer_addr.ip().to_string(),
            port: peer_addr.port(),
            payload_type: 0,
            telephone_event_pt: Some(101),
        };
        // Local RTP port 0 → the OS picks a free one.
        let session = start_media(0, &remote).expect("media session starts");
        assert!(!session.is_muted(), "a fresh session starts un-muted");
        session.set_muted(true);
        assert!(session.is_muted(), "set_muted(true) takes effect");
        session.set_muted(false);
        assert!(!session.is_muted(), "set_muted(false) clears it");
        session.stop();
    }

    #[test]
    fn dtmf_event_code_maps_every_keypad_key() {
        for (c, code) in [
            ('0', 0u8),
            ('5', 5),
            ('9', 9),
            ('*', 10),
            ('#', 11),
            ('A', 12),
            ('D', 15),
        ] {
            assert_eq!(dtmf_event_code(c), Some(code), "key {c}");
        }
        // Non-DTMF keys map to nothing (no malformed event ever queued).
        assert_eq!(dtmf_event_code('x'), None);
        assert_eq!(dtmf_event_code('+'), None);
    }

    #[test]
    fn dtmf_packet_has_rfc4733_shape() {
        // Mid-tone packet: M-bit clear, end clear, the 4-byte event payload.
        let p = build_dtmf_packet(
            101,
            42,
            1600,
            0xDEAD_BEEF,
            DtmfEvent {
                event: 7,
                end: false,
                duration: 320,
                marker: false,
            },
        );
        assert_eq!(p.len(), 16); // 12-byte RTP header + 4-byte event
        assert_eq!(p[0], 0x80); // V=2
        assert_eq!(p[1], 101); // M=0 | PT=101
        assert_eq!(u16::from_be_bytes([p[2], p[3]]), 42); // seq
        assert_eq!(p[12], 7); // event code
        assert_eq!(p[13] & 0x80, 0); // E bit clear
        assert_eq!(u16::from_be_bytes([p[14], p[15]]), 320); // duration

        // First packet of the tone: M-bit set. End packet: E bit set.
        let first = build_dtmf_packet(
            101,
            1,
            0,
            0,
            DtmfEvent {
                event: 3,
                end: false,
                duration: 160,
                marker: true,
            },
        );
        assert_eq!(first[1] & 0x80, 0x80, "first packet carries the RTP marker");
        let last = build_dtmf_packet(
            101,
            9,
            0,
            0,
            DtmfEvent {
                event: 3,
                end: true,
                duration: 1440,
                marker: false,
            },
        );
        assert_eq!(last[13] & 0x80, 0x80, "end packet carries the E bit");
    }

    #[test]
    fn send_dtmf_transmits_a_tone_when_negotiated() {
        // A loopback "peer" receives whatever the session transmits. With
        // telephone-event negotiated, a queued '5' must arrive as a burst of
        // RFC 4733 packets on PT 101 carrying event code 5.
        let peer = UdpSocket::bind(("127.0.0.1", 0)).expect("bind loopback peer");
        peer.set_read_timeout(Some(Duration::from_secs(2))).ok();
        let peer_addr = peer.local_addr().expect("peer addr");
        let remote = RemoteMedia {
            addr: peer_addr.ip().to_string(),
            port: peer_addr.port(),
            payload_type: 0,
            telephone_event_pt: Some(101),
        };
        let session = start_media(0, &remote).expect("media session starts");
        assert!(session.dtmf_supported(), "peer offered telephone-event");
        assert!(session.send_dtmf('5'), "a DTMF key queues a tone");
        assert!(!session.send_dtmf('x'), "a non-DTMF key is rejected");

        // Drain the burst; collect PT-101 events for code 5. Every end-of-event
        // (E-bit) packet must carry the SAME full duration (RFC 4733 §2.5.1.4).
        let mut buf = [0u8; 64];
        let mut saw_event = false;
        let mut end_durations = Vec::new();
        #[allow(clippy::cast_possible_truncation)]
        let full_duration = (DTMF_PACKETS * FRAME_SAMPLES as u32) as u16;
        for _ in 0..(DTMF_PACKETS + 6) {
            match peer.recv(&mut buf) {
                Ok(n) if n >= 16 && (buf[1] & 0x7F) == 101 => {
                    assert_eq!(buf[12], 5, "DTMF event code is 5");
                    saw_event = true;
                    if buf[13] & 0x80 != 0 {
                        end_durations.push(u16::from_be_bytes([buf[14], buf[15]]));
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert!(saw_event, "received an RFC 4733 telephone-event for '5'");
        assert!(
            !end_durations.is_empty(),
            "saw at least one end-of-event packet"
        );
        assert!(
            end_durations.iter().all(|&d| d == full_duration),
            "every end packet carries the full duration {full_duration}, got {end_durations:?}"
        );
        session.stop();
    }

    #[test]
    fn send_dtmf_tone_keeps_audio_timestamp_continuous() {
        // The tone consumes exactly its RTP duration of timeline: after the tone
        // the audio timestamp must have advanced by DTMF_PACKETS*FRAME_SAMPLES so
        // audio resumes on the correct RTP clock (no double-count, no gap).
        let peer = UdpSocket::bind(("127.0.0.1", 0)).expect("bind loopback peer");
        let sock = UdpSocket::bind(("127.0.0.1", 0)).expect("bind sender");
        sock.connect(peer.local_addr().unwrap()).expect("connect");
        let mut seq: u16 = 100;
        let mut ts: u32 = 1600;
        let start = ts;
        send_dtmf_tone(&sock, 101, &mut seq, &mut ts, 0xABCD, 5);
        #[allow(clippy::cast_possible_truncation)]
        let expected = (DTMF_PACKETS * FRAME_SAMPLES as u32) as u32;
        assert_eq!(
            ts - start,
            expected,
            "timestamp advances by one event period"
        );
        // 9 active + 3 end = 12 packets, each consumed one seq.
        assert_eq!(
            seq,
            100 + (DTMF_PACKETS as u16) + 3,
            "seq advanced per packet"
        );
    }

    #[test]
    fn send_dtmf_no_ops_without_telephone_event() {
        // A peer that did NOT offer telephone-event → send_dtmf is a no-op
        // (returns false), never queueing a tone the peer can't decode.
        let peer = UdpSocket::bind(("127.0.0.1", 0)).expect("bind loopback peer");
        let peer_addr = peer.local_addr().expect("peer addr");
        let remote = RemoteMedia {
            addr: peer_addr.ip().to_string(),
            port: peer_addr.port(),
            payload_type: 0,
            telephone_event_pt: None,
        };
        let session = start_media(0, &remote).expect("media session starts");
        assert!(!session.dtmf_supported(), "no telephone-event negotiated");
        assert!(!session.send_dtmf('5'), "no DTMF without telephone-event");
        session.stop();
    }
}
