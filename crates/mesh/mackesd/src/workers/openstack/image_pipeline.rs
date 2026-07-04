//! QC-9 (QUASAR-CLOUD) — the diskimage-builder (DIB) → Glance image pipeline.
//!
//! This module replaces `install-helpers/build-mde-vm-golden.sh` and the
//! on-disk golden qcow2s (design Q36/53): the platform's standard VM images are
//! **built declaratively by diskimage-builder from a pinned element set**,
//! uploaded into Glance once, then **replicated to every API node's local file
//! store** ([`super::config_render`] renders the store + cache + `copy-image`
//! import that receive them).
//!
//! ## Why DIB retires the golden script
//!
//! `build-mde-vm-golden.sh` took a hand-prepared, *booted* Fedora-cloud VM and
//! **generalized it imperatively** — `cloud-init clean`, blank
//! `/etc/machine-id`, strip the SSH host keys, truncate logs — so a clone would
//! regenerate its identity on first boot. That is a manual, unversioned,
//! snowflake step whose correctness lived in a shell script and an operator's
//! memory, and whose output (a golden qcow2 marked an XCP template) lived
//! outside any store the cloud could serve.
//!
//! DIB produces the same generalized image **by construction and from a
//! versioned manifest**: the `vm` + `cloud-init` elements bake an image that
//! carries NO machine-id and NO SSH host keys — cloud-init regenerates both on
//! first boot from the instance's metadata (the exact effect the golden script
//! hand-rolled) — and `growroot` grows the root partition to the flavor disk.
//! There is nothing to "generalize" after the fact: the build output is already
//! a clean, cloneable image, and the element list ([`StandardImage::elements`])
//! is the reviewable source of truth the script never was. The golden script is
//! then dead — its physical `git rm` lands with the QC-15 hard cutover (the
//! cutover-map's "`build-mde-vm-golden.sh` deleted" verification, `docs/ops/
//! quasar-cloud-cutover-map.md`), which retires the whole cloud-hypervisor/XCP
//! template stack the script fed.
//!
//! ## The pipeline (three stages; the run itself is a deploy-time operator step)
//!
//! 1. **Build** — [`StandardImage::dib_build_command`]: `disk-image-create`
//!    turns the pinned element set into a `<name>.qcow2` on a build host.
//! 2. **Land in Glance** — [`StandardImage::glance_create_command`]: `glance
//!    image-create` uploads the qcow2 into the local **file** store
//!    ([`super::config_render`]'s `[glance_store] stores = file`) of ONE API
//!    node (design Q53).
//! 3. **Replicate** — [`replicate_command`]: `glance-replicator livecopy` fans
//!    the image (metadata + data) from that node to every other API node's local
//!    store, so any node serves the standard image locally (Q53 — replication
//!    between API nodes). The per-node **image cache** ([`super::config_render`])
//!    then keeps ad-hoc images hot between replications.
//!
//! Like the QC-3 Kolla mirror procedure, stages 1–3 run at deploy time (a build
//! host + a live cloud), so this module is the **pinned, testable definition**
//! of what those stages run — the §7 unit gate is the definition, not a live
//! build. QC-11's typed `image` verb and QC-12's Cloud-plane image view consume
//! this vocabulary to drive the build/upload/replicate from the shell.

use super::catalog::ServiceKind;

/// The Glance disk format the pipeline builds + uploads (Q36 — qcow2: thin,
/// snapshot-friendly, the Nova/libvirt default root-disk format).
pub const DISK_FORMAT: &str = "qcow2";

/// The Glance container format — `bare`: a raw disk image with no OVF/AMI
/// wrapper (the only format a libvirt/QEMU instance boots directly).
pub const CONTAINER_FORMAT: &str = "bare";

/// The `[glance_store]` local file-store name the upload targets — matches the
/// `--store` this renders and the `stores`/`default_store` in
/// [`super::config_render`]'s glance-api.conf.
pub const GLANCE_FILE_STORE: &str = "file";

