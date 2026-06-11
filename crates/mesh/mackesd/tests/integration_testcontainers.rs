//! OBS-1 — end-to-end Nebula overlay integration test (Phase E1).
//!
//! Spins up real container nodes running the **freshly-built `mackesd`** plus
//! the real `nebula`/`nebula-cert` binaries and drives the actual v2.5 mesh
//! lifecycle end-to-end, then asserts **overlay reachability** between two
//! nodes — the thing unit-level fakes can never prove:
//!
//!   1. lighthouse: `mesh-init` (mint CA, self-sign, issue token) → `serve`
//!      (the auto-signer + nebula-supervisor render `/etc/nebula/` + materialize
//!      certs) → start `nebula` on the rendered config.
//!   2. peer: `role-pin` → `enroll --token` (publishes a CSR to the shared
//!      QNM-Shared; the lighthouse's auto-signer signs it + writes the bundle
//!      back) → `serve` (materializes `/etc/nebula/`) → start `nebula`.
//!   3. assert: a bidirectional ICMP ping across the `10.42.0.0/17` overlay.
//!
//! This is the regression net for the whole enrollment/CA/overlay path —
//! exactly where the 2026-06-11 cluster of bed bugs lived (signing under the
//! wrong mesh, hostname-vs-real lighthouse addr, stale scratch cert, peer-cert
//! upsert, mesh-init/serve node-id divergence). Each of those produced a green
//! unit suite but a dead overlay; this test would have caught every one.
//!
//! ## Why podman directly (not testcontainers)
//!
//! Nebula's `tun` device needs a privileged container on a **root** daemon —
//! rootless userns cannot open `/dev/net/tun`. Driving `sudo podman` directly
//! gives explicit control over `--privileged --device /dev/net/tun`, the shared
//! QNM-Shared bind mount, the inter-node network, and the `nebula` process start
//! (the systemd units the supervisor would normally `systemctl start` aren't
//! present in a bare container — so the harness starts `nebula` on the config
//! mackesd rendered, which is mackesd's real deliverable). Talking to a root
//! daemon through testcontainers' socket abstraction is fragile in exactly the
//! way that breeds flaky tests; explicit `podman` calls are deterministic.
//!
//! ## Why feature-gated + how to run
//!
//! Needs root podman + a network-egress to pull `fedora:42` + build the node
//! image. A fresh `cargo test` must not require any of that, so the whole file
//! is behind `#[cfg(feature = "docker-tests")]`:
//!
//! ```bash
//! cargo test -p mackesd --features docker-tests -- --test-threads=1
//! ```
//!
//! ## Graceful skip
//!
//! `skip_if_no_nebula_containers!()` probes that `sudo podman` can run a
//! privileged container AND create a tun device. When it can't (no podman, no
//! passwordless sudo, a sandbox that blocks tun) the test logs a `[skip]` line
//! and returns green — no false negatives where the prerequisite is absent.

#![cfg(all(unix, feature = "docker-tests"))]

use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// podman plumbing
// ---------------------------------------------------------------------------

const IMAGE: &str = "mackes-nebula-test:latest";
const CONTAINERFILE: &str = "install-helpers/nebula-test-node.Containerfile";

/// Path to the freshly-built `mackesd` binary (cargo sets this for bin tests).
fn mackesd_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mackesd"))
}

/// Repo root — the test runs with CWD at the crate dir, so the Containerfile
/// lives two levels up (`crates/mesh/mackesd` → repo root).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Run `sudo podman <args>`; return `(stdout, stderr, exit_code)`.
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

