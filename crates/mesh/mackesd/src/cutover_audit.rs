//! QC-15 cutover audit: prove the retired VM stack is gone and VM rebuild
//! evidence exists.
//!
//! This is intentionally a read-only audit surface. It does not mutate a node or
//! attempt a migration. A clean report means the repository-side old-stack
//! artifacts are absent and the operator supplied a ledger proving either no
//! pre-cutover VMs existed or every pre-cutover VM was rebuilt as a fresh Nova
//! instance/Heat stack.

use std::fs;
use std::path::{Path, PathBuf};

/// Default operator ledger path for Q58 fresh-VM rebuild evidence.
pub const DEFAULT_VM_REBUILD_LEDGER: &str = "/var/lib/mackesd/quasar-vm-rebuild-ledger.json";

/// Overall audit status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CutoverAuditStatus {
    /// The check passed.
    Pass,
    /// The check failed and blocks a clean cutover claim.
    Fail,
}

/// One audit row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CutoverAuditCheck {
    /// Stable machine-readable check id.
    pub id: &'static str,
    /// Pass/fail.
    pub status: CutoverAuditStatus,
    /// Human-readable detail.
    pub detail: String,
}

impl CutoverAuditCheck {
    fn pass(id: &'static str, detail: impl Into<String>) -> Self {
        Self {
            id,
            status: CutoverAuditStatus::Pass,
            detail: detail.into(),
        }
    }

    fn fail(id: &'static str, detail: impl Into<String>) -> Self {
        Self {
            id,
            status: CutoverAuditStatus::Fail,
            detail: detail.into(),
        }
    }
}

/// The full QC-15 audit report.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CutoverAuditReport {
    /// Repository root that was audited.
    pub repo_root: PathBuf,
    /// VM rebuild ledger path that was audited.
    pub vm_rebuild_ledger: PathBuf,
    /// All checks.
    pub checks: Vec<CutoverAuditCheck>,
}

impl CutoverAuditReport {
    /// `true` when every check passes.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.checks
            .iter()
            .all(|c| c.status == CutoverAuditStatus::Pass)
    }

    /// Number of failed checks.
    #[must_use]
    pub fn failures(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.status == CutoverAuditStatus::Fail)
            .count()
    }
}

/// One legacy VM's Q58 rebuild evidence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VmRebuildRecord {
    /// Pre-cutover VM name or id.
    pub legacy_name: String,
    /// Fresh Nova server id.
    pub nova_server_id: String,
    /// Managed Heat stack that created/rebuilt the server.
    pub heat_stack: String,
    /// Glance image id/name or Cinder volume id/name the fresh server came from.
    pub image_or_volume: String,
    /// Must be false. QC-15/Q58 requires fresh rebuild, not importing the old VM.
    #[serde(default)]
    pub imported_legacy_disk: bool,
}

/// Operator-authored Q58 rebuild ledger.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VmRebuildLedger {
    /// Node or fleet scope this ledger describes.
    pub node: String,
    /// Either `none` when there were no pre-cutover VMs, or `rebuilt`.
    pub legacy_inventory: String,
    /// Evidence rows for rebuilt VMs.
    #[serde(default)]
    pub vms: Vec<VmRebuildRecord>,
}

/// Run the QC-15 audit.
#[must_use]
pub fn audit_cutover(repo_root: &Path, vm_rebuild_ledger: &Path) -> CutoverAuditReport {
    let mut checks = Vec::new();
    checks.extend(audit_retired_artifacts(repo_root));
    checks.push(audit_node_virt_recipe(repo_root));
    checks.push(audit_forbidden_symbols(repo_root));
    checks.push(audit_vm_rebuild_ledger(vm_rebuild_ledger));
    CutoverAuditReport {
        repo_root: repo_root.to_path_buf(),
        vm_rebuild_ledger: vm_rebuild_ledger.to_path_buf(),
        checks,
    }
}

fn audit_retired_artifacts(repo_root: &Path) -> Vec<CutoverAuditCheck> {
    let retired = [
        "crates/services/mde-kvm/Cargo.toml",
        "crates/desktop/mde-shell-egui/src/instances.rs",
        "install-helpers/build-mde-vm-golden.sh",
    ];
    retired
        .into_iter()
        .map(|rel| {
            let path = repo_root.join(rel);
            if path.exists() {
                CutoverAuditCheck::fail("retired-artifact-absent", format!("{rel} still exists"))
            } else {
                CutoverAuditCheck::pass("retired-artifact-absent", format!("{rel} absent"))
            }
        })
        .collect()
}

fn audit_node_virt_recipe(repo_root: &Path) -> CutoverAuditCheck {
    let rel = "infra/ansible/node-virt.yml";
    let path = repo_root.join(rel);
    let Ok(body) = fs::read_to_string(&path) else {
        return CutoverAuditCheck::fail(
            "node-virt-old-packages-absent",
            format!("{rel} could not be read"),
        );
    };

    let forbidden = [
        "- cloud-hypervisor",
        "- cockpit",
        "- cockpit-machines",
        "- cockpit-podman",
        "cockpit.socket",
    ];
    let hits: Vec<&str> = forbidden
        .into_iter()
        .filter(|needle| body.contains(needle))
        .collect();
    if hits.is_empty() {
        CutoverAuditCheck::pass(
            "node-virt-old-packages-absent",
            "node-virt recipe does not install old VM console/hypervisor packages",
        )
    } else {
        CutoverAuditCheck::fail(
            "node-virt-old-packages-absent",
            format!("{rel} still contains {}", hits.join(", ")),
        )
    }
}

