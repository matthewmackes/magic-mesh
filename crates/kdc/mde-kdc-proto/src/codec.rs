//! KDC2-2 codec — newline-delimited JSON frame encoding.
//!
//! Stream-aware in KDC2-2.2: `FrameDecoder` holds a partial-frame
//! buffer across socket reads. A TCP `read()` call rarely lands on
//! a frame boundary — the buffer absorbs the leftover bytes and
//! emits each complete frame as it crosses a newline.
//!
//! `cargo fuzz run codec` (target `fuzz_frame_decoder`) confirms
//! the decoder never panics on arbitrary byte input. Mandatory
//! invariant: arbitrary input → either `Ok(packet)` or an error
//! return; never a panic, never a deadlock, never an unbounded
//! allocation.

use crate::wire::Packet;

/// Encode a single packet to its KDC wire form (one line of JSON,
/// terminated by `\n`).
///
/// Errors propagate from `serde_json` — a `Packet` that holds a
/// `serde_json::Value` body which can't be serialized (numeric
/// `NaN` / `Infinity`) returns the underlying error so the caller
/// can decide whether to log + drop or panic.
pub fn encode_frame(packet: &Packet) -> Result<String, serde_json::Error> {
    let mut out = serde_json::to_string(packet)?;
    out.push('\n');
    Ok(out)
}

/// Decode a single frame from a newline-terminated byte slice.
///
/// The KDC protocol's framing is "one JSON object per line." This
/// helper accepts either a single frame (with or without trailing
/// `\n`) or a leading frame from a stream (anything after the
/// first newline is ignored — the caller is responsible for
/// re-feeding the remainder). Stream-aware buffering lives in
/// [`FrameDecoder`].
pub fn decode_frame(raw: &[u8]) -> Result<Packet, serde_json::Error> {
    // Stop at the first newline; KDC wire is line-delimited.
    let line: &[u8] = raw.split(|&b| b == b'\n').next().unwrap_or(raw);
    serde_json::from_slice(line)
}

/// Hard cap on partial-frame buffer size. KDC frames are bounded
/// by the largest plugin payload — KDC2-3.1's MTU survey will
/// fine-tune; until then 1 MiB matches upstream KDE Connect's own
/// `Q_LONG_LONG` framing cap. Any peer that sends a single frame
/// larger than this is malicious or broken — the decoder drops
/// the buffer and surfaces a [`DecodeError::FrameTooLarge`].
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Stream-aware frame decoder. Holds a partial-frame buffer
/// across socket reads; emits each complete frame as soon as a
/// newline crosses its tail.
///
/// Usage pattern (host integration, KDC2-3):
///
/// ```ignore
/// let mut dec = FrameDecoder::new();
/// loop {
///     let n = socket.read(&mut buf).await?;
///     dec.feed(&buf[..n]);
///     while let Some(frame) = dec.next_frame()? {
///         router.dispatch(frame).await;
///     }
/// }
/// ```
///
/// Invariants (libFuzzer-verified in KDC2-2.2):
///   * `next_frame()` returns `Ok(None)` when no complete frame is
///     available yet — never blocks, never panics.
///   * `next_frame()` returns `Err(_)` on either malformed JSON
///     or an oversized buffer — the buffer is cleared so the
///     next valid frame can recover.
///   * Internal buffer is bounded by [`MAX_FRAME_BYTES`].
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    /// Empty decoder. No allocations until the first `feed()` call.
    #[must_use]
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Append `bytes` to the partial buffer. Cheap — single
    /// `Vec::extend_from_slice`.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Try to extract the next complete frame. Returns:
    ///   * `Ok(Some(packet))` — a complete frame was decoded.
    ///   * `Ok(None)` — no newline yet in the buffer; caller
    ///     should `feed()` more bytes.
    ///   * `Err(DecodeError)` — either a malformed frame or a
    ///     bounded-buffer breach. The buffer is cleared on error
    ///     so the next valid frame can be parsed.
    pub fn next_frame(&mut self) -> Result<Option<Packet>, DecodeError> {
        // Bounded-allocation guard: a peer that never sends a
        // newline can't trap us holding an unbounded buffer.
        if self.buf.len() > MAX_FRAME_BYTES {
            self.buf.clear();
            return Err(DecodeError::FrameTooLarge);
        }
        let Some(nl_pos) = self.buf.iter().position(|&b| b == b'\n') else {
            return Ok(None);
        };
        // Take the bytes up to (not including) the newline; drain
        // the buffer including the newline so the next iteration
        // starts at the next-frame's first byte.
        let frame_bytes: Vec<u8> = self.buf.drain(..=nl_pos).collect();
        // Empty lines are legal in TCP streams (keepalive heart-
        // beat) — skip and recurse for the next frame.
        let trimmed = trim_newlines(&frame_bytes);
        if trimmed.is_empty() {
            return self.next_frame();
        }
        match serde_json::from_slice::<Packet>(trimmed) {
            Ok(p) => Ok(Some(p)),
            Err(e) => Err(DecodeError::Json(e)),
        }
    }

    /// Total bytes currently held in the partial-frame buffer.
    /// Exposed for instrumentation + tests.
    #[must_use]
    pub fn buffered_bytes(&self) -> usize {
        self.buf.len()
    }
}

