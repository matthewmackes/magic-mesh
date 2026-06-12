//! EFF-36 — property tests for the untrusted KDC wire decoder.
//!
//! The codec module's mandatory invariant ("arbitrary input → either
//! Ok or an error; never a panic, never unbounded allocation") was
//! previously *claimed* via a cargo-fuzz target that never existed
//! in-tree. These proptest suites make the invariant machine-checked
//! in plain `cargo test` (and therefore CI) instead.

use mde_kdc_proto::codec::{decode_frame, encode_frame, FrameDecoder, MAX_FRAME_BYTES};
use mde_kdc_proto::wire::Packet;
use proptest::prelude::*;

proptest! {
    /// Arbitrary bytes through the one-shot decoder: error or packet,
    /// never a panic.
    #[test]
    fn decode_frame_never_panics(raw in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let _ = decode_frame(&raw);
    }

    /// Arbitrary bytes streamed through the stateful decoder in
    /// arbitrary chunkings: never a panic, and the partial-frame
    /// buffer stays bounded by MAX_FRAME_BYTES + one feed's worth.
    #[test]
    fn frame_decoder_never_panics_and_stays_bounded(
        chunks in proptest::collection::vec(
            proptest::collection::vec(any::<u8>(), 0..512),
            0..32,
        )
    ) {
        let mut dec = FrameDecoder::new();
        for chunk in &chunks {
            dec.feed(chunk);
            // Drain every available frame / error after each feed —
            // exactly how the transport pump drives it.
            loop {
                match dec.next_frame() {
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            prop_assert!(
                dec.buffered_bytes() <= MAX_FRAME_BYTES + 512,
                "buffer must stay bounded (got {})",
                dec.buffered_bytes()
            );
        }
    }

    /// encode → decode round-trips a packet with an arbitrary type
    /// token + arbitrary JSON-string body content.
    #[test]
    fn encode_decode_round_trips(
        id in any::<i64>(),
        kind in "[a-zA-Z0-9._-]{1,64}",
        body_key in "[a-zA-Z0-9_]{1,16}",
        body_val in "\\PC{0,128}",  // arbitrary printable unicode
    ) {
        let packet = Packet {
            id,
            kind: kind.clone(),
            body: serde_json::json!({ body_key.clone(): body_val.clone() }),
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        let line = encode_frame(&packet).expect("encode");
        let back = decode_frame(line.as_bytes()).expect("decode");
        prop_assert_eq!(back.id, id);
        prop_assert_eq!(back.kind, kind);
        prop_assert_eq!(&back.body[body_key.as_str()], &serde_json::json!(body_val));
    }

    /// A split point anywhere in a valid frame must still decode via
    /// the streaming path (TCP reads rarely land on frame edges).
    #[test]
    fn streaming_reassembles_split_frames(split in 0usize..100) {
        let packet = Packet {
            id: 42,
            kind: "kdeconnect.ping".into(),
            body: serde_json::json!({"msg": "split-me"}),
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        };
        let line = encode_frame(&packet).expect("encode");
        let bytes = line.as_bytes();
        let cut = split.min(bytes.len());
        let mut dec = FrameDecoder::new();
        dec.feed(&bytes[..cut]);
        // Nothing decodes until the newline has been fed; when the
        // cut lands at the very end the first drain already yields it.
        let early = dec.next_frame().expect("no decode error");
        if cut < bytes.len() {
            prop_assert!(early.is_none());
        }
        dec.feed(&bytes[cut..]);
        let got = match early {
            Some(p) => p, // whole frame arrived in the first feed
            None => dec.next_frame().expect("decode").expect("frame present"),
        };
        prop_assert_eq!(got.id, 42);
    }
}

/// Oversize junk (no newline) past MAX_FRAME_BYTES must surface the
/// bounded-buffer error and reset — not grow forever. Deterministic
/// (one 2 MiB case is enough; a proptest loop over multi-MiB inputs
/// would just burn CI time).
#[test]
fn oversize_junk_errors_and_resets() {
    let mut dec = FrameDecoder::new();
    dec.feed(&vec![b'x'; MAX_FRAME_BYTES + 1]);
    assert!(dec.next_frame().is_err(), "over-budget buffer must error");
    // After the error the decoder must be usable again.
    let packet = Packet {
        id: 7,
        kind: "kdeconnect.ping".into(),
        body: serde_json::json!({}),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    };
    let line = encode_frame(&packet).expect("encode");
    dec.feed(line.as_bytes());
    let got = dec.next_frame().expect("recovered").expect("frame");
    assert_eq!(got.id, 7);
}