fn audit_forbidden_symbols(repo_root: &Path) -> CutoverAuditCheck {
    let roots = [
        "Cargo.toml",
        "Cargo.lock",
        "crates/mesh/mackesd/src",
        "crates/desktop/mde-shell-egui/src",
        "crates/desktop/mde-seat/src",
        "crates/platform/mde-role/src",
        "crates/platform/mde-role-chooser/src",
        "crates/services/mde-chat/Cargo.toml",
        "crates/desktop/mde-media-core/Cargo.toml",
        "packaging/bootc/Containerfile",
    ];
    let forbidden = [
        "mde_kvm",
        "mde-kvm =",
        "name = \"mde-kvm\"",
        "api_socket_path",
        "RUNTIME_DIR",
        "hotplug_disk_id",
        "ImageAttach",
        "ImageDetach",
        "build_ch_config",
        "cloud_hypervisor",
        "vhost-user-gpu",
    ];

    let mut hits = Vec::new();
    for rel in roots {
        let path = repo_root.join(rel);
        if path.is_file() {
            scan_file(repo_root, &path, &forbidden, &mut hits);
        } else if path.is_dir() {
            scan_dir(repo_root, &path, &forbidden, &mut hits);
        }
    }

    if hits.is_empty() {
        CutoverAuditCheck::pass(
            "old-stack-symbols-absent",
            "no live old-stack symbols found in audited source/manifests",
        )
    } else {
        CutoverAuditCheck::fail("old-stack-symbols-absent", hits.join("; "))
    }
}

fn scan_dir(repo_root: &Path, dir: &Path, forbidden: &[&str], hits: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name == ".git" {
            continue;
        }
        if path.is_dir() {
            scan_dir(repo_root, &path, forbidden, hits);
        } else if path.is_file() {
            scan_file(repo_root, &path, forbidden, hits);
        }
    }
}

fn scan_file(repo_root: &Path, path: &Path, forbidden: &[&str], hits: &mut Vec<String>) {
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => ext,
        None => {
            if path.file_name().and_then(|n| n.to_str()) != Some("Cargo.toml")
                && path.file_name().and_then(|n| n.to_str()) != Some("Cargo.lock")
                && path.file_name().and_then(|n| n.to_str()) != Some("Containerfile")
            {
                return;
            }
            ""
        }
    };
    if !matches!(ext, "rs" | "toml" | "lock" | "") {
        return;
    }
    let Ok(body) = fs::read_to_string(path) else {
        return;
    };
    for (idx, line) in body.lines().enumerate() {
        for needle in forbidden {
            if line.contains(needle) {
                let rel = path.strip_prefix(repo_root).unwrap_or(path);
                hits.push(format!("{}:{}:{needle}", rel.display(), idx + 1));
            }
        }
    }
}

fn audit_vm_rebuild_ledger(path: &Path) -> CutoverAuditCheck {
    let Ok(body) = fs::read_to_string(path) else {
        return CutoverAuditCheck::fail(
            "vm-rebuild-ledger",
            format!(
                "{} missing; Q58 needs proof that legacy VMs were rebuilt fresh or none existed",
                path.display()
            ),
        );
    };
    match serde_json::from_str::<VmRebuildLedger>(&body) {
        Ok(ledger) => validate_vm_rebuild_ledger(&ledger),
        Err(e) => CutoverAuditCheck::fail("vm-rebuild-ledger", format!("invalid JSON: {e}")),
    }
}

