//! The integration-gated LIVE gates for mde-kvm (see the crate docs).
//!
//! Everything mechanically checkable is pure + unit-tested inside the crate;
//! these tests exercise the two things that need a real environment — a
//! **live guest boot** and a **live migration** — against real
//! `cloud-hypervisor --api-socket …` processes. The build farm has neither
//! KVM VMMs nor golden images, so both tests are `#[ignore]`d and env-gated;
//! run them on a Workstation with `cargo test -p mde-kvm -- --ignored`.
//!
//! Gates:
//! - boot: `MDE_KVM_TEST_SOCKET` (a live VMM api-socket) +
//!   `MDE_KVM_TEST_DISK` (a bootable disk image).
//! - migration: `MDE_KVM_TEST_MIGRATE_SRC_SOCKET` (a VMM running a guest) +
//!   `MDE_KVM_TEST_MIGRATE_DST_SOCKET` (an empty VMM) +
//!   `MDE_KVM_TEST_MIGRATE_URL` (the stream endpoint both sides share, e.g.
//!   `unix:/tmp/mig.sock` same-host, or a `tcp:<overlay-ip>:4444` pair for a
//!   cross-host run driven from a host that can reach both sockets).

use mde_kvm::{plan_migration, run_migration, MigrateRequest, MigrationUrl, Nic, Vm, VmSpec};

/// Read a gate variable; `None` (with a note) when unset so the test skips
/// gracefully instead of failing the `--ignored` sweep.
fn gate(var: &str) -> Option<String> {
    let value = std::env::var(var).ok();
    if value.is_none() {
        eprintln!("{var} unset; skipping the live gate");
    }
    value
}

/// Parse a `tcp:host:port` / `unix:path` gate value into a [`MigrationUrl`].
fn parse_url(raw: &str) -> Option<MigrationUrl> {
    if let Some(path) = raw.strip_prefix("unix:") {
        return Some(MigrationUrl::unix(path));
    }
    let rest = raw.strip_prefix("tcp:")?;
    let (host, port) = rest.rsplit_once(':')?;
    Some(MigrationUrl::tcp(host, port.parse().ok()?))
}

/// The end-to-end live boot: create → boot → Running → shutdown → delete
/// against a real VMM. Gated on `MDE_KVM_TEST_SOCKET` + `MDE_KVM_TEST_DISK`.
#[test]
#[ignore = "needs a live cloud-hypervisor VMM + a bootable image; set MDE_KVM_TEST_SOCKET/_DISK"]
fn live_boot_runs_the_full_lifecycle() {
    let (Some(socket), Some(disk)) = (gate("MDE_KVM_TEST_SOCKET"), gate("MDE_KVM_TEST_DISK"))
    else {
        return;
    };
    let vm = Vm::connect(socket);
    let spec = VmSpec::new("live-boot", 2, 2048, disk).with_nic(Nic::mesh("mvm-live-mesh"));
    vm.create(&spec).expect("vm.create against the live VMM");
    vm.boot().expect("vm.boot");
    let info = vm.info().expect("vm.info");
    assert!(info.is_running(), "guest not Running: {}", info.state);
    vm.shutdown().expect("vm.shutdown");
    vm.delete().expect("vm.delete");
}

/// The live migration: a running guest moves from the source VMM to the
/// target VMM and reports `Running` there. Gated on the
/// `MDE_KVM_TEST_MIGRATE_*` variables.
#[test]
#[ignore = "needs two live VMMs + a running guest; set MDE_KVM_TEST_MIGRATE_SRC_SOCKET/_DST_SOCKET/_URL"]
fn live_migration_moves_a_running_guest() {
    let (Some(src), Some(dst), Some(url)) = (
        gate("MDE_KVM_TEST_MIGRATE_SRC_SOCKET"),
        gate("MDE_KVM_TEST_MIGRATE_DST_SOCKET"),
        gate("MDE_KVM_TEST_MIGRATE_URL"),
    ) else {
        return;
    };
    let stream =
        parse_url(&url).expect("MDE_KVM_TEST_MIGRATE_URL must be tcp:host:port or unix:path");
    let source = Vm::connect(src);
    let target = Vm::connect(dst);
    let plan = plan_migration(MigrateRequest::new("live-migrate", stream.clone(), stream));
    run_migration(&source, &target, &plan).expect("live migration");
    // run_migration already verified Running on the target (VerifyRunning);
    // double-check the handle the operator will keep using.
    assert!(target.info().expect("target vm.info").is_running());
}