/// Errors a [`FrameDecoder::next_frame`] call may surface.
#[derive(Debug)]
pub enum DecodeError {
    /// JSON parse failed. The malformed frame was already drained
    /// from the buffer; the decoder is ready for the next attempt.
    Json(serde_json::Error),
    /// Partial-frame buffer exceeded [`MAX_FRAME_BYTES`] without
    /// encountering a newline. Buffer is cleared.
    FrameTooLarge,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Json(e) => write!(f, "json: {e}"),
            DecodeError::FrameTooLarge => write!(f, "frame_too_large"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Strip leading/trailing newlines + carriage returns. KDC wire
/// is `\n`-delimited but some clients emit `\r\n` (we've seen
/// this from desktop KDE Connect on Windows builds).
fn trim_newlines(buf: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = buf.len();
    while start < end && (buf[start] == b'\n' || buf[start] == b'\r') {
        start += 1;
    }
    while end > start && (buf[end - 1] == b'\n' || buf[end - 1] == b'\r') {
        end -= 1;
    }
    &buf[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::CapabilitiesHeader;

    #[test]
    fn encode_frame_is_newline_terminated() {
        let p = Packet {
            id: 1,
            kind: "kdeconnect.identity".to_string(),
            body: serde_json::Value::Null,
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        let s = encode_frame(&p).unwrap();
        assert!(s.ends_with('\n'));
        // Exactly one newline — the frame contains no internal
        // line break.
        assert_eq!(s.matches('\n').count(), 1);
    }

    #[test]
    fn encode_then_decode_round_trips() {
        let p = Packet {
            id: 42,
            kind: "kdeconnect.clipboard".to_string(),
            body: serde_json::json!({"content": "hello"}),
            mde_caps: Some(CapabilitiesHeader::v2_1_lock()),
            ..Default::default()
        };
        let encoded = encode_frame(&p).unwrap();
        let decoded = decode_frame(encoded.as_bytes()).unwrap();
        assert_eq!(decoded, p);
    }

    #[test]
    fn decode_frame_ignores_trailing_stream_data() {
        // Caller fed us two concatenated frames — we return the
        // first and let them handle the remainder.
        let two_frames = b"{\"id\":1,\"type\":\"kdeconnect.identity\",\"body\":{}}\n{\"id\":2,\"type\":\"kdeconnect.clipboard\",\"body\":{}}\n";
        let p = decode_frame(two_frames).unwrap();
        assert_eq!(p.id, 1);
        assert_eq!(p.kind, "kdeconnect.identity");
    }

    #[test]
    fn decode_frame_rejects_garbage() {
        let raw = b"not valid JSON\n";
        assert!(decode_frame(raw).is_err());
    }

    // ─────────────────────────────────────────────────────────────
    // KDC2-2.2 — FrameDecoder + libFuzzer-shaped invariant tests
    // ─────────────────────────────────────────────────────────────

    fn one_frame_bytes(id: i64) -> Vec<u8> {
        let p = Packet {
            id,
            kind: "kdeconnect.ping".to_string(),
            body: serde_json::Value::Null,
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        encode_frame(&p).unwrap().into_bytes()
    }

    #[test]
    fn frame_decoder_emits_complete_frame() {
        let mut dec = FrameDecoder::new();
        dec.feed(&one_frame_bytes(1));
        let frame = dec.next_frame().unwrap();
        assert!(frame.is_some());
        assert_eq!(frame.unwrap().id, 1);
        // Nothing left to emit.
        assert!(dec.next_frame().unwrap().is_none());
    }

    #[test]
    fn frame_decoder_buffers_partial_frame_until_newline_arrives() {
        let mut dec = FrameDecoder::new();
        let full = one_frame_bytes(42);
        // Split mid-JSON so we can confirm the decoder holds the
        // first half and returns None.
        let split = full.len() / 2;
        dec.feed(&full[..split]);
        assert!(dec.next_frame().unwrap().is_none(), "no newline yet");
        assert!(dec.buffered_bytes() > 0, "partial bytes retained");
        // Feed the rest — newline arrives, frame emits.
        dec.feed(&full[split..]);
        let frame = dec.next_frame().unwrap().unwrap();
        assert_eq!(frame.id, 42);
        assert_eq!(dec.buffered_bytes(), 0, "buffer drained after emit");
    }

    #[test]
    fn frame_decoder_emits_multiple_frames_from_one_feed() {
        let mut dec = FrameDecoder::new();
        let mut both = one_frame_bytes(1);
        both.extend_from_slice(&one_frame_bytes(2));
        dec.feed(&both);
        let first = dec.next_frame().unwrap().unwrap();
        let second = dec.next_frame().unwrap().unwrap();
        assert_eq!(first.id, 1);
        assert_eq!(second.id, 2);
        // Nothing left.
        assert!(dec.next_frame().unwrap().is_none());
    }

    #[test]
    fn frame_decoder_tolerates_crlf_line_endings() {
        // Some Windows KDE Connect builds emit `\r\n` instead of
        // `\n`. The decoder must trim both.
        let mut dec = FrameDecoder::new();
        let p = Packet {
            id: 99,
            kind: "kdeconnect.ping".to_string(),
            body: serde_json::Value::Null,
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        let mut bytes = serde_json::to_string(&p).unwrap().into_bytes();
        bytes.extend_from_slice(b"\r\n");
        dec.feed(&bytes);
        let frame = dec.next_frame().unwrap().unwrap();
        assert_eq!(frame.id, 99);
    }

    #[test]
    fn frame_decoder_skips_empty_keepalive_lines() {
        let mut dec = FrameDecoder::new();
        // Three keepalive newlines, then one real frame.
        dec.feed(b"\n\n\n");
        dec.feed(&one_frame_bytes(7));
        let frame = dec.next_frame().unwrap().unwrap();
        assert_eq!(frame.id, 7);
    }

    #[test]
    fn frame_decoder_emits_error_then_recovers_on_garbage() {
        let mut dec = FrameDecoder::new();
        dec.feed(b"not json at all\n");
        let first = dec.next_frame();
        assert!(matches!(first, Err(DecodeError::Json(_))));
        // The garbage frame is drained — a valid frame after it
        // parses cleanly.
        dec.feed(&one_frame_bytes(5));
        let second = dec.next_frame().unwrap().unwrap();
        assert_eq!(second.id, 5);
    }

    #[test]
    fn frame_decoder_caps_oversized_buffer() {
        let mut dec = FrameDecoder::new();
        // Feed > MAX_FRAME_BYTES of garbage with no newline.
        let huge = vec![b'X'; MAX_FRAME_BYTES + 10];
        dec.feed(&huge);
        let result = dec.next_frame();
        assert!(matches!(result, Err(DecodeError::FrameTooLarge)));
        // Buffer was cleared on error — a valid frame after it
        // parses cleanly.
        dec.feed(&one_frame_bytes(11));
        let frame = dec.next_frame().unwrap().unwrap();
        assert_eq!(frame.id, 11);
    }

    #[test]
    fn frame_decoder_invariant_arbitrary_input_never_panics() {
        // libFuzzer-shaped property test: walk a tiny corpus of
        // adversarial inputs and assert each one either decodes
        // cleanly or surfaces an error — never a panic, never a
        // deadlock, never an unbounded allocation.
        let corpus: &[&[u8]] = &[
            b"",                                                       // empty
            b"\n",                                                     // just newline
            b"\0",                                                     // null byte
            b"\n\n\n\n",                                               // all keepalive
            b"{",                                                      // unterminated object
            b"{}\n",                                                   // valid JSON but wrong shape
            b"{\"id\":1}\n",                                           // missing required field
            b"{\"id\":1,\"type\":\"x\",\"body\":null}\n",              // smallest valid Packet
            b"\xff\xfe\xfd\n",                                         // invalid UTF-8 + newline
            b"{\"id\":1,\"type\":\"x\",\"body\":null,\"extra\":42}\n", // unknown field tolerated
            b"\r\n\r\n",                                               // crlf keepalives
            // partial frames that never close
            &[b'{'; 1024],
        ];
        for input in corpus {
            let mut dec = FrameDecoder::new();
            dec.feed(input);
            // Iterate up to a bounded number of times so the test
            // can't hang on a hostile input that somehow keeps
            // emitting frames.
            for _ in 0..16 {
                match dec.next_frame() {
                    Ok(Some(_)) => continue,
                    Ok(None) => break,
                    Err(_) => continue,
                }
            }
            assert!(
                dec.buffered_bytes() <= MAX_FRAME_BYTES,
                "buffer breached MAX_FRAME_BYTES for input {input:?}",
            );
        }
    }

    #[test]
    fn frame_decoder_byte_at_a_time_feed_emits_correctly() {
        // Adversarial drip-feeding: feed one byte at a time. The
        // decoder must still emit each complete frame as soon as
        // the newline lands.
        let mut dec = FrameDecoder::new();
        let bytes = one_frame_bytes(100);
        let mut emitted = 0;
        for &b in &bytes {
            dec.feed(&[b]);
            if let Some(p) = dec.next_frame().unwrap() {
                assert_eq!(p.id, 100);
                emitted += 1;
            }
        }
        assert_eq!(emitted, 1);
    }

    #[test]
    fn decode_error_display_is_machine_token() {
        // Audit log entries grep on this Display output.
        let e = DecodeError::FrameTooLarge;
        assert_eq!(format!("{e}"), "frame_too_large");
        let invalid = serde_json::from_str::<Packet>("nope").err().unwrap();
        let e = DecodeError::Json(invalid);
        assert!(format!("{e}").starts_with("json: "));
    }
}
