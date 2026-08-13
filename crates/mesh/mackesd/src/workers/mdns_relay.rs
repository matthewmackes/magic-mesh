//! MESH-MDNS-RELAY — native cross-LAN-segment mDNS service relay.
//!
//! Rebuilds (operator decision, 2026-06-05) the v1.x `mackes/mdns_relay.py`
//! relay natively, with **no python and no avahi shell-outs** — `mdns-sd` does
//! both the local browse and the LAN republish.
//!
//! On each peer:
//!   1. **Browse** the local LAN for the curated relayed service types
//!      (`_jellyfin._tcp`, `_googlecast._tcp`, …) via `mdns_sd::ServiceDaemon`.
//!   2. **Publish** each discovered local service to the `mesh/mdns/announce`
//!      Bus topic as an [`MdnsAnnounce`] tagged with this peer's mesh IP.
//!   3. **Republish (inbound half)** — poll the announce topic for other
//!      peers' services and republish them on the LOCAL LAN, substituting
//!      the originating peer's mesh IP for the source LAN IP (see the
//!      INBOUND block in `run_relay_blocking`; landed, no longer a
//!      follow-up).
//!
//! **Anti-loop:** republished services carry an `mde-relay-origin` TXT record;
//! the browse step skips anything carrying it, so a relayed service is never
//! re-relayed. Each announce is tagged with its origin peer, and the inbound
//! half drops announces whose origin is ourselves.
//!
//! **Type policy (v1.x §9 lock):** only the media/discovery allowlist is
//! relayed; the privacy-sensitive types (ssh / smb / printers) never are.
//!
//! **Graceful degrade:** no `nebula1` interface (pre-enrolment) or no
//! multicast-capable interface → the worker idles until shutdown, never panics.

#![cfg(feature = "async-services")]

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

/// Bus topic every peer writes its discovered local services to. Readers
/// filter by origin (`peer != self`) and republish on their own LAN.
pub const ANNOUNCE_TOPIC: &str = "mesh/mdns/announce";

/// TXT key marking a service WE republished from a peer — the browse step
/// skips these so a relayed service is never re-relayed (anti-loop).
pub const RELAY_ORIGIN_TXT: &str = "mde-relay-origin";

/// Idle sleep between browse-drain passes when no events are pending.
const IDLE_SLEEP: Duration = Duration::from_millis(500);

/// BUG-BROWSER-7: cap inbound peer-service registrations so a noisy/flapping
/// announce lane cannot grow mdns-sd registrations and their sockets without
/// bound. Duplicates are still accepted as no-ops through the `registered` set.
const MAX_REPUBLISHED_SERVICES: usize = 512;

/// A relay pass is one complete, bounded Bus transaction. Refuse a larger
/// transient batch instead of applying a partial prefix and advancing past work
/// that was never inspected.
const MAX_ANNOUNCES_PER_PASS: usize = 256;

/// One untrusted announce cannot force an oversized JSON allocation downstream.
const MAX_ANNOUNCE_BODY_BYTES: usize = 64 * 1024;

/// Local discoveries survive a late/replaced Bus in a bounded corrected-forward
/// queue. The payload itself is idempotent by service identity.
const MAX_PENDING_OUTBOUND: usize = 512;

/// Service types relayed by default (v1.x §9 lock — media + discovery).
pub const RELAYED_TYPES: &[&str] = &[
    "_jellyfin._tcp",
    "_googlecast._tcp",
    "_airplay._tcp",
    "_spotify-connect._tcp",
    "_home-assistant._tcp",
    "_syncthing._tcp",
    "_netdata._tcp",
    "_subsonic._tcp",
];

/// Service types NEVER relayed (privacy — printers, file shares, ssh).
pub const PRIVATE_TYPES: &[&str] = &[
    "_ipp._tcp",
    "_pdl-datastream._tcp",
    "_smb._tcp",
    "_afpovertcp._tcp",
    "_ssh._tcp",
];

/// A relayed service announce — the JSON body that crosses the Bus to peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MdnsAnnounce {
    /// Origin peer's mesh IP (anti-loop key + the host clients connect to).
    pub peer: String,
    /// mDNS instance name (e.g. `Jellyfin Media Server`).
    pub service: String,
    /// Bare service type (e.g. `_jellyfin._tcp`).
    pub service_type: String,
    /// Advertised port.
    pub port: u16,
    /// TXT records (key, value) — forwarded for client compatibility.
    pub txt: Vec<(String, String)>,
}

