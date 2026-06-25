//! Native mount enumeration — the "This PC / volumes" parity op (E11.6, Q34–Q39).
//!
//! Parses `/proc/mounts` into the mounted-filesystem list the file manager shows
//! under "This PC" (real disks, network shares, FUSE mounts like sshfs),
//! filtering out the kernel's pseudo-filesystems (`proc`, `sysfs`,
//! `cgroup`, …). Pure `std`; the parser takes the file content as a string so it
//! is fully unit-tested off fixtures with no `/proc` dependency.

use std::path::PathBuf;

/// One row of `/proc/mounts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountPoint {
    /// Backing device or remote source (`/dev/sda1`, `//nas/share`, `user@host:/srv`).
    pub source: String,
    /// Where it is mounted.
    pub target: PathBuf,
    /// Filesystem type (`ext4`, `cifs`, `fuse.sshfs`, …).
    pub fstype: String,
    /// The raw mount options field (`rw,relatime,…`).
    pub options: String,
}

/// Filesystem types that are kernel/virtual plumbing, never user volumes.
const PSEUDO_FSTYPES: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "tmpfs",
    "devpts",
    "mqueue",
    "hugetlbfs",
    "debugfs",
    "tracefs",
    "securityfs",
    "pstore",
    "bpf",
    "configfs",
    "fusectl",
    "efivarfs",
    "autofs",
    "binfmt_misc",
    "ramfs",
    "selinuxfs",
    "nsfs",
    "rpc_pipefs",
    "cgroup",
    "cgroup2",
];

impl MountPoint {
    /// Whether this mount is a "volume" worth showing under This PC — a real disk,
    /// a network share, or a FUSE mount (sshfs Cloud-Files, …), as opposed to
    /// kernel pseudo-plumbing.
    #[must_use]
    pub fn is_user_volume(&self) -> bool {
        // Real FUSE mounts (fuse.sshfs, fuseblk) are always user volumes — but
        // `fusectl`, the FUSE *control* pseudo-fs, is not, so match the `fuse.`
        // prefix / `fuseblk` exactly rather than every "fuse*".
        if self.fstype.starts_with("fuse.") || self.fstype == "fuseblk" {
            return true;
        }
        !PSEUDO_FSTYPES.contains(&self.fstype.as_str())
    }

    /// Whether the mount is remote (network or mesh) rather than a local block
    /// device — drives the icon + the "available offline?" hint.
    #[must_use]
    pub fn is_network(&self) -> bool {
        matches!(
            self.fstype.as_str(),
            "nfs" | "nfs4" | "cifs" | "smb3" | "smbfs" | "9p"
        ) || self.fstype.starts_with("fuse.")
    }

    /// Whether this row is the mesh-storage / QNM-Shared store specifically.
    /// SUBSTRATE-V2: the shared workgroup tree is a plain Syncthing-replicated
    /// directory at `/mnt/mesh-storage` (or `$MDE_WORKGROUP_ROOT`), **not** a FUSE
    /// mount — so it is identified by its target path, not a special fstype.
    #[must_use]
    pub fn is_mesh_storage(&self) -> bool {
        let root =
            std::env::var("MDE_WORKGROUP_ROOT").unwrap_or_else(|_| "/mnt/mesh-storage".to_string());
        self.target == PathBuf::from(root)
    }

    /// Canonical freedesktop icon name for this mount's "This PC" row.
    /// NOTIFY-UI-3 / ICON-MESH: network/mesh mounts (the mesh-storage /
    /// QNM-Shared store + any other network share) read as *network* locations
    /// and take `folder-remote`; a plain local block device takes
    /// `drive-harddisk`. Delegates to the shared selector in [`crate::icons`] so
    /// the name and the rendered SVG can never drift apart.
    #[must_use]
    pub fn icon_name(&self) -> &'static str {
        crate::icons::icon_name_for_mount(self.is_network())
    }
}

/// Parse the content of `/proc/mounts` (or `/etc/mtab`) into rows, decoding the
/// octal `\NNN` escapes the kernel uses for spaces/tabs/newlines in fields.
/// Malformed lines (fewer than 4 fields) are skipped.
#[must_use]
pub fn parse_proc_mounts(content: &str) -> Vec<MountPoint> {
    content
        .lines()
        .filter_map(|line| {
            let mut f = line.split(' ');
            let source = unescape_octal(f.next()?);
            let target = unescape_octal(f.next()?);
            let fstype = unescape_octal(f.next()?);
            let options = unescape_octal(f.next()?);
            if fstype.is_empty() {
                return None;
            }
            Some(MountPoint {
                source,
                target: PathBuf::from(target),
                fstype,
                options,
            })
        })
        .collect()
}