/// True when root podman can run a privileged container that creates a tun
/// device — the hard prerequisite for a nebula overlay.
fn nebula_containers_available() -> bool {
    // `sudo -n true` first: a non-interactive sudo that can't elevate means we
    // can't run a root daemon, so skip rather than block on a password prompt.
    let sudo_ok = Command::new("sudo")
        .args(["-n", "true"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !sudo_ok {
        return false;
    }
    let (_o, _e, code) = podman(&[
        "run",
        "--rm",
        "--cap-add=NET_ADMIN",
        "--device",
        "/dev/net/tun",
        "docker.io/library/fedora:42",
        "sh",
        "-c",
        "dnf install -q -y iproute >/dev/null 2>&1; ip tuntap add dev probe0 mode tun",
    ]);
    code == 0
}

/// Skip the test (green) when the container/tun prerequisites are absent.
macro_rules! skip_if_no_nebula_containers {
    () => {
        if !nebula_containers_available() {
            eprintln!(
                "[skip] {}: root podman + /dev/net/tun unavailable — run with \
                 `--features docker-tests` on a host where `sudo podman` can \
                 create a tun device.",
                module_path!()
            );
            return;
        }
    };
}

/// Build the nebula node image from the committed Containerfile (idempotent;
/// podman caches layers). Returns false on build failure so the test can skip
/// with a clear message rather than spuriously fail on a transient pull error.
fn ensure_image() -> bool {
    let root = repo_root();
    let cf = root.join(CONTAINERFILE);
    let (_o, e, code) = podman(&[
        "build",
        "-q",
        "-t",
        IMAGE,
        "-f",
        cf.to_str().expect("containerfile path utf8"),
        root.to_str().expect("repo root utf8"),
    ]);
    if code != 0 {
        eprintln!("[skip] could not build {IMAGE}: {e}");
    }
    code == 0
}

// ---------------------------------------------------------------------------
// node orchestration
// ---------------------------------------------------------------------------

/// A running test node container. `Drop` force-removes it so a panicking
/// assertion never leaks containers.
struct Node {
    name: String,
    node_id: String,
}

impl Node {
    /// Boot a detached idle container on `net` with the QNM-Shared dir + the
    /// freshly-built mackesd bind-mounted, privileged + tun for nebula.
    fn boot(name: &str, node_id: &str, net: &str, qnm: &Path) -> Self {
        let mk = format!(
            "{}:/opt/mackes/mackesd:ro,Z",
            mackesd_binary().to_str().expect("mackesd path utf8")
        );
        let qnm_mount = format!("{}:/qnm:Z", qnm.to_str().expect("qnm path utf8"));
        let (_o, e, code) = podman(&[
            "run",
            "-d",
            "--name",
            name,
            "--hostname",
            name,
            "--network",
            net,
            "--privileged",
            "--device",
            "/dev/net/tun",
            "-v",
            &qnm_mount,
            "-v",
            &mk,
            IMAGE,
            "sleep",
            "infinity",
        ]);
        assert_eq!(code, 0, "boot {name} failed: {e}");
        let node = Self {
            name: name.to_owned(),
            node_id: node_id.to_owned(),
        };
        let (_o, e, code) = podman(&[
            "exec",
            name,
            "mkdir",
            "-p",
            "/var/lib/mde",
            "/var/lib/mackesd/nebula-ca",
        ]);
        assert_eq!(code, 0, "mkdir in {name} failed: {e}");
        node
    }

    /// `podman exec` a mackesd subcommand with the node's env (deterministic
    /// node-id + shared QNM root + per-node `MDE_HOME`).
    fn mackesd(&self, args: &[&str]) -> (String, String, i32) {
        let nid = format!("MACKESD_NODE_ID={}", self.node_id);
        let mut full = vec![
            "exec",
            &self.name,
            "env",
            "QNM_SHARED_ROOT=/qnm",
            "MDE_HOME=/var/lib/mde",
            &nid,
            "/opt/mackes/mackesd",
        ];
        full.extend_from_slice(args);
        podman(&full)
    }

    /// Launch `mackesd serve` detached (the auto-signer + nebula-supervisor).
    fn serve(&self) {
        let cmd = format!(
            "env QNM_SHARED_ROOT=/qnm MDE_HOME=/var/lib/mde MACKESD_NODE_ID={} \
             /opt/mackes/mackesd serve > /tmp/serve.log 2>&1",
            self.node_id
        );
        let (_o, e, code) = podman(&["exec", "-d", &self.name, "sh", "-c", &cmd]);
        assert_eq!(code, 0, "serve {} failed: {e}", self.name);
    }

    /// The container's IP on `net` (the nebula underlay address).
    fn underlay_ip(&self, net: &str) -> String {
        let fmt = format!("{{{{(index .NetworkSettings.Networks \"{net}\").IPAddress}}}}");
        let (o, e, code) = podman(&["inspect", &self.name, "--format", &fmt]);
        assert_eq!(code, 0, "inspect {} failed: {e}", self.name);
        o.trim().to_owned()
    }

    /// Start `nebula` on the mackesd-rendered config (the data-plane step the
    /// systemd unit performs in production). Detached.
    fn start_nebula(&self) {
        let (_o, e, code) = podman(&[
            "exec",
            "-d",
            &self.name,
            "sh",
            "-c",
            "nebula -config /etc/nebula/config.yaml > /tmp/nebula.log 2>&1",
        ]);
        assert_eq!(code, 0, "start nebula {} failed: {e}", self.name);
    }

    /// Poll until `/etc/nebula/config.yaml` exists (the supervisor rendered it).
    fn wait_for_config(&self) -> bool {
        for _ in 0..40 {
            let (_o, _e, code) =
                podman(&["exec", &self.name, "test", "-f", "/etc/nebula/config.yaml"]);
            if code == 0 {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        false
    }

    /// Poll until `nebula1` carries a `10.42.x.y` overlay address; return it.
    fn wait_for_overlay(&self) -> Option<String> {
        for _ in 0..25 {
            let (o, _e, _c) = podman(&["exec", &self.name, "ip", "-4", "addr", "show", "nebula1"]);
            if let Some(line) = o
                .lines()
                .find(|l| l.trim_start().starts_with("inet 10.42."))
            {
                return line
                    .split_whitespace()
                    .nth(1)
                    .map(std::borrow::ToOwned::to_owned);
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        None
    }

    /// `ping -c <n>` an overlay address from inside this node; true on 0% loss.
    fn can_ping(&self, target: &str) -> bool {
        let (o, _e, code) = podman(&["exec", &self.name, "ping", "-c", "3", "-W", "2", target]);
        code == 0 && o.contains("0% packet loss")
    }

    /// Dump the in-container nebula log — called on failure for a real diagnosis.
    fn nebula_log(&self) -> String {
        podman(&["exec", &self.name, "tail", "-20", "/tmp/nebula.log"]).0
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = podman(&["rm", "-f", &self.name]);
    }
}

/// A throwaway podman network removed on drop.
struct Network(String);
impl Network {
    fn create(name: &str) -> Self {
        let _ = podman(&["network", "rm", name]); // clear a stale one
        let (_o, e, code) = podman(&["network", "create", name]);
        assert_eq!(code, 0, "network create {name} failed: {e}");
        Self(name.to_owned())
    }
}
impl Drop for Network {
    fn drop(&mut self) {
        let _ = podman(&["network", "rm", &self.0]);
    }
}

// ---------------------------------------------------------------------------
// the E2E
// ---------------------------------------------------------------------------

/// Full happy-path: a lighthouse + one peer form a real Nebula overlay and can
/// ping each other across it. Proves the v2.5 enrollment/CA/config-render path
/// end-to-end against the real nebula data plane.
#[test]
fn nebula_two_node_overlay_reachability() {
    skip_if_no_nebula_containers!();
    if !ensure_image() {
        return; // build failure already logged a [skip] line
    }

    // Shared QNM-Shared rendezvous (the enrollment exchange) — world-writable
    // so the in-container root daemons + this test user both touch it.
    let qnm = tempfile::tempdir().expect("qnm tempdir");
    let _ = Command::new("chmod")
        .args(["777", qnm.path().to_str().unwrap()])
        .status();

    let net = Network::create("mackes-e2e-net");
    let lh = Node::boot("lh01", "peer:lh01", &net.0, qnm.path());
    let pe = Node::boot("pe01", "peer:pe01", &net.0, qnm.path());

    // --- lighthouse: mesh-init → serve → nebula ---
    let lh_ip = lh.underlay_ip(&net.0);
    assert!(!lh_ip.is_empty(), "lighthouse got no underlay ip");
    let ext_addr = format!("{lh_ip}:4242");
    let (_o, e, code) = lh.mackesd(&[
        "mesh-init",
        "--mesh-id",
        "test-mesh",
        "--external-addr",
        &ext_addr,
    ]);
    assert_eq!(code, 0, "mesh-init failed: {e}");
    lh.serve();
    assert!(
        lh.wait_for_config(),
        "lighthouse never rendered /etc/nebula/config.yaml"
    );
    lh.start_nebula();
    let lh_overlay = lh.wait_for_overlay();
    assert!(
        lh_overlay.is_some(),
        "lighthouse nebula1 never came up; nebula log:\n{}",
        lh.nebula_log()
    );
    assert!(
        lh_overlay.as_deref().unwrap().starts_with("10.42.0.1/"),
        "lighthouse overlay ip should be 10.42.0.1, got {lh_overlay:?}"
    );

    // --- mint a fresh join token carrying the real underlay addr ---
    let (tok_out, e, code) = lh.mackesd(&[
        "enroll-token",
        "--mesh-id",
        "test-mesh",
        "--lighthouse",
        &ext_addr,
    ]);
    assert_eq!(code, 0, "enroll-token failed: {e}");
    let token = tok_out
        .split_whitespace()
        .find(|w| w.starts_with("mesh:"))
        .expect("enroll-token printed a mesh: join token")
        .to_owned();

    // --- peer: role-pin → enroll → serve → nebula ---
    let (_o, e, code) = pe.mackesd(&["role-pin", "server"]);
    assert_eq!(code, 0, "peer role-pin failed: {e}");
    let (eo, ee, ecode) = pe.mackesd(&["enroll", "--token", &token]);
    assert_eq!(
        ecode, 0,
        "peer enroll failed — the lighthouse auto-signer didn't sign the CSR.\n\
         stdout={eo}\nstderr={ee}"
    );
    assert!(
        eo.contains("enrolled into mesh") || eo.contains("overlay 10.42"),
        "enroll output didn't confirm a signed bundle: {eo}"
    );
    pe.serve();
    assert!(
        pe.wait_for_config(),
        "peer never materialized /etc/nebula/config.yaml from its bundle"
    );
    pe.start_nebula();
    let pe_overlay = pe.wait_for_overlay();
    assert!(
        pe_overlay.is_some(),
        "peer nebula1 never came up; nebula log:\n{}",
        pe.nebula_log()
    );

    // --- the payoff: bidirectional overlay reachability ---
    assert!(
        pe.can_ping("10.42.0.1"),
        "peer could not reach the lighthouse over the overlay; peer nebula log:\n{}",
        pe.nebula_log()
    );
    assert!(
        lh.can_ping("10.42.0.2"),
        "lighthouse could not reach the peer over the overlay; lighthouse nebula log:\n{}",
        lh.nebula_log()
    );
}

// ---------------------------------------------------------------------------
// host-side tests (no container; exercise the production binary + lib on a
// real filesystem). Kept here behind the same feature so the whole "real
// integration" suite runs together.
// ---------------------------------------------------------------------------

/// Two concurrent leader claims against one lockfile: exactly one wins, the
/// other yields, and a force-take bumps the epoch — the fs2-backed advisory
/// lock exercised on a real filesystem.
#[test]
fn leader_election_under_contention() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lock_path = dir.path().join(".mackesd-leader.lock");

    let lease_a =
        mackesd_core::leader::force_take(&lock_path, "peer:alpha").expect("alpha force-take");
    assert_eq!(lease_a.node_id, "peer:alpha");

    let result =
        mackesd_core::leader::try_acquire(&lock_path, "peer:beta").expect("beta try_acquire");
    match result {
        mackesd_core::leader::AcquireResult::HeldBy { leader_id, .. } => {
            assert_eq!(leader_id, "peer:alpha", "follower must see alpha as leader");
        }
        other => panic!("expected HeldBy(alpha), got {other:?}"),
    }

    let lease_c =
        mackesd_core::leader::force_take(&lock_path, "peer:gamma").expect("gamma force-take");
    assert_eq!(lease_c.node_id, "peer:gamma");
    assert!(
        lease_c.epoch > lease_a.epoch,
        "force_take must bump epoch (alpha={} → gamma={})",
        lease_a.epoch,
        lease_c.epoch
    );
}

/// Malformed passcodes are rejected at the CLI boundary (exit != 0) before any
/// mesh side effects — the `enrollment::build_request` guard, through the
/// production binary.
#[test]
fn passcode_rejection_on_invalid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("mackesd.db");
    let run = |args: &[&str]| -> (String, i32) {
        let out = Command::new(mackesd_binary())
            .arg("--db")
            .arg(&db)
            .args(args)
            .output()
            .expect("spawning mackesd");
        (
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    };
    let (_e, code) = run(&["migrate"]);
    assert_eq!(code, 0, "migrate failed");

    for bad in [
        "short",
        "way-too-long-passcode-that-exceeds-sixteen-chars",
        "AAAAAAAAAAAAAAA=",
        "AAAAAAAAAAAAAAA+",
        "AAAAAAAAAAAAAAA/",
    ] {
        let (stderr, code) = run(&["enroll", "--passcode", bad, "--name", "anvil"]);
        assert_ne!(code, 0, "invalid passcode {bad:?} was accepted");
        assert!(
            stderr.contains("passcode") || stderr.contains("16"),
            "stderr should explain the 16-char URL-safe rule, got: {stderr}"
        );
    }
}