/// The mdns-sd browse string for a bare type (`_jellyfin._tcp` →
/// `_jellyfin._tcp.local.`). Shared with the CHOOSER-1 `desktop_sources`
/// worker, which browses the desktop protocol types through the same
/// machinery.
pub(crate) fn browse_type(bare: &str) -> String {
    format!("{bare}.local.")
}

/// True when `service_type` is on the relayed allowlist (and not private).
///
/// Accepts a bare type, a `.local.`-qualified type, or a fullname —
/// `_jellyfin._tcp.local.` / `_ssh._tcp` / `Name._airplay._tcp.local.` all
/// resolve to their bare type first.
#[must_use]
pub fn is_relayed(service_type: &str) -> bool {
    let base = bare_type(service_type);
    !PRIVATE_TYPES.contains(&base.as_str()) && RELAYED_TYPES.contains(&base.as_str())
}

/// Extract the trailing `_proto._tcp`/`_udp` token from a type string or
/// fullname, stripping a trailing `.local.` domain.
fn bare_type(s: &str) -> String {
    let s = s.trim_end_matches('.');
    let s = s.strip_suffix(".local").unwrap_or(s);
    // The bare type is the last two dot-separated tokens (`_x._tcp`); a
    // fullname (`Name._x._tcp`) has the instance before them.
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() >= 2 {
        let last2 = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
        if (last2.ends_with("._tcp") || last2.ends_with("._udp")) && last2.starts_with('_') {
            return last2;
        }
    }
    s.to_string()
}

/// The instance name from a resolved service's fullname, stripping the
/// `.<type>.local.` suffix. Shared with the CHOOSER-1 `desktop_sources`
/// worker.
pub(crate) fn instance_name(info: &ServiceInfo, bare: &str) -> String {
    let full = info.get_fullname();
    full.strip_suffix(&format!(".{}", browse_type(bare)))
        .unwrap_or(full)
        .trim_end_matches('.')
        .to_string()
}

/// Build an [`MdnsAnnounce`] from a resolved local service, or `None` when it
/// shouldn't be relayed.
///
/// Skips non-allowlisted types and any service WE republished (it carries
/// [`RELAY_ORIGIN_TXT`]) — the anti-loop guard. `own_ip` is this peer's mesh IP,
/// stamped as the announce origin + the host clients connect to.
#[must_use]
pub fn announce_from_info(
    bare_type: &str,
    info: &ServiceInfo,
    own_ip: &str,
) -> Option<MdnsAnnounce> {
    if !is_relayed(bare_type) {
        return None;
    }
    if info.get_property_val_str(RELAY_ORIGIN_TXT).is_some() {
        return None; // already a relayed service — don't loop it back
    }
    let txt: Vec<(String, String)> = info
        .get_properties()
        .iter()
        .map(|p| (p.key().to_string(), p.val_str().to_string()))
        .collect();
    Some(MdnsAnnounce {
        peer: own_ip.to_string(),
        service: instance_name(info, bare_type),
        service_type: bare_type.to_string(),
        port: info.get_port(),
        txt,
    })
}

/// Publish an announce through the current identity-bound Bus transaction.
fn publish_announce(transaction: &MdnsBusTransaction, ann: &MdnsAnnounce) -> io::Result<()> {
    let body = serde_json::to_string(ann).map_err(io::Error::other)?;
    transaction.verify_current()?;
    transaction
        .persist
        .write(ANNOUNCE_TOPIC, Priority::Default, None, Some(&body))
        .map_err(|error| io::Error::other(error.to_string()))?;
    transaction.verify_current()
}

/// Peer-suffixed instance name for a republished service — avoids colliding
/// with the peer's own LAN advertisement and with other peers' services.
fn republish_name(ann: &MdnsAnnounce) -> String {
    format!("{}-{}", ann.service, ann.peer.replace('.', "-"))
}

/// Dedup key for an inbound announce (origin peer + type + instance).
fn service_key(ann: &MdnsAnnounce) -> String {
    format!("{}|{}|{}", ann.peer, ann.service_type, ann.service)
}

