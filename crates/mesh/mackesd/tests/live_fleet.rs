//! WL-TEST-002 — crown-jewel LIVE integration probe against the RUNNING fleet.
//!
//! The docker-tests suites (`tests/substrate_etcd.rs`,
//! `tests/integration_testcontainers.rs`) prove the etcd election + Nebula
//! overlay path against *disposable* containers they spin themselves. This test
//! is the other half of WL-TEST-002: the FIRST harness target named in the
//! worklist — the **existing live lighthouses**. It reads the fleet coordinates
//! from the environment and asserts, through the production `mackesd_core`
//! substrate client, that:
//!
//!   1. **etcd is reachable + quorate** — a real `member_list` returns ≥1 member
//!      and `status` reports a non-zero Raft leader (a converged cluster);
//!   2. **the Nebula overlay is live** — the replicated `/mesh/peers` directory
//!      (which a node can only populate by heartbeating *over the overlay*)
//!      carries ≥1 peer with an overlay IP;
//!   3. **the lighthouse underlay is up** — a UDP probe to the lighthouse's
//!      Nebula port is not ICMP-refused.
//!
//! ## Fail-loud, never self-skip when armed
//!
//! Mirrors the OpenStack/VNC live-test convention
//! (`docs/ops/openstack-live-test.md`, `crates/desktop/mde-vdi-vnc/tests/`):
//! `#[ignore]` so the normal suite never runs it, and env-gated so a stray
//! `--ignored` run with no target self-skips green. But once the env IS set, an
//! unreachable target is a hard **panic** — the whole point is to catch a fleet
//! whose tests are green but whose substrate is dead (the ONBOARD-6 audit-gap
//! lesson). It never returns green on a set-but-broken target.
//!
//! ## The false-green guard (load-bearing)
//!
//! Every branch prints a `CROWN-JEWEL-LIVE:` sentinel line: `SKIP` when the env
//! is unset, `RUN` + `PASS` when it runs to completion. The farm-runner wrapper
//! (`automation/testing/crown-jewel-live.sh`) keys on those: seeing `SKIP` while
//! it required the env to be set means the env **did not reach this process** —
//! the `xcp-build.sh` `remote()` SSH-env-not-forwarded false-green — and the
//! wrapper fails loud. Do NOT route this test through `xcp-build.sh cargo`
//! (`remote()` runs `cargo …` over SSH with no `SendEnv`, so the target env is
//! stripped AND a farm build VM is not on the overlay anyway); run it on a mesh
//! member where `cargo` sees the env directly.
//!
//! ## Run
//!
//! ```text
//! MCNF_LIVE_ETCD=http://10.42.0.1:2379,http://10.42.0.2:2379 \
//! MCNF_LIVE_LH=10.42.0.1:4242 \
//!   cargo test -p mackesd --test live_fleet -- --ignored --nocapture --test-threads=1
//! ```
//!
//! or, with artifact capture + the false-green guard, via
//! `automation/testing/crown-jewel-live.sh`.
//!
//! Gated behind `async-services` (the default feature) because it links the
//! substrate etcd client; a `--no-default-features` build simply omits it.

#![cfg(feature = "async-services")]
#![allow(
    clippy::panic,
    reason = "test-only live probe: a set-but-unreachable fleet MUST abort with \
              typed evidence, and panicking IS the test failure mechanism"
)]

use std::net::{ToSocketAddrs, UdpSocket};
use std::time::Duration;

use mackesd_core::substrate::{etcd, peers};

/// Sentinel prefix the farm-runner wrapper greps for (RUN / PASS present, SKIP
/// absent ⇒ the env reached this process, not a stripped-env false-green).
const SENTINEL: &str = "CROWN-JEWEL-LIVE";

/// Default the Nebula underlay port to 4242 when `MCNF_LIVE_LH` is a bare host.
/// (The fleet overlay is IPv4, so the simple "port after the last colon" rule is
/// unambiguous here.)
fn normalize_lh(raw: &str) -> String {
    let s = raw.trim();
    if let Some((_host, port)) = s.rsplit_once(':') {
        if port.parse::<u16>().is_ok() {
            return s.to_owned();
        }
    }
    format!("{s}:4242")
}

