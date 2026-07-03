//! FILEMGR-11 — the render-agnostic state of the operation dialogs.
//!
//! Three of the four FILEMGR-11 dialogs keep their decision state here, with no
//! egui in it (the fourth — the interactive conflict dialog — folds its state in
//! [`crate::ops`], where the FILEMGR-2 channel resolver lives). Everything
//! decision-shaped is a plain data fold so it can be unit-tested without a GPU:
//!
//! * [`Perms`] — the permission bits behind the Properties dialog's rwx grid,
//!   kept in exact lock-step with the octal value (grid toggle ↔ octal edit).
//! * [`PropertiesDialog`] — reads a path's metadata through the FILEMGR-1
//!   [`FileOps`] seam, offers the rwx grid + octal + owner/group, and applies a
//!   real `chmod`/`chown` back through the same seam. The chown control is
//!   offered **only when permitted** (`chown_permitted`), honestly disabled
//!   otherwise (§7 — never a faked success).
//! * [`ConfirmDelete`] + [`Arming`] — the permanent-delete confirm (lock 3/6 —
//!   no trash, no undo), with **typed-arming** (lock 19) layered on when the
//!   deletion targets a remote / escalated mesh mount: the user must type the
//!   node name to arm, mirroring the storage plane's typed-arming echo.
//!
//! Reuse (§6): the Properties dialog never re-derives a permission read or a
//! chmod/chown — it drives the shipped FILEMGR-1 `FileOps` (`metadata` /
//! `set_permissions` / `chown`), whose [`FakeFileOps`](mde_files::fileops::FakeFileOps)
//! double makes the whole load → edit → apply round-trip testable with zero disk
//! I/O and a deterministic privilege model.

use std::io;
use std::path::PathBuf;

use mde_files::fileops::FileOps;

// ═══════════════════════════════════════════════════════════════════════════
// Permission bits ↔ the rwx grid ↔ the octal value.
// ═══════════════════════════════════════════════════════════════════════════

/// Which owner class a permission bit belongs to (the grid's three rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermClass {
    /// The owning user (`u`).
    Owner,
    /// The owning group (`g`).
    Group,
    /// Everyone else (`o`).
    Other,
}

impl PermClass {
    /// The three classes, in grid-row order.
    pub const ALL: [Self; 3] = [Self::Owner, Self::Group, Self::Other];

    /// How far this class's rwx triad is shifted in the mode word.
    const fn shift(self) -> u32 {
        match self {
            Self::Owner => 6,
            Self::Group => 3,
            Self::Other => 0,
        }
    }

    /// The row label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Owner => "Owner",
            Self::Group => "Group",
            Self::Other => "Other",
        }
    }
}

/// Which permission a grid cell toggles (the grid's three columns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perm {
    /// Read (`r`, bit `4`).
    Read,
    /// Write (`w`, bit `2`).
    Write,
    /// Execute / search (`x`, bit `1`).
    Exec,
}

impl Perm {
    /// The three permissions, in grid-column order.
    pub const ALL: [Self; 3] = [Self::Read, Self::Write, Self::Exec];

    /// The triad bit value (before the class shift).
    const fn bit(self) -> u32 {
        match self {
            Self::Read => 0o4,
            Self::Write => 0o2,
            Self::Exec => 0o1,
        }
    }

    /// The single-letter symbolic glyph.
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Read => "r",
            Self::Write => "w",
            Self::Exec => "x",
        }
    }
}

/// The permission + special bits of a mode (`mode & 0o7777`).
///
/// The Properties dialog edits these through the rwx grid and the octal field;
/// the two views are always the same underlying value (the nine rwx bits plus
/// `setuid`/`setgid`/`sticky`), so there is no separate "octal state" to drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Perms(u32);

impl Perms {
    /// Wrap a raw mode, masking off the file-type bits (`S_IFMT`) so only the
    /// permission + special bits remain.
    #[must_use]
    pub const fn from_mode(mode: u32) -> Self {
        Self(mode & 0o7777)
    }