/// Whether a peer announce should create a new local mDNS registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepublishDecision {
    /// First sighting of this peer/type/instance and still under the cap.
    Register,
    /// Already registered; nothing to do, but this is not pressure.
    Duplicate,
    /// New unique service would exceed the process cap.
    AtCap,
}

fn republish_decision(
    registered: &HashMap<String, MdnsAnnounce>,
    ann: &MdnsAnnounce,
    max: usize,
) -> RepublishDecision {
    let key = service_key(ann);
    if registered.get(&key) == Some(ann) {
        return RepublishDecision::Duplicate;
    }
    if !registered.contains_key(&key) && registered.len() >= max {
        return RepublishDecision::AtCap;
    }
    RepublishDecision::Register
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MdnsBusIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

struct MdnsBusTransaction {
    root: PathBuf,
    persist: Persist,
    identity: MdnsBusIdentity,
}

impl MdnsBusTransaction {
    fn open(root: PathBuf) -> io::Result<Self> {
        let before = match mdns_bus_identity(&root) {
            Ok(identity) => identity,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                drop(
                    Persist::open(root.clone())
                        .map_err(|error| io::Error::other(error.to_string()))?,
                );
                mdns_bus_identity(&root)?
            }
            Err(error) => return Err(error),
        };
        let persist =
            Persist::open(root.clone()).map_err(|error| io::Error::other(error.to_string()))?;
        let after = mdns_bus_identity(&root)?;
        #[cfg(unix)]
        let handle_matches_path = persist.index_inode() == Some(after.inode);
        #[cfg(not(unix))]
        let handle_matches_path = true;
        if before != after || !handle_matches_path {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "mDNS relay Bus changed while opening a transaction",
            ));
        }
        Ok(Self {
            root,
            persist,
            identity: after,
        })
    }

    fn verify_current(&self) -> io::Result<()> {
        if mdns_bus_identity(&self.root)? == self.identity {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "mDNS relay Bus changed during a transaction",
            ))
        }
    }
}

#[derive(Default)]
struct InboundBusState {
    active_identity: Option<MdnsBusIdentity>,
    cursor: Option<String>,
}

#[derive(Debug)]
struct StagedInbound {
    announces: Vec<MdnsAnnounce>,
    next_cursor: Option<String>,
}

impl InboundBusState {
    /// Activate one generation atomically at its complete bounded tail. Bus
    /// announcements are transient actions, so retained rows are never replayed
    /// after startup or same-path replacement; the first later row remains live.
    fn activate(&mut self, transaction: &MdnsBusTransaction) -> io::Result<()> {
        let retained = bounded_announce_rows(&transaction.persist, None)?;
        transaction.verify_current()?;
        self.cursor = retained.last().map(|message| message.ulid.clone());
        self.active_identity = Some(transaction.identity);
        Ok(())
    }

    fn stage(
        &mut self,
        transaction: &MdnsBusTransaction,
        own_ip: &str,
    ) -> io::Result<StagedInbound> {
        if self.active_identity != Some(transaction.identity) {
            self.activate(transaction)?;
        }
        let rows = bounded_announce_rows(&transaction.persist, self.cursor.as_deref())?;
        let next_cursor = rows
            .last()
            .map(|message| message.ulid.clone())
            .or_else(|| self.cursor.clone());
        let mut announces = Vec::with_capacity(rows.len());
        for message in rows {
            let Some(body) = message.body.as_deref() else {
                continue;
            };
            let Some(announce) = parse_bounded_announce(body) else {
                continue;
            };
            if announce.peer != own_ip {
                announces.push(announce);
            }
        }
        transaction.verify_current()?;
        Ok(StagedInbound {
            announces,
            next_cursor,
        })
    }

    fn commit(
        &mut self,
        transaction: &MdnsBusTransaction,
        staged: StagedInbound,
    ) -> io::Result<()> {
        transaction.verify_current()?;
        self.cursor = staged.next_cursor;
        Ok(())
    }
}

fn mdns_bus_identity(root: &Path) -> io::Result<MdnsBusIdentity> {
    let metadata = std::fs::metadata(root.join("index.sqlite"))?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "mDNS relay Bus index is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Ok(MdnsBusIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(MdnsBusIdentity {})
    }
}

