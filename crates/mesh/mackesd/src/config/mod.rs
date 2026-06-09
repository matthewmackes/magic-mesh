//! HYP-8.5 — operator-edited configuration files mded reads at
//! startup.
//!
//! Each submodule owns one config-file family:
//!
//! - [`tag_manifest`] — `~/.config/mde/tags/<name>.toml` per-tag
//!   compositor + UX policy. Source of truth for HYP-9 / HYP-10 /
//!   HYP-11 / HYP-12 / HYP-14 / HYP-22 + the Portal-* tag-aware
//!   features.
//! - [`daemon`] — `/etc/mackesd/mackesd.toml` system daemon config
//!   (E1.3 #3). The root-owned, per-deployment-role cadence knobs
//!   mackesd reads once at worker-section startup.
//!
//! Future submodules (per the v6.5 roadmap) will sit alongside
//! these rather than scattered across the workers tree.

pub mod daemon;
pub mod tag_manifest;

pub use daemon::{load as load_daemon_config, MackesdConfig};
pub use tag_manifest::{
    default_manifests_dir, load_all as load_tag_manifests, parse_file as parse_tag_manifest,
    system_manifests_dir, LoadError as TagManifestLoadError, TagManifest,
};