    /// The raw mode (`& 0o7777`) to hand to `chmod`.
    #[must_use]
    pub const fn mode(self) -> u32 {
        self.0
    }

    /// `true` when `class` has `perm` set — drives a grid cell's checked state.
    #[must_use]
    pub const fn get(self, class: PermClass, perm: Perm) -> bool {
        self.0 & (perm.bit() << class.shift()) != 0
    }

    /// Flip `class`'s `perm` bit (a grid-cell click). The octal view updates for
    /// free because it reads the same word.
    pub fn toggle(&mut self, class: PermClass, perm: Perm) {
        self.0 ^= perm.bit() << class.shift();
    }

    /// The four-digit octal string (`"0644"`, `"4755"`) — the special-bit digit
    /// first, so a `setuid`/`setgid`/`sticky` bit is honestly visible even though
    /// the 3×3 grid only covers rwx.
    #[must_use]
    pub fn octal(self) -> String {
        format!("{:04o}", self.0)
    }

    /// The nine-character symbolic string (`"rwxr-xr-x"`) for the summary line.
    #[must_use]
    pub fn symbolic(self) -> String {
        let mut s = String::with_capacity(9);
        for class in PermClass::ALL {
            for perm in Perm::ALL {
                s.push_str(if self.get(class, perm) {
                    perm.glyph()
                } else {
                    "-"
                });
            }
        }
        s
    }