/// The crown-jewel live acceptance: etcd quorum + Nebula overlay directory +
/// lighthouse underlay reachability against the RUNNING fleet.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live fleet required — set MCNF_LIVE_ETCD + MCNF_LIVE_LH (see automation/testing/crown-jewel-live.sh)"]
async fn live_fleet_etcd_quorum_and_nebula_overlay() {
    // ── env gate — self-skip (green) ONLY when unset; armed ⇒ fail loud ───────
    let etcd_raw = std::env::var("MCNF_LIVE_ETCD").unwrap_or_default();
    let lh_raw = std::env::var("MCNF_LIVE_LH").unwrap_or_default();
    if etcd_raw.trim().is_empty() || lh_raw.trim().is_empty() {
        println!(
            "{SENTINEL}: SKIP env-unset — MCNF_LIVE_ETCD and/or MCNF_LIVE_LH not \
             set; #[ignore]-only probe, nothing run"
        );
        return;
    }
    println!("{SENTINEL}: RUN etcd={etcd_raw} lh={lh_raw}");

    // ── 1. etcd reachable + quorate (production client, member_list + status) ─
    let endpoints = etcd::parse_endpoints(&etcd_raw);
    assert!(
        !endpoints.is_empty(),
        "{SENTINEL}: FAIL MCNF_LIVE_ETCD parsed to zero endpoints: {etcd_raw:?}"
    );

    let mut client = etcd::connect(&endpoints).await.unwrap_or_else(|e| {
        panic!(
            "{SENTINEL}: FAIL etcd connect {endpoints:?}: {e} — env is set but the \
             fleet etcd is unreachable"
        )
    });

    let list = client
        .member_list()
        .await
        .unwrap_or_else(|e| panic!("{SENTINEL}: FAIL etcd member_list {endpoints:?}: {e}"));
    let members = list.members();
    for m in members {
        println!(
            "{SENTINEL}: etcd member id={:#x} name={:?} peer_urls={:?} learner={}",
            m.id(),
            m.name(),
            m.peer_urls(),
            m.is_learner()
        );
    }
    assert!(
        !members.is_empty(),
        "{SENTINEL}: FAIL etcd cluster reported zero members"
    );

    let status = client
        .status()
        .await
        .unwrap_or_else(|e| panic!("{SENTINEL}: FAIL etcd status {endpoints:?}: {e}"));
    assert!(
        status.leader() != 0,
        "{SENTINEL}: FAIL etcd has NO Raft leader (leader id 0) — quorum unhealthy"
    );
    let member_count = members.len();
    println!(
        "{SENTINEL}: etcd OK members={member_count} leader_id={:#x}",
        status.leader()
    );

    // ── 2. Nebula overlay membership — the replicated peer directory ──────────
    // A node populates /mesh/peers only by heartbeating over the overlay, so a
    // non-empty directory carrying overlay IPs is proof the Nebula plane is live.
    let dir = peers::read_peers(&mut client)
        .await
        .unwrap_or_else(|e| panic!("{SENTINEL}: FAIL reading /mesh/peers directory: {e}"));
    for p in &dir {
        println!(
            "{SENTINEL}: peer host={} role={:?} overlay={:?} health={} mde={:?} last_seen_ms={}",
            p.hostname, p.role, p.overlay_ip, p.health, p.mde_version, p.last_seen_ms
        );
    }
    assert!(
        !dir.is_empty(),
        "{SENTINEL}: FAIL /mesh/peers is EMPTY — no mesh node is heartbeating"
    );
    let overlay_peers = dir.iter().filter(|p| p.overlay_ip.is_some()).count();
    assert!(
        overlay_peers >= 1,
        "{SENTINEL}: FAIL no peer in the directory carries a Nebula overlay IP"
    );

    // ── 3. lighthouse underlay handshake probe ────────────────────────────────
    // We cannot forge a Nebula stage-1 handshake without the mesh CA, so this is
    // a reachability probe: on a CLOSED underlay port Linux surfaces ICMP
    // port-unreachable as ConnectionRefused on a *connected* UDP socket — a hard
    // FAIL. A silent drop / timeout is the healthy case (Nebula ignores
    // unauthenticated packets), recorded honestly rather than asserted.
    let lh_addr = normalize_lh(&lh_raw);
    let target = lh_addr
        .to_socket_addrs()
        .unwrap_or_else(|e| {
            panic!("{SENTINEL}: FAIL MCNF_LIVE_LH {lh_addr:?} did not resolve: {e}")
        })
        .next()
        .unwrap_or_else(|| {
            panic!("{SENTINEL}: FAIL MCNF_LIVE_LH {lh_addr:?} resolved to no address")
        });
    let sock = UdpSocket::bind("0.0.0.0:0").expect("bind ephemeral udp socket");
    sock.set_read_timeout(Some(Duration::from_millis(1500)))
        .expect("set udp read timeout");
    sock.connect(target)
        .unwrap_or_else(|e| panic!("{SENTINEL}: FAIL udp connect {target}: {e}"));
    let _ = sock.send(&[0u8; 1]);
    let mut buf = [0u8; 64];
    match sock.recv(&mut buf) {
        Ok(n) => println!("{SENTINEL}: nebula underlay {target} answered {n} bytes (listening)"),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => panic!(
            "{SENTINEL}: FAIL nebula underlay {target} REFUSED (ICMP port-unreachable) \
             — the lighthouse Nebula port is closed"
        ),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            println!(
                "{SENTINEL}: nebula underlay {target} open/filtered (no reply to an \
                 unauthenticated probe — expected for a live lighthouse)"
            );
        }
        Err(e) => println!("{SENTINEL}: nebula underlay {target} probe inconclusive: {e}"),
    }

    println!(
        "{SENTINEL}: PASS etcd({member_count} members, leader present) + \
         nebula peer directory({overlay_peers} overlay peers) + lighthouse underlay {target}"
    );
}
