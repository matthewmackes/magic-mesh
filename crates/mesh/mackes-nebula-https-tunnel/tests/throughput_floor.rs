//! NF-1.6 (v2.5) — throughput floor bench test.
//!
//! Validates the Q10 covert-path SLO: with a localhost-only
//! peer ↔ lighthouse tunnel, framed Nebula packets transit at
//! ≥ 5 Mbps. The test doesn't spin up real Nebula or rustls —
//! it pumps framed bytes through the `framing` layer + a
//! tokio TCP loopback pair, which is the throughput-bound
//! piece (frame encode/decode + buffer copy + tokio scheduler
//! overhead). Real TLS + real Nebula add fixed latency but
//! don't dominate the bytes/sec budget on commodity hardware.
//!
//! Run under `cargo test -p mackes-nebula-https-tunnel
//! --test throughput_floor -- --include-ignored`. The
//! `--include-ignored` flag is required because the bench
//! takes ~5s + we don't want it on every PR — the operator
//! runs it pre-cut. CI gates on the same harness via the
//! NF-9.4 acceptance scenario.

use std::time::{Duration, Instant};

use bytes::BytesMut;
use mackes_nebula_https_tunnel::{decode_frame, encode_frame, MAX_FRAME_SIZE};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Q10 lock — 5 Mbps minimum localhost throughput on x86_64
/// Fedora 44 CI. Real bench hardware typically clears 50-100
/// Mbps; this floor is the safety net.
const FLOOR_MBPS: f64 = 5.0;

/// Bytes per frame for the bench (full MTU). Matches Nebula's
/// default packet size.
const BYTES_PER_FRAME: usize = MAX_FRAME_SIZE;

/// Total bytes to transfer. 100 MB matches the worklist's
/// NF-1.6 entry.
const TOTAL_BYTES: usize = 100 * 1024 * 1024;

#[tokio::test]
#[ignore = "slow — operator runs pre-cut via --include-ignored"]
async fn throughput_floor_5mbps_on_localhost_loopback() {
    // Spin up a tokio TCP server that decodes framed bytes +
    // discards the payloads. Pumps a frame count derived from
    // TOTAL_BYTES through the client side + measures wall-clock.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let server_addr = listener.local_addr().unwrap();

    let frame_count = TOTAL_BYTES.div_ceil(BYTES_PER_FRAME);
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut buf = BytesMut::with_capacity(BYTES_PER_FRAME * 4);
        let mut chunk = vec![0u8; BYTES_PER_FRAME * 2];
        let mut decoded = 0usize;
        while decoded < frame_count {
            let n = stream.read(&mut chunk).await.expect("read");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            while let Some(_frame) = decode_frame(&mut buf).expect("decode") {
                decoded += 1;
                if decoded >= frame_count {
                    break;
                }
            }
        }
        decoded
    });

    // Client side: encode + send.
    let mut client = TcpStream::connect(server_addr).await.expect("connect");
    let payload = vec![0xABu8; BYTES_PER_FRAME];
    let mut frame_buf = BytesMut::with_capacity(BYTES_PER_FRAME + 32);

    let started = Instant::now();
    for _ in 0..frame_count {
        frame_buf.clear();
        encode_frame(&payload, &mut frame_buf).expect("encode");
        client.write_all(&frame_buf).await.expect("write");
    }
    client.flush().await.expect("flush");
    drop(client); // signal EOF so server task exits

    let decoded = tokio::time::timeout(Duration::from_secs(30), server_task)
        .await
        .expect("server didn't finish in 30s")
        .expect("server task joined");
    let elapsed = started.elapsed();

    let bits = (decoded * BYTES_PER_FRAME * 8) as f64;
    let mbps = bits / elapsed.as_secs_f64() / 1_000_000.0;
    eprintln!(
        "NF-1.6: transferred {decoded} frames ({} MB) in {:.2}s = {:.2} Mbps",
        decoded * BYTES_PER_FRAME / (1024 * 1024),
        elapsed.as_secs_f64(),
        mbps,
    );
    assert!(
        mbps >= FLOOR_MBPS,
        "NF-1.6 throughput floor: {mbps:.2} Mbps < {FLOOR_MBPS:.2} Mbps minimum",
    );
}

#[test]
fn floor_constant_matches_q10_lock() {
    // Sanity check: the operator-visible doc copy
    // (docs/help/mesh-nebula.md + the NF-1.6 worklist entry)
    // both quote "5 Mbps". Keep this constant in lock-step.
    assert_eq!(FLOOR_MBPS, 5.0);
}

#[test]
fn total_bytes_matches_worklist_lock() {
    // Worklist NF-1.6: "pushes 100 MB through a localhost tunnel".
    assert_eq!(TOTAL_BYTES, 100 * 1024 * 1024);
}
