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
/// * **Rank 1 (Workstation only)** — the desktop adds the DRM-seat shell.
///   The base role owns the supported provisioning units; retired Browser
///   runtime setup is not part of the package or role contract.
const ROLE_UNITS: &[(&str, u8)] = &[
    // ── Rank 0 — universal control/data plane (CONVERGE_SERVICES + status timer).
    ("nebula.service", 0),
    ("mackesd.service", 0),
    ("etcd.service", 0),
    ("syncthing.service", 0),
    ("mesh-health.timer", 0),
    ("mesh-status.timer", 0),
    ("mcnf-lifecycle-firstboot.service", 0),
    // ── Rank 1 — Workstation-only: the DRM-seat shell.
    ("mde-shell-egui.service", 1),
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

/// Return the units expected to run for a role.
///
/// Status and readiness consumers use this catalog instead of maintaining a
/// second service list that can drift from role provisioning.
#[must_use]
pub fn units_for_role(role: Role) -> Vec<&'static str> {
    plan(role)
        .into_iter()
        .filter(|unit| unit.action == UnitAction::Enable)
        .map(|unit| unit.unit)
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
            "mcnf-lifecycle-firstboot.service",
        ] {
            assert_eq!(
                action_for(&p, u).action,
                UnitAction::Enable,
                "lighthouse must enable {u}"
            );
        }
        // Rank-1 Workstation units → masked (a lighthouse never runs them).
        for u in ["mde-shell-egui.service"] {
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
    fn readiness_unit_catalog_matches_role_plan() {
        let lighthouse = units_for_role(Role::Lighthouse);
        assert!(lighthouse.contains(&"mesh-status.timer"));
        assert!(!lighthouse.contains(&"mde-shell-egui.service"));

        let workstation = units_for_role(Role::Workstation);
        assert!(workstation.contains(&"mde-shell-egui.service"));
        assert!(workstation.contains(&"mesh-status.timer"));
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
            rpm["requires"]["hunspell"].as_str(),
            Some("*"),
            "the base RPM must require hunspell for offline editor spell checking"
        );
        assert_eq!(
            rpm["requires"]["hunspell-en-US"].as_str(),
            Some("*"),
            "the base RPM must require a default hunspell dictionary"
        );
        for package in [
            "pipewire",
            "wireplumber",
            "pipewire-alsa",
            "pipewire-pulseaudio",
            "alsa-ucm",
            "alsa-sof-firmware",
            "alsa-utils",
        ] {
            assert_eq!(
                rpm["requires"][package].as_str(),
                Some("*"),
                "the base RPM must require {package} so Workstation audio has a complete PipeWire/Pulse/ALSA-UCM stack"
            );
        }
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
            asset_exists(
                base_assets,
                "packaging/systemd/mcnf-lifecycle-firstboot.service",
                "/usr/lib/systemd/system/mcnf-lifecycle-firstboot.service",
                "644",
            ),
            "base RPM must ship the first-boot baseline unit"
        );
        assert!(
            post_install.contains("mcnf-lifecycle-firstboot.service"),
            "base RPM post-install must enable first-boot baseline convergence"
        );
        assert!(
            post_install.contains("usermod -aG audio \"$user\""),
            "base RPM post-install must grant known non-root seat users the audio group so PipeWire can open /dev/snd on DRM seats without logind ACLs"
        );
        assert!(
            post_install.contains("loginctl enable-linger \"$user\"")
                && post_install.contains("systemctl start \"user@$uid.service\""),
            "base RPM post-install must keep the primary seat PipeWire user manager boot-durable"
        );
        let seat_unit = include_str!("../../../../../packaging/bootc/units/mde-shell-egui.service");
        assert!(
            seat_unit.contains("Environment=XDG_RUNTIME_DIR=/run/user/1000")
                && seat_unit.contains("Wants=user@1000.service"),
            "the root DRM shell must connect to the persistent primary seat PipeWire graph"
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
    fn thin_lighthouse_rpm_is_control_plane_only() {
        let manifest = rpm_manifest();
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let lighthouse = &rpm["variants"]["lighthouse"];
        assert_eq!(
            lighthouse["name"].as_str(),
            Some("magic-mesh-lighthouse"),
            "DO provisioning must target the dedicated thin package"
        );
        let assets = lighthouse["assets"]
            .as_array()
            .expect("thin lighthouse asset array");
        assert!(
            asset_exists(assets, "target/release/mackesd", "/usr/bin/mackesd", "755"),
            "thin lighthouse RPM must carry the daemon"
        );
        for forbidden in [
            "install-helpers/setup-media-navidrome.sh",
            "install-helpers/mcnf-music-ingest.sh",
            "install-helpers/setup-syncthing.sh",
            "install-helpers/syncthing-reconcile.sh",
            "install-helpers/cutover-substrate-v2.sh",
            "packaging/systemd/syncthing.service",
        ] {
            assert!(
                !assets
                    .iter()
                    .any(|asset| asset["source"].as_str() == Some(forbidden)),
                "thin lighthouse RPM must not ship forbidden asset {forbidden}"
            );
        }
        let requires = lighthouse["requires"].as_table().expect("thin requires");
        assert_eq!(requires.get("nebula").and_then(|v| v.as_str()), Some("*"));
        for forbidden in ["podman", "rclone", "syncthing", "libvirt", "qemu-img"] {
            assert!(
                !requires.contains_key(forbidden),
                "thin lighthouse RPM must not hard-require {forbidden}"
            );
        }
        assert!(
            lighthouse.get("recommends").is_none(),
            "thin lighthouse RPM must not weak-pull optional media/fileshare stacks"
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
            "cloud-init-local.service",
            "cloud-init.service",
            "cloud-config.service",
            "cloud-final.service",
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
            "seat unit restores tty1 only after terminal failure",
            "seat unit does not race normal restarts with getty",
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
        let unit = "/usr/lib/systemd/system/magic-mesh-selinux-policy.service";
        assert!(
            assets
                .iter()
                .any(|asset| asset["dest"].as_str() == Some(unit)),
            "base RPM must ship the async SELinux loader unit {unit}"
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
    fn full_rpm_ships_offline_map_installer_and_persistent_map_root() {
        let manifest: toml::Value =
            toml::from_str(include_str!("../../Cargo.toml")).expect("mackesd Cargo.toml parses");
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let base_assets = rpm["assets"].as_array().expect("base assets array");
        let server_assets = rpm["variants"]["server"]["assets"]
            .as_array()
            .expect("server assets array");
        let lighthouse_assets = rpm["variants"]["lighthouse"]["assets"]
            .as_array()
            .expect("lighthouse assets array");
        let source = "install-helpers/install-offline-map-region.sh";
        let dest = "/usr/libexec/mackesd/install-offline-map-region";

        assert!(
            asset_exists(base_assets, source, dest, "755"),
            "full Workstation RPM must ship the offline Maps region installer"
        );
        for assets in [server_assets, lighthouse_assets] {
            assert!(
                dest_absent(assets, dest),
                "headless RPM variants must not ship the Workstation Maps installer"
            );
        }

        let tmpfiles = include_str!("../../../../../packaging/tmpfiles/magic-mesh.conf");
        assert!(
            tmpfiles.contains("d /var/lib/mde/maps 0755 root root -"),
            "tmpfiles must create the persistent offline Maps root"
        );

        let unit = include_str!("../../../../../packaging/bootc/units/mde-shell-egui.service");
        assert!(
            unit.contains("Environment=MDE_MAPS_DIR=/var/lib/mde/maps"),
            "the DRM shell unit must pin Maps to persistent storage, not /run/mde-bus"
        );
        assert!(
            unit.contains("OnFailure=getty@tty1.service") && !unit.contains("ExecStopPost="),
            "the DRM shell must recover tty1 only after terminal failure, never race a normal restart with getty"
        );

        let helper = include_str!("../../../../../install-helpers/install-offline-map-region.sh");
        assert!(
            helper.contains("DEFAULT_DEST_ROOT=\"${MDE_MAPS_DIR:-/var/lib/mde/maps}\"")
                && helper.contains("--self-test"),
            "the packaged helper must default to the persistent map root and carry a self-test"
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
    fn do_lighthouse_cloudinit_requires_the_thin_rpm_variant() {
        for (name, script) in [
            (
                "found",
                include_str!("../../../../../install-helpers/do-lighthouse-cloudinit.sh"),
            ),
            (
                "join",
                include_str!("../../../../../install-helpers/do-lighthouse-join-cloudinit.sh"),
            ),
        ] {
            assert!(
                script.contains("magic-mesh-lighthouse || fail"),
                "{name} cloud-init must install the dedicated magic-mesh-lighthouse package"
            );
            assert!(
                script.contains("thin lighthouse RPM"),
                "{name} cloud-init must label direct RPM installs as thin-only"
            );
        }
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
    fn every_mackesd_group_raises_the_process_fd_budget() {
        for unit in grouped_mackesd_units() {
            assert!(
                unit.contains("LimitNOFILE=65536"),
                "every mackesd group must raise nofile above the default 1024"
            );
        }
    }

    #[test]
    fn every_mackesd_group_pins_the_packaged_repo_root() {
        for unit in grouped_mackesd_units() {
            assert!(
                unit.contains("Environment=MCNF_REPO=/opt/mcnf"),
                "mackesd must resolve packaged mesh helpers instead of a developer checkout"
            );
        }
    }

    #[test]
    fn every_daemon_rpm_bootstraps_mesh_secret_recipients() {
        let manifest: toml::Value =
            toml::from_str(include_str!("../../Cargo.toml")).expect("mackesd Cargo.toml parses");
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];

        for requires in [
            &rpm["requires"],
            &rpm["variants"]["lighthouse"]["requires"],
            &rpm["variants"]["server"]["requires"],
        ] {
            assert_eq!(
                requires["age"].as_str(),
                Some("*"),
                "every daemon-bearing RPM needs the age runtime"
            );
        }
        for assets in [
            rpm["assets"].as_array().expect("base assets array"),
            rpm["variants"]["lighthouse"]["assets"]
                .as_array()
                .expect("lighthouse assets array"),
            rpm["variants"]["server"]["assets"]
                .as_array()
                .expect("server assets array"),
        ] {
            for destination in [
                "/opt/mcnf/automation/secrets/mcnf-secret.sh",
                "/usr/libexec/mackesd/mesh-secret-recipient-reconcile",
                "/usr/lib/systemd/system/mcnf-mesh-secret-recipient.service",
                "/usr/lib/systemd/system/mcnf-mesh-secret-recipient.timer",
            ] {
                assert_eq!(
                    assets
                        .iter()
                        .filter(|asset| asset["dest"].as_str() == Some(destination))
                        .count(),
                    1,
                    "each RPM shape must ship {destination} exactly once"
                );
            }
        }
        assert!(
            rpm["post_install_script"]
                .as_str()
                .is_some_and(|script| script.contains("mcnf-mesh-secret-recipient.timer")),
            "the base RPM must enable ongoing recipient reconciliation"
        );
        let tmpfiles = include_str!("../../../../../packaging/tmpfiles/magic-mesh.conf");
        assert!(
            tmpfiles
                .lines()
                .any(|line| line == "z /etc/machine-id 0444 root root -"),
            "package convergence must repair a group-writable machine identity before overlay publication"
        );
    }

    #[test]
    fn grouped_mackesd_units_do_not_abort_on_slow_stop() {
        for unit in grouped_mackesd_units() {
            assert!(unit.contains("TimeoutStopSec=90"));
            assert!(
                unit.contains("TimeoutStopFailureMode=terminate"),
                "every group must override Fedora's abort-on-timeout policy"
            );
        }
    }

    fn grouped_mackesd_units() -> [&'static str; 6] {
        [
            include_str!("../../../../../packaging/systemd/mackesd-control.service"),
            include_str!("../../../../../packaging/systemd/mackesd-observation.service"),
            include_str!("../../../../../packaging/systemd/mackesd-actions.service"),
            include_str!("../../../../../packaging/systemd/mackesd-data.service"),
            include_str!("../../../../../packaging/systemd/mackesd-compute.service"),
            include_str!("../../../../../packaging/systemd/mackesd-integrations.service"),
        ]
    }

    #[test]
    fn postinstall_removes_stale_local_abort_watchdog_dropin() {
        let manifest: toml::Value =
            toml::from_str(include_str!("../../Cargo.toml")).expect("mackesd Cargo.toml parses");
        let rpm = &manifest["package"]["metadata"]["generate-rpm"];
        let script = rpm["post_install_script"]
            .as_str()
            .expect("base post install script");
        let retired_service_cleanup = script
            .split_once("rm -f /etc/systemd/system/mackesd.service")
            .and_then(|(_, cleanup)| cleanup.split_once(" || :"))
            .map(|(cleanup, _)| cleanup)
            .expect("base postinstall must remove the retired monolithic service");

        assert!(
            retired_service_cleanup
                .split_ascii_whitespace()
                .any(|path| path == "/etc/systemd/system/mackesd.service.d/watchdog.conf"),
            "postinstall must remove the legacy local watchdog drop-in"
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
        // Lighthouse masks exactly the current rank-1 Workstation unit.
        assert_eq!(
            calls.iter().filter(|(v, _)| v == "mask").count(),
            1,
            "lighthouse masks the rank-1 shell unit"
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
