//! KDC2-2 plugins — per-feature plugin trait + the canonical
//! plugin registry (ping, clipboard, share, notification,
//! findmyphone, battery, mpris, sms, telephony).
//!
//! KDE Connect's wire format multiplexes nine plugins through a
//! single TLS session, distinguished by the `Packet.kind` string
//! (`kdeconnect.<plugin>`). KDC2 keeps the same nine plugins for
//! wire compatibility with stock clients. Extending with MDE-only
//! plugins is a v2.2+ deferred feature — the trait + registry
//! below are the seam.
//!
//! Per-plugin body types live in submodules. KDC2-2.5 lands
//! `clipboard` first (smallest body shape); KDC2-2.6..2.9 land
//! the remaining seven.

pub mod battery;
pub mod clipboard;
pub mod findmyphone;
pub mod mpris;
pub mod notification;
pub mod ping;
pub mod run_command;
pub mod share;
pub mod sms;
pub mod telephony;

pub use battery::{battery_packet, BatteryBody, BatteryPlugin};
pub use clipboard::{clipboard_packet, from_packet_body, ClipboardBody, ClipboardPlugin};
pub use findmyphone::{find_my_phone_packet, FindMyPhoneBody, FindMyPhonePlugin};
pub use mpris::{mpris_command_packet, MprisBody, MprisKind, MprisPlugin};
pub use notification::{notification_packet, NotificationBody, NotificationPlugin};
pub use ping::{ping_packet, PingBody, PingPlugin};
pub use run_command::{run_command_packet, RunCommandBody};
pub use share::{file_share_packet, url_share_packet, ShareBody, ShareKind, SharePlugin};
pub use sms::{sms_messages_packet, SmsMessage, SmsMessagesBody, SmsPlugin};
pub use telephony::{telephony_packet, TelephonyBody, TelephonyEvent, TelephonyPlugin};

use std::fmt;

/// The canonical set of KDE Connect plugin types v2.1 KDC2 ships
/// at wire-compat parity with upstream — plus `RunCommand`,
/// which exists in upstream as an optional non-default plugin
/// and ships deny-default in MDE's policy.toml per the v2.1
/// security-review lock.
///
/// The serde token (snake_case via Display) matches the `Packet
/// .kind` suffix (`kdeconnect.<token>`). Adding a new plugin
/// means a new variant here + a `PluginRegistry::default()`
/// update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginKind {
    /// Connection liveness check.
    Ping,
    /// Clipboard sync.
    Clipboard,
    /// File transfer.
    Share,
    /// Mirror notifications from one peer to another.
    Notification,
    /// Trigger phone ring / find-my-device.
    FindMyPhone,
    /// Mirror phone/laptop battery state.
    Battery,
    /// MPRIS media-player control.
    Mpris,
    /// SMS read/send (Android only).
    Sms,
    /// Phone-call state mirror.
    Telephony,
    /// KDC2-2.19 — Remote command execution. Upstream KDE
    /// Connect ships this as an optional plugin; MDE registers
    /// it for wire compat with phones that offer it BUT denies
    /// it by default in `policy.toml`'s `[plugins].deny`. The
    /// dispatch-time check (KDC2-3.11) refuses the packet
    /// before the body type's handler runs. Operators opt in
    /// per-device via the KDC2-3.11.a per-device allow list
    /// (deferred).
    RunCommand,
}

impl PluginKind {
    /// Every plugin KDC2 ships at v2.1 parity with upstream.
    /// Iteration order matters: it's the **default registration
    /// order** the host integration walks at startup, so handshake
    /// `incomingCapabilities` / `outgoingCapabilities` lists land
    /// in a deterministic shape (some KDC clients are sensitive to
    /// list order during pairing).
    #[must_use]
    pub const fn all() -> [PluginKind; 10] {
        [
            PluginKind::Ping,
            PluginKind::Clipboard,
            PluginKind::Share,
            PluginKind::Notification,
            PluginKind::FindMyPhone,
            PluginKind::Battery,
            PluginKind::Mpris,
            PluginKind::Sms,
            PluginKind::Telephony,
            PluginKind::RunCommand,
        ]
    }

    /// Wire token sans the `kdeconnect.` prefix.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            PluginKind::Ping => "ping",
            PluginKind::Clipboard => "clipboard",
            PluginKind::Share => "share.request",
            PluginKind::Notification => "notification",
            PluginKind::FindMyPhone => "findmyphone.request",
            PluginKind::Battery => "battery",
            PluginKind::Mpris => "mpris",
            PluginKind::Sms => "sms.messages",
            PluginKind::Telephony => "telephony",
            PluginKind::RunCommand => "runcommand",
        }
    }

    /// Full `Packet.kind` string for this plugin (`kdeconnect.<token>`).
    /// Used by the wire decoder to dispatch incoming packets to the
    /// right `Plugin::on_packet` handler.
    #[must_use]
    pub fn packet_kind(self) -> String {
        format!("kdeconnect.{}", self.token())
    }
}

