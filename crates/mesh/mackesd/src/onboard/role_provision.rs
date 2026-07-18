//! OW-2 — `mackesd onboard role-provision`: apply a deployment role's systemd
//! unit set.
//!
//! A node's role decides which top-level systemd units it should run. This verb
//! makes the on-disk enable/mask state match the role: **enable** every unit the
//! role runs and **mask** every unit it does not (so a lighthouse can never
//! accidentally start the Workstation-only voice/desktop units, even via a
//! dependency pull-in).
//!
//! The role→units set is derived from the same rank model
//! [`crate::worker_role`] tiers the in-process workers by, reusing
//! [`mde_role::Role::rank`]: a unit sits at the *minimum role rank* that runs it
//! (0 = every node's control/data plane; 1 = Workstation-only). The pure mapping
//! ([`plan`]) is what the unit tests pin; [`apply`] folds that plan through an
//! injectable [`UnitManager`] so the fold is testable without a live systemd.

use mde_role::Role;

/// What [`apply`] does to a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitAction {
    /// The role runs this unit — ensure it is unmasked + boot-enabled.
    Enable,
    /// The role does not run this unit — mask it so nothing can start it.
    Mask,
}

/// One unit in the role plan: the unit, its rank floor, and the action for the
/// target role.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PlannedUnit {
    /// The systemd unit name (e.g. `nebula.service`).
    pub unit: &'static str,
    /// The minimum role rank that runs it (0 lighthouse · 1 workstation).
    pub min_rank: u8,
    /// Enable (role runs it) or Mask (role does not).
    pub action: UnitAction,
}

/// The role-gated **systemd unit** catalog — the top-level units the RPM ships,
/// tiered by the minimum deployment rank that runs each, mirroring
/// [`crate::worker_role`]'s worker census for the in-process workers.
///
/// * **Rank 0 (every node)** — the control/data plane: the Nebula overlay, the
///   `mackesd` daemon, the etcd + Syncthing substrate, and the health + status
///   timers. This is [`crate::site_yml::CONVERGE_SERVICES`] plus the status
///   timer (a unit test pins that superset relationship).
/// * **Rank 1 (Workstation only)** — the desktop adds: the DRM-seat shell and
///   the voice stack (kamailio/rtpengine, gated to the rank-1 `voice_config`
///   worker's tier). Optional Browser setup units are owned by the
///   co-installable `magic-mesh-browser` RPM so base-only installs do not try to
///   enable units whose files are intentionally absent.
const ROLE_UNITS: &[(&str, u8)] = &[
    // ── Rank 0 — universal control/data plane (CONVERGE_SERVICES + status timer).
    ("nebula.service", 0),
    ("mackesd.service", 0),
    ("etcd.service", 0),
    ("syncthing.service", 0),
    ("mesh-health.timer", 0),
    ("mesh-status.timer", 0),
    // ── Rank 1 — Workstation-only: the DRM-seat shell + voice stack.
    ("mde-shell-egui.service", 1),
    ("kamailio-mde.service", 1),
    ("rtpengine-mde.service", 1),
];

/// The pure role→unit-actions mapping.
///
/// A unit is **enabled** when the role's rank meets its floor, else **masked**.
/// Deterministic + side-effect-free — this is the tested core; [`apply`] is the
/// shell that runs it.
#[must_use]
pub fn plan(role: Role) -> Vec<PlannedUnit> {
    ROLE_UNITS
        .iter()
        .map(|&(unit, min_rank)| PlannedUnit {
            unit,
            min_rank,
            action: if role.rank() >= min_rank {
                UnitAction::Enable
            } else {
                UnitAction::Mask
            },
        })
        .collect()
}

/// Injectable seam over the two systemd operations, so [`apply`] is testable
/// without a live systemd. Production wires [`SystemctlUnits`]; tests pass a fake.
///
/// Both operations are idempotent: `enable` on an already-enabled (and unmasked)
/// unit is a no-op, `mask` on an already-masked unit is a no-op — so re-running
/// `role-provision` for the same role changes nothing.
pub trait UnitManager {
    /// Ensure `unit` is unmasked and boot-enabled.
    ///
    /// # Errors
    /// A human-readable message when the operation fails.
    fn enable(&self, unit: &str) -> Result<(), String>;

    /// Ensure `unit` is masked (cannot be started).
    ///
    /// # Errors
    /// A human-readable message when the operation fails.
    fn mask(&self, unit: &str) -> Result<(), String>;
}

/// Production [`UnitManager`]: drives `systemctl`.
///
/// `enable` first unmasks (best-effort — so a lighthouse→workstation upgrade can
/// enable a unit the earlier lighthouse pass masked) then boot-enables; `mask`
/// masks. No `--now`: this sets boot-durable state, it does not start/stop
/// services mid-provision.
pub struct SystemctlUnits;

impl UnitManager for SystemctlUnits {
    fn enable(&self, unit: &str) -> Result<(), String> {
        // Best-effort unmask: a first-ever enable has nothing to unmask, and we
        // don't want that to look like a failure — so the result is ignored and
        // only the enable is load-bearing.
        let _ = systemctl(&["unmask", unit]);
        systemctl(&["enable", unit])
    }

    fn mask(&self, unit: &str) -> Result<(), String> {
        systemctl(&["mask", unit])
    }
}

/// Run `systemctl <args…>`; `Ok` on exit 0, else an error naming the command. A
/// missing `systemctl` (a dev box) surfaces as an error the caller records.
fn systemctl(args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new("systemctl")
        .args(args)
        .status()
        .map_err(|e| format!("spawn `systemctl {}`: {e}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`systemctl {}` exited {status}", args.join(" ")))
    }
}

/// The result of applying one [`PlannedUnit`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UnitOutcome {
    /// The unit acted on.
    pub unit: &'static str,
    /// The action taken.
    pub action: UnitAction,
    /// Whether the action succeeded.
    pub ok: bool,
    /// The failure message when `!ok`.
    pub error: Option<String>,
}

/// Apply a `plan` through `mgr`, recording each unit's outcome.
///
/// Best-effort: a failed unit is recorded and the rest still run (a partial
/// systemd state should not abort the whole provision). Idempotent when the
/// manager's ops are (the production [`SystemctlUnits`] is).
#[must_use]
pub fn apply(plan: &[PlannedUnit], mgr: &dyn UnitManager) -> Vec<UnitOutcome> {
    plan.iter()
        .map(|p| {
            let res = match p.action {
                UnitAction::Enable => mgr.enable(p.unit),
                UnitAction::Mask => mgr.mask(p.unit),
            };
            UnitOutcome {
                unit: p.unit,
                action: p.action,
                ok: res.is_ok(),
                error: res.err(),
            }
        })
        .collect()
}

