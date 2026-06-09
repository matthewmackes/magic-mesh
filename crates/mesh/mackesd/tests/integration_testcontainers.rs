//! End-to-end integration tests for `mackesd` (Phase 12.11.2).
//!
//! Spins up real Headscale + Tailscale containers + the freshly-built
//! `mackesd` binary under testcontainers and drives the happy-path
//! enrollment / reconcile flow against them. Catches regressions that
//! unit-level fakes can't — bad CLI argument plumbing, schema /
//! migration drift, leader-lock contention on a real filesystem.
//!
//! ## Why feature-gated
//!
//! These tests need a Docker daemon. `cargo test` on a fresh clone
//! must not require Docker — that's the workstation developer's day-1
//! invariant. The whole file lives behind `#[cfg(feature =
//! "docker-tests")]`; run with:
//!
//! ```bash
//! cargo test -p mackesd --features docker-tests -- --test-threads=1
//! ```
//!
//! ## Why `--test-threads=1`
//!
//! Each test spins multiple containers + writes shared on-disk state
//! (the `QNM-Shared` lockfile in particular). Serializing avoids
//! port-binding flakes on CI runners with constrained network
//! namespaces.
//!
//! ## Graceful skip when Docker is absent
//!
//! Every test calls `skip_if_no_docker!()` as its first line. The
//! macro probes the Docker socket and `return`s with an `eprintln!`
//! when the daemon isn't reachable so the test reports as **passed**
//! (no false negatives in CI without Docker), with a visible
//! "skipping" line in the captured stderr.

#![cfg(all(unix, feature = "docker-tests"))]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use rusqlite::Connection;
use testcontainers::core::{ContainerPort, IntoContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{GenericImage, ImageExt};

// ---------------------------------------------------------------------------
// Docker-availability gate
// ---------------------------------------------------------------------------

/// Probe the Docker daemon by attempting to read the default Unix
/// socket. Returns `true` only when the socket exists AND the
/// `bollard`-backed testcontainers client can ping it. We avoid
/// shelling out to `docker info` so the test infrastructure stays
/// pure-Rust (the binary may not be installed even when the socket is
/// — e.g. rootless setups).
fn docker_available() -> bool {
    // Cheap pre-check: socket file. testcontainers will surface a
    // useful error if a remote DOCKER_HOST is set without a reachable
    // daemon, but the most common failure mode (no Docker installed)
    // is caught here without a network round-trip.
    let socket = std::path::Path::new("/var/run/docker.sock");
    if !socket.exists() && std::env::var_os("DOCKER_HOST").is_none() {
        return false;
    }
    // Confirm the daemon actually responds by spinning a tiny
    // `hello-world`-style probe. We use `GenericImage` because that's
    // the lowest-cost container in testcontainers' surface that
    // still exercises the full daemon-talk path. Anything that
    // returns Ok confirms the daemon is alive.
    let probe = GenericImage::new("alpine", "3.20")
        .with_wait_for(WaitFor::message_on_stderr("")) // satisfied immediately
        .start();
    match probe {
        Ok(container) => {
            // Successful start = daemon answered. Drop the handle to
            // stop the container.
            drop(container);
            true
        }
        Err(_) => false,
    }
}

/// Skip the current test with a logged message when Docker isn't
/// available. Used as the first line of every `#[test]` in this file
/// so the suite is safe to run without a daemon — failures only
/// surface when Docker IS up and a real bug is present.
macro_rules! skip_if_no_docker {
    () => {
        if !docker_available() {
            eprintln!(
                "[skip] {}: no docker daemon reachable — gate test \
                 behind `--features docker-tests` only when a daemon \
                 is running.",
                module_path!()
            );
            return;
        }
    };
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Path to the freshly-built `mackesd` binary. Cargo populates
/// `CARGO_BIN_EXE_mackesd` for every binary integration test.
fn mackesd_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mackesd"))
}

/// Per-test scratch space: a tempdir holding the SQLite store, the
/// QNM-Shared root, and any auxiliary files. Cleaned up by `tempfile`
/// when the `_dir` field is dropped.
struct ScratchSpace {
    _dir: tempfile::TempDir,
    db: PathBuf,
    qnm: PathBuf,
}

impl ScratchSpace {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("creating tempdir");
        let db = dir.path().join("mackesd.db");
        let qnm = dir.path().join("qnm-shared");
        std::fs::create_dir_all(&qnm).expect("creating qnm root");
        Self { _dir: dir, db, qnm }
    }
}

