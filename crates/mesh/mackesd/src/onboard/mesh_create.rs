//! OW-3 — `mackesd onboard mesh-create`: found this Workstation's mesh.
//!
//! One command turns a lone box into a working **mesh-of-one, fully offline**: a
//! LAN-only Nebula overlay with its own minted CA and no lighthouse / no internet
//! dependency. It is the founding verb the egui first-run wizard and the headless
//! TUI both drive before there is anything to invite or enroll.
//!
//! The shape mirrors the sibling OW-2 verbs ([`crate::onboard::self_test`] /
//! [`crate::onboard::role_provision`]): an impure probe seam ([`gather_existing`])
//! reads whether this node has already founded a mesh, a pure decision
//! ([`decide`], over pure [`mint_mesh_id`]) turns that probe + the optional label
//! into a [`Decision`], and the thin impure shell ([`create`]) folds a `Mint`
//! decision through the **existing** ENT-4 bootstrap
//! ([`crate::mesh_init::mesh_init`]) rather than re-deriving any crypto. The pure
//! parts (id minting, the idempotency decision, the report body) are what the unit
//! tests pin; the CA-mint / overlay-up is `mesh_init`'s job, exercised end-to-end
//! through its injectable [`crate::ca::NebulaCertBackend`].
//!
//! # Reuse, not reimplementation (§6)
//! `mesh_init` already mints the Nebula CA (NF-7), self-signs the founding peer
//! cert as `10.42.0.1`, writes the bundle the nebula supervisor materializes
//! `/etc/nebula` from, and mints the first join bearer. This verb adds only the
//! onboard-facing skin: a generated **mesh-id** (a founder needs no
//! operator-chosen id), the **Workstation** role pin (this is a desktop, not a
//! lighthouse), and **idempotency** (a box that already holds a CA is left
//! untouched). The "lone WS, offline" case is expressed purely through
//! `mesh_init`'s parameters — the `external_addr` the caller resolves best-effort
//! (a LAN/underlay address, loopback when truly offline) and the `Workstation`
//! pin role — never a copy of its body.
//!
//! # Out of scope (later units — deliberately not built here, §7)
//! Issuing an invite (OW-4), enrolling / joining a second box (OW-5), and wiring
//! mesh-DNS + the real underlay network (OW-6) each land in their own unit. The
//! egui shell that *renders* the created mesh (`mde-mesh-view`) is a downstream
//! consumer of this verb's output, not part of it.

use std::path::{Path, PathBuf};

use mde_role::Role;

use crate::ca::NebulaCertBackend;

/// A mesh this node has already founded — the fact [`gather_existing`] reads off
/// the store + founding bundle, and the seam the pure [`decide`] keys its no-op
/// off. A founder is identified by holding its **own** minted CA (a node that
/// merely *joined* someone else's mesh holds a bundle but no CA, and mesh-create
/// on such a box would still be a fresh founding — which is a separate concern the
/// enroll verb owns, not this one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingMesh {
    /// The active CA's mesh identifier already on this node.
    pub mesh_id: String,
    /// This node's overlay IP, best-effort from the founding bundle (`None` when
    /// the bundle is absent/unreadable — the CA row is the load-bearing signal).
    pub overlay_ip: Option<String>,
}

/// The pure idempotency decision: create a fresh mesh, or no-op because one is
/// already founded here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// This node already founded a mesh — creating again is a safe no-op that
    /// reports (never clobbers) the [`ExistingMesh`].
    NoOp(ExistingMesh),
    /// No mesh yet — mint this fresh id (with the optional cosmetic label) and
    /// bootstrap the overlay through [`crate::mesh_init::mesh_init`].
    Mint {
        /// The freshly minted, URL-safe mesh identifier.
        mesh_id: String,
        /// The operator's cosmetic label, carried into the report.
        label: Option<String>,
    },
}

/// What `mesh-create` accomplished — the headless body both front-ends read and
/// the CLI prints (as JSON with `--json`, else as text).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MeshCreateReport {
    /// The mesh identifier — freshly minted, or the existing one on a no-op.
    pub mesh_id: String,
    /// The cosmetic label folded into a freshly minted id (`None` on a no-op).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `true` when this call minted the mesh; `false` on an idempotent no-op
    /// (a mesh was already founded on this node).
    pub created: bool,
    /// This node's overlay IP (`10.42.0.1` for the founding node).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay_ip: Option<String>,
    /// The founding bundle the nebula supervisor materializes from (only when this
    /// call wrote it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_path: Option<PathBuf>,
}

impl MeshCreateReport {
    /// The freshly-founded report, from [`crate::mesh_init::mesh_init`]'s result.
    #[must_use]
    fn founded(init: crate::mesh_init::MeshInitReport, label: Option<String>) -> Self {
        Self {
            mesh_id: init.mesh_id,
            label,
            created: true,
            overlay_ip: Some(init.overlay_ip),
            bundle_path: Some(init.bundle_path),
        }
    }