fn bounded_announce_rows(
    persist: &Persist,
    cursor: Option<&str>,
) -> io::Result<Vec<mde_bus::persist::StoredMessage>> {
    let rows = persist
        .list_since_limit(ANNOUNCE_TOPIC, cursor, MAX_ANNOUNCES_PER_PASS + 1)
        .map_err(|error| io::Error::other(error.to_string()))?;
    if rows.len() > MAX_ANNOUNCES_PER_PASS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "mDNS relay announce batch exceeds the complete-read bound",
        ));
    }
    Ok(rows)
}

fn parse_bounded_announce(body: &str) -> Option<MdnsAnnounce> {
    if body.len() > MAX_ANNOUNCE_BODY_BYTES {
        return None;
    }
    let announce: MdnsAnnounce = serde_json::from_str(body).ok()?;
    let txt_bytes = announce
        .txt
        .iter()
        .try_fold(0usize, |total, (key, value)| {
            total.checked_add(key.len())?.checked_add(value.len())
        })?;
    if !is_relayed(&announce.service_type)
        || announce.peer.len() > 64
        || announce.peer.parse::<std::net::IpAddr>().is_err()
        || announce.service.is_empty()
        || announce.service.len() > 256
        || announce.service_type.len() > 64
        || announce.txt.len() > 64
        || txt_bytes > MAX_ANNOUNCE_BODY_BYTES
    {
        return None;
    }
    Some(announce)
}

/// Build the `ServiceInfo` to register a peer's service on the LOCAL LAN:
/// advertised at the peer's **mesh IP** (so LAN clients connect over the
/// overlay), peer-suffixed instance name, carrying the [`RELAY_ORIGIN_TXT`] tag
/// so our own browse skips it (anti-loop). `None` when `peer` isn't a valid IP.
fn build_republish_info(ann: &MdnsAnnounce) -> Option<ServiceInfo> {
    let ip: std::net::IpAddr = ann.peer.parse().ok()?;
    let instance = republish_name(ann);
    let hostname = format!("{instance}.local.");
    let mut txt = ann.txt.clone();
    txt.push((RELAY_ORIGIN_TXT.to_string(), ann.peer.clone()));
    let txt_refs: Vec<(&str, &str)> = txt.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    ServiceInfo::new(
        &browse_type(&ann.service_type),
        &instance,
        &hostname,
        ip,
        ann.port,
        &txt_refs[..],
    )
    .ok()
}

/// This host's mesh IP (`nebula1`), or `None` pre-enrolment.
fn own_mesh_ip() -> Option<String> {
    crate::voip_rtt::own_nebula_ip()
}