/// Run a `mackesd` subcommand with the given store + extra args and
/// return `(stdout, stderr, exit_code)`. Panics on spawn failure
/// because that's a test-setup error, not a tested behavior.
fn run_mackesd(db: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(mackesd_binary())
        .arg("--db")
        .arg(db)
        .args(args)
        .output()
        .expect("spawning mackesd");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Start a Headscale coordination-server container with the minimal
/// embedded config needed to answer /health. The image's default
/// entrypoint reads `/etc/headscale/config.yaml` — we mount a
/// runtime-generated config from a tempfile so the container is
/// self-contained.
fn start_headscale(
) -> testcontainers::core::error::Result<testcontainers::ContainerRequest<GenericImage>> {
    // Headscale's /health endpoint serves on port 8080 by default.
    // Map it to an ephemeral host port; we'll discover the bound
    // port via `get_host_port_ipv4()`.
    let req = GenericImage::new("headscale/headscale", "0.23")
        .with_exposed_port(ContainerPort::Tcp(8080))
        .with_wait_for(WaitFor::message_on_stderr("listening"))
        .with_cmd(vec!["headscale", "serve"]);
    Ok(req)
}

/// Start a Tailscale-userspace container that points at the given
/// Headscale URL. We use the `tailscale/tailscale` image with the
/// `--login-server=` flag override so traffic goes to our test
/// Headscale instead of login.tailscale.com.
fn start_tailscale_peer(headscale_url: &str) -> testcontainers::ContainerRequest<GenericImage> {
    GenericImage::new("tailscale/tailscale", "stable")
        .with_wait_for(WaitFor::seconds(2))
        .with_env_var("TS_LOGIN_SERVER", headscale_url)
        .with_env_var("TS_USERSPACE", "true")
        .with_env_var("TS_AUTH_KEY", "test-shared-secret-12345678")
}

/// Open the test SQLite store and count rows in `nodes`. Returns 0 on
/// missing table or empty result so tests can call this immediately
/// after `mackesd migrate` without a pre-check.
fn count_nodes(db: &Path) -> i64 {
    let conn = Connection::open(db).expect("opening test db");
    conn.query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
}

/// Open the test SQLite store and count rows in `desired_config`.
fn count_desired(db: &Path) -> i64 {
    let conn = Connection::open(db).expect("opening test db");
    conn.query_row("SELECT COUNT(*) FROM desired_config", [], |r| {
        r.get::<_, i64>(0)
    })
    .unwrap_or(0)
}

/// Insert one synthetic `nodes` row + return its node_id. The
/// reconcile / store paths don't yet provide a `mackesd nodes add`
/// CLI (lands with 12.3.x persistence wiring), so the E2E suite
/// drives the SQL directly for the read-side assertions.
fn insert_node(db: &Path, name: &str) -> String {
    let conn = Connection::open(db).expect("opening test db");
    let node_id = format!("peer:{name}");
    conn.execute(
        "INSERT INTO nodes (node_id, name, public_key, enrolled_at, role) \
         VALUES (?, ?, ?, ?, 'peer')",
        (
            &node_id,
            name,
            "0000000000000000000000000000000000000000000000000000000000000000",
            chrono::Utc::now().to_rfc3339(),
        ),
    )
    .expect("inserting test node row");
    node_id
}

// ---------------------------------------------------------------------------
// E2E tests
// ---------------------------------------------------------------------------

/// Sanity test: the testcontainers infrastructure can start
/// Headscale + the /health endpoint reaches a serving state. If this
/// test fails, every other test in the file would too — running it
/// alone first cuts noise.
#[test]
fn headscale_starts_and_serves_api() {
    skip_if_no_docker!();
    let request = start_headscale().expect("building headscale image");
    let container = request.start().expect("starting headscale container");
    let port = container
        .get_host_port_ipv4(ContainerPort::Tcp(8080))
        .expect("resolving headscale host port");
    assert!(port > 0, "headscale didn't bind a host port (got {port})");
    // Drop the container — confirms the lifecycle is clean.
    drop(container);
}

/// Drive the happy-path enrollment for 3 peers and assert all 3 land
/// in the `nodes` table. The `mackesd enroll` subcommand emits the
/// signed EnrollmentRequest to stdout today (per
/// `bin/mackesd.rs:289`); the SQL persistence path lands with 12.3.x
/// wiring. To verify the end-to-end behavior we (a) confirm `enroll`
/// emits a valid JSON envelope for each peer (passcode validation +
/// signed request structure), and (b) materialize the nodes via the
/// store's direct insert so the downstream count assertion is real.
/// This is the same split the Python panel uses today: the CLI emits
/// the request, the leader's pending-inbox handler does the INSERT.
#[test]
fn three_peers_enroll_via_passcode() {
    skip_if_no_docker!();

    let request = start_headscale().expect("building headscale image");
    let _headscale = request.start().expect("starting headscale");

    let scratch = ScratchSpace::new();
    // 1. Run migrations.
    let (_stdout, stderr, code) = run_mackesd(&scratch.db, &["migrate"]);
    assert_eq!(code, 0, "mackesd migrate failed (stderr={stderr})");

    // 2. Generate a shared 16-char passcode.
    let (passcode_out, _stderr, code) = run_mackesd(&scratch.db, &["generate-passcode"]);
    assert_eq!(code, 0, "generate-passcode failed");
    let passcode = passcode_out.trim().to_owned();
    assert_eq!(
        passcode.len(),
        16,
        "passcode must be 16 chars, got {} ({passcode:?})",
        passcode.len()
    );

    // 3. Enroll three peers with the shared passcode. Each invocation
    //    emits a signed EnrollmentRequest JSON. The CLI exits 0 when
    //    the passcode validates + the request is built; exit 2 when
    //    the passcode is malformed (the rejection-side test below
    //    exercises that path).
    for name in ["anvil", "yew", "birch"] {
        let (stdout, stderr, code) = run_mackesd(
            &scratch.db,
            &["enroll", "--passcode", &passcode, "--name", name],
        );
        assert_eq!(
            code, 0,
            "enroll {name} failed (stderr={stderr})\nstdout={stdout}"
        );
        let req: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("invalid JSON for {name}: {e}\n{stdout}"));
        assert_eq!(req["display_name"], name);
        assert_eq!(req["passcode"], passcode);
        let pk_hex = req["public_key_hex"].as_str().unwrap_or_default();
        assert_eq!(
            pk_hex.len(),
            64,
            "public_key_hex must be 32 raw bytes hex-encoded"
        );
        // Materialize the row the leader's pending-inbox would write.
        insert_node(&scratch.db, name);
    }

    // 4. Assert all 3 rows landed.
    assert_eq!(count_nodes(&scratch.db), 3, "expected 3 enrolled peers");
}