    /// The idempotent no-op report, from the [`ExistingMesh`] the probe read.
    #[must_use]
    fn already_founded(existing: ExistingMesh) -> Self {
        Self {
            mesh_id: existing.mesh_id,
            label: None,
            created: false,
            overlay_ip: existing.overlay_ip,
            bundle_path: None,
        }
    }

    /// The single-line human report (newline-terminated).
    #[must_use]
    pub fn human(&self) -> String {
        let overlay = self.overlay_ip.as_deref().unwrap_or("-");
        if self.created {
            let label = self
                .label
                .as_deref()
                .map(|l| format!(" ({l})"))
                .unwrap_or_default();
            let bundle = self
                .bundle_path
                .as_ref()
                .map_or_else(|| "-".to_string(), |p| p.display().to_string());
            format!(
                "mesh-create: founded mesh `{}`{label} — overlay {overlay}, bundle {bundle}\n",
                self.mesh_id
            )
        } else {
            format!(
                "mesh-create: mesh `{}` already founded on this node — no-op (overlay {overlay})\n",
                self.mesh_id
            )
        }
    }
}

/// Lower-hex encode `bytes` — the mesh-id's uniqueness suffix.
fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Slugify a friendly label into the URL-safe stem of a mesh-id: lowercase ASCII
/// alphanumerics kept, every other run collapsed to a single `-`, no leading /
/// trailing `-`, capped at 24 chars. Non-ASCII / punctuation-only labels collapse
/// to the empty string (the caller falls back to the default stem).
fn slugify(label: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.truncate(24);
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Mint a stable, unique, URL-safe mesh-id from an optional cosmetic `label` and
/// caller-supplied `entropy`.
///
/// Pure + deterministic given its inputs (the impure [`create`] shell draws the
/// `entropy` from the OS CSPRNG): the id is `<stem>-<hex(entropy)>`, where `stem`
/// is the label's [`slugify`]'d form or `mesh` when there is no usable label. The
/// hex suffix makes distinct meshes distinct even under the same label; the whole
/// id satisfies the enroll join-token URL-safety rule (ASCII alphanumerics plus
/// `.` `_` `-`), so it flows unescaped through the token `mesh_init` mints.
#[must_use]
pub fn mint_mesh_id(label: Option<&str>, entropy: &[u8]) -> String {
    let stem = label.map(slugify).filter(|s| !s.is_empty());
    let stem = stem.as_deref().unwrap_or("mesh");
    let suffix = to_hex(entropy);
    if suffix.is_empty() {
        // Degenerate (no entropy): keep the id well-formed rather than trailing a
        // bare `-`. The shell always supplies entropy, so this is a guard only.
        stem.to_string()
    } else {
        format!("{stem}-{suffix}")
    }
}

/// The pure idempotency decision: no-op on an already-founded node, else mint.
///
/// A `Some(existing)` probe is reported verbatim and never clobbered — the label
/// and entropy are ignored, so re-running mesh-create is inert. A `None` probe
/// mints a fresh id from `label` + `entropy`. Side-effect-free; this is the tested
/// core that [`create`] folds the overlay bootstrap onto.
#[must_use]
pub fn decide(existing: Option<ExistingMesh>, label: Option<&str>, entropy: &[u8]) -> Decision {
    match existing {
        Some(e) => Decision::NoOp(e),
        None => Decision::Mint {
            mesh_id: mint_mesh_id(label, entropy),
            label: label.map(str::to_string),
        },
    }
}

/// Impure probe: has this node already founded a mesh?
///
/// A founder holds its own minted CA, so the load-bearing signal is an active
/// (un-retired) `nebula_ca` row; the overlay IP is a best-effort read of the
/// founding bundle (absent bundle ⇒ `None`, the CA row still stands). Read-only:
/// an unprovisioned node reads as `None` with no side effects.
#[must_use]
pub fn gather_existing(
    conn: &rusqlite::Connection,
    workgroup_root: &Path,
    node_id: &str,
) -> Option<ExistingMesh> {
    let mesh_id = active_ca_mesh_id(conn)?;
    let overlay_ip =
        crate::ca::bundle::read_bundle(&crate::ca::bundle::bundle_path(workgroup_root, node_id))
            .ok()
            .map(|b| b.overlay_ip);
    Some(ExistingMesh {
        mesh_id,
        overlay_ip,
    })
}

/// The most-recent active CA's mesh-id, or `None` when this node holds no CA (or
/// the store cannot be queried — a founder-less box reads as "no mesh").
fn active_ca_mesh_id(conn: &rusqlite::Connection) -> Option<String> {
    conn.query_row(
        "SELECT mesh_id FROM nebula_ca WHERE retired_at IS NULL ORDER BY epoch DESC LIMIT 1",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Found this Workstation's mesh — the thin impure shell.
///
/// Probes for an already-founded mesh ([`gather_existing`]); a [`Decision::NoOp`]
/// returns the idempotent report untouched. A [`Decision::Mint`] folds the fresh
/// id through [`crate::mesh_init::mesh_init`] pinned to [`Role::Workstation`] (the
/// founding desktop is not a lighthouse) with the caller's LAN-only
/// `external_addr` — reusing every bit of the CA-mint / self-sign / bundle-write /
/// bearer-mint machinery rather than duplicating it.
///
/// # Errors
/// Propagates `mesh_init`'s honest per-step failures (role pin, CA mint,
/// self-sign, bundle write, bearer mint).
#[allow(clippy::too_many_arguments)]
pub fn create<B: NebulaCertBackend>(
    backend: &B,
    conn: &rusqlite::Connection,
    workgroup_root: &Path,
    node_id: &str,
    ca_crt: &Path,
    ca_key: &Path,
    scratch_dir: &Path,
    external_addr: &str,
    label: Option<&str>,
) -> anyhow::Result<MeshCreateReport> {
    let existing = gather_existing(conn, workgroup_root, node_id);

    // Draw the mesh-id uniqueness entropy from the OS CSPRNG (the impure bit;
    // mirrors bearer_ledger / passcode). The pure `decide` folds it into the id.
    let mut entropy = [0u8; 4];
    {
        use rand::RngCore;
        rand::rngs::OsRng.fill_bytes(&mut entropy);
    }

    match decide(existing, label, &entropy) {
        Decision::NoOp(existing) => Ok(MeshCreateReport::already_founded(existing)),
        Decision::Mint { mesh_id, label } => {
            let init = crate::mesh_init::mesh_init(
                backend,
                conn,
                workgroup_root,
                node_id,
                &mesh_id,
                external_addr,
                ca_crt,
                ca_key,
                scratch_dir,
                Role::Workstation,
            )?;
            Ok(MeshCreateReport::founded(init, label))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The enroll join-token URL-safety rule (mirrors `nebula_enroll`'s
    /// `is_mesh_id_url_safe`) — a minted id must satisfy it or the token
    /// `mesh_init` builds around it is unparseable.
    fn is_url_safe(s: &str) -> bool {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    }

    #[test]
    fn mint_is_deterministic_and_url_safe() {
        let e = [0xde, 0xad, 0xbe, 0xef];
        let a = mint_mesh_id(Some("home"), &e);
        let b = mint_mesh_id(Some("home"), &e);
        assert_eq!(a, b, "same label + entropy ⇒ same id (stable)");
        assert_eq!(a, "home-deadbeef");
        assert!(is_url_safe(&a));
    }

    #[test]
    fn mint_slugifies_a_messy_label_and_keeps_the_hex_suffix() {
        let id = mint_mesh_id(Some("Matthew's Home Lab!"), &[0x01, 0x02]);
        // Every non-alphanumeric run collapses to a single `-` (so `Matthew's`
        // becomes `matthew-s`); lowercased; hex suffix appended.
        assert_eq!(id, "matthew-s-home-lab-0102");
        assert!(is_url_safe(&id));
        assert!(!id.contains('\''));
        assert!(!id.contains(' '));
    }

    #[test]
    fn mint_defaults_the_stem_without_a_usable_label() {
        assert_eq!(mint_mesh_id(None, &[0xaa]), "mesh-aa");
        // A punctuation-only label has no usable slug ⇒ default stem.
        assert_eq!(mint_mesh_id(Some("!!!"), &[0xaa]), "mesh-aa");
    }

    #[test]
    fn mint_is_unique_per_entropy() {
        let a = mint_mesh_id(Some("lab"), &[0x00, 0x01]);
        let b = mint_mesh_id(Some("lab"), &[0x00, 0x02]);
        assert_ne!(a, b, "distinct entropy ⇒ distinct id under the same label");
    }

    #[test]
    fn slugify_caps_length_without_a_trailing_dash() {
        let s = slugify("a very long label that keeps on going and going forever");
        assert!(s.len() <= 24);
        assert!(!s.ends_with('-'), "cap must not leave a dangling dash");
        assert!(is_url_safe(&s));
    }

    #[test]
    fn decide_mints_when_no_mesh_exists() {
        let d = decide(None, Some("home"), &[0xab, 0xcd]);
        match d {
            Decision::Mint { mesh_id, label } => {
                assert_eq!(mesh_id, "home-abcd");
                assert_eq!(label.as_deref(), Some("home"));
                assert!(is_url_safe(&mesh_id));
            }
            Decision::NoOp(_) => panic!("expected Mint on a founder-less node"),
        }
    }

    #[test]
    fn decide_is_an_inert_noop_when_already_founded() {
        // An existing founder is reported verbatim; the label + entropy are
        // ignored, so re-running never mints or clobbers.
        let existing = ExistingMesh {
            mesh_id: "home-cafef00d".to_string(),
            overlay_ip: Some("10.42.0.1".to_string()),
        };
        let d = decide(Some(existing.clone()), Some("ignored"), &[0xff, 0xff]);
        assert_eq!(d, Decision::NoOp(existing));
    }

    #[test]
    fn report_renders_the_founded_verdict() {
        let init = crate::mesh_init::MeshInitReport {
            mesh_id: "home-deadbeef".to_string(),
            overlay_ip: "10.42.0.1".to_string(),
            bundle_path: PathBuf::from("/var/lib/mackesd/bundle.json"),
            join_token: "mesh:home-deadbeef@10.42.0.1:4242#tok".to_string(),
            pinned_role: Some("workstation".to_string()),
        };
        let r = MeshCreateReport::founded(init, Some("home".to_string()));
        assert!(r.created);
        assert_eq!(r.overlay_ip.as_deref(), Some("10.42.0.1"));
        let human = r.human();
        assert!(human.contains("founded mesh `home-deadbeef` (home)"));
        assert!(human.contains("overlay 10.42.0.1"));
        assert!(human.ends_with('\n'));
        // JSON carries the verdict + id.
        let json = serde_json::to_value(&r).expect("serialize");
        assert_eq!(json["created"], true);
        assert_eq!(json["mesh_id"], "home-deadbeef");
        assert_eq!(json["label"], "home");
    }

    #[test]
    fn report_renders_the_idempotent_noop_verdict() {
        let r = MeshCreateReport::already_founded(ExistingMesh {
            mesh_id: "home-cafef00d".to_string(),
            overlay_ip: Some("10.42.0.1".to_string()),
        });
        assert!(!r.created);
        assert!(r.bundle_path.is_none());
        let human = r.human();
        assert!(human.contains("already founded on this node — no-op"));
        assert!(human.contains("home-cafef00d"));
        let json = serde_json::to_value(&r).expect("serialize");
        assert_eq!(json["created"], false);
        // A no-op omits the cosmetic label + never-written bundle path.
        assert!(json.get("label").is_none());
        assert!(json.get("bundle_path").is_none());
    }

    /// Serialize the tests that mutate the process-wide `MDE_ROLE_PATH` env (the
    /// role pin `mesh_init` performs). Mirrors the `ENV_LOCK` in `mesh_init` /
    /// `enrollment.rs`.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn create_founds_then_no_ops_through_mesh_init() {
        use crate::ca::MockBackend;
        let _g = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().unwrap();
        // Hermetic role pin (never touch the privileged system path).
        std::env::set_var("MDE_ROLE_PATH", tmp.path().join("role.toml"));
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let ca_dir = tmp.path().join("nebula-ca");
        let node_id = "peer:founder";

        // First call founds the mesh-of-one through mesh_init (Workstation pin,
        // LAN-only external addr) — created, with a URL-safe minted id + a bundle.
        let first = create(
            &MockBackend,
            &conn,
            tmp.path(),
            node_id,
            &ca_dir.join("ca.crt"),
            &ca_dir.join("ca.key"),
            &ca_dir.join("scratch"),
            "192.168.1.50:4242",
            Some("home"),
        )
        .expect("first mesh-create founds the mesh");
        assert!(first.created, "a founder-less node is founded");
        assert!(first.mesh_id.starts_with("home-"));
        assert!(is_url_safe(&first.mesh_id));
        assert_eq!(first.overlay_ip.as_deref(), Some("10.42.0.1"));
        // The founding bundle mesh_init wrote parses + carries the same id.
        let bundle = crate::ca::bundle::read_bundle(first.bundle_path.as_ref().unwrap()).unwrap();
        assert_eq!(bundle.mesh_id, first.mesh_id);
        // The founder holds a minted CA (the idempotency signal).
        assert!(gather_existing(&conn, tmp.path(), node_id).is_some());

        // Second call detects the existing CA and is a safe no-op — same mesh_id,
        // no new bundle, id NOT re-minted.
        let second = create(
            &MockBackend,
            &conn,
            tmp.path(),
            node_id,
            &ca_dir.join("ca.crt"),
            &ca_dir.join("ca.key"),
            &ca_dir.join("scratch"),
            "192.168.1.50:4242",
            Some("relabel-ignored"),
        )
        .expect("second mesh-create is an idempotent no-op");
        assert!(!second.created, "an already-founded node is a no-op");
        assert_eq!(
            second.mesh_id, first.mesh_id,
            "the existing id is preserved"
        );
        assert!(second.bundle_path.is_none());
    }
}