fn validate_vm_rebuild_ledger(ledger: &VmRebuildLedger) -> CutoverAuditCheck {
    if ledger.node.trim().is_empty() {
        return CutoverAuditCheck::fail("vm-rebuild-ledger", "ledger node is empty");
    }
    match ledger.legacy_inventory.as_str() {
        "none" if ledger.vms.is_empty() => CutoverAuditCheck::pass(
            "vm-rebuild-ledger",
            format!("{} recorded no pre-cutover VMs", ledger.node),
        ),
        "rebuilt" if !ledger.vms.is_empty() => {
            let mut bad = Vec::new();
            for vm in &ledger.vms {
                if vm.legacy_name.trim().is_empty()
                    || vm.nova_server_id.trim().is_empty()
                    || vm.heat_stack.trim().is_empty()
                    || vm.image_or_volume.trim().is_empty()
                {
                    bad.push(format!("{} has empty evidence fields", vm.legacy_name));
                }
                if vm.imported_legacy_disk {
                    bad.push(format!("{} imported a legacy disk", vm.legacy_name));
                }
            }
            if bad.is_empty() {
                CutoverAuditCheck::pass(
                    "vm-rebuild-ledger",
                    format!("{} rebuilt {} VM(s) fresh", ledger.node, ledger.vms.len()),
                )
            } else {
                CutoverAuditCheck::fail("vm-rebuild-ledger", bad.join("; "))
            }
        }
        "none" => CutoverAuditCheck::fail(
            "vm-rebuild-ledger",
            "`legacy_inventory=none` must not list VM rebuild records",
        ),
        "rebuilt" => CutoverAuditCheck::fail(
            "vm-rebuild-ledger",
            "`legacy_inventory=rebuilt` requires at least one VM record",
        ),
        other => CutoverAuditCheck::fail(
            "vm-rebuild-ledger",
            format!("unsupported legacy_inventory `{other}`"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    fn clean_repo(root: &Path) {
        write(
            root.join("Cargo.toml").as_path(),
            "[workspace]\nmembers=[]\n",
        );
        write(root.join("Cargo.lock").as_path(), "# lock\n");
        write(
            root.join("infra/ansible/node-virt.yml").as_path(),
            "name:\n  - qemu-kvm\n  - libvirt\n",
        );
        write(
            root.join("crates/mesh/mackesd/src/lib.rs").as_path(),
            "pub mod openstack;\n",
        );
        write(
            root.join("crates/desktop/mde-shell-egui/src/main.rs")
                .as_path(),
            "fn main() {}\n",
        );
        write(
            root.join("crates/desktop/mde-seat/src/lib.rs").as_path(),
            "pub struct Seat;\n",
        );
        write(
            root.join("crates/platform/mde-role/src/lib.rs").as_path(),
            "pub enum Role { Workstation }\n",
        );
        write(
            root.join("crates/platform/mde-role-chooser/src/main.rs")
                .as_path(),
            "fn main() {}\n",
        );
        write(
            root.join("crates/services/mde-chat/Cargo.toml").as_path(),
            "[package]\nname=\"mde-chat\"\n",
        );
        write(
            root.join("crates/desktop/mde-media-core/Cargo.toml")
                .as_path(),
            "[package]\nname=\"mde-media-core\"\n",
        );
        write(
            root.join("packaging/bootc/Containerfile").as_path(),
            "FROM scratch\n",
        );
    }

    #[test]
    fn clean_cutover_audit_passes_with_no_legacy_vm_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        clean_repo(tmp.path());
        let ledger = tmp.path().join("ledger.json");
        write(
            &ledger,
            r#"{"node":"node-a","legacy_inventory":"none","vms":[]}"#,
        );

        let report = audit_cutover(tmp.path(), &ledger);
        assert!(report.ok(), "{report:?}");
    }

    #[test]
    fn audit_fails_when_retired_artifacts_or_symbols_remain() {
        let tmp = tempfile::tempdir().unwrap();
        clean_repo(tmp.path());
        write(
            tmp.path()
                .join("crates/services/mde-kvm/Cargo.toml")
                .as_path(),
            "[package]\nname=\"mde-kvm\"\n",
        );
        write(
            tmp.path().join("crates/mesh/mackesd/src/old.rs").as_path(),
            "fn stale() { let _ = \"api_socket_path\"; }\n",
        );
        let ledger = tmp.path().join("ledger.json");
        write(
            &ledger,
            r#"{"node":"node-a","legacy_inventory":"none","vms":[]}"#,
        );

        let report = audit_cutover(tmp.path(), &ledger);
        assert!(!report.ok());
        assert!(report
            .checks
            .iter()
            .any(|c| c.detail.contains("crates/services/mde-kvm/Cargo.toml")));
        assert!(report
            .checks
            .iter()
            .any(|c| c.detail.contains("api_socket_path")));
    }

    #[test]
    fn vm_rebuild_ledger_rejects_imports_and_empty_fields() {
        let bad = VmRebuildLedger {
            node: "node-a".to_string(),
            legacy_inventory: "rebuilt".to_string(),
            vms: vec![VmRebuildRecord {
                legacy_name: "win10".to_string(),
                nova_server_id: String::new(),
                heat_stack: "vdi-win10".to_string(),
                image_or_volume: "glance:win10".to_string(),
                imported_legacy_disk: true,
            }],
        };
        let check = validate_vm_rebuild_ledger(&bad);
        assert_eq!(check.status, CutoverAuditStatus::Fail);
        assert!(check.detail.contains("empty evidence"));
        assert!(check.detail.contains("imported"));
    }

    #[test]
    fn vm_rebuild_ledger_accepts_fresh_rebuild_records() {
        let good = VmRebuildLedger {
            node: "node-a".to_string(),
            legacy_inventory: "rebuilt".to_string(),
            vms: vec![VmRebuildRecord {
                legacy_name: "win10".to_string(),
                nova_server_id: "server-1".to_string(),
                heat_stack: "vdi-win10".to_string(),
                image_or_volume: "glance:win10".to_string(),
                imported_legacy_disk: false,
            }],
        };
        let check = validate_vm_rebuild_ledger(&good);
        assert_eq!(check.status, CutoverAuditStatus::Pass);
    }
}