/// The relay loop (blocking). Each pass does BOTH halves: the **outbound** half
/// drains the mDNS browsers and publishes discovered local services to the Bus;
/// the **inbound** half polls the Bus for peers' announces and registers them on
/// the local LAN (at the peer's mesh IP). Runs until `stop` is set. Idles
/// gracefully when there's no mesh IP yet or no multicast-capable interface.
fn run_relay_blocking(stop: &AtomicBool) {
    let Some(own_ip) = own_mesh_ip() else {
        tracing::info!("mdns_relay: no nebula1 mesh IP (pre-enrolment); relay idle");
        wait_until_stop(stop);
        return;
    };
    let daemon = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "mdns_relay: no mDNS daemon; relay idle");
            wait_until_stop(stop);
            return;
        }
    };
    let bus_root =
        mde_bus::default_data_dir().unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT));

    let mut browsers = Vec::new();
    for bare in RELAYED_TYPES {
        match daemon.browse(&browse_type(bare)) {
            Ok(rx) => browsers.push((*bare, rx)),
            Err(e) => tracing::warn!(error = %e, service_type = bare, "mdns_relay: browse failed"),
        }
    }

    // Inbound republish state: a cursor over the announce topic + the set of
    // already-registered service keys (a peer service is registered once).
    let mut inbound = InboundBusState::default();
    let mut registered: HashMap<String, MdnsAnnounce> = HashMap::new();
    let mut pending_outbound = VecDeque::new();

    while !stop.load(Ordering::Relaxed) {
        let mut got_any = false;

        // OUTBOUND — drain every browser, publish local services.
        for (bare, rx) in &browsers {
            while let Ok(event) = rx.try_recv() {
                got_any = true;
                if let ServiceEvent::ServiceResolved(info) = event {
                    if let Some(ann) = announce_from_info(bare, &info, &own_ip) {
                        enqueue_outbound(&mut pending_outbound, ann);
                    }
                }
            }
        }

        // Every pass fresh-opens and identity-binds the live index. Failed opens
        // leave both outbound discoveries and inbound cursor state untouched.
        let transaction = match MdnsBusTransaction::open(bus_root.clone()) {
            Ok(transaction) => transaction,
            Err(error) => {
                tracing::debug!(%error, "mdns_relay: Bus unavailable; transaction deferred");
                if !got_any {
                    std::thread::sleep(IDLE_SLEEP);
                }
                continue;
            }
        };

        // OUTBOUND publication is corrected forward after a late or replaced
        // Bus. Keep an item queued unless the write and post-write identity
        // check both succeed; duplicate corrected-forward rows are semantically
        // idempotent because inbound registration keys the full service identity.
        while let Some(announce) = pending_outbound.front() {
            if let Err(error) = publish_announce(&transaction, announce) {
                tracing::debug!(%error, "mdns_relay: outbound Bus publication deferred");
                break;
            }
            pending_outbound.pop_front();
            got_any = true;
        }

        // INBOUND — stage the complete bounded batch before any registration.
        let staged = match inbound.stage(&transaction, &own_ip) {
            Ok(staged) => staged,
            Err(error) => {
                tracing::debug!(%error, "mdns_relay: inbound Bus transaction deferred");
                continue;
            }
        };
        let mut effects_complete = true;
        for announce in &staged.announces {
            match republish_decision(&registered, announce, MAX_REPUBLISHED_SERVICES) {
                RepublishDecision::Register => {
                    let Some(info) = build_republish_info(announce) else {
                        continue;
                    };
                    if let Err(error) = transaction.verify_current() {
                        tracing::debug!(%error, "mdns_relay: registration deferred before effect");
                        effects_complete = false;
                        break;
                    }
                    // mdns-sd defines register of an existing fullname as an
                    // idempotent update. We record success only after its
                    // command is accepted; on a later batch failure, replayed
                    // successful effects therefore collapse to Duplicate.
                    if let Err(error) = daemon.register(info) {
                        tracing::warn!(error = %error, service = %announce.service, "mdns_relay: republish failed; batch will retry");
                        effects_complete = false;
                        break;
                    }
                    if let Err(error) = transaction.verify_current() {
                        tracing::debug!(%error, "mdns_relay: Bus changed after idempotent registration");
                        effects_complete = false;
                        break;
                    }
                    registered.insert(service_key(announce), announce.clone());
                    got_any = true;
                }
                RepublishDecision::Duplicate => {}
                RepublishDecision::AtCap => {
                    tracing::warn!(
                        cap = MAX_REPUBLISHED_SERVICES,
                        service = %announce.service,
                        peer = %announce.peer,
                        "mdns_relay: republish cap reached; skipping peer service"
                    );
                }
            }
        }
        if effects_complete {
            if let Err(error) = inbound.commit(&transaction, staged) {
                tracing::debug!(%error, "mdns_relay: inbound cursor commit deferred");
            }
        }

        if !got_any {
            std::thread::sleep(IDLE_SLEEP);
        }
    }
}

fn enqueue_outbound(queue: &mut VecDeque<MdnsAnnounce>, announce: MdnsAnnounce) {
    let key = service_key(&announce);
    if let Some(existing) = queue.iter_mut().find(|item| service_key(item) == key) {
        *existing = announce;
        return;
    }
    if queue.len() == MAX_PENDING_OUTBOUND {
        queue.pop_front();
        tracing::warn!(
            cap = MAX_PENDING_OUTBOUND,
            "mdns_relay: outbound discovery queue full; evicting oldest service"
        );
    }
    queue.push_back(announce);
}

/// Park the thread until `stop` is set (the graceful-degrade idle path).
fn wait_until_stop(stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(IDLE_SLEEP);
    }
}

/// Supervised worker: runs the outbound relay on a blocking thread, stopping it
/// when the supervisor signals shutdown.
pub struct MdnsRelayWorker;

impl Default for MdnsRelayWorker {
    fn default() -> Self {
        Self
    }
}