impl fmt::Display for PluginKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.token())
    }
}

/// Object-safe trait every plugin implements. Lives in this crate
/// so plugin registry walks can dispatch through a `Box<dyn
/// Plugin>` regardless of which crate owns the actual impl.
///
/// KDC2-2.12 refactor: `process` is the dispatch entry-point;
/// `handles` enumerates which `Packet.kind` strings this plugin
/// claims. Concrete per-plugin impls land in KDC2-2.13+ — for
/// now the registry routes against the trait shape.
pub trait Plugin: Send + Sync + std::fmt::Debug {
    /// Which plugin variant is this implementation for. The
    /// registry uses this to route incoming packets without
    /// downcasting.
    fn kind(&self) -> PluginKind;

    /// Packet kinds (`kdeconnect.<token>` strings) this plugin
    /// claims. The dispatch table builds a `kind → plugin
    /// index` lookup from these. Most plugins return a single-
    /// element slice; `Share` returns two (`share.request` for
    /// both URL + file shares).
    fn handles(&self) -> &[&'static str];

    /// `kdeconnect.identity.outgoingCapabilities` value this
    /// plugin contributes — packet kinds the plugin *emits*
    /// (as distinct from `handles` which is what it *receives*).
    /// Most plugins return the same slice as `handles()` because
    /// KDC's request/response pattern reuses the kind string.
    fn outgoing_kinds(&self) -> &[&'static str] {
        self.handles()
    }

    /// Process one inbound packet. Returns a (possibly-empty)
    /// list of response packets to send back. Mutability lets
    /// plugin impls hold per-peer state (e.g. SMS thread cache,
    /// MPRIS last-state mirror) without forcing every plugin
    /// to wear `Mutex<…>` internally.
    ///
    /// `ctx` carries the dispatch-time information about the
    /// sender (peer-id, paired-state) so plugins don't have to
    /// duplicate the lookup.
    fn process(
        &mut self,
        packet: &crate::wire::Packet,
        ctx: &PluginContext,
    ) -> Vec<crate::wire::Packet>;
}

/// Per-dispatch context handed to every `Plugin::process` call.
///
/// Carries the sender identity + pairing state so plugins don't
/// need to reach into the host's pairing store on every packet.
/// Future fields (rate-limit budget, per-device policy snapshot,
/// audit-chain hook) land here behind sensible defaults.
#[derive(Debug, Clone)]
pub struct PluginContext {
    /// Peer that sent the inbound packet.
    pub peer_id: String,
    /// True when the peer is currently in the host's pairing
    /// store. Plugins MAY use this for additional gating
    /// (e.g. SMS thread access is paired-only), though the
    /// host's dispatch-check (`mde_kdc::dispatch`) already
    /// enforces the per-plugin policy.
    pub paired: bool,
}

impl PluginContext {
    /// Construct from peer-id + paired bit. Convenience for
    /// tests + simple callers.
    #[must_use]
    pub fn new(peer_id: impl Into<String>, paired: bool) -> Self {
        Self {
            peer_id: peer_id.into(),
            paired,
        }
    }
}

/// Dispatch table mapping `Packet.kind` → registered plugin.
/// Built once at host startup from the policy.toml allow list;
/// reused for the daemon's lifetime.
///
/// `Box<dyn Plugin>` storage so plugins from different impl
/// crates coexist. Lookup is O(N) over the registered set —
/// at 9 plugins the linear scan beats hashmap overhead.
#[derive(Debug, Default)]
pub struct Registry {
    plugins: Vec<Box<dyn Plugin>>,
}

impl Registry {
    /// Empty registry. Use [`Registry::insert`] to add plugins.
    #[must_use]
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a plugin. Replaces any previous plugin claiming
    /// the same `PluginKind` — last-registered wins so a host
    /// integration can override a default impl with a custom
    /// one.
    pub fn insert(&mut self, plugin: Box<dyn Plugin>) {
        let kind = plugin.kind();
        self.plugins.retain(|p| p.kind() != kind);
        self.plugins.push(plugin);
    }

    /// How many plugins are currently registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// True when no plugin is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Iterate over the registered plugins (in registration
    /// order). Used by the host to build the
    /// `incomingCapabilities` / `outgoingCapabilities` lists
    /// advertised in the identity packet.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Plugin> {
        self.plugins.iter().map(std::ops::Deref::deref)
    }

