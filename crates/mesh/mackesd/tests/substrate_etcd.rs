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

use mackes_mesh_types::peers::PeerRecord;
use mackesd_core::leader::AcquireResult;
use mackesd_core::substrate::etcd as etcd_client_mod;
use mackesd_core::substrate::leader;
use mackesd_core::substrate::peers as etcd_peers;

const IMAGE: &str = "quay.io/coreos/etcd:v3.5.17";
const NAME: &str = "mcnf-substrate-etcd-test";
const HOST_PORT: u16 = 23790;
const PEERS_NAME: &str = "mcnf-substrate-etcd-peers-test";
const PEERS_PORT: u16 = 23791;

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

fn start_etcd_named(name: &str, host_port: u16) -> bool {
    let _ = podman(&["rm", "-f", name]);
    let port = format!("{host_port}:2379");
    let (_o, e, code) = podman(&[
        "run",
        "-d",
        "--name",
        name,
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

/// Connect a client once the container is serving; `None` on timeout.
async fn connect_ready(endpoints: &[String]) -> Option<etcd_client::Client> {
    for _ in 0..60 {
        if let Ok(mut c) = etcd_client_mod::connect(endpoints).await {
            if c.get("__probe__", None).await.is_ok() {
                return Some(c);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn etcd_leader_election_elects_one_renews_and_force_takes() {
    if !podman_available() {
        eprintln!("[skip] sudo podman unavailable — etcd election test skipped");
        return;
    }
    if !start_etcd_named(NAME, HOST_PORT) {
        return;
    }
    // Ensure teardown even on assert panic.
    let _guard = Teardown(NAME);

    let endpoints = vec![format!("http://127.0.0.1:{HOST_PORT}")];
    let mut client = connect_ready(&endpoints)
        .await
        .expect("etcd serving within 30s");

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn etcd_peer_directory_round_trips_and_deletes() {
    if !podman_available() {
        eprintln!("[skip] sudo podman unavailable — etcd peer-directory test skipped");
        return;
    }
    if !start_etcd_named(PEERS_NAME, PEERS_PORT) {
        return;
    }
    let _guard = Teardown(PEERS_NAME);

    let endpoints = vec![format!("http://127.0.0.1:{PEERS_PORT}")];
    let mut client = connect_ready(&endpoints)
        .await
        .expect("etcd serving within 30s");

    // Empty directory to start.
    let dir = etcd_peers::read_peers(&mut client)
        .await
        .expect("read empty");
    assert!(dir.is_empty(), "fresh etcd has no peers");

    // Two nodes publish their records (out of hostname order).
    let mut b = PeerRecord::now("node-b", Some("11.0.0".to_string()), "healthy");
    b.overlay_ip = Some("10.42.0.3".into());
    let a = PeerRecord::now("node-a", Some("11.0.0".to_string()), "degraded");
    etcd_peers::put_peer(&mut client, &b).await.expect("put b");
    etcd_peers::put_peer(&mut client, &a).await.expect("put a");

    // read_peers returns both, sorted by hostname, with fields intact.
    let dir = etcd_peers::read_peers(&mut client).await.expect("read two");
    assert_eq!(dir.len(), 2);
    assert_eq!(dir[0].hostname, "node-a");
    assert_eq!(dir[1].hostname, "node-b");
    assert_eq!(dir[1].overlay_ip.as_deref(), Some("10.42.0.3"));
    assert_eq!(dir[0].health, "degraded");

    // An explicit leave removes just that row.
    etcd_peers::delete_peer(&mut client, "node-a")
        .await
        .expect("delete a");
    let dir = etcd_peers::read_peers(&mut client)
        .await
        .expect("read after delete");
    assert_eq!(dir.len(), 1);
    assert_eq!(dir[0].hostname, "node-b");

    // The blocking wrappers (the heartbeat/directory bridges) work too. They
    // build a private current-thread runtime, so they must run OFF the test's
    // tokio worker — exactly how the real callers (the heartbeat std::thread +
    // the directory responder thread) invoke them. spawn_blocking models that.
    let c = PeerRecord::now("node-c", None, "healthy");
    let eps = endpoints.clone();
    let cc = c.clone();
    let put_ok = tokio::task::spawn_blocking(move || etcd_peers::put_peer_blocking(&eps, &cc))
        .await
        .unwrap();
    assert!(put_ok, "blocking put");
    let eps = endpoints.clone();
    let via_blocking = tokio::task::spawn_blocking(move || etcd_peers::read_peers_blocking(&eps))
        .await
        .unwrap()
        .expect("blocking read");
    assert!(via_blocking.iter().any(|p| p.hostname == "node-c"));
}

/// Minimal RAII teardown (removes the named container) without the `scopeguard`
/// crate.
struct Teardown(&'static str);
impl Drop for Teardown {
    fn drop(&mut self) {
        let _ = podman(&["rm", "-f", self.0]);
    }
}