impl MdnsRelayWorker {
    /// Construct the relay worker.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Worker for MdnsRelayWorker {
    fn name(&self) -> &'static str {
        "mdns_relay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = tokio::task::spawn_blocking(move || run_relay_blocking(&stop2));
        shutdown.wait().await;
        stop.store(true, Ordering::Relaxed);
        let _ = handle.await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_relayed_allows_media_types() {
        assert!(is_relayed("_jellyfin._tcp"));
        assert!(is_relayed("_googlecast._tcp"));
        assert!(is_relayed("_subsonic._tcp.local."));
    }

    #[test]
    fn is_relayed_rejects_private_and_unknown() {
        assert!(!is_relayed("_ssh._tcp"));
        assert!(!is_relayed("_smb._tcp"));
        assert!(!is_relayed("_ipp._tcp.local."));
        assert!(!is_relayed("_http._tcp"));
    }

    #[test]
    fn bare_type_reduces_fullnames_and_domains() {
        assert_eq!(bare_type("_jellyfin._tcp.local."), "_jellyfin._tcp");
        assert_eq!(
            bare_type("Living Room._airplay._tcp.local."),
            "_airplay._tcp"
        );
        assert_eq!(bare_type("_ssh._tcp"), "_ssh._tcp");
    }

    #[test]
    fn announce_round_trips_through_json() {
        let ann = MdnsAnnounce {
            peer: "10.42.0.3".into(),
            service: "Jellyfin".into(),
            service_type: "_jellyfin._tcp".into(),
            port: 8096,
            txt: vec![("Path".into(), "/web".into())],
        };
        let body = serde_json::to_string(&ann).unwrap();
        let back: MdnsAnnounce = serde_json::from_str(&body).unwrap();
        assert_eq!(ann, back);
    }

    fn svc(bare: &str, instance: &str, port: u16, txt: &[(&str, &str)]) -> ServiceInfo {
        ServiceInfo::new(
            &browse_type(bare),
            instance,
            &format!("{instance}.local."),
            "192.168.1.50",
            port,
            txt,
        )
        .unwrap()
    }

    #[test]
    fn announce_from_info_lifts_a_relayed_service() {
        let info = svc("_jellyfin._tcp", "Jellyfin", 8096, &[("Path", "/web")]);
        let ann = announce_from_info("_jellyfin._tcp", &info, "10.42.0.3").unwrap();
        assert_eq!(ann.peer, "10.42.0.3"); // origin = our mesh IP, not the LAN IP
        assert_eq!(ann.service, "Jellyfin");
        assert_eq!(ann.service_type, "_jellyfin._tcp");
        assert_eq!(ann.port, 8096);
        assert!(ann.txt.iter().any(|(k, v)| k == "Path" && v == "/web"));
    }

    #[test]
    fn announce_from_info_skips_non_relayed_types() {
        let info = svc("_ssh._tcp", "shell", 22, &[]);
        assert!(announce_from_info("_ssh._tcp", &info, "10.42.0.3").is_none());
    }

    #[test]
    fn announce_from_info_skips_our_own_relayed_services_anti_loop() {
        // A service WE republished carries the relay-origin TXT — don't loop it.
        let info = svc(
            "_jellyfin._tcp",
            "Jellyfin-peerB",
            8096,
            &[(RELAY_ORIGIN_TXT, "10.42.0.9")],
        );
        assert!(announce_from_info("_jellyfin._tcp", &info, "10.42.0.3").is_none());
    }

    fn ann(peer: &str, service: &str, ty: &str, port: u16) -> MdnsAnnounce {
        MdnsAnnounce {
            peer: peer.into(),
            service: service.into(),
            service_type: ty.into(),
            port,
            txt: vec![],
        }
    }

    #[test]
    fn republish_name_is_peer_suffixed_and_collision_safe() {
        let a = ann("10.42.0.9", "Jellyfin", "_jellyfin._tcp", 8096);
        assert_eq!(republish_name(&a), "Jellyfin-10-42-0-9");
    }

    #[test]
    fn service_key_distinguishes_peer_type_instance() {
        let a = ann("10.42.0.9", "Jellyfin", "_jellyfin._tcp", 8096);
        let b = ann("10.42.0.8", "Jellyfin", "_jellyfin._tcp", 8096);
        assert_ne!(service_key(&a), service_key(&b)); // different peer
        assert_eq!(service_key(&a), service_key(&a)); // stable
    }