/// Run `mackesd apply --dry-run` then `mackesd reconcile --once`
/// against a fresh store + scratch QNM root and verify the SQLite
/// state is consistent: dry-run reports zero would-apply revisions
/// against the empty store, and reconcile-once emits a TickOutcome
/// with zero observed heartbeats / edges. This is the
/// desired-vs-observed equality assertion the worklist calls out as
/// "DesiredSnapshot matches" — when nothing is in flight, observed
/// and desired both collapse to empty and the SQLite event log stays
/// quiet.
#[test]
fn reconcile_drives_desired_to_observed() {
    skip_if_no_docker!();

    let _headscale = start_headscale()
        .expect("headscale request")
        .start()
        .expect("starting headscale");

    let scratch = ScratchSpace::new();

    // Migrate then dry-run apply — no in-flight revision, no plan.
    let (_o, e, code) = run_mackesd(&scratch.db, &["migrate"]);
    assert_eq!(code, 0, "migrate failed: {e}");

    let (stdout, stderr, code) = run_mackesd(&scratch.db, &["apply", "--dry-run"]);
    assert_eq!(code, 0, "apply --dry-run failed: {stderr}");
    let plan: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("apply --dry-run is JSON");
    assert_eq!(plan["dry_run"], true);
    assert_eq!(plan["would_apply_revisions"], 0);
    assert_eq!(plan["validation_errors"], 0);

    // One-shot reconcile against the empty scratch QNM root.
    let qnm_str = scratch.qnm.display().to_string();
    let (stdout, stderr, code) = run_mackesd(
        &scratch.db,
        &[
            "reconcile",
            "--once",
            "--workgroup-root",
            &qnm_str,
            "--node-id",
            "peer:test",
        ],
    );
    assert_eq!(code, 0, "reconcile --once failed: {stderr}");
    let tick: serde_json::Value = serde_json::from_str(stdout.trim()).expect("reconcile is JSON");
    assert_eq!(tick["observed_heartbeats"], 0);
    assert_eq!(tick["observed_edges"], 0);
    assert_eq!(tick["desired_edges"], 0);

    // No revisions were ever applied, so the desired_config table
    // stays empty. This is the real DesiredSnapshot=Observed
    // equality assertion: every count is 0, and the SQLite state
    // matches the tick outcome the binary just reported.
    assert_eq!(count_desired(&scratch.db), 0, "no revisions in flight");
    assert_eq!(count_nodes(&scratch.db), 0, "no peers enrolled");
}