    /// Parse an octal string typed into the field (`"644"`, `"0755"`, `"4755"`);
    /// on a valid `0..=0o7777` value it replaces the bits and returns `true`. An
    /// unparseable or out-of-range entry leaves the bits untouched and returns
    /// `false`, so the grid never shows a value the user didn't actually enter.
    pub fn set_octal(&mut self, text: &str) -> bool {
        match u32::from_str_radix(text.trim(), 8) {
            Ok(v) if v <= 0o7777 => {
                self.0 = v;
                true
            }
            _ => false,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The Properties dialog.
// ═══════════════════════════════════════════════════════════════════════════

/// The Properties / permissions dialog's live state (lock 8).
///
/// Loaded from a path's metadata through the FILEMGR-1 [`FileOps`] seam; edited
/// via the rwx grid + octal field + owner/group entries; applied back as a real
/// `chmod` / `chown` through the same seam.
pub struct PropertiesDialog {
    /// The path whose properties are shown + edited.
    pub path: PathBuf,
    /// The display name (the listing row's name).
    pub name: String,
    /// Whether the entry is a directory (worded in the header).
    pub is_dir: bool,
    /// Size in bytes (display only).
    pub size: u64,
    /// The editable permission bits (grid ↔ octal, always in sync).
    pub perms: Perms,
    /// The octal text field's buffer. Kept as the user typed it (so an
    /// intermediate `"7"` doesn't snap) while [`perms`](Self::perms) tracks the
    /// last *valid* parse; a grid toggle rewrites it to the canonical form.
    pub octal_edit: String,
    /// The permission bits as loaded — so Apply only `chmod`s on a real change.
    original_mode: u32,
    /// The current owning uid (loaded).
    pub uid: u32,
    /// The current owning gid (loaded).
    pub gid: u32,
    /// The editable owner uid text (only meaningful when `chown_permitted`).
    pub uid_edit: String,
    /// The editable owner gid text.
    pub gid_edit: String,
    /// Whether this caller may `chown` (root / `CAP_CHOWN`). When `false` the
    /// owner/group fields render read-only — honestly disabled, never faked.
    pub chown_permitted: bool,
    /// The result of the most recent Apply: `Ok` on success, `Err(reason)` on an
    /// honest typed failure (an unprivileged chown, a bad uid, …). `None` before
    /// the first Apply.
    pub outcome: Option<Result<(), String>>,
}

impl PropertiesDialog {
    /// Read `path`'s metadata through `ops` and build the dialog. Follows
    /// symlinks (the `stat(2)` shape) so the properties describe the real target.
    ///
    /// # Errors
    /// The [`FileOps::metadata`] error (a broken symlink, a vanished path) — the
    /// caller surfaces it as an honest note and does not open the dialog.
    pub fn load(
        ops: &dyn FileOps,
        path: PathBuf,
        name: String,
        chown_permitted: bool,
    ) -> io::Result<Self> {
        let st = ops.metadata(&path)?;
        let perms = Perms::from_mode(st.mode);
        Ok(Self {
            path,
            name,
            is_dir: st.is_dir,
            size: st.len,
            perms,
            octal_edit: perms.octal(),
            original_mode: st.mode & 0o7777,
            uid: st.uid,
            gid: st.gid,
            uid_edit: st.uid.to_string(),
            gid_edit: st.gid.to_string(),
            chown_permitted,
            outcome: None,
        })
    }

    /// Flip one rwx grid cell and re-sync the octal field to the canonical
    /// four-digit form — the two views never drift.
    pub fn toggle_perm(&mut self, class: PermClass, perm: Perm) {
        self.perms.toggle(class, perm);
        self.octal_edit = self.perms.octal();
    }

    /// Take a keystroke in the octal field: remember exactly what was typed, and
    /// move the grid to it when (and only when) it parses to a valid mode. An
    /// invalid entry leaves the grid on the last good value (the view flags it).
    pub fn set_octal_edit(&mut self, text: String) {
        self.perms.set_octal(&text);
        self.octal_edit = text;
    }

    /// `true` when the octal buffer doesn't parse to the grid's current bits —
    /// the view shows an honest "invalid" hint rather than silently ignoring it.
    #[must_use]
    pub fn octal_is_valid(&self) -> bool {
        u32::from_str_radix(self.octal_edit.trim(), 8).is_ok_and(|v| v <= 0o7777)
    }

    /// `true` when the permission bits differ from what was loaded (drives the
    /// Apply button's enabled state alongside a pending chown).
    #[must_use]
    pub fn perms_changed(&self) -> bool {
        self.perms.mode() != self.original_mode
    }

    /// The parsed owner change to apply, if the caller may chown and typed a
    /// different, valid `(uid, gid)`. `Ok(None)` = nothing to chown; `Err` = the
    /// caller typed a non-numeric id.
    fn pending_chown(&self) -> Result<Option<(u32, u32)>, String> {
        if !self.chown_permitted {
            return Ok(None);
        }
        let uid = self.uid_edit.trim().parse::<u32>().map_err(|_| {
            format!(
                "Not a numeric user id: \u{201c}{}\u{201d}",
                self.uid_edit.trim()
            )
        })?;
        let gid = self.gid_edit.trim().parse::<u32>().map_err(|_| {
            format!(
                "Not a numeric group id: \u{201c}{}\u{201d}",
                self.gid_edit.trim()
            )
        })?;
        if uid == self.uid && gid == self.gid {
            Ok(None)
        } else {
            Ok(Some((uid, gid)))
        }
    }

    /// `true` when Apply would actually change something (a permission edit or a
    /// valid owner change) — so the button is honestly inert with nothing to do.
    #[must_use]
    pub fn can_apply(&self) -> bool {
        self.perms_changed() || matches!(self.pending_chown(), Ok(Some(_)))
    }

    /// Apply the pending permission + owner changes through `ops` as a real
    /// `chmod` / `chown`, then re-stat so the dialog reflects exactly what took
    /// (an unprivileged chown fails honestly and leaves the loaded owner shown).
    /// The result is recorded in [`outcome`](Self::outcome).
    pub fn apply(&mut self, ops: &dyn FileOps) {
        self.outcome = Some(self.apply_inner(ops));
        // Re-read the truth so a partial success (chmod took, chown denied) shows
        // the real on-disk state rather than the optimistic edit.
        if let Ok(st) = ops.metadata(&self.path) {
            self.perms = Perms::from_mode(st.mode);
            self.octal_edit = self.perms.octal();
            self.original_mode = st.mode & 0o7777;
            self.uid = st.uid;
            self.gid = st.gid;
            self.uid_edit = st.uid.to_string();
            self.gid_edit = st.gid.to_string();
        }
    }

    fn apply_inner(&self, ops: &dyn FileOps) -> Result<(), String> {
        // A bad uid/gid is caught before any mutation, so a typo never half-
        // applies.
        let chown = self.pending_chown()?;
        if self.perms_changed() {
            ops.set_permissions(&self.path, self.perms.mode())
                .map_err(|e| format!("chmod failed: {e}"))?;
        }
        if let Some((uid, gid)) = chown {
            ops.chown(&self.path, Some(uid), Some(gid))
                .map_err(|e| format!("chown failed: {e}"))?;
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The confirm-delete dialog + typed-arming.
// ═══════════════════════════════════════════════════════════════════════════

/// The extra typed-arming a destructive op on a **remote / escalated** mesh mount
/// demands (lock 19).
///
/// The user must type [`node`](Self::node) verbatim to arm the confirm — the same
/// "echo the target to prove intent" gate the storage plane uses before it writes
/// a disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arming {
    /// The mesh node the deletion lands on — the string the user must type.
    pub node: String,
    /// `true` when the mount is escalated to the full filesystem (`/`), so the
    /// confirm can say so (lock 19 — "any op under an escalated full-fs mount").
    pub full_fs: bool,
    /// A representative target path on the node, named in the confirm.
    pub path: String,
}

/// The permanent-delete confirm (lock 3/6 — no trash, no undo).
///
/// Names the items and, when they live on a remote / escalated mount, layers
/// [`Arming`] on top so the delete can't fire until the node name is typed.
pub struct ConfirmDelete {
    /// The paths that will be permanently removed.
    pub targets: Vec<PathBuf>,
    /// Their display names (the summary the confirm shows).
    pub names: Vec<String>,
    /// The typed-arming challenge, present only for a remote / escalated target.
    pub arming: Option<Arming>,
    /// The user's typed-arming echo (matched against [`Arming::node`]).
    pub echo: String,
}

impl ConfirmDelete {
    /// Build the confirm for `targets` (with their `names`) and an optional
    /// [`Arming`] challenge (present when the deletion touches a remote /
    /// escalated mount).
    #[must_use]
    pub fn new(targets: Vec<PathBuf>, names: Vec<String>, arming: Option<Arming>) -> Self {
        Self {
            targets,
            names,
            arming,
            echo: String::new(),
        }
    }

    /// How many items the delete removes.
    #[must_use]
    pub fn count(&self) -> usize {
        self.targets.len()
    }

    /// `true` once the delete may proceed: a local delete is always armed (the
    /// confirm itself is the safeguard); a remote / escalated delete needs the
    /// typed node echo to match exactly.
    #[must_use]
    pub fn armed(&self) -> bool {
        self.arming
            .as_ref()
            .is_none_or(|a| self.echo.trim() == a.node && !a.node.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_files::fileops::FakeFileOps;
    use std::path::Path;

    // ── Perms: grid ↔ octal stay in lock-step ────────────────────────────────

    #[test]
    fn octal_and_grid_are_the_same_value() {
        let p = Perms::from_mode(0o644);
        assert_eq!(p.octal(), "0644");
        assert_eq!(p.symbolic(), "rw-r--r--");
        // Owner r+w, everyone else r only.
        assert!(p.get(PermClass::Owner, Perm::Read));
        assert!(p.get(PermClass::Owner, Perm::Write));
        assert!(!p.get(PermClass::Owner, Perm::Exec));
        assert!(!p.get(PermClass::Group, Perm::Write));
        assert!(p.get(PermClass::Other, Perm::Read));
    }

    #[test]
    fn toggling_a_grid_cell_moves_the_octal() {
        let mut p = Perms::from_mode(0o644);
        // Add owner-exec → 0o744.
        p.toggle(PermClass::Owner, Perm::Exec);
        assert_eq!(p.octal(), "0744");
        assert!(p.get(PermClass::Owner, Perm::Exec));
        // Add group-write + other-write → 0o766.
        p.toggle(PermClass::Group, Perm::Write);
        p.toggle(PermClass::Other, Perm::Write);
        assert_eq!(p.octal(), "0766");
        // Toggling the same cell off returns it.
        p.toggle(PermClass::Owner, Perm::Exec);
        assert_eq!(p.octal(), "0666");
    }

    #[test]
    fn typing_octal_moves_the_grid_and_rejects_garbage() {
        let mut p = Perms::from_mode(0o644);
        assert!(p.set_octal("755"), "a bare 3-digit octal parses");
        assert_eq!(p.octal(), "0755");
        assert!(p.get(PermClass::Owner, Perm::Exec));
        assert!(!p.get(PermClass::Group, Perm::Write));
        // A special-bit value round-trips (setuid).
        assert!(p.set_octal("4755"));
        assert_eq!(p.octal(), "4755");
        // Garbage / out-of-range is rejected and leaves the value untouched.
        assert!(!p.set_octal("8a9"));
        assert!(!p.set_octal("10000"));
        assert_eq!(p.octal(), "4755", "a rejected entry never mutates the grid");
    }

    #[test]
    fn the_octal_field_buffers_intermediate_typing_and_flags_invalid() {
        let fs = seeded_fake(false);
        let mut dlg = PropertiesDialog::load(
            &fs,
            PathBuf::from("/d/report.txt"),
            "report.txt".into(),
            false,
        )
        .expect("loads");
        // A partial, not-yet-valid entry is kept verbatim; the grid holds.
        dlg.set_octal_edit("7".into());
        assert!(dlg.octal_is_valid(), "\"7\" is a valid (if small) mode");
        assert_eq!(dlg.perms.octal(), "0007");
        // Non-octal is flagged, and the grid does not move to it.
        dlg.set_octal_edit("zzz".into());
        assert!(!dlg.octal_is_valid());
        assert_eq!(
            dlg.perms.octal(),
            "0007",
            "an invalid entry never moves the grid"
        );
    }

    // ── PropertiesDialog: real chmod / chown through FileOps ─────────────────

    fn seeded_fake(privileged: bool) -> FakeFileOps {
        let fs = if privileged {
            FakeFileOps::privileged()
        } else {
            FakeFileOps::new()
        };
        fs.create_dir(Path::new("/d")).expect("mkdir");
        fs.seed_file("/d/report.txt", b"hello").expect("seed");
        fs.set_permissions(Path::new("/d/report.txt"), 0o644)
            .expect("seed mode");
        fs
    }

    #[test]
    fn properties_loads_metadata_through_the_fileops_seam() {
        let fs = seeded_fake(false);
        let dlg = PropertiesDialog::load(
            &fs,
            PathBuf::from("/d/report.txt"),
            "report.txt".into(),
            false,
        )
        .expect("loads");
        assert_eq!(dlg.perms.octal(), "0644");
        assert_eq!(dlg.size, 5);
        assert!(!dlg.is_dir);
        assert!(!dlg.chown_permitted, "an unprivileged caller can't chown");
        assert!(
            !dlg.can_apply(),
            "a freshly-loaded dialog has nothing to apply"
        );
    }

    #[test]
    fn applying_a_grid_edit_chmods_for_real() {
        let fs = seeded_fake(false);
        let mut dlg = PropertiesDialog::load(
            &fs,
            PathBuf::from("/d/report.txt"),
            "report.txt".into(),
            false,
        )
        .expect("loads");
        // Make it executable by the owner: 0o644 → 0o744.
        dlg.toggle_perm(PermClass::Owner, Perm::Exec);
        assert_eq!(dlg.octal_edit, "0744", "the octal field tracked the grid");
        assert!(dlg.perms_changed() && dlg.can_apply());
        dlg.apply(&fs);
        assert!(matches!(dlg.outcome, Some(Ok(()))), "chmod succeeded");
        // The real on-disk mode changed, and the dialog re-synced to it.
        assert_eq!(
            fs.metadata(Path::new("/d/report.txt")).expect("stat").mode,
            0o744
        );
        assert_eq!(dlg.perms.octal(), "0744");
        assert!(!dlg.perms_changed(), "re-stat cleared the pending change");
    }

    #[test]
    fn chown_is_offered_and_succeeds_only_when_permitted() {
        // Privileged caller: chown to another uid/gid takes.
        let fs = seeded_fake(true);
        let mut dlg = PropertiesDialog::load(
            &fs,
            PathBuf::from("/d/report.txt"),
            "report.txt".into(),
            true,
        )
        .expect("loads");
        dlg.uid_edit = "42".into();
        dlg.gid_edit = "7".into();
        assert!(dlg.can_apply(), "a permitted owner change is applyable");
        dlg.apply(&fs);
        assert!(matches!(dlg.outcome, Some(Ok(()))));
        let st = fs.metadata(Path::new("/d/report.txt")).expect("stat");
        assert_eq!((st.uid, st.gid), (42, 7));
        assert_eq!(dlg.uid, 42, "the dialog re-synced to the new owner");
    }

    #[test]
    fn an_unprivileged_chown_never_fires_even_if_the_text_changes() {
        // chown_permitted=false → the owner edit is ignored entirely (the field
        // is read-only in the view); Apply has nothing to do.
        let fs = seeded_fake(false);
        let mut dlg = PropertiesDialog::load(
            &fs,
            PathBuf::from("/d/report.txt"),
            "report.txt".into(),
            false,
        )
        .expect("loads");
        dlg.uid_edit = "0".into(); // would give the file to root
        assert!(
            !dlg.can_apply(),
            "no chown is offered without the privilege"
        );
        // Even if forced, the honest FileOps chown error would surface — prove the
        // gate holds by checking the owner is untouched after an Apply of nothing.
        dlg.apply(&fs);
        let st = fs.metadata(Path::new("/d/report.txt")).expect("stat");
        assert_ne!(st.uid, 0, "the file was never given away");
    }

    #[test]
    fn a_privileged_chown_to_a_bad_id_is_an_honest_error_not_a_mutation() {
        let fs = seeded_fake(true);
        let mut dlg = PropertiesDialog::load(
            &fs,
            PathBuf::from("/d/report.txt"),
            "report.txt".into(),
            true,
        )
        .expect("loads");
        // Also stage a real chmod so we can prove the bad id aborts before it.
        dlg.toggle_perm(PermClass::Owner, Perm::Exec);
        dlg.uid_edit = "not-a-number".into();
        dlg.apply(&fs);
        assert!(
            matches!(&dlg.outcome, Some(Err(e)) if e.contains("numeric user id")),
            "a non-numeric uid is a typed error"
        );
        // The chmod was NOT applied — a bad field aborts the whole apply.
        assert_eq!(
            fs.metadata(Path::new("/d/report.txt")).expect("stat").mode,
            0o644
        );
    }

    // ── ConfirmDelete + typed-arming ─────────────────────────────────────────

    #[test]
    fn a_local_delete_is_armed_without_typing() {
        let confirm = ConfirmDelete::new(
            vec![PathBuf::from("/home/mac/a.txt")],
            vec!["a.txt".into()],
            None,
        );
        assert_eq!(confirm.count(), 1);
        assert!(confirm.armed(), "a local delete needs no typed echo");
    }

    #[test]
    fn a_remote_delete_arms_only_on_an_exact_node_echo() {
        let mut confirm = ConfirmDelete::new(
            vec![PathBuf::from("/run/user/1000/mde-mesh/oak/docs/a.txt")],
            vec!["a.txt".into()],
            Some(Arming {
                node: "oak".into(),
                full_fs: false,
                path: "/run/user/1000/mde-mesh/oak/docs/a.txt".into(),
            }),
        );
        assert!(!confirm.armed(), "an un-typed remote delete is not armed");
        confirm.echo = "oa".into();
        assert!(!confirm.armed(), "a partial echo does not arm");
        confirm.echo = "  oak ".into();
        assert!(confirm.armed(), "the exact node name (trimmed) arms it");
        confirm.echo = "birch".into();
        assert!(!confirm.armed(), "the wrong node never arms");
    }
}
