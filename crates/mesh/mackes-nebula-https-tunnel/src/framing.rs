//! NF-1.3 — 4-byte length-prefixed framing layer.
//!
//! Every Nebula UDP frame is wrapped in a 4-byte big-endian
//! length header before being written to the TLS stream. The
//! reader buffers partial frames and yields complete payloads
//! one at a time. To a passive observer the wire shape is
//! `[length: u32 BE][payload: <length> bytes]` repeated, indistinguishable from any other framed TLS stream.
//!
//! Locked invariants per the v2.5 Nebula Fabric design doc:
//!
//!   * Max frame size **1408 bytes** — Nebula's default MTU.
//!     `encode_frame` rejects oversized payloads with
//!     [`FrameError::Oversized`]; `decode_frame` rejects them
//!     symmetrically so a malicious peer can't OOM the reader.
//!   * Zero-length frames are valid — Nebula uses them as
//!     keepalives.
//!   * Length prefix is big-endian per network-byte-order
//!     convention.

use bytes::{Buf, BufMut, Bytes, BytesMut};

/// Nebula's default MTU. Locked per v2.5 design doc.
pub const MAX_FRAME_SIZE: usize = 1408;

/// 4-byte length-prefix header size.
pub const HEADER_LEN: usize = 4;

/// Framing errors. Both encode + decode paths surface the same
/// error set so callers don't need parallel match arms.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    /// Payload exceeds [`MAX_FRAME_SIZE`]. Defensive on both
    /// encode (caller bug) and decode (malicious / corrupt
    /// peer stream).
    #[error("frame payload exceeds MAX_FRAME_SIZE ({0} > {limit})", limit = MAX_FRAME_SIZE)]
    Oversized(usize),
}

/// Encode a single frame onto the output buffer. Returns the
/// number of bytes appended (always `HEADER_LEN + payload.len()`
/// on success). Caller-side guard against pathological payload
/// sizes — anything Nebula would emit is well under
/// [`MAX_FRAME_SIZE`], so this returning `Oversized` indicates
/// a producer bug, not a runtime condition.
///
/// # Errors
/// Returns [`FrameError::Oversized`] when `payload.len() >
/// MAX_FRAME_SIZE`.
pub fn encode_frame(payload: &[u8], out: &mut BytesMut) -> Result<usize, FrameError> {
    if payload.len() > MAX_FRAME_SIZE {
        return Err(FrameError::Oversized(payload.len()));
    }
    out.reserve(HEADER_LEN + payload.len());
    out.put_u32(payload.len() as u32);
    out.put_slice(payload);
    Ok(HEADER_LEN + payload.len())
}

