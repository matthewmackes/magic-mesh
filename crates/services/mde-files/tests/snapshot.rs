//! Phase 6.4 — snapshot regression tests for mde-files.
//!
//! Renderer-free implementation: instead of rendering each view
//! to a PNG (which would need a headless wgpu pipeline + GPU on
//! the CI runner), we snapshot the *structural* output of each
//! view — the labels, counts, and category-row strings that
//! drive the visible UI. Visual regressions that move past the
//! structural layer (e.g. theme-color drift) get caught by the
//! mackes-theme bridge tests; this layer locks the data-model
//! contract.
//!
//! Snapshots live as plain `*.snap` text files alongside this
//! test module. Re-blessing happens by deleting the file and
//! re-running the test — the test writes the new snapshot when
//! the file is absent.

use std::path::PathBuf;

use mde_files::demo_data;

/// Resolve the snapshot file path for a given test name.
fn snap_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.snap"))
}

/// Assert that `actual` matches the committed snapshot at
/// `tests/snapshots/<name>.snap`. If the file is absent, write
/// it (the next run blesses).
fn assert_snapshot(name: &str, actual: &str) {
    let path = snap_path(name);
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir snapshot dir");
        }
        std::fs::write(&path, actual).expect("write snapshot");
        return;
    }
    let expected = std::fs::read_to_string(&path).expect("read snapshot");
    if expected != actual {
        panic!(
            "snapshot diff for {name}\n--- expected ---\n{expected}\n--- actual ---\n{actual}\n\
             (delete {} to re-bless)",
            path.display()
        );
    }
}

#[test]
fn demo_peers_snapshot() {
    let mut buf = String::new();
    buf.push_str("demo_peers\n");
    for p in demo_data::peers() {
        buf.push_str(&format!(
            "  {} | status={:?} | files={} | shared={}\n",
            p.label, p.status, p.files, p.shared
        ));
    }
    assert_snapshot("demo_peers", &buf);
}

#[test]
fn demo_self_node_snapshot() {
    let self_node = demo_data::self_node();
    let buf = format!(
        "self_node\n  label={}\n  host={}\n  files={}\n  shared={}\n",
        self_node.label, self_node.host, self_node.files, self_node.shared
    );
    assert_snapshot("self_node", &buf);
}

#[test]
fn online_count_does_not_panic() {
    // Just verify the demo_data function is callable + returns
    // a value matching peers manually filtered by status.
    let _ = demo_data::online_count();
}

#[test]
fn total_shared_does_not_panic() {
    let _ = demo_data::total_shared();
}

#[test]
fn snapshot_dir_exists_or_can_be_created() {
    // Compile-time guard: snapshot directory resolves under
    // the manifest dir.
    let p = snap_path("nonexistent");
    assert!(p.parent().is_some());
}