    #[test]
    fn republish_candidates_are_capped_but_duplicates_remain_noops() {
        let mut registered = HashMap::new();
        let a = ann("10.42.0.9", "Jellyfin", "_jellyfin._tcp", 8096);
        let b = ann("10.42.0.8", "Cast", "_googlecast._tcp", 8009);

        assert_eq!(
            republish_decision(&registered, &a, 1),
            RepublishDecision::Register
        );
        registered.insert(service_key(&a), a.clone());
        assert_eq!(registered.len(), 1);
        assert_eq!(
            republish_decision(&registered, &a, 1),
            RepublishDecision::Duplicate
        );
        assert_eq!(registered.len(), 1);
        assert_eq!(
            republish_decision(&registered, &b, 1),
            RepublishDecision::AtCap
        );
        assert_eq!(registered.len(), 1);
    }

    #[test]
    fn build_republish_info_advertises_peer_mesh_ip_and_origin_tag() {
        let a = ann("10.42.0.9", "Jellyfin", "_jellyfin._tcp", 8096);
        let info = build_republish_info(&a).expect("valid mesh IP");
        assert_eq!(info.get_port(), 8096);
        // peer-suffixed instance name + the relay-origin TXT (anti-loop).
        assert!(info.get_fullname().starts_with("Jellyfin-10-42-0-9."));
        assert_eq!(
            info.get_property_val_str(RELAY_ORIGIN_TXT),
            Some("10.42.0.9")
        );
        // advertised at the peer's mesh IP, not our LAN address.
        assert!(info
            .get_addresses()
            .iter()
            .any(|ip| ip.to_string() == "10.42.0.9"));
    }

    #[test]
    fn build_republish_info_rejects_a_non_ip_peer() {
        let a = ann("not-an-ip", "Jellyfin", "_jellyfin._tcp", 8096);
        assert!(build_republish_info(&a).is_none());
    }

    fn publish(persist: &Persist, announce: &MdnsAnnounce) {
        let body = serde_json::to_string(announce).unwrap();
        persist
            .write(ANNOUNCE_TOPIC, Priority::Default, None, Some(&body))
            .unwrap();
    }

    fn replace_bus_index(root: &Path, retained: &MdnsAnnounce) {
        let replacement_root = root.parent().unwrap().join("replacement-bus");
        let replacement = Persist::open(replacement_root.clone()).unwrap();
        publish(&replacement, retained);
        drop(replacement);
        std::fs::rename(
            replacement_root.join("index.sqlite"),
            root.join("index.sqlite"),
        )
        .unwrap();
    }

    #[test]
    fn mdns_r91_same_path_replacement_skips_retained_and_consumes_first_forward_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("bus");
        let initial = Persist::open(root.clone()).unwrap();
        publish(
            &initial,
            &ann("10.42.0.8", "initial-retained", "_jellyfin._tcp", 8096),
        );
        drop(initial);

        let mut state = InboundBusState::default();
        let initial_transaction = MdnsBusTransaction::open(root.clone()).unwrap();
        let staged = state.stage(&initial_transaction, "10.42.0.3").unwrap();
        assert!(staged.announces.is_empty());
        state.commit(&initial_transaction, staged).unwrap();

        let retained = ann("10.42.0.9", "replacement-retained", "_jellyfin._tcp", 8096);
        replace_bus_index(&root, &retained);
        let replacement_transaction = MdnsBusTransaction::open(root.clone()).unwrap();
        let staged = state.stage(&replacement_transaction, "10.42.0.3").unwrap();
        assert!(staged.announces.is_empty());
        state.commit(&replacement_transaction, staged).unwrap();