/// Convenience: [`plan`] then [`apply`] against the live systemd, for the CLI
/// dispatcher + a front-end that wants the one-call provision.
#[must_use]
pub fn provision(role: Role) -> Vec<UnitOutcome> {
    apply(&plan(role), &SystemctlUnits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn action_for<'a>(plan: &'a [PlannedUnit], unit: &str) -> &'a PlannedUnit {
        plan.iter().find(|p| p.unit == unit).expect("unit in plan")
    }

    fn rpm_manifest() -> toml::Value {
        toml::from_str(include_str!("../../Cargo.toml")).expect("mackesd Cargo.toml parses")
    }

    fn asset_exists(assets: &[toml::Value], source: &str, dest: &str, mode: &str) -> bool {
        assets.iter().any(|asset| {
            asset["source"].as_str() == Some(source)
                && asset["dest"].as_str() == Some(dest)
                && asset["mode"].as_str() == Some(mode)
        })
    }

    fn dest_absent(assets: &[toml::Value], dest: &str) -> bool {
        assets
            .iter()
            .all(|asset| asset["dest"].as_str() != Some(dest))
    }

    fn assert_exit_78_gate_is_retryable(unit: &str, label: &str) {
        assert!(
            unit.contains("SuccessExitStatus=78"),
            "{label} must treat an unconfigured manifest as a clean gate"
        );
        assert!(
            !unit.contains("RemainAfterExit=yes"),
            "{label} must stay retryable after exit 78 so an operator-filled manifest can rerun with systemctl start"
        );
    }

    #[test]
    fn lighthouse_enables_control_plane_and_masks_workstation_units() {
        let p = plan(Role::Lighthouse);
        // Rank-0 control plane → enabled.
        for u in [
            "nebula.service",
            "mackesd.service",
            "etcd.service",
            "syncthing.service",
            "mesh-health.timer",
            "mesh-status.timer",
        ] {
            assert_eq!(
                action_for(&p, u).action,
                UnitAction::Enable,
                "lighthouse must enable {u}"
            );
        }
        // Rank-1 Workstation units → masked (a lighthouse never runs them).
        for u in [
            "mde-shell-egui.service",
            "kamailio-mde.service",
            "rtpengine-mde.service",
        ] {
            assert_eq!(
                action_for(&p, u).action,
                UnitAction::Mask,
                "lighthouse must mask {u}"
            );
        }
    }

    #[test]
    fn workstation_enables_every_unit() {
        let p = plan(Role::Workstation);
        assert!(
            p.iter().all(|u| u.action == UnitAction::Enable),
            "workstation (top rank) runs the full unit set"
        );
        // Same catalog for both roles — only the actions differ.
        assert_eq!(p.len(), plan(Role::Lighthouse).len());
    }

    #[test]
    fn plan_is_deterministic() {
        assert_eq!(plan(Role::Lighthouse), plan(Role::Lighthouse));
        assert_eq!(plan(Role::Workstation), plan(Role::Workstation));
    }

    #[test]
    fn rank_zero_units_are_a_superset_of_converge_services() {
        // The role catalog's rank-0 tier must cover the canonical boot-durable
        // service set, so a provisioned node keeps CONVERGE_SERVICES enabled.
        let rank0: Vec<&str> = ROLE_UNITS
            .iter()
            .filter(|(_, r)| *r == 0)
            .map(|(u, _)| *u)
            .collect();
        for svc in crate::site_yml::CONVERGE_SERVICES {
            assert!(
                rank0.contains(&svc),
                "{svc} (CONVERGE_SERVICES) missing from the rank-0 role units"
            );
        }
    }

    #[test]
    fn base_rpm_ships_and_enables_the_drm_seat_unit() {
        let manifest = rpm_manifest();
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let post_install = rpm["post_install_script"]
            .as_str()
            .expect("base post install script");
        assert_eq!(
            rpm["recommends"]["magic-mesh-browser"].as_str(),
            Some("*"),
            "default Workstation installs should pull the split Browser package as a weak dependency"
        );
        let base_requires = rpm["requires"].as_table().expect("base requires table");
        assert!(
            !base_requires.contains_key("bzip2"),
            "the base RPM must not hard-require browser-only bzip2 after the browser split"
        );
        assert_eq!(
            rpm["variants"]["browser"]["requires"]["magic-mesh"].as_str(),
            Some("*"),
            "the Browser package must require the base shell package that launches it"
        );
        assert_eq!(
            rpm["variants"]["browser"]["requires"]["bzip2"].as_str(),
            Some("*"),
            "the Browser package must require bzip2 so the CEF .tar.bz2 runtime extracts on fresh Workstations"
        );
        assert_eq!(
            rpm["requires"]["hunspell"].as_str(),
            Some("*"),
            "the base RPM must require hunspell for offline editor spell checking"
        );
        assert_eq!(
            rpm["requires"]["hunspell-en-US"].as_str(),
            Some("*"),
            "the base RPM must require a default hunspell dictionary"
        );
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        assert!(
            base_assets.iter().any(|asset| {
                asset["dest"].as_str() == Some("/usr/lib/systemd/system/mde-shell-egui.service")
                    && asset["source"].as_str()
                        == Some("packaging/bootc/units/mde-shell-egui.service")
            }),
            "base RPM must ship the DRM-seat unit"
        );
        assert!(
            post_install.contains("systemctl enable mde-shell-egui.service"),
            "base RPM post-install must enable the self-gated seat unit"
        );
        assert!(
            post_install.contains("/etc/systemd/system/mde-shell.service")
                && post_install.contains("grep -q '/usr/bin/mde-shell-egui'")
                && post_install.contains("systemctl disable --now mde-shell.service"),
            "base RPM post-install must remove the known legacy local DRM-seat launcher so it cannot race mde-shell-egui.service"
        );

        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");
        assert!(
            server_assets.iter().all(|asset| {
                asset["dest"].as_str() != Some("/usr/lib/systemd/system/mde-shell-egui.service")
            }),
            "headless server RPM must not ship a seat unit without the shell binary"
        );
    }

    #[test]
    fn base_rpm_recommends_workstation_media_helpers() {
        let manifest: toml::Value =
            toml::from_str(include_str!("../../Cargo.toml")).expect("mackesd Cargo.toml parses");
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];

        assert_eq!(
            rpm["recommends"]["libcanberra-gtk3"].as_str(),
            Some("*"),
            "the full RPM should pull canberra-gtk-play for shell notification sounds"
        );
        assert_eq!(
            rpm["recommends"]["playerctl"].as_str(),
            Some("*"),
            "the full RPM should pull playerctl for phone-originated MPRIS media keys"
        );
        let base_recommends = rpm["recommends"].as_table().expect("base recommends table");
        for package in [
            "libvirt-daemon-driver-qemu",
            "libvirt-daemon-config-network",
        ] {
            assert!(
                !base_recommends.contains_key(package),
                "the base RPM must not weak-pull {package}; it can drag swtpm SELinux scriptlets into lighthouse installs"
            );
        }
        let server_recommends = rpm["variants"]["server"]["recommends"]
            .as_table()
            .expect("server recommends table");
        for package in [
            "libvirt-daemon-driver-qemu",
            "libvirt-daemon-config-network",
        ] {
            assert_eq!(
                server_recommends
                    .get(package)
                    .and_then(|value| value.as_str()),
                Some("*"),
                "the server variant should still weak-pull {package} for compute hosts"
            );
        }
        assert!(
            !server_recommends.contains_key("libcanberra-gtk3"),
            "the headless server variant should not pull the desktop notification sound player"
        );
        assert!(
            !server_recommends.contains_key("playerctl"),
            "the headless server variant should not pull the desktop media-key helper"
        );
    }

    #[test]
    fn bootc_image_lane_bakes_qemu_libvirt_ovn_and_excludes_cloud_hypervisor() {
        let containerfile = include_str!("../../../../../packaging/bootc/Containerfile");
        for needle in [
            "libvirt-client",
            "libvirt-daemon-driver-qemu",
            "libvirt-daemon-config-network",
            "qemu-kvm",
            "virt-install",
            "openvswitch",
            "ovn-host",
            "cloud-init",
            "qemu-guest-agent",
            "datasource_list: [ NoCloud, None ]",
            "cloud-init-local.service cloud-init.service cloud-config.service cloud-final.service",
            "openvswitch.service",
            "dnf -y install --allowerasing",
            "/usr/lib/bootc/install/50-magic-mesh.toml",
            "dnf -y remove ${base_kernels}",
        ] {
            assert!(
                containerfile.contains(needle),
                "bootc image must install QC-1 host virt package {needle}"
            );
        }
        for stale in [
            "ARG CH_VERSION",
            "ARG CH_SHA256",
            "cloud-hypervisor-static",
            "install -m 0755 /tmp/cloud-hypervisor",
            "dnf -y --allowerasing install",
        ] {
            assert!(
                !containerfile.contains(stale),
                "QC-1 bootc image must not keep the retired cloud-hypervisor bake: {stale}"
            );
        }

        let verifier = include_str!("../../../../../packaging/bootc/verify-image.sh");
        for needle in [
            "virsh",
            "virsh --version",
            "ovs-vsctl",
            "cloud-init",
            "qemu-ga",
            "rpm -q \"$p\"",
            "qemu-kvm libvirt-daemon-driver-qemu libvirt-daemon-config-network ovn-host openvswitch cloud-init qemu-guest-agent",
            "[ ! -e /usr/bin/cloud-hypervisor ]",
            "bootc install rootfs default = xfs",
            "cloud-init constrained to NoCloud/None",
            "openvswitch.service",
            "single kernel modules tree present",
            "surface kernel is the bootc kernel",
        ] {
            assert!(
                verifier.contains(needle),
                "bootc verifier must pin QC-1 payload check {needle}"
            );
        }

        let install_config =
            include_str!("../../../../../packaging/bootc/install/50-magic-mesh.toml");
        assert!(
            install_config.contains("[install.filesystem.root]")
                && install_config.contains("type = \"xfs\""),
            "bootc-image-builder needs a default root filesystem type"
        );
    }

    #[test]
    fn postinstall_bounds_optional_helper_runtime() {
        let manifest = rpm_manifest();
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let script = rpm["post_install_script"]
            .as_str()
            .expect("base post install script");
        let assets = rpm["assets"].as_array().expect("base assets array");
        let browser = &rpm["variants"]["browser"];
        let browser_script = browser["post_install_script"]
            .as_str()
            .expect("browser post install script");
        let browser_uninstall_script = browser["post_uninstall_script"]
            .as_str()
            .expect("browser post uninstall script");
        let browser_assets = browser["assets"].as_array().expect("browser assets array");

        for guarded in [
            "timeout 60 systemd-tmpfiles --create /usr/lib/tmpfiles.d/magic-mesh.conf",
            "timeout 60 gtk-update-icon-cache -q -f /usr/share/icons/hicolor",
            "timeout 60 gtk-update-icon-cache -q -f /usr/share/icons/YAMIS",
            "timeout 60 update-desktop-database -q",
        ] {
            assert!(
                script.contains(guarded),
                "postinstall helper must be timeout-bounded: {guarded}"
            );
        }
        for (source, dest) in [
            (
                "assets/icons/YAMIS/YAMIS/index.theme",
                "/usr/share/icons/YAMIS/index.theme",
            ),
            ("assets/icons/YAMIS/YAMIS/*/**/*", "/usr/share/icons/YAMIS/"),
            (
                "assets/icons/YAMIS/YAMIS/LICENSE",
                "/usr/share/licenses/magic-mesh/YAMIS-LICENSE",
            ),
        ] {
            assert!(
                asset_exists(assets, source, dest, "644"),
                "base RPM must ship the YAMIS platform icon payload {source} -> {dest}"
            );
        }
        assert!(
            script.contains("gtk-icon-theme-name=YAMIS")
                && script.contains("set_gtk_icon_theme /etc/gtk-3.0/settings.ini")
                && script.contains("set_gtk_icon_theme /etc/gtk-4.0/settings.ini"),
            "base RPM post-install must make YAMIS the default toolkit icon theme"
        );
        assert!(
            script.contains("systemctl enable magic-mesh-selinux-policy.service"),
            "base SELinux policy loader must be enabled without starting inside dnf %post"
        );
        assert!(
            !script.contains("mde-web-preview-selinux.service")
                && !script.contains("mde-web-cef-selinux.service"),
            "base RPM post-install must not enable Browser SELinux units owned by the split Browser package"
        );
        assert!(
            browser_script.contains("systemctl enable $BROWSER_UNITS")
                && browser_script.contains("systemctl start --no-block $BROWSER_UNITS")
                && browser_script.contains("mde-web-preview-selinux.service")
                && browser_script.contains("mde-web-cef-selinux.service"),
            "Browser RPM post-install must enable and non-blocking queue Browser setup units so live installs do not wait for a reboot"
        );
        assert!(
            browser_script.contains("BROWSER_RETRYABLE_UNITS=")
                && browser_script.contains("systemctl show \"$unit\" -p ExecMainStatus --value")
                && browser_script.contains("systemctl stop \"$unit\""),
            "Browser RPM post-install must clear legacy exit-78 active/exited state before retrying optional setup units"
        );
        assert!(
            !script.contains("systemctl enable --now --no-block magic-mesh-selinux-policy.service"),
            "SELinux policy loaders must not start from dnf %post"
        );
        assert!(
            !script.contains("systemctl start --no-block magic-mesh-selinux-policy.service"),
            "base SELinux policy loader must not start from dnf %post"
        );
        assert!(
            !script.contains("/usr/libexec/mackesd/setup-selinux-policy >/dev/null"),
            "setup-selinux-policy must not run synchronously from dnf %post"
        );
        assert!(
            !script.contains("/usr/libexec/mackesd/setup-selinux-web-preview >/dev/null"),
            "setup-selinux-web-preview must not run synchronously from dnf %post"
        );
        for unit in ["/usr/lib/systemd/system/magic-mesh-selinux-policy.service"] {
            assert!(
                assets
                    .iter()
                    .any(|asset| asset["dest"].as_str() == Some(unit)),
                "base RPM must ship the async SELinux loader unit {unit}"
            );
        }
        for unit in [
            "/usr/lib/systemd/system/mde-web-preview-selinux.service",
            "/usr/lib/systemd/system/mde-web-cef-selinux.service",
        ] {
            assert!(
                dest_absent(assets, unit),
                "base RPM must not ship Browser SELinux loader unit {unit}"
            );
            assert!(
                browser_assets
                    .iter()
                    .any(|asset| asset["dest"].as_str() == Some(unit)),
                "Browser RPM must ship Browser SELinux loader unit {unit}"
            );
        }

        let browser_requires = browser["requires"]
            .as_table()
            .expect("browser requires table");
        for package in ["selinux-policy-devel", "checkpolicy"] {
            assert_eq!(
                browser_requires[package].as_str(),
                Some("*"),
                "Browser RPM must hard-require {package} so Enforcing seats can compile and load browser SELinux domains"
            );
        }
        assert!(
            browser_uninstall_script.contains("semodule -r mde_web_preview mde_web_cef"),
            "Browser RPM uninstall must remove the underscore-named SELinux modules declared by policy_module(), not the hyphenated source filenames"
        );
        assert!(
            !browser_uninstall_script.contains("semodule -r mde-web-preview mde-web-cef"),
            "Browser RPM uninstall must not try to remove nonexistent hyphenated SELinux module names"
        );
    }

    #[test]
    fn browser_rpm_ships_browser_helpers_but_base_and_server_do_not() {
        let manifest = rpm_manifest();
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        let browser_assets = rpm["variants"]["browser"]["assets"]
            .as_array()
            .expect("browser assets array");
        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");

        for (source, dest) in [
            ("target/release/mde-web-preview", "/usr/bin/mde-web-preview"),
            ("target/release/mde-web-cef", "/usr/bin/mde-web-cef"),
            (
                "target/release/mde-web-cef-renderer",
                "/usr/libexec/mackesd/mde-web-cef-renderer",
            ),
            (
                "target/release/cef-verify",
                "/usr/libexec/mackesd/cef-verify",
            ),
        ] {
            assert!(
                asset_exists(browser_assets, source, dest, "755"),
                "Browser RPM must ship browser helper {dest}"
            );
            assert!(
                dest_absent(base_assets, dest),
                "base RPM must not ship browser helper {dest}"
            );
            assert!(
                dest_absent(server_assets, dest),
                "headless server RPM must not ship browser helper {dest}"
            );
        }
    }

    #[test]
    fn browser_rpm_ships_two_engine_operational_verifier_but_base_and_server_do_not() {
        let manifest = rpm_manifest();
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        let browser_assets = rpm["variants"]["browser"]["assets"]
            .as_array()
            .expect("browser assets array");
        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");
        let source = "install-helpers/browser-verify-engines.sh";
        let dest = "/usr/libexec/mackesd/browser-verify-engines";

        assert!(
            asset_exists(browser_assets, source, dest, "755"),
            "Browser RPM must ship the two-engine Browser operational verifier"
        );
        assert!(
            dest_absent(base_assets, dest),
            "base RPM must not ship the Browser operational verifier"
        );
        assert!(
            dest_absent(server_assets, dest),
            "headless server RPM must not ship the Browser operational verifier"
        );

        let verifier = include_str!("../../../../../install-helpers/browser-verify-engines.sh");
        for needle in [
            "/usr/libexec/mackesd/cef-verify",
            "/usr/bin/mde-web-cef",
            "/usr/bin/mde-web-preview",
            "MDE_BROWSER_VERIFY_INPUT=1",
            "VERIFY RESULT=PASS",
            "VERIFY on_paint_ready",
            "mde-browser-verify-p1-k1-tm|P:1 K:1 T:m",
            "process cleanup passed",
        ] {
            assert!(
                verifier.contains(needle),
                "Browser operational verifier must contain {needle}"
            );
        }
    }

    #[test]
    fn browser_rpm_ships_browser_read_aloud_tts_wrapper_but_base_and_server_do_not() {
        let manifest = rpm_manifest();
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        let browser_assets = rpm["variants"]["browser"]["assets"]
            .as_array()
            .expect("browser assets array");
        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");
        let source = "install-helpers/browser-read-aloud-tts.sh";
        let dest = "/usr/libexec/mackesd/browser-read-aloud-tts";

        assert!(
            asset_exists(browser_assets, source, dest, "755"),
            "Browser RPM must ship the Browser read-aloud TTS wrapper"
        );
        assert!(
            dest_absent(base_assets, dest),
            "base RPM must not ship the Browser read-aloud TTS wrapper"
        );
        assert!(
            dest_absent(server_assets, dest),
            "headless server RPM must not ship the Browser read-aloud TTS wrapper"
        );
    }

    #[test]
    fn browser_rpm_ships_browser_tts_voice_provisioning_but_base_and_server_do_not() {
        let manifest = rpm_manifest();
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        let browser_assets = rpm["variants"]["browser"]["assets"]
            .as_array()
            .expect("browser assets array");
        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");
        let post_install = rpm["variants"]["browser"]["post_install_script"]
            .as_str()
            .expect("browser post install script");

        for (source, dest, mode) in [
            (
                "install-helpers/install-browser-tts-voice.sh",
                "/usr/libexec/mackesd/install-browser-tts-voice",
                "755",
            ),
            (
                "packaging/browser/browser-read-aloud-voice.env",
                "/usr/share/magic-mesh/browser/browser-read-aloud-voice.env",
                "644",
            ),
            (
                "packaging/systemd/mde-browser-tts-voice-setup.service",
                "/usr/lib/systemd/system/mde-browser-tts-voice-setup.service",
                "644",
            ),
        ] {
            assert!(
                asset_exists(browser_assets, source, dest, mode),
                "Browser RPM must ship Browser TTS voice provisioning asset {dest}"
            );
            assert!(
                dest_absent(base_assets, dest),
                "base RPM must not ship Browser TTS voice provisioning asset {dest}"
            );
            assert!(
                dest_absent(server_assets, dest),
                "headless server RPM must not ship Browser TTS voice provisioning asset {dest}"
            );
        }
        assert!(
            post_install.contains("mde-browser-tts-voice-setup.service"),
            "Browser RPM post-install must enable the Browser TTS voice setup unit"
        );

        let unit =
            include_str!("../../../../../packaging/systemd/mde-browser-tts-voice-setup.service");
        assert_exit_78_gate_is_retryable(unit, "Browser TTS voice setup unit");
        assert!(
            unit.contains("ExecCondition=/usr/bin/mackesd role-gate --min-rank 1")
                && unit.contains("ExecStart=/usr/libexec/mackesd/install-browser-tts-voice"),
            "Browser TTS voice setup unit must be Workstation-gated"
        );
    }

    #[test]
    fn browser_rpm_ships_browser_stt_provisioning_but_base_and_server_do_not() {
        let manifest = rpm_manifest();
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        let browser_assets = rpm["variants"]["browser"]["assets"]
            .as_array()
            .expect("browser assets array");
        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");
        let post_install = rpm["variants"]["browser"]["post_install_script"]
            .as_str()
            .expect("browser post install script");

        for (source, dest, mode) in [
            (
                "install-helpers/browser-voice-command-stt.sh",
                "/usr/libexec/mackesd/browser-voice-command-stt",
                "755",
            ),
            (
                "install-helpers/install-browser-stt-model.sh",
                "/usr/libexec/mackesd/install-browser-stt-model",
                "755",
            ),
            (
                "packaging/browser/browser-voice-command-stt.env",
                "/usr/share/magic-mesh/browser/browser-voice-command-stt.env",
                "644",
            ),
            (
                "packaging/systemd/mde-browser-stt-model-setup.service",
                "/usr/lib/systemd/system/mde-browser-stt-model-setup.service",
                "644",
            ),
        ] {
            assert!(
                asset_exists(browser_assets, source, dest, mode),
                "Browser RPM must ship Browser STT asset {dest}"
            );
            assert!(
                dest_absent(base_assets, dest),
                "base RPM must not ship Browser STT asset {dest}"
            );
            assert!(
                dest_absent(server_assets, dest),
                "headless server RPM must not ship Browser STT asset {dest}"
            );
        }
        assert!(
            post_install.contains("mde-browser-stt-model-setup.service"),
            "Browser RPM post-install must enable the Browser STT model setup unit"
        );

        let unit =
            include_str!("../../../../../packaging/systemd/mde-browser-stt-model-setup.service");
        assert_exit_78_gate_is_retryable(unit, "Browser STT model setup unit");
        assert!(
            unit.contains("ExecCondition=/usr/bin/mackesd role-gate --min-rank 1")
                && unit.contains("ExecStart=/usr/libexec/mackesd/install-browser-stt-model"),
            "Browser STT model setup unit must be Workstation-gated"
        );
    }

    #[test]
    fn browser_rpm_ships_browser_translate_provisioning_but_base_and_server_do_not() {
        let manifest = rpm_manifest();
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        let browser_assets = rpm["variants"]["browser"]["assets"]
            .as_array()
            .expect("browser assets array");
        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");
        let post_install = rpm["variants"]["browser"]["post_install_script"]
            .as_str()
            .expect("browser post install script");

        for (source, dest, mode) in [
            (
                "install-helpers/browser-translate.sh",
                "/usr/libexec/mackesd/browser-translate",
                "755",
            ),
            (
                "install-helpers/install-browser-translate-model.sh",
                "/usr/libexec/mackesd/install-browser-translate-model",
                "755",
            ),
            (
                "packaging/browser/browser-translate.env",
                "/usr/share/magic-mesh/browser/browser-translate.env",
                "644",
            ),
            (
                "packaging/systemd/mde-browser-translate-model-setup.service",
                "/usr/lib/systemd/system/mde-browser-translate-model-setup.service",
                "644",
            ),
        ] {
            assert!(
                asset_exists(browser_assets, source, dest, mode),
                "Browser RPM must ship Browser translation asset {dest}"
            );
            assert!(
                dest_absent(base_assets, dest),
                "base RPM must not ship Browser translation asset {dest}"
            );
            assert!(
                dest_absent(server_assets, dest),
                "headless server RPM must not ship Browser translation asset {dest}"
            );
        }
        assert!(
            post_install.contains("mde-browser-translate-model-setup.service"),
            "Browser RPM post-install must enable the Browser translation model setup unit"
        );

        let unit = include_str!(
            "../../../../../packaging/systemd/mde-browser-translate-model-setup.service"
        );
        assert_exit_78_gate_is_retryable(unit, "Browser translation model setup unit");
        assert!(
            unit.contains("ExecCondition=/usr/bin/mackesd role-gate --min-rank 1")
                && unit.contains("ExecStart=/usr/libexec/mackesd/install-browser-translate-model"),
            "Browser translation model setup unit must be Workstation-gated"
        );
    }

    #[test]
    fn full_rpm_ships_seat_remote_input_helper_but_server_variant_does_not() {
        let manifest: toml::Value =
            toml::from_str(include_str!("../../Cargo.toml")).expect("mackesd Cargo.toml parses");
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");
        let source = "install-helpers/seat-remote-input.py";
        let dest = "/usr/libexec/mackesd/seat-remote-input";

        assert!(
            base_assets.iter().any(|asset| {
                asset["source"].as_str() == Some(source)
                    && asset["dest"].as_str() == Some(dest)
                    && asset["mode"].as_str() == Some("755")
            }),
            "full Workstation RPM must ship the KDC remote-input seat helper"
        );
        assert!(
            server_assets
                .iter()
                .all(|asset| asset["dest"].as_str() != Some(dest)),
            "headless server RPM must not ship the KDC remote-input seat helper"
        );
    }

    #[test]
    fn full_rpm_ships_remote_proofing_bridge_but_server_variant_does_not() {
        let manifest: toml::Value =
            toml::from_str(include_str!("../../Cargo.toml")).expect("mackesd Cargo.toml parses");
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");
        let post_install = rpm["post_install_script"]
            .as_str()
            .expect("base RPM post-install script");

        for (source, dest, mode) in [
            (
                "install-helpers/mde-remote-proofing-apply.py",
                "/usr/libexec/mackesd/mde-remote-proofing-apply",
                "755",
            ),
            (
                "packaging/systemd/mde-remote-proofing-plan.service",
                "/usr/lib/systemd/system/mde-remote-proofing-plan.service",
                "644",
            ),
            (
                "packaging/systemd/mde-remote-proofing-plan.path",
                "/usr/lib/systemd/system/mde-remote-proofing-plan.path",
                "644",
            ),
        ] {
            assert!(
                asset_exists(base_assets, source, dest, mode),
                "full Workstation RPM must ship Remote Proofing bridge asset {dest}"
            );
            assert!(
                dest_absent(server_assets, dest),
                "headless server RPM must not ship Remote Proofing bridge asset {dest}"
            );
        }
        assert!(
            post_install.contains("mde-remote-proofing-plan.path"),
            "base RPM post-install must enable the Remote Proofing plan watcher"
        );

        let unit =
            include_str!("../../../../../packaging/systemd/mde-remote-proofing-plan.service");
        assert_exit_78_gate_is_retryable(unit, "Remote Proofing plan service");
        assert!(
            unit.contains("ExecCondition=/usr/bin/mackesd role-gate --min-rank 1")
                && unit.contains("/usr/libexec/mackesd/mde-remote-proofing-apply")
                && unit.contains("--write-plan /run/mde/remote-proofing/plan.json")
                && unit.contains("--write-config /run/mde/remote-proofing/sunshine.conf")
                && unit.contains("--write-lifecycle /run/mde/remote-proofing/lifecycle.json")
                && unit.contains("--apply-lifecycle"),
            "Remote Proofing plan service must be Workstation-gated and render/apply plan/config/lifecycle artifacts"
        );

        let path = include_str!("../../../../../packaging/systemd/mde-remote-proofing-plan.path");
        assert!(
            path.contains("PathChanged=/run/mde-bus/settings-remote-proofing.json")
                && path.contains("PathChanged=/run/mde/mesh-status.json")
                && path.contains("Unit=mde-remote-proofing-plan.service")
                && !path
                    .lines()
                    .any(|line| line.trim_start().starts_with("PathExists=")),
            "Remote Proofing path unit must watch settings/status changes without a level-triggered PathExists loop"
        );
    }

    #[test]
    fn browser_rpm_ships_cef_runtime_provisioning_but_base_and_server_do_not() {
        let manifest = rpm_manifest();
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        let browser_assets = rpm["variants"]["browser"]["assets"]
            .as_array()
            .expect("browser assets array");
        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");
        let post_install = rpm["variants"]["browser"]["post_install_script"]
            .as_str()
            .expect("browser post install script");

        for (source, dest, mode) in [
            (
                "install-helpers/install-cef-runtime.sh",
                "/usr/libexec/mackesd/install-cef-runtime",
                "755",
            ),
            (
                "packaging/browser/cef-linux64-minimal.env",
                "/usr/share/magic-mesh/browser/cef-linux64-minimal.env",
                "644",
            ),
            (
                "packaging/systemd/mde-cef-runtime-setup.service",
                "/usr/lib/systemd/system/mde-cef-runtime-setup.service",
                "644",
            ),
        ] {
            assert!(
                asset_exists(browser_assets, source, dest, mode),
                "Browser RPM must ship CEF runtime provisioning asset {dest}"
            );
            assert!(
                dest_absent(base_assets, dest),
                "base RPM must not ship CEF runtime provisioning asset {dest}"
            );
            assert!(
                dest_absent(server_assets, dest),
                "headless server RPM must not ship CEF runtime provisioning asset {dest}"
            );
        }

        assert!(
            post_install.contains("mde-cef-runtime-setup.service"),
            "Browser RPM post-install must enable the deferred CEF runtime setup service"
        );

        let service =
            include_str!("../../../../../packaging/systemd/mde-cef-runtime-setup.service");
        for needle in [
            "ConditionPathExists=/usr/bin/mde-web-cef",
            "ConditionPathExists=!/opt/mde/cef/Release/libcef.so",
            "ExecCondition=/usr/bin/mackesd role-gate --min-rank 1",
            "ExecStart=/usr/libexec/mackesd/install-cef-runtime",
        ] {
            assert!(
                service.contains(needle),
                "CEF runtime setup unit must contain {needle}"
            );
        }

        let installer = include_str!("../../../../../install-helpers/install-cef-runtime.sh");
        assert!(
            installer.contains("/usr/share/magic-mesh/browser/cef-linux64-minimal.env")
                && installer.contains("/var/cache/magic-mesh/cef")
                && installer.contains("need_cmd bzip2")
                && installer.contains("render-once")
                && installer.contains("CEF_BROWSER_PAINT_READY"),
            "installed CEF runtime installer must use installed manifest/cache paths and gate activation on a render smoke"
        );
        assert!(
            installer.contains("/usr/libexec/mackesd/cef-verify")
                && installer.contains("VERIFY RESULT=PASS")
                && installer.contains("VERIFY on_paint_ready")
                && installer.contains("wire smoke passed"),
            "installed CEF runtime installer must gate the shell-equivalent tab wire path when cef-verify is shipped"
        );
    }

    #[test]
    fn browser_cef_selinux_policy_allows_proc_pressure_reads() {
        let policy = include_str!("../../../../../packaging/selinux/mde-web-cef.te");
        assert!(
            policy.contains("type proc_psi_t;")
                && policy.contains("allow mde_web_cef_t proc_psi_t:dir")
                && policy.contains("search")
                && policy.contains("allow mde_web_cef_t proc_psi_t:file")
                && policy.contains("read"),
            "CEF SELinux policy must allow read-only PSI /proc/pressure probes without AVC flood"
        );
    }

    #[test]
    fn browser_rpm_ships_cef_webextensions_smoke_assets_but_base_and_server_do_not() {
        let manifest = rpm_manifest();
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        let browser_assets = rpm["variants"]["browser"]["assets"]
            .as_array()
            .expect("browser assets array");
        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");

        for (source, dest, mode) in [
            (
                "install-helpers/browser-cef-webextension-smoke.sh",
                "/usr/libexec/mackesd/browser-cef-webextension-smoke",
                "755",
            ),
            (
                "packaging/browser/webextensions-allowlist.env",
                "/usr/share/magic-mesh/browser/webextensions-allowlist.env",
                "644",
            ),
            (
                "packaging/browser/webextensions-smoke.env",
                "/usr/share/magic-mesh/browser/webextensions-smoke.env",
                "644",
            ),
            (
                "packaging/browser/smoke-extension/manifest.json",
                "/usr/share/magic-mesh/browser/smoke-extension/manifest.json",
                "644",
            ),
            (
                "packaging/browser/smoke-extension/smoke.js",
                "/usr/share/magic-mesh/browser/smoke-extension/smoke.js",
                "644",
            ),
        ] {
            assert!(
                asset_exists(browser_assets, source, dest, mode),
                "Browser RPM must ship CEF WebExtensions asset {dest}"
            );
            assert!(
                dest_absent(base_assets, dest),
                "base RPM must not ship CEF WebExtensions asset {dest}"
            );
            assert!(
                dest_absent(server_assets, dest),
                "headless server RPM must not ship CEF WebExtensions asset {dest}"
            );
        }

        let runner =
            include_str!("../../../../../install-helpers/browser-cef-webextension-smoke.sh");
        for needle in [
            "/usr/share/magic-mesh/browser/webextensions-smoke.env",
            "MDE_CEF_BROWSER_PROBE=1",
            "MDE_CEF_EXTENSION_POWER_MODE=true",
            "MDE_CEF_TEXT_PROBE_EXPECT",
            "mde-cef-extension-autofill-ok",
            "ReuseTcpServer",
            "CEF_EXTENSION_AUTOFILL_SMOKE_READY",
            "CEF_EXTENSIONS_WINDOWLESS_ALLOY_GATED",
        ] {
            assert!(
                runner.contains(needle),
                "CEF WebExtensions smoke runner must contain {needle}"
            );
        }
    }

    #[test]
    fn browser_rpm_ships_widevine_provisioning_but_base_and_server_do_not() {
        let manifest = rpm_manifest();
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        let browser_assets = rpm["variants"]["browser"]["assets"]
            .as_array()
            .expect("browser assets array");
        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");
        let post_install = rpm["variants"]["browser"]["post_install_script"]
            .as_str()
            .expect("browser post install script");

        for (source, dest, mode) in [
            (
                "install-helpers/install-widevine-cdm.sh",
                "/usr/libexec/mackesd/install-widevine-cdm",
                "755",
            ),
            (
                "packaging/browser/widevine-linux64.env",
                "/usr/share/magic-mesh/browser/widevine-linux64.env",
                "644",
            ),
            (
                "packaging/systemd/mde-widevine-cdm-setup.service",
                "/usr/lib/systemd/system/mde-widevine-cdm-setup.service",
                "644",
            ),
        ] {
            assert!(
                asset_exists(browser_assets, source, dest, mode),
                "Browser RPM must ship Widevine provisioning asset {dest}"
            );
            assert!(
                dest_absent(base_assets, dest),
                "base RPM must not ship Widevine provisioning asset {dest}"
            );
            assert!(
                dest_absent(server_assets, dest),
                "headless server RPM must not ship Widevine provisioning asset {dest}"
            );
        }

        assert!(
            post_install.contains("mde-widevine-cdm-setup.service"),
            "Browser RPM post-install must enable the deferred Widevine CDM setup service"
        );

        let service =
            include_str!("../../../../../packaging/systemd/mde-widevine-cdm-setup.service");
        assert_exit_78_gate_is_retryable(service, "Widevine setup unit");
        for needle in [
            "ConditionPathExists=/usr/bin/mde-web-cef",
            "ConditionPathExists=!/opt/mde/widevine/libwidevinecdm.so",
            "ExecCondition=/usr/bin/mackesd role-gate --min-rank 1",
            "ExecStart=/usr/libexec/mackesd/install-widevine-cdm",
        ] {
            assert!(
                service.contains(needle),
                "Widevine setup unit must contain {needle}"
            );
        }

        let installer = include_str!("../../../../../install-helpers/install-widevine-cdm.sh");
        assert!(
            installer.contains("/usr/share/magic-mesh/browser/widevine-linux64.env")
                && installer.contains("/var/cache/magic-mesh/widevine")
                && installer.contains("operator must provide WIDEVINE_URL and WIDEVINE_SHA256"),
            "installed Widevine installer must use installed manifest/cache paths and an honest config gate"
        );
    }

    #[test]
    fn fedora_rpm_builder_builds_workspace_excluded_browser_helpers() {
        let script = include_str!("../../../../../install-helpers/build-rpm-fedora43.sh");
        let farm_script = include_str!("../../../../../install-helpers/xcp-build.sh");
        for manifest in [
            "crates/desktop/mde-web-preview/Cargo.toml",
            "crates/desktop/mde-web-cef/Cargo.toml",
        ] {
            assert!(
                script.contains(&format!("--manifest-path {manifest}")),
                "full Fedora RPM builder must build excluded helper {manifest} before generate-rpm"
            );
            assert!(
                farm_script.contains(&format!(
                    "CARGO_TARGET_DIR=\\\"\\$PWD/target\\\" cargo build --release $MDE_RPM_LOCKED --manifest-path {manifest}"
                )),
                "farm RPM builder must build excluded helper {manifest} into target/release before generate-rpm"
            );
        }
        assert!(
            script.contains("building the Chromium/CEF browser helper + renderer bridge"),
            "the CEF helper build step should be named in the RPM build log"
        );
        assert!(
            script.contains("--bin cef-verify")
                && script.contains("-p mde-web-preview-client --features live-helper"),
            "full Fedora RPM builder must build the CEF wire verifier before generate-rpm"
        );
        assert!(
            farm_script.contains("--bin cef-verify")
                && farm_script.contains("-p mde-web-preview-client --features live-helper"),
            "farm RPM builder must build the CEF wire verifier before generate-rpm"
        );
        assert!(
            script.contains("cargo generate-rpm -p crates/mesh/mackesd --variant browser"),
            "full Fedora RPM builder must emit the split Browser RPM"
        );
        assert!(
            farm_script.contains("cargo generate-rpm -p crates/mesh/mackesd --variant browser"),
            "farm RPM builder must emit the split Browser RPM"
        );
        assert!(
            script.contains(
                "verify-rpm-payload.sh size /src/target-f43/generate-rpm/magic-mesh-[0-9]*.rpm"
            ) && script.contains(
                "verify-rpm-payload.sh size /src/target-f43/generate-rpm/magic-mesh-browser-*.rpm"
            ),
            "full Fedora RPM builder must size-gate both base and Browser RPM artifacts"
        );
        assert!(
            farm_script
                .contains("verify-rpm-payload.sh size target/generate-rpm/magic-mesh-[0-9]*.rpm")
                && farm_script.contains(
                    "verify-rpm-payload.sh size target/generate-rpm/magic-mesh-browser-*.rpm"
                ),
            "farm RPM builder must size-gate both base and Browser RPM artifacts"
        );
        assert!(
            include_str!("../../Cargo.toml").contains("mde-web-cef-renderer"),
            "the CEF renderer bridge must be built by the helper crate and shipped by the Browser RPM"
        );
        assert!(
            include_str!("../../Cargo.toml").contains("target/release/cef-verify"),
            "the CEF wire verifier must be shipped by the Browser RPM"
        );
    }

    #[test]
    fn lighthouse_caddy_provisioning_is_timeout_bounded() {
        let helper = include_str!("../../../../../install-helpers/setup-caddy.sh");
        let cli = include_str!("../bin/mackesd.rs");

        assert!(
            helper.contains("timeout 300 dnf install -y --setopt=install_weak_deps=False caddy"),
            "setup-caddy must not let caddy dnf install hold lighthouse enrollment forever"
        );
        assert!(
            helper.contains("timeout 60 systemctl enable caddy.service"),
            "setup-caddy must bound caddy service enablement"
        );
        assert!(
            cli.contains(".args([\"360\", \"/usr/libexec/mackesd/setup-caddy\"])"),
            "mackesd found/join must bound setup-caddy as a best-effort ingress step"
        );
    }

    #[test]
    fn workstation_units_use_the_typed_rank_one_role_gate() {
        for (name, unit) in [
            (
                "mde-shell-egui.service",
                include_str!("../../../../../packaging/bootc/units/mde-shell-egui.service"),
            ),
            (
                "mde-musicd.service",
                include_str!("../../../../../packaging/systemd/mde-musicd.service"),
            ),
        ] {
            assert!(
                unit.contains("ExecCondition=/usr/bin/mackesd role-gate --min-rank 1"),
                "{name} must gate on the current Workstation rank"
            );
            assert!(
                !unit.contains("grep -Eq"),
                "{name} must not use shell-grep role parsing"
            );
            assert!(
                !unit.contains("--min-rank 2"),
                "{name} must not reference the retired rank-2 Workstation tier"
            );
        }
    }

    #[test]
    fn drm_seat_unit_starts_on_rpm_and_bootc_boot_targets() {
        let unit = include_str!("../../../../../packaging/bootc/units/mde-shell-egui.service");

        assert!(
            unit.contains("WantedBy=multi-user.target graphical.target"),
            "the DRM seat unit must be wanted by multi-user.target for RPM-installed seats and graphical.target for bootc seats"
        );
    }

    #[test]
    fn drm_seat_unit_delegates_cgroups_for_browser_sandbox_caps() {
        let unit = include_str!("../../../../../packaging/bootc/units/mde-shell-egui.service");

        assert!(
            unit.contains("Delegate=yes"),
            "the DRM seat unit must delegate its cgroup subtree so browser helpers can create per-tab memory/CPU capped child cgroups"
        );
        assert!(
            unit.contains("DelegateSubgroup=shell"),
            "the DRM seat unit must keep the shell process in a child subgroup so the delegated service root can host capped browser cgroups"
        );
        assert!(
            unit.contains("Environment=MDE_WEB_SANDBOX_DELEGATE_SUBGROUP=shell"),
            "the Browser sandbox must know which systemd subgroup to escape when creating capped helper cgroups"
        );
    }

    #[test]
    fn mackesd_unit_raises_the_process_fd_budget() {
        let unit = include_str!("../../../../../packaging/systemd/mackesd.service");

        assert!(
            unit.contains("LimitNOFILE=65536"),
            "mackesd must raise nofile above the default 1024 so worker fds cannot exhaust the process"
        );
    }

    #[test]
    fn mackesd_unit_does_not_abort_on_slow_stop() {
        let unit = include_str!("../../../../../packaging/systemd/mackesd.service");
        let dropin =
            include_str!("../../../../../packaging/systemd/mackesd.service.d/90-stop-policy.conf");
        let manifest: toml::Value =
            toml::from_str(include_str!("../../Cargo.toml")).expect("mackesd Cargo.toml parses");
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];

        assert!(
            unit.contains("TimeoutStopSec=90"),
            "mackesd must have enough stop time to drain live lighthouse workers during promotion"
        );
        assert!(
            unit.contains("TimeoutStopFailureMode=terminate"),
            "mackesd must override Fedora's global abort-on-timeout drop-in so promotion restarts do not create SIGABRT coredumps"
        );
        assert!(
            dropin.contains("TimeoutStopSec=90")
                && dropin.contains("TimeoutStopFailureMode=terminate"),
            "mackesd must ship a per-service drop-in because Fedora's global service.d drop-in overrides the base unit file"
        );
        for assets in [
            rpm["assets"].as_array().expect("base assets array"),
            rpm["variants"]["server"]["assets"]
                .as_array()
                .expect("server assets array"),
        ] {
            assert!(
                assets.iter().any(|asset| {
                    asset["dest"].as_str()
                        == Some("/usr/lib/systemd/system/mackesd.service.d/90-stop-policy.conf")
                }),
                "each RPM shape must ship the mackesd per-service stop-policy drop-in"
            );
        }
    }

    #[test]
    fn postinstall_removes_stale_local_abort_watchdog_dropin() {
        let manifest: toml::Value =
            toml::from_str(include_str!("../../Cargo.toml")).expect("mackesd Cargo.toml parses");
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let script = rpm["post_install_script"]
            .as_str()
            .expect("base post install script");

        assert!(
            script.contains("/etc/systemd/system/mackesd.service.d/watchdog.conf"),
            "postinstall must inspect the legacy local watchdog drop-in"
        );
        assert!(
            script.contains("TimeoutStop(FailureMode=abort|USec=20s|Sec=20)"),
            "postinstall must only match the stale 20s/abort stop policy"
        );
        assert!(
            script.contains("rm -f /etc/systemd/system/mackesd.service.d/watchdog.conf"),
            "postinstall must remove the stale local drop-in so the packaged REL-2 stop policy wins"
        );
    }

    /// Fake manager: records every call and always succeeds.
    struct Recorder {
        calls: RefCell<Vec<(String, String)>>,
    }
    impl Recorder {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<(String, String)> {
            self.calls.borrow().clone()
        }
    }
    impl UnitManager for Recorder {
        fn enable(&self, unit: &str) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(("enable".to_string(), unit.to_string()));
            Ok(())
        }
        fn mask(&self, unit: &str) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(("mask".to_string(), unit.to_string()));
            Ok(())
        }
    }

    #[test]
    fn apply_folds_plan_through_the_manager() {
        let rec = Recorder::new();
        let plan = plan(Role::Lighthouse);
        let outcomes = apply(&plan, &rec);
        // One outcome per planned unit, all ok.
        assert_eq!(outcomes.len(), plan.len());
        assert!(outcomes.iter().all(|o| o.ok && o.error.is_none()));
        // Every planned action reached the manager as the matching call.
        let calls = rec.calls();
        assert_eq!(calls.len(), plan.len());
        for pu in &plan {
            let verb = match pu.action {
                UnitAction::Enable => "enable",
                UnitAction::Mask => "mask",
            };
            assert!(
                calls.contains(&(verb.to_string(), pu.unit.to_string())),
                "expected {verb} {}",
                pu.unit
            );
        }
        // Lighthouse masks exactly the base Workstation units; optional Browser
        // runtime setup is owned by the split Browser package.
        assert_eq!(
            calls.iter().filter(|(v, _)| v == "mask").count(),
            3,
            "lighthouse masks the rank-1 shell + voice units"
        );
    }

    /// Fake manager that fails one specific unit — proves a partial failure is
    /// recorded without aborting the rest.
    struct FailOne(&'static str);
    impl UnitManager for FailOne {
        fn enable(&self, unit: &str) -> Result<(), String> {
            if unit == self.0 {
                Err("boom".to_string())
            } else {
                Ok(())
            }
        }
        fn mask(&self, _unit: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn apply_records_a_partial_failure_and_continues() {
        let outcomes = apply(&plan(Role::Workstation), &FailOne("mackesd.service"));
        let failed: Vec<&UnitOutcome> = outcomes.iter().filter(|o| !o.ok).collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].unit, "mackesd.service");
        assert_eq!(failed[0].error.as_deref(), Some("boom"));
        // Every other unit still ran and succeeded.
        assert_eq!(outcomes.iter().filter(|o| o.ok).count(), outcomes.len() - 1);
    }
}
