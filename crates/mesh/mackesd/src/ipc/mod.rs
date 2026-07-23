//! mackesd's IPC surface — **Mackes Bus responders, not D-Bus
//! services** (EPIC-RETIRE-DBUS / §2: the MDE-private
//! `org.mackes.*` session-bus services this module once served
//! were retired to `mde-bus` `action/<domain>/<verb>` topics;
//! only FDO interop remains anywhere in the tree, and under the
//! E11 pivot Cosmic owns `org.freedesktop.Notifications`).
//!
//! What lives here now:
//! - `directory` / `fleet` / `jobs` / `files` / `nebula` /
//!   `shell` / `settings` — Bus responder + record-shape modules
//!   consumed by `run_serve` and the workers.
//!
//! (The mackesd-side FDO Notifications *server* scaffolding and
//! the `org.mackes.mackesd` well-known-name constant were removed
//! 2026-06-13, AUD6-3/4 — zero production callers.)

#![cfg(feature = "async-services")]
// zbus's #[interface] macro expands to additional dispatch methods
// that don't carry doc comments; the workspace's #[warn(missing_docs)]
// would otherwise flag every one. Silence at the module level so the
// rest of the crate's missing_docs hygiene stays loud.
#![allow(missing_docs)]

// APPS-1 — the apps_aggregator: action/apps/list for the Applications Panel
// launcher (docs/design/apps-launcher.md). Thin applet; this worker is the
// single source of truth (local XDG+flatpak, mesh peers, workloads, services).
/// Shared fail-closed authorization gate for privileged local-Bus mutations.
pub mod action_auth;
pub mod apps;
// CLIP-SYNC-1 — action/clipboard/* responder (list/pin/unpin/delete/clear)
// for the mesh-global clipboard history the clipboard_sync worker maintains.
pub mod clipboard;
// CONNECT-1 — action/connect/* exposure-policy responder.
pub mod connect;
// DATACENTER (action layer) — action/dc/vm-power Xen VM power control responder.
pub mod datacenter;
// DATACENTER-12 (storage action layer) — action/dc/{sr,vdi}-* Xen storage control.
// Pure command-builder + reply layer dispatched into by `datacenter::build_reply`
// (shares the one already-spawned datacenter responder thread).
pub mod storage_ops;
// DATACENTER-16 (action layer) — action/dc/wol Wake-on-LAN power-orchestration
// primitive (broadcasts the magic packet to wake a machine).
pub mod dc_power;
// DATACENTER-10 (action layer) — action/dc/host-power Xen host (dom0)
// maintenance + reboot control responder.
pub mod host_ops;
// DDNS-EGRESS-3 — action/ddns/* config responder.
pub mod ddns;
pub mod directory;
pub mod files;
pub mod fleet;
pub mod jobs;
pub mod route;
// VPN-GW-2 — leader-managed, age-encrypted tunnel-secret distribution over the
// mesh secret store; consumed by vpn_gw on add-tunnel (put) + tunnel-up (get).
pub mod secret_store;
// FILEMGR-6 — the shared mesh SSH key provisioner + sshd overlay bind: generates
// + seals the shared keypair under `mesh-ssh-key` (the ref FILEMGR-5's mesh_mount
// worker reads), installs the public half overlay-only, and owns the re-key path.
pub mod mesh_ssh_key;
// FILEMGR-7 — the peer-side direct-transfer helper: serves `action/mesh-transfer/
// direct` and drives a remote-to-remote rsync A→B over the overlay (reusing the
// FILEMGR-5/6 shared key + `<host>.mesh` DNS + mount scope) so a cross-node copy
// never double-hops through the browsing node. The live ssh/rsync leg is honestly
// gated behind an injectable backend seam (§9/§7); the plan + parsing folds are pure.
pub mod mesh_transfer;
// VPN-GW-1 — action/vpn/* tunnel CRUD + wg-quick/openvpn bring-up responder.
pub mod vpn_gw;
// VPN-GW-6 — tunnel health + exit-IP/leak verification + auto-failover + alerts.
pub mod vpn_health;
// NF-Bundle-0 (v2.5) — dev.mackes.MDE.Nebula.Status surface.
// Foundation that NF-10..NF-18 desktop consumers chain on.
// Reachable from run_serve at boot.
pub mod nebula;
// E4.20 (2026-06-04): the `portal` Bus-publish client (action/shell/<verb>) was
// retired with mde-portal — its only caller (alert_relay's CRITICAL goto) is
// redundant with the notify-send → notifyd → Action Center path.
// DBUS-1 (2026-05-30): the `session` D-Bus interface module was retired
// — the session lifecycle surface migrated to Bus `action/session/<verb>`
// in `crates/mde-session/src/session.rs` (Q96 / EPIC-RETIRE-DBUS). The
// `mackesd` placeholder (never served) + its dead `org.mackes.session`
// name + `/org/mackes/Session` path were removed with it.
pub mod settings;
pub mod shell;
// DC-15 (action layer) — action/dc/tofu-plan read-only OpenTofu plan responder.
pub mod tofu;
pub mod voip;

/// EFF-23 — maximum inbound RPC body size a Bus responder will hand to
/// `serde_json::from_str`. Bodies above this are answered with an error
/// envelope and never parsed, bounding the CPU/allocation an untrusted
/// Bus writer can force with one oversized message. 64 KiB comfortably
/// fits every real action body (selectors, peer lists, a send-to source
/// list for a ≤8-peer mesh); legitimate bulk transfer goes over the
/// Syncthing-replicated volume, not an action body.
pub const MAX_RPC_BODY_BYTES: usize = 64 * 1024;

/// True when `body` is absent or within [`MAX_RPC_BODY_BYTES`]. A
/// responder calls this before parsing; an over-cap body is refused
/// with an error envelope instead of reaching `from_str`.
#[must_use]
pub fn body_within_cap(body: Option<&str>) -> bool {
    body.is_none_or(|b| b.len() <= MAX_RPC_BODY_BYTES)
}

/// EFF-23 — the `{"error": …}` envelope a responder returns when it
/// refuses an over-cap body. Shared so every surface answers the same
/// shape (callers already branch on `error`).
#[must_use]
pub fn body_too_large_reply(verb: &str) -> String {
    serde_json::json!({ "error": format!("{verb}: request body too large") }).to_string()
}

/// Convenience: the canonical object path for each service.
pub mod paths {
    /// `/org/mackes/Shell`
    pub const SHELL: &str = "/org/mackes/Shell";
    /// `/org/mackes/Settings`
    pub const SETTINGS: &str = "/org/mackes/Settings";
    /// `/org/freedesktop/Notifications` — matches the freedesktop
    /// spec so libnotify clients work unchanged.
    pub const NOTIFICATIONS: &str = "/org/freedesktop/Notifications";
    /// `/org/mackes/Fleet`
    pub const FLEET: &str = "/org/mackes/Fleet";
    /// `/dev/mackes/MDE/Portal` — v6.0 Portal-1.
    pub const PORTAL: &str = "/dev/mackes/MDE/Portal";
}