        let live = Persist::open(root.clone()).unwrap();
        let forward = ann("10.42.0.9", "first-forward", "_jellyfin._tcp", 8096);
        publish(&live, &forward);
        drop(live);
        let forward_transaction = MdnsBusTransaction::open(root.clone()).unwrap();
        let staged = state.stage(&forward_transaction, "10.42.0.3").unwrap();
        assert_eq!(staged.announces, vec![forward]);
        state.commit(&forward_transaction, staged).unwrap();
        let final_transaction = MdnsBusTransaction::open(root).unwrap();
        assert!(state
            .stage(&final_transaction, "10.42.0.3")
            .unwrap()
            .announces
            .is_empty());
    }

    #[test]
    fn mdns_r91_late_bus_recovers_without_replaying_retained_rows() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("bus");
        std::fs::write(&root, b"blocks Persist::open").unwrap();
        assert!(MdnsBusTransaction::open(root.clone()).is_err());

        std::fs::remove_file(&root).unwrap();
        let persist = Persist::open(root.clone()).unwrap();
        publish(
            &persist,
            &ann("10.42.0.8", "late-retained", "_jellyfin._tcp", 8096),
        );
        drop(persist);
        let mut state = InboundBusState::default();
        let activation = MdnsBusTransaction::open(root.clone()).unwrap();
        let staged = state.stage(&activation, "10.42.0.3").unwrap();
        assert!(staged.announces.is_empty());
        state.commit(&activation, staged).unwrap();

        let persist = Persist::open(root.clone()).unwrap();
        let forward = ann("10.42.0.8", "late-forward", "_jellyfin._tcp", 8096);
        publish(&persist, &forward);
        drop(persist);
        let transaction = MdnsBusTransaction::open(root).unwrap();
        assert_eq!(
            state.stage(&transaction, "10.42.0.3").unwrap().announces,
            vec![forward]
        );
    }

    #[test]
    fn mdns_r91_complete_bound_refuses_partial_activation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("bus");
        let persist = Persist::open(root.clone()).unwrap();
        for index in 0..=MAX_ANNOUNCES_PER_PASS {
            publish(
                &persist,
                &ann(
                    "10.42.0.8",
                    &format!("retained-{index}"),
                    "_jellyfin._tcp",
                    8096,
                ),
            );
        }
        drop(persist);
        let transaction = MdnsBusTransaction::open(root).unwrap();
        let mut state = InboundBusState::default();
        let error = state.stage(&transaction, "10.42.0.3").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(state.active_identity, None);
        assert_eq!(state.cursor, None);
    }

    #[test]
    fn mdns_r91_hostile_rows_are_rejected_before_any_effect_batch() {
        assert!(parse_bounded_announce(&"x".repeat(MAX_ANNOUNCE_BODY_BYTES + 1)).is_none());
        assert!(parse_bounded_announce("{not-json").is_none());
        assert!(parse_bounded_announce(
            &serde_json::to_string(&ann("10.42.0.8", "ssh", "_ssh._tcp", 22)).unwrap()
        )
        .is_none());
        assert!(parse_bounded_announce(
            &serde_json::to_string(&ann("not-an-ip", "cast", "_googlecast._tcp", 8009)).unwrap()
        )
        .is_none());
    }

    #[test]
    fn mdns_r91_registration_claim_is_success_bound_and_updates_are_idempotent() {
        let original = ann("10.42.0.9", "Jellyfin", "_jellyfin._tcp", 8096);
        let mut registered = HashMap::new();
        assert_eq!(
            republish_decision(&registered, &original, 1),
            RepublishDecision::Register
        );
        // A failed external command stores no claim and is therefore retried.
        assert_eq!(
            republish_decision(&registered, &original, 1),
            RepublishDecision::Register
        );
        registered.insert(service_key(&original), original.clone());
        assert_eq!(
            republish_decision(&registered, &original, 1),
            RepublishDecision::Duplicate
        );
        let mut updated = original;
        updated.port = 8097;
        assert_eq!(
            republish_decision(&registered, &updated, 1),
            RepublishDecision::Register
        );
    }

    #[test]
    fn mdns_r91_outbound_queue_is_bounded_and_coalesces_service_identity() {
        let mut queue = VecDeque::new();
        let first = ann("10.42.0.3", "Jellyfin", "_jellyfin._tcp", 8096);
        enqueue_outbound(&mut queue, first.clone());
        let mut updated = first;
        updated.port = 8097;
        enqueue_outbound(&mut queue, updated.clone());
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.front(), Some(&updated));
        for index in 0..=MAX_PENDING_OUTBOUND {
            enqueue_outbound(
                &mut queue,
                ann(
                    "10.42.0.3",
                    &format!("service-{index}"),
                    "_jellyfin._tcp",
                    8096,
                ),
            );
        }
        assert_eq!(queue.len(), MAX_PENDING_OUTBOUND);
    }
}