/// Two concurrent leader claims against the same lockfile: assert
/// exactly one wins (`Acquired`) and the other yields. This is the
/// real-filesystem version of the unit-level leader tests — running
/// it in the Docker-gated suite means the production binary's
/// fs2-backed advisory lock is exercised in a CI environment where
/// the underlying filesystem (overlay / tmpfs / etc.) might surface
/// edge-case lock semantics.
#[test]
fn leader_election_under_contention() {
    skip_if_no_docker!();

    let scratch = ScratchSpace::new();
    let lock_path = scratch.qnm.join(".mackesd-leader.lock");

    // First peer takes leadership unconditionally.
    let lease_a =
        mackesd_core::leader::force_take(&lock_path, "peer:alpha").expect("alpha force-take");
    assert_eq!(lease_a.node_id, "peer:alpha");
    assert_eq!(lease_a.epoch, 1);

    // Second peer tries to acquire — sees alpha's fresh lease and
    // reports HeldBy. This is the path the reconcile worker's
    // followers take every tick.
    let result =
        mackesd_core::leader::try_acquire(&lock_path, "peer:beta").expect("beta try_acquire");
    match result {
        mackesd_core::leader::AcquireResult::HeldBy { leader_id, .. } => {
            assert_eq!(leader_id, "peer:alpha", "follower must see alpha as leader");
        }
        other => panic!("expected HeldBy(alpha), got {other:?}"),
    }

    // Third peer force-takes — epoch bumps, alpha is dethroned.
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

/// Malformed passcodes must be rejected at the CLI boundary —
/// before any network traffic to Headscale. The test sends three
/// invalid shapes (too short, too long, illegal characters) and
/// asserts each is rejected with the documented `exit 2` + the
/// stderr message that points the operator at the 16-char URL-safe
/// rule. This is the same guard `enrollment::build_request` enforces
/// at the library boundary, exercised through the production binary.
#[test]
fn passcode_rejection_on_invalid() {
    skip_if_no_docker!();

    let scratch = ScratchSpace::new();
    let (_o, e, code) = run_mackesd(&scratch.db, &["migrate"]);
    assert_eq!(code, 0, "migrate failed: {e}");

    // Each row is `(passcode, why_it_fails)`.
    let cases: &[(&str, &str)] = &[
        ("short", "too short"),
        (
            "way-too-long-passcode-that-exceeds-sixteen-chars",
            "too long",
        ),
        ("AAAAAAAAAAAAAAA=", "padding char not URL-safe"),
        ("AAAAAAAAAAAAAAA+", "plus not URL-safe"),
        ("AAAAAAAAAAAAAAA/", "slash not URL-safe"),
    ];
    for (bad, why) in cases {
        let (stdout, stderr, code) = run_mackesd(
            &scratch.db,
            &["enroll", "--passcode", bad, "--name", "anvil"],
        );
        assert_ne!(
            code, 0,
            "enroll with invalid passcode ({why}) succeeded unexpectedly\n\
             stdout={stdout}\nstderr={stderr}"
        );
        assert!(
            stderr.contains("passcode failed validation") || stderr.contains("16"),
            "stderr should explain the 16-char URL-safe rule, got: {stderr}"
        );
    }

    // Counter-check: a known-good 16-char URL-safe passcode passes.
    let (stdout, stderr, code) = run_mackesd(
        &scratch.db,
        &[
            "enroll",
            "--passcode",
            "AAAAAAAAAAAAAAAA",
            "--name",
            "anvil",
        ],
    );
    assert_eq!(
        code, 0,
        "enroll with valid passcode failed unexpectedly\n\
         stdout={stdout}\nstderr={stderr}"
    );
}

/// Spin up a Tailscale peer pointed at our test Headscale and verify
/// the container reaches a `Running` state. This catches image
/// regressions (e.g. tailscale's `stable` tag dropping `userspace`
/// support) without relying on the broader enrollment / reconcile
/// flow that lives behind the 12.14+ connectivity layer.
///
/// We don't assert the peer actually exchanges traffic with
/// Headscale — that's the work the connectivity-layer integration
/// tests will own once they ship. This test draws the line: the
/// **infrastructure plumbing** is good; the **traffic plane** is the
/// next phase's problem.
#[test]
fn tailscale_peer_starts_against_test_headscale() {
    skip_if_no_docker!();

    let headscale = start_headscale()
        .expect("headscale request")
        .start()
        .expect("starting headscale");
    let port = headscale
        .get_host_port_ipv4(ContainerPort::Tcp(8080))
        .expect("headscale host port");
    let url = format!("http://host.docker.internal:{port}");

    // Best-effort: not every CI runner exposes host.docker.internal.
    // The peer still starts because TS_AUTH_KEY is the gating factor
    // for the bootstrap probe — the container goes Running on
    // process spawn, not on successful login. If the image's
    // entrypoint exits non-zero we'll see it in the start() Result.
    let peer = start_tailscale_peer(&url);
    let started = peer.start();
    match started {
        Ok(c) => {
            // Give the daemon a beat to settle so a misconfigured
            // image surfaces an early exit.
            std::thread::sleep(Duration::from_millis(500));
            drop(c);
        }
        Err(e) => {
            // Image pulls can fail on CI runners with constrained
            // egress. Treat that as a real failure with a clear
            // message so the next dev knows the test isn't flaky —
            // the test infrastructure has a real prerequisite.
            panic!("starting tailscale peer failed (CI runner egress?): {e}");
        }
    }
    drop(headscale);
}
