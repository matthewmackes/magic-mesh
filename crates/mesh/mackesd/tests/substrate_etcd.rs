//! SUBSTRATE-2 (SUBSTRATE-V2) — leader election against a REAL etcd.
//!
//! Unit fakes can't prove the etcd lease + compare-and-swap election actually
//! elects exactly one leader, renews, and force-takes — the ONBOARD-6 audit-gap
//! lesson (a tempdir/mock behaves identically to the real substrate while the
//! mesh shows NO LEADER). This spins a real single-node `etcd` container via
//! `sudo podman` and drives [`mackesd_core::substrate::leader`] against it.
//!
//! Feature-gated (needs root podman + a network pull) + self-skipping, exactly
//! like `integration_testcontainers.rs`:
//!
//! ```bash
//! cargo test -p mackesd --features docker-tests -- --test-threads=1
//! ```

#![cfg(all(unix, feature = "docker-tests"))]

use std::process::Command;

use mackes_mesh_types as _; // keep the dev-graph honest
use mackesd_core::leader::AcquireResult;
use mackesd_core::substrate::etcd as etcd_client_mod;
use mackesd_core::substrate::leader;

const IMAGE: &str = "quay.io/coreos/etcd:v3.5.17";
const NAME: &str = "mcnf-substrate-etcd-test";
const HOST_PORT: u16 = 23790;

fn podman(args: &[&str]) -> (String, String, i32) {
    let out = Command::new("sudo")
        .arg("podman")
        .args(args)
        .output()
        .expect("spawning sudo podman");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// True when `sudo podman` can run at all (else the test self-skips green).
fn podman_available() -> bool {
    Command::new("sudo")
        .args(["-n", "podman", "version", "--format", "{{.Client.Version}}"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn start_etcd() -> bool {
    let _ = podman(&["rm", "-f", NAME]);
    let port = format!("{HOST_PORT}:2379");
    let (_o, e, code) = podman(&[
        "run",
        "-d",
        "--name",
        NAME,
        "-p",
        &port,
        "-e",
        "ETCD_LISTEN_CLIENT_URLS=http://0.0.0.0:2379",
        "-e",
        "ETCD_ADVERTISE_CLIENT_URLS=http://0.0.0.0:2379",
        "-e",
        "ETCD_LISTEN_PEER_URLS=http://0.0.0.0:2380",
        "-e",
        "ETCD_INITIAL_ADVERTISE_PEER_URLS=http://0.0.0.0:2380",
        "-e",
        "ETCD_INITIAL_CLUSTER=default=http://0.0.0.0:2380",
        "-e",
        "ETCD_NAME=default",
        IMAGE,
    ]);
    if code != 0 {
        eprintln!("[skip] could not start etcd container: {e}");
        return false;
    }
    true
}

fn stop_etcd() {
    let _ = podman(&["rm", "-f", NAME]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn etcd_leader_election_elects_one_renews_and_force_takes() {
    if !podman_available() {
        eprintln!("[skip] sudo podman unavailable — etcd election test skipped");
        return;
    }
    if !start_etcd() {
        return;
    }
    // Ensure teardown even on assert panic.
    let _guard = scopeguard();

    let endpoints = vec![format!("http://127.0.0.1:{HOST_PORT}")];

    // Wait for the client port to accept connections (pull + boot can take a bit).
    let mut client = None;
    for _ in 0..60 {
        if let Ok(c) = etcd_client_mod::connect(&endpoints).await {
            client = Some(c);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let mut client = client.expect("etcd client connect within 30s");
    // A get confirms the cluster is actually serving.
    for _ in 0..30 {
        if client.get("__probe__", None).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // node-a campaigns → becomes leader.
    let r = leader::campaign(&mut client, "node-a")
        .await
        .expect("campaign a");
    assert_eq!(
        r,
        AcquireResult::Acquired,
        "node-a should win the empty election"
    );
    let cur = leader::current_leader(&mut client)
        .await
        .expect("read leader");
    assert_eq!(cur.as_ref().map(|l| l.node_id.as_str()), Some("node-a"));
    assert_eq!(cur.as_ref().map(|l| l.epoch), Some(1));

    // node-b campaigns while node-a holds a live lease → follows.
    let r = leader::campaign(&mut client, "node-b")
        .await
        .expect("campaign b");
    assert!(
        matches!(r, AcquireResult::HeldBy { ref leader_id, .. } if leader_id == "node-a"),
        "node-b should follow node-a, got {r:?}"
    );

    // node-a renews → still Acquired, still epoch 1.
    let r = leader::campaign(&mut client, "node-a")
        .await
        .expect("renew a");
    assert_eq!(
        r,
        AcquireResult::Acquired,
        "node-a should renew its own lease"
    );
    let cur = leader::current_leader(&mut client)
        .await
        .expect("read leader");
    assert_eq!(
        cur.as_ref().map(|l| l.epoch),
        Some(1),
        "renew keeps the epoch"
    );

    // node-b force-takes → epoch bumps, leader flips.
    let forced = leader::force(&mut client, "node-b").await.expect("force b");
    assert_eq!(forced.node_id, "node-b");
    assert_eq!(
        forced.epoch, 2,
        "force bumps the epoch past the prior holder"
    );
    let cur = leader::current_leader(&mut client)
        .await
        .expect("read leader");
    assert_eq!(cur.as_ref().map(|l| l.node_id.as_str()), Some("node-b"));
    assert_eq!(cur.as_ref().map(|l| l.epoch), Some(2));
}

/// Minimal RAII teardown without pulling the `scopeguard` crate.
fn scopeguard() -> Teardown {
    Teardown
}
struct Teardown;
impl Drop for Teardown {
    fn drop(&mut self) {
        stop_etcd();
    }
}