/// Try to consume one frame from `buf`. Returns:
///
///   * `Ok(Some(bytes))` — a complete frame was extracted and
///     `buf` has been advanced past it.
///   * `Ok(None)` — the buffer contains a partial frame (not
///     enough bytes for either the header or the payload yet).
///     The caller should accumulate more bytes from the wire
///     and try again.
///   * `Err(FrameError::Oversized)` — the inbound length prefix
///     exceeds [`MAX_FRAME_SIZE`]. The caller MUST drop the
///     connection — the stream is corrupt or hostile.
///
/// `buf` is mutated in-place via `Buf::advance` on success.
/// The returned `Bytes` is zero-copy when the input buffer
/// supports it.
///
/// # Errors
/// Returns [`FrameError::Oversized`] when the header advertises
/// a frame larger than [`MAX_FRAME_SIZE`].
pub fn decode_frame(buf: &mut BytesMut) -> Result<Option<Bytes>, FrameError> {
    if buf.len() < HEADER_LEN {
        return Ok(None);
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(FrameError::Oversized(len));
    }
    if buf.len() < HEADER_LEN + len {
        return Ok(None);
    }
    buf.advance(HEADER_LEN);
    let payload = buf.split_to(len).freeze();
    Ok(Some(payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_small_payload() {
        let mut buf = BytesMut::new();
        let payload = b"hello nebula";
        let written = encode_frame(payload, &mut buf).expect("encode");
        assert_eq!(written, HEADER_LEN + payload.len());

        let decoded = decode_frame(&mut buf).expect("decode").expect("frame");
        assert_eq!(&decoded[..], payload);
        assert!(buf.is_empty(), "buf must be fully consumed");
    }

    #[test]
    fn zero_length_frame_round_trips() {
        let mut buf = BytesMut::new();
        encode_frame(&[], &mut buf).expect("encode keepalive");
        let decoded = decode_frame(&mut buf).expect("decode").expect("frame");
        assert!(decoded.is_empty());
        assert!(buf.is_empty());
    }

    #[test]
    fn max_size_frame_round_trips() {
        let mut buf = BytesMut::new();
        let payload = vec![0xAB_u8; MAX_FRAME_SIZE];
        encode_frame(&payload, &mut buf).expect("encode max");
        let decoded = decode_frame(&mut buf).expect("decode").expect("frame");
        assert_eq!(decoded.len(), MAX_FRAME_SIZE);
    }

    #[test]
    fn encode_rejects_oversized_payload() {
        let mut buf = BytesMut::new();
        let payload = vec![0x00_u8; MAX_FRAME_SIZE + 1];
        let err = encode_frame(&payload, &mut buf).unwrap_err();
        assert_eq!(err, FrameError::Oversized(MAX_FRAME_SIZE + 1));
        assert!(buf.is_empty(), "no bytes written on error");
    }

    #[test]
    fn decode_rejects_oversized_header_prefix() {
        let mut buf = BytesMut::new();
        // Craft a header advertising MAX_FRAME_SIZE + 1.
        buf.put_u32((MAX_FRAME_SIZE + 1) as u32);
        let err = decode_frame(&mut buf).unwrap_err();
        assert_eq!(err, FrameError::Oversized(MAX_FRAME_SIZE + 1));
    }

    #[test]
    fn decode_returns_none_on_short_header() {
        let mut buf = BytesMut::from(&b"\x00\x00"[..]);
        assert_eq!(decode_frame(&mut buf).unwrap(), None);
        // Buffer must be untouched so the next call sees the
        // accumulated bytes.
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn decode_returns_none_on_partial_payload() {
        let mut buf = BytesMut::new();
        buf.put_u32(10);
        buf.put_slice(b"hello"); // only 5 of 10 bytes
        assert_eq!(decode_frame(&mut buf).unwrap(), None);
        assert_eq!(buf.len(), HEADER_LEN + 5);
    }

    #[test]
    fn decode_handles_multiple_frames_in_one_buffer() {
        let mut buf = BytesMut::new();
        encode_frame(b"first", &mut buf).expect("encode a");
        encode_frame(b"second", &mut buf).expect("encode b");
        encode_frame(b"third", &mut buf).expect("encode c");

        assert_eq!(&decode_frame(&mut buf).unwrap().unwrap()[..], b"first");
        assert_eq!(&decode_frame(&mut buf).unwrap().unwrap()[..], b"second");
        assert_eq!(&decode_frame(&mut buf).unwrap().unwrap()[..], b"third");
        assert_eq!(decode_frame(&mut buf).unwrap(), None);
    }

    #[test]
    fn decode_handles_partial_frame_across_multiple_reads() {
        // Simulate two TLS reads: header alone, then payload.
        let mut buf = BytesMut::new();
        buf.put_u32(5);
        // First call: header present, payload missing.
        assert_eq!(decode_frame(&mut buf).unwrap(), None);
        buf.put_slice(b"hi"); // partial — only 2 of 5
        assert_eq!(decode_frame(&mut buf).unwrap(), None);
        buf.put_slice(b"abc"); // completes the 5-byte payload.
        let frame = decode_frame(&mut buf).unwrap().unwrap();
        assert_eq!(&frame[..], b"hiabc");
        assert!(buf.is_empty());
    }
}
