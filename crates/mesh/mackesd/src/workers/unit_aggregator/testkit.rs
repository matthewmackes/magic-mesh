//! EXPLORER-1 — in-memory fakes for the three source seams (the cloud worker's
//! `testkit` shape): one fake per seam, shared by the worker unit tests so
//! the fold + publish + verb-drain pipeline runs with no etcd / Bus / network.

use std::sync::Mutex;

use super::sources::{
    CloudMirrorSource, CloudObjectRecord, LanHostRecord, LanScanSource, MeshMirrorSource,
    MeshSnapshot,
};

/// A [`MeshMirrorSource`] answering a fixed (mutable) snapshot.
pub struct FakeMeshMirror {
    snap: Mutex<MeshSnapshot>,
}

impl FakeMeshMirror {
    /// Answer `snap` on every read.
    pub fn new(snap: MeshSnapshot) -> Self {
        Self {
            snap: Mutex::new(snap),
        }
    }
}

impl MeshMirrorSource for FakeMeshMirror {
    fn read(&self) -> MeshSnapshot {
        self.snap.lock().unwrap().clone()
    }
}

/// A [`CloudMirrorSource`] answering a fixed union of cloud objects.
pub struct FakeCloud {
    recs: Mutex<Vec<CloudObjectRecord>>,
}

impl FakeCloud {
    /// Answer `recs` on every read.
    pub fn new(recs: Vec<CloudObjectRecord>) -> Self {
        Self {
            recs: Mutex::new(recs),
        }
    }
}

impl CloudMirrorSource for FakeCloud {
    fn read(&self) -> Vec<CloudObjectRecord> {
        self.recs.lock().unwrap().clone()
    }
}

/// A [`LanScanSource`] that records the last scan-active flag it saw and answers
/// its hosts ONLY when active — so a test can prove the surface gate (lock #24)
/// reaches the scan seam.
pub struct FakeLanScan {
    hosts: Mutex<Vec<LanHostRecord>>,
    last_active: Mutex<Option<bool>>,
}

impl FakeLanScan {
    /// A fake that would return `hosts` while active.
    pub fn new(hosts: Vec<LanHostRecord>) -> Self {
        Self {
            hosts: Mutex::new(hosts),
            last_active: Mutex::new(None),
        }
    }

    /// The scan-active flag the last `scan` saw (`None` before any scan).
    pub fn last_active(&self) -> Option<bool> {
        *self.last_active.lock().unwrap()
    }
}

impl LanScanSource for FakeLanScan {
    fn scan(&self, scan_active: bool) -> Vec<LanHostRecord> {
        *self.last_active.lock().unwrap() = Some(scan_active);
        if scan_active {
            self.hosts.lock().unwrap().clone()
        } else {
            Vec::new()
        }
    }
}