/// One standard platform image the DIB pipeline builds and lands in Glance.
///
/// A pinned, reviewable manifest — the antithesis of the golden script's
/// imperative generalize-a-booted-VM step (see the module docs).
pub struct StandardImage {
    /// The Glance image name members launch from (the launch-picker label,
    /// Q83) and the `disk-image-create -o` output stem.
    pub name: &'static str,
    /// The pinned base-distro release `disk-image-create` fetches the base
    /// cloud image for (`DIB_RELEASE`) — so a rebuild is reproducible, not
    /// "whatever the latest cloud image is today".
    pub release: &'static str,
    /// The ordered diskimage-builder element set (the first is the base-distro
    /// element; the rest layer onto it). Each element is a versioned,
    /// reviewable unit — the manifest the golden script never had.
    pub elements: &'static [&'static str],
}

impl StandardImage {
    /// The `disk-image-create` invocation that builds this image to a local
    /// `<name>.qcow2` (stage 1). `DIB_RELEASE` pins the base cloud image, `-t
    /// qcow2` sets the output format, and the elements are the manifest.
    #[must_use]
    pub fn dib_build_command(&self) -> String {
        format!(
            "DIB_RELEASE={release} disk-image-create -t {fmt} -o {name} {elements}",
            release = self.release,
            fmt = DISK_FORMAT,
            name = self.name,
            elements = self.elements.join(" "),
        )
    }

    /// The build artifact `disk-image-create -o {name} -t qcow2` writes —
    /// `<name>.qcow2`.
    #[must_use]
    pub fn artifact(&self) -> String {
        format!("{}.{DISK_FORMAT}", self.name)
    }

    /// The `glance image-create` invocation that lands the built qcow2 in one
    /// API node's local **file** store (stage 2). `--visibility public` so every
    /// member can launch it (Q81/82 — one cloud for all, no per-user image silo).
    #[must_use]
    pub fn glance_create_command(&self) -> String {
        format!(
            "glance image-create --name {name} --disk-format {disk} \
             --container-format {cont} --store {store} --visibility public --file {artifact}",
            name = self.name,
            disk = DISK_FORMAT,
            cont = CONTAINER_FORMAT,
            store = GLANCE_FILE_STORE,
            artifact = self.artifact(),
        )
    }
}

/// The platform's standard image set (Q36 — DIB replaces the golden qcow2s).
///
/// Today one image: the mesh base — a Fedora cloud root whose element set makes
/// a clone self-identify on first boot (the exact property the golden script
/// hand-rolled) and join the overlay:
///
/// - `fedora` — the base cloud root filesystem (the pinned `DIB_RELEASE`).
/// - `vm` — a bootable, partitioned disk image (not a chroot tarball).
/// - `growroot` — grow the root partition to the flavor disk on first boot
///   (Q56 — ephemeral root; a bigger flavor just gets more room).
/// - `cloud-init` — regenerate machine-id + SSH host keys per instance and apply
///   the instance metadata; this is what **retires the golden generalize step**.
/// - `cloud-init-datasources` — read the Nova config-drive metadata the instance
///   boots with (hostname, the member's SSH key — QC-13).
/// - `openssh-server` — the sshd QC-13 injects the member's mesh-derived key into.
/// - `dhcp-all-interfaces` — the flat mesh net (QC-7) addresses the instance via
///   DHCP, so it comes up mesh-reachable with no per-instance config.
/// - `mcnf-mesh-agent` — the platform-provided local element that bakes the mesh
///   enrollment bits so a launched instance joins the overlay (shipped beside
///   the deploy, under `ELEMENTS_PATH`).
pub const STANDARD_IMAGES: &[StandardImage] = &[StandardImage {
    name: "mcnf-base",
    release: "41",
    elements: &[
        "fedora",
        "vm",
        "growroot",
        "cloud-init",
        "cloud-init-datasources",
        "openssh-server",
        "dhcp-all-interfaces",
        "mcnf-mesh-agent",
    ],
}];

/// The Glance API port the replicator connects to on each node (the catalog's
/// glance-api port — kept in lockstep with [`ServiceKind::GlanceApi`]).
#[must_use]
pub fn glance_port() -> u16 {
    ServiceKind::GlanceApi.api_port().unwrap_or(9292)
}