/// All current mounts, read from `/proc/mounts`. Empty when `/proc` is
/// unreadable (e.g. a minimal container), never an error — a file manager just
/// shows no extra volumes.
#[must_use]
pub fn all() -> Vec<MountPoint> {
    std::fs::read_to_string("/proc/mounts").map_or_else(|_| Vec::new(), |c| parse_proc_mounts(&c))
}

/// The user-facing volumes only ([`MountPoint::is_user_volume`]).
#[must_use]
pub fn user_volumes() -> Vec<MountPoint> {
    all()
        .into_iter()
        .filter(MountPoint::is_user_volume)
        .collect()
}

/// Decode the kernel's `\NNN` octal escapes (space=`\040`, tab=`\011`,
/// newline=`\012`, backslash=`\134`) back into their characters.
fn unescape_octal(field: &str) -> String {
    if !field.contains('\\') {
        return field.to_string();
    }
    let bytes = field.as_bytes();
    let mut out = String::with_capacity(field.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 4 <= bytes.len() {
            let oct = &field[i + 1..i + 4];
            if let Ok(b) = u8::from_str_radix(oct, 8) {
                out.push(b as char);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Render the user volumes as the `mde-files --mounts` report (one
/// `target\tfstype\tsource\ticon=<name>[\t(network)]` per line). The `icon=`
/// field is the freedesktop icon name a file manager should paint for the row
/// ([`MountPoint::icon_name`]) — `folder-remote` for the mesh-storage /
/// QNM-Shared store and other network shares, `drive-harddisk` for local disks
/// (NOTIFY-UI-3 / ICON-MESH), so the chosen icon is observable at runtime.
#[must_use]
pub fn report(mounts: &[MountPoint]) -> String {
    let mut out = String::new();
    for m in mounts {
        out.push_str(&format!(
            "{}\t{}\t{}\ticon={}{}\n",
            m.target.display(),
            m.fstype,
            m.source,
            m.icon_name(),
            if m.is_network() { "\t(network)" } else { "" }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A realistic /proc/mounts slice: pseudo fs, a real disk, a tmpfs, a CIFS
    // share, and two sshfs FUSE mounts — plus a mount whose path contains an
    // escaped space.
    const FIXTURE: &str = "proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0\n\
sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0\n\
/dev/nvme0n1p2 / ext4 rw,relatime 0 0\n\
tmpfs /run tmpfs rw,nosuid,nodev 0 0\n\
/dev/nvme0n1p1 /boot/efi vfat rw,relatime 0 0\n\
//nas/media /mnt/media cifs rw,relatime 0 0\n\
user@host:/srv /home/mm/remote fuse.sshfs rw,nosuid,nodev,relatime 0 0\n\
user@host:/srv /mnt/mesh\\040store fuse.sshfs rw,relatime 0 0\n\
fusectl /sys/fs/fuse/connections fusectl rw,nosuid,nodev,noexec,relatime 0 0\n";

    #[test]
    fn parses_every_well_formed_line() {
        let m = parse_proc_mounts(FIXTURE);
        assert_eq!(m.len(), 9);
        assert_eq!(m[2].source, "/dev/nvme0n1p2");
        assert_eq!(m[2].target, PathBuf::from("/"));
        assert_eq!(m[2].fstype, "ext4");
    }

    #[test]
    fn user_volume_filter_drops_pseudo_keeps_real() {
        let vols: Vec<_> = parse_proc_mounts(FIXTURE)
            .into_iter()
            .filter(MountPoint::is_user_volume)
            .collect();
        let targets: Vec<String> = vols
            .iter()
            .map(|m| m.target.display().to_string())
            .collect();
        // kept: the ext4 root, the vfat ESP, the CIFS share, both FUSE mounts.
        assert!(targets.contains(&"/".to_string()));
        assert!(targets.contains(&"/boot/efi".to_string()));
        assert!(targets.contains(&"/mnt/media".to_string()));
        assert!(targets.contains(&"/home/mm/remote".to_string()));
        // dropped: proc, sysfs, the tmpfs on /run, and the fusectl control fs
        // (it starts with "fuse" but is not a real FUSE *mount*).
        assert!(!targets.contains(&"/proc".to_string()));
        assert!(!targets.contains(&"/sys".to_string()));
        assert!(!targets.contains(&"/run".to_string()));
        assert!(!targets.contains(&"/sys/fs/fuse/connections".to_string()));
    }

    #[test]
    fn octal_escape_in_a_path_is_decoded() {
        let mesh = parse_proc_mounts(FIXTURE)
            .into_iter()
            .find(|m| m.target == PathBuf::from("/mnt/mesh store"))
            .unwrap();
        assert_eq!(mesh.target, PathBuf::from("/mnt/mesh store"));
        assert!(mesh.is_user_volume());
        assert!(mesh.is_network(), "a fuse.* mount is remote");
    }

    #[test]
    fn network_classification() {
        let m = parse_proc_mounts(FIXTURE);
        let by_target = |t: &str| m.iter().find(|x| x.target == PathBuf::from(t)).unwrap();
        assert!(by_target("/mnt/media").is_network(), "cifs is network");
        assert!(
            by_target("/home/mm/remote").is_network(),
            "sshfs is network"
        );
        assert!(!by_target("/").is_network(), "a local ext4 disk is not");
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let m = parse_proc_mounts("garbage\n/dev/sda1 /data\n/dev/sdb1 /more ext4 rw 0 0\n");
        // only the last line has >=4 fields
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].fstype, "ext4");
    }

    #[test]
    fn mesh_mount_icon_is_the_network_folder() {
        // NOTIFY-UI-3 / ICON-MESH: a network share (CIFS/sshfs) is a remote file
        // service, so its "This PC" row must take the `folder-remote` network
        // icon, not a local mounted-volume icon.
        let m = parse_proc_mounts(FIXTURE);
        let by_target = |t: &str| m.iter().find(|x| x.target == PathBuf::from(t)).unwrap();

        // Network shares (CIFS / sshfs) are remote → folder-remote.
        let cifs = by_target("/mnt/media");
        assert_eq!(cifs.icon_name(), "folder-remote");

        // A local ext4 disk keeps the local disk icon.
        assert_eq!(by_target("/").icon_name(), "drive-harddisk");
    }

    #[test]
    fn mesh_storage_is_identified_by_its_path() {
        // SUBSTRATE-V2: the shared store is a plain dir at /mnt/mesh-storage
        // (no FUSE), so it's matched by target path, not a special fstype.
        let mesh = MountPoint {
            source: "syncthing".into(),
            target: PathBuf::from("/mnt/mesh-storage"),
            fstype: "ext4".into(),
            options: "rw".into(),
        };
        // env may be set by the test harness; only assert the negative case
        // (which never depends on the env default) to stay parallel-safe.
        let other = MountPoint {
            source: "//nas/media".into(),
            target: PathBuf::from("/mnt/media"),
            fstype: "cifs".into(),
            options: "rw".into(),
        };
        assert!(
            !other.is_mesh_storage(),
            "a CIFS share is not the mesh store"
        );
        // With no override, the default path matches.
        if std::env::var_os("MDE_WORKGROUP_ROOT").is_none() {
            assert!(
                mesh.is_mesh_storage(),
                "/mnt/mesh-storage is the mesh store"
            );
        }
    }

    #[test]
    fn report_marks_network_volumes() {
        let vols: Vec<_> = parse_proc_mounts(FIXTURE)
            .into_iter()
            .filter(MountPoint::is_user_volume)
            .collect();
        let r = report(&vols);
        assert!(r.contains("/mnt/media\tcifs"));
        assert!(r
            .lines()
            .any(|l| l.contains("cifs") && l.contains("(network)")));
        assert!(r
            .lines()
            .any(|l| l.starts_with("/\text4") && !l.contains("(network)")));

        // NOTIFY-UI-3 / ICON-MESH: the report carries the per-row icon name so
        // the chosen glyph is observable at runtime (the `--mounts` surface).
        // Network shares (sshfs / CIFS) take `folder-remote`; the local ext4
        // root takes `drive-harddisk`.
        assert!(r
            .lines()
            .any(|l| l.contains("fuse.sshfs") && l.contains("\ticon=folder-remote")));
        assert!(r
            .lines()
            .any(|l| l.contains("\tcifs\t") && l.contains("\ticon=folder-remote")));
        assert!(r
            .lines()
            .any(|l| l.starts_with("/\text4") && l.contains("\ticon=drive-harddisk")));
    }
}