    /// Drop every plugin whose token isn't in the allow set.
    /// Called once after construction to apply the policy.toml
    /// `[plugins].allow` list. Empty allow set is treated as
    /// "no filtering" — the registry stays untouched.
    pub fn filter_to_allow_list(&mut self, allow: &[&str]) {
        if allow.is_empty() {
            return;
        }
        self.plugins.retain(|p| allow.contains(&p.kind().token()));
    }

    /// Dispatch an inbound packet. Walks the registry, finds
    /// the first plugin whose `handles()` claims `packet.kind`,
    /// hands it the packet via `process`, and returns the
    /// response packets the plugin emitted. Returns an empty
    /// Vec when no plugin claims the kind (the host drops the
    /// packet silently — KDE Connect's wire protocol treats
    /// unknown kinds as no-ops for forward compat).
    pub fn dispatch(
        &mut self,
        packet: &crate::wire::Packet,
        ctx: &PluginContext,
    ) -> Vec<crate::wire::Packet> {
        for plugin in &mut self.plugins {
            if plugin.handles().iter().any(|k| *k == packet.kind) {
                return plugin.process(packet, ctx);
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_kind_count_matches_upstream_kdc() {
        // v2.1 KDC2: parity with upstream KDE Connect's 9
        // canonical plugins + RunCommand (KDC2-2.19, deny-by-
        // default in policy.toml).
        assert_eq!(PluginKind::all().len(), 10);
    }

    #[test]
    fn plugin_kind_packet_kind_includes_kdeconnect_prefix() {
        for k in PluginKind::all() {
            let s = k.packet_kind();
            assert!(
                s.starts_with("kdeconnect."),
                "plugin {k:?} packet kind {s:?} missing kdeconnect. prefix",
            );
        }
    }

    #[test]
    fn share_plugin_uses_request_suffix() {
        // Upstream's share plugin's kind is `kdeconnect.share.request`,
        // NOT `kdeconnect.share`. A drop of `.request` would silently
        // break file transfer with stock clients.
        assert_eq!(PluginKind::Share.packet_kind(), "kdeconnect.share.request");
    }

    #[test]
    fn findmyphone_plugin_uses_request_suffix() {
        // Same upstream quirk as Share — the trigger packet is
        // `kdeconnect.findmyphone.request`.
        assert_eq!(
            PluginKind::FindMyPhone.packet_kind(),
            "kdeconnect.findmyphone.request",
        );
    }

    #[test]
    fn sms_plugin_uses_messages_suffix() {
        // The Android KDE Connect SMS plugin emits
        // `kdeconnect.sms.messages` (plural).
        assert_eq!(PluginKind::Sms.packet_kind(), "kdeconnect.sms.messages");
    }

    #[test]
    fn plugin_kind_tokens_are_unique() {
        // Two plugins sharing the same token would silently merge
        // in the registry. Hard-lock uniqueness across all 10
        // variants (9 canonical + RunCommand).
        let mut tokens: Vec<&'static str> = PluginKind::all().iter().map(|k| k.token()).collect();
        tokens.sort_unstable();
        tokens.dedup();
        assert_eq!(tokens.len(), 10);
    }

    // ─────────────────────────────────────────────────────────
    // KDC2-2.12 — Plugin trait + Registry dispatch table
    // ─────────────────────────────────────────────────────────

    use crate::wire::Packet;

    /// Test plugin that records every received packet kind.
    /// One of these per `PluginKind` we want to exercise — the
    /// registry treats them as independent.
    #[derive(Debug)]
    struct TestPlugin {
        kind: PluginKind,
        handles: Vec<&'static str>,
        received: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        emit_echo: bool,
    }

    impl TestPlugin {
        fn new(kind: PluginKind, emit_echo: bool) -> Self {
            // Build a `Vec<&'static str>` from the plugin's
            // canonical packet kind. Using `String::leak` here
            // would also work but `packet_kind()` returns a
            // String, not a `&'static str`. We hard-code the
            // tokens to the matching variants.
            let handles: Vec<&'static str> = match kind {
                PluginKind::Ping => vec!["kdeconnect.ping"],
                PluginKind::Clipboard => vec!["kdeconnect.clipboard"],
                PluginKind::Notification => vec!["kdeconnect.notification"],
                PluginKind::Battery => vec!["kdeconnect.battery"],
                PluginKind::Mpris => vec!["kdeconnect.mpris"],
                PluginKind::Share => vec!["kdeconnect.share.request"],
                PluginKind::FindMyPhone => vec!["kdeconnect.findmyphone.request"],
                PluginKind::Sms => vec!["kdeconnect.sms.messages"],
                PluginKind::Telephony => vec!["kdeconnect.telephony"],
                PluginKind::RunCommand => vec!["kdeconnect.runcommand"],
            };
            Self {
                kind,
                handles,
                received: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                emit_echo,
            }
        }
    }

    impl Plugin for TestPlugin {
        fn kind(&self) -> PluginKind {
            self.kind
        }
        fn handles(&self) -> &[&'static str] {
            &self.handles
        }
        fn process(&mut self, packet: &Packet, _ctx: &PluginContext) -> Vec<Packet> {
            self.received.lock().unwrap().push(packet.kind.clone());
            if self.emit_echo {
                vec![packet.clone()]
            } else {
                vec![]
            }
        }
    }

    fn ping_packet(id: i64) -> Packet {
        Packet {
            id,
            kind: "kdeconnect.ping".to_string(),
            body: serde_json::Value::Null,
            mde_caps: None,
            payload_size: None,
            payload_transfer_info: None,
        }
    }

    #[test]
    fn empty_registry_dispatches_to_no_one() {
        let mut reg = Registry::new();
        let ctx = PluginContext::new("alice", true);
        let responses = reg.dispatch(&ping_packet(1), &ctx);
        assert!(responses.is_empty());
        assert!(reg.is_empty());
    }

    #[test]
    fn insert_grows_the_registry() {
        let mut reg = Registry::new();
        reg.insert(Box::new(TestPlugin::new(PluginKind::Ping, false)));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn insert_same_kind_twice_replaces_not_duplicates() {
        let mut reg = Registry::new();
        reg.insert(Box::new(TestPlugin::new(PluginKind::Ping, false)));
        reg.insert(Box::new(TestPlugin::new(PluginKind::Ping, true)));
        assert_eq!(reg.len(), 1, "second insert must replace, not append");
    }

    #[test]
    fn dispatch_routes_to_the_claiming_plugin() {
        let plugin = TestPlugin::new(PluginKind::Ping, false);
        let received = plugin.received.clone();
        let mut reg = Registry::new();
        reg.insert(Box::new(plugin));
        let ctx = PluginContext::new("alice", true);
        reg.dispatch(&ping_packet(7), &ctx);
        let r = received.lock().unwrap();
        assert_eq!(r.as_slice(), &["kdeconnect.ping".to_string()]);
    }

    #[test]
    fn dispatch_returns_responses_from_process() {
        let mut reg = Registry::new();
        reg.insert(Box::new(TestPlugin::new(PluginKind::Ping, true)));
        let ctx = PluginContext::new("alice", true);
        let out = reg.dispatch(&ping_packet(7), &ctx);
        assert_eq!(out.len(), 1, "echo plugin emits one response");
        assert_eq!(out[0].id, 7);
        assert_eq!(out[0].kind, "kdeconnect.ping");
    }

    #[test]
    fn dispatch_unknown_kind_returns_empty_silently() {
        // KDE Connect's wire protocol treats unknown packet kinds
        // as forward-compat no-ops. The registry must NOT panic
        // or surface an error.
        let mut reg = Registry::new();
        reg.insert(Box::new(TestPlugin::new(PluginKind::Clipboard, false)));
        let ctx = PluginContext::new("alice", true);
        let out = reg.dispatch(&ping_packet(1), &ctx);
        assert!(out.is_empty());
    }

    #[test]
    fn filter_to_allow_list_drops_denied_plugins() {
        let mut reg = Registry::new();
        reg.insert(Box::new(TestPlugin::new(PluginKind::Ping, false)));
        reg.insert(Box::new(TestPlugin::new(PluginKind::Clipboard, false)));
        reg.insert(Box::new(TestPlugin::new(PluginKind::Sms, false)));
        // Allow only ping + clipboard; SMS gets dropped.
        reg.filter_to_allow_list(&["ping", "clipboard"]);
        assert_eq!(reg.len(), 2);
        let kinds: Vec<PluginKind> = reg.iter().map(|p| p.kind()).collect();
        assert!(kinds.contains(&PluginKind::Ping));
        assert!(kinds.contains(&PluginKind::Clipboard));
        assert!(!kinds.contains(&PluginKind::Sms));
    }

    #[test]
    fn filter_to_allow_list_empty_is_noop() {
        // Empty allow list means "no filtering" — every plugin
        // stays. Matches the policy.toml semantic for an
        // unspecified [plugins].allow.
        let mut reg = Registry::new();
        reg.insert(Box::new(TestPlugin::new(PluginKind::Ping, false)));
        reg.insert(Box::new(TestPlugin::new(PluginKind::Clipboard, false)));
        reg.filter_to_allow_list(&[]);
        assert_eq!(reg.len(), 2);
    }
}