/// The `glance-replicator livecopy` invocation (stage 3) — the cross-node
/// replication verb (Q53).
///
/// Fans every image from the node that holds them (`from_host`) to a peer API
/// node (`to_host`), metadata + data, so each API node's local file store holds
/// the standard images.
///
/// Both arguments are `host:port` endpoints reached **over the overlay** (QC-6,
/// Q23): distinct peer nodes (their Nebula overlay IPs or per-node mesh names),
/// never the shared `glance.mesh` catalog name — the replicator copies from one
/// concrete store to another. Run once per peer at deploy time (the fan-out),
/// or on a schedule to heal a node that re-provisioned with an empty store; the
/// per-node image cache ([`super::config_render`]) covers ad-hoc images between
/// replications.
#[must_use]
pub fn replicate_command(from_host: &str, to_host: &str) -> String {
    let port = glance_port();
    format!("glance-replicator livecopy {from_host}:{port} {to_host}:{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standard_set_is_non_empty_and_names_the_base_image() {
        assert!(!STANDARD_IMAGES.is_empty(), "at least one standard image");
        let base = &STANDARD_IMAGES[0];
        assert_eq!(base.name, "mcnf-base");
        assert!(!base.elements.is_empty(), "an image needs an element manifest");
        // The base-distro element leads the manifest.
        assert_eq!(base.elements[0], "fedora");
    }

    #[test]
    fn dib_build_command_pins_the_release_and_lists_the_manifest() {
        let base = &STANDARD_IMAGES[0];
        let cmd = base.dib_build_command();
        // Reproducible: the base cloud image is pinned, not "latest".
        assert!(cmd.contains("DIB_RELEASE=41"), "{cmd}");
        assert!(cmd.contains("disk-image-create -t qcow2"), "{cmd}");
        assert!(cmd.contains("-o mcnf-base"), "{cmd}");
        // Every element in the manifest appears in the invocation.
        for element in base.elements {
            assert!(cmd.contains(element), "element {element} missing: {cmd}");
        }
    }

    #[test]
    fn the_manifest_generalizes_by_construction_not_by_a_script() {
        // The golden script's job (a cloneable image: fresh machine-id + host
        // keys per boot, root grown to the disk) is done by these elements at
        // BUILD time — so there is no imperative post-build generalize step.
        let base = &STANDARD_IMAGES[0];
        assert!(
            base.elements.contains(&"cloud-init"),
            "cloud-init regenerates machine-id + host keys per instance"
        );
        assert!(
            base.elements.contains(&"vm"),
            "a bootable disk image, not a chroot tarball"
        );
        assert!(
            base.elements.contains(&"growroot"),
            "grow the root partition to the flavor disk"
        );
    }

    #[test]
    fn glance_create_lands_the_qcow2_in_the_file_store() {
        let base = &STANDARD_IMAGES[0];
        assert_eq!(base.artifact(), "mcnf-base.qcow2");
        let cmd = base.glance_create_command();
        assert!(cmd.contains("--name mcnf-base"), "{cmd}");
        assert!(cmd.contains("--disk-format qcow2"), "{cmd}");
        assert!(cmd.contains("--container-format bare"), "{cmd}");
        // The local file store (design Q53), and the built artifact.
        assert!(cmd.contains("--store file"), "{cmd}");
        assert!(cmd.contains("--file mcnf-base.qcow2"), "{cmd}");
        // One cloud for all — the image is launchable by every member (Q82).
        assert!(cmd.contains("--visibility public"), "{cmd}");
    }

    #[test]
    fn replicate_is_livecopy_between_two_concrete_nodes_on_the_glance_port() {
        // Q53 — the replication verb is a node→node livecopy on the glance-api
        // port, kept in lockstep with the catalog (not a hardcoded 9292).
        assert_eq!(glance_port(), 9292);
        let cmd = replicate_command("10.42.0.9", "10.42.0.10");
        assert_eq!(
            cmd,
            "glance-replicator livecopy 10.42.0.9:9292 10.42.0.10:9292"
        );
    }
}
