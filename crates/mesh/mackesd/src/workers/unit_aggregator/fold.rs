//! EXPLORER-1 — the pure fold: three sources → one ordered, deduped, time-stamped
//! `Unit` stream.
//!
//! No I/O — the sources are read by `super::sources`, and this is the
//! deterministic decision the worker + the tests share (mirrors QC-2's
//! `converge_cycle` being pure over its seams).
//!
//! Order (lock #7 proximity + lock #23 self-first): **self**, then the other mesh
//! peers by name, then LAN hosts, then cloud objects (kind, then name). Cloud
//! objects are deduped by unit id across nodes (lock #20). Every produced unit is
//! stamped with its first/last-seen via a [`SeenTracker`] that survives across
//! ticks (E10): a unit that vanishes then returns keeps its original `first_seen`.

use std::collections::{BTreeSet, HashMap};

use mackes_mesh_types::peers::PeerRecord;

use super::enrich;
use super::sources::{CloudObjectRecord, LanHostRecord, MeshSnapshot};
use super::unit::{
    lan_unit_id, peer_unit_id, Extras, Health, MeshFacts, Reachability, Unit, UnitKind,
};

/// Per-unit-id first-seen memory carried across ticks (E10).
///
/// A vanished unit's entry is retained, so if it returns its original
/// `first_seen` is restored — the tracker never forgets an id it has stamped.
#[derive(Debug, Default)]
pub struct SeenTracker {
    first_seen: HashMap<String, u64>,
}

impl SeenTracker {
    /// A fresh tracker with no observed units.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stamp `unit` with its first/last-seen: first-seen is the value remembered
    /// from the earliest observation of this id (or `now_ms` on first sight);
    /// last-seen is always `now_ms`.
    fn stamp(&mut self, unit: &mut Unit, now_ms: u64) {
        let first = *self.first_seen.entry(unit.id.clone()).or_insert(now_ms);
        unit.first_seen_ms = first;
        unit.last_seen_ms = now_ms;
    }

    /// How many distinct unit ids the tracker has ever stamped.
    #[must_use]
    pub fn len(&self) -> usize {
        self.first_seen.len()
    }

    /// Whether the tracker has stamped nothing yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.first_seen.is_empty()
    }
}

/// The E5 enrichment folded onto a `Peer` from its replicated directory row (no
/// new probe, §7): the pinned mesh role (`cert_role`), the peer's own reported MAC
/// → offline OUI vendor, its remote-access listeners → the fingerprint + openable
/// actions + a coarse type hint, and the media-lighthouse / external-address tags.
/// Honestly empty where the row is silent (a pre-PD-2 writer with no descriptors).
fn peer_extras(rec: &PeerRecord) -> Extras {
    let mut extras = Extras {
        cert_role: rec.role.clone(),
        ..Extras::default()
    };
    if let Some(d) = &rec.descriptors {
        // The peer's own-reported remote-access listeners are its service
        // fingerprint (the mesh-mirror counterpart to the LAN port scan, E5).
        let mut services: Vec<String> = Vec::new();
        if d.remote_access.ssh {
            services.push("ssh".to_string());
        }
        if d.remote_access.rdp {
            services.push("rdp".to_string());
        }
        if d.remote_access.vnc {
            services.push("vnc".to_string());
        }
        if !services.is_empty() {
            extras.fingerprint = Some(services.join(", "));
            let actions = enrich::openable_actions(&services);
            if !actions.is_empty() {
                extras
                    .extra
                    .insert("actions".to_string(), actions.join(","));
            }
            if let Some(t) = enrich::service_type_hint(&services) {
                extras.extra.insert("type_guess".to_string(), t.to_string());
            }
        }
        // The peer's own LAN MAC (own-row authority) → offline OUI vendor.
        if let Some(mac) = d.lan_macs.first() {
            extras.oui_vendor = enrich::oui_vendor(mac).map(str::to_string);
        }
    }
    if rec.media {
        extras.extra.insert("media".to_string(), "true".to_string());
    }
    if let Some(addr) = &rec.external_addr {
        extras
            .extra
            .insert("external_addr".to_string(), addr.clone());
    }
    extras
}

/// Build a `Peer` unit from a directory row + the current leader hostname.
fn peer_unit(rec: &PeerRecord, leader: Option<&str>) -> Unit {
    Unit {
        id: peer_unit_id(&rec.hostname),
        kind: UnitKind::Peer,
        name: rec.hostname.clone(),
        reachability: Reachability::InMesh,
        address: rec.overlay_ip.clone(),
        health: Some(Health::from_mesh(&rec.health)),
        telemetry: None,
        mesh: Some(MeshFacts {
            role: rec.role.clone(),
            leader: leader == Some(rec.hostname.as_str()),
            mde_version: rec.mde_version.clone(),
        }),
        cloud: None,
        first_seen_ms: 0,
        last_seen_ms: 0,
        extras: peer_extras(rec),
    }
}

/// The synthesized self unit when this node has no directory row yet (pre-first-
/// heartbeat): we still know ourselves (lock #23), with honest unknowns for the
/// fields the directory would carry (§7).
fn self_unit_synthetic(self_host: &str, leader: Option<&str>) -> Unit {
    Unit {
        id: peer_unit_id(self_host),
        kind: UnitKind::Peer,
        name: self_host.to_string(),
        reachability: Reachability::InMesh,
        address: None,
        health: None,
        telemetry: None,
        mesh: Some(MeshFacts {
            role: None,
            leader: leader == Some(self_host),
            mde_version: None,
        }),
        cloud: None,
        first_seen_ms: 0,
        last_seen_ms: 0,
        extras: Extras::default(),
    }
}

/// Build a `LanHost` unit (EXPLORER-2 producer feeds these).
///
/// The scan's port fingerprint folds into the open [`Extras`] block (E5): the
/// service-label list (`extras.fingerprint`), the coarse type guess + raw open
/// ports (`extras.extra`), and the rDNS/mDNS name (`extras.rdns`). EXPLORER-9 adds
/// the offline OUI vendor from the MAC key (`extras.oui_vendor`) and the openable
/// actions the discovered services imply (`extras.extra["actions"]`). Every field
/// stays honestly absent when nothing answered it (§7).
fn lan_unit(rec: &LanHostRecord) -> Unit {
    let mut extras = Extras {
        rdns: rec.rdns.clone(),
        // The stable key is the host's MAC (or an IP fallback); the OUI lookup
        // vendors a real MAC and honestly ignores an IP key (§7).
        oui_vendor: enrich::oui_vendor(&rec.key).map(str::to_string),
        ..Extras::default()
    };
    if !rec.services.is_empty() {
        extras.fingerprint = Some(rec.services.join(", "));
        let actions = enrich::openable_actions(&rec.services);
        if !actions.is_empty() {
            extras
                .extra
                .insert("actions".to_string(), actions.join(","));
        }
    }
    if let Some(guess) = &rec.type_guess {
        extras.extra.insert("type_guess".to_string(), guess.clone());
    }
    if !rec.open_ports.is_empty() {
        extras.extra.insert(
            "open_ports".to_string(),
            rec.open_ports
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    Unit {
        id: lan_unit_id(&rec.key),
        kind: UnitKind::LanHost,
        name: rec.name.clone(),
        reachability: Reachability::OnLan,
        address: rec.address.clone(),
        health: None,
        telemetry: None,
        mesh: None,
        cloud: None,
        first_seen_ms: 0,
        last_seen_ms: 0,
        extras,
    }
}

/// Build a cloud-object unit, tagged with its host node (lock #20).
///
/// The E4 detail sheet (flavor/state/IPs/keypair/secgroups/…) folds from the
/// mirror object's [`CloudObjectRecord::detail`] onto the unit's `cloud` block —
/// `None` for a bare object with no detail (today's service-only mirror, §7).
fn cloud_unit(rec: &CloudObjectRecord) -> Unit {
    Unit {
        id: rec.kind.unit_id(&rec.id),
        kind: rec.kind.unit_kind(),
        name: rec.name.clone(),
        reachability: Reachability::CloudObject {
            node: rec.node.clone(),
        },
        address: rec.address.clone(),
        health: None,
        telemetry: None,
        mesh: None,
        cloud: if rec.detail.is_empty() {
            None
        } else {
            Some(rec.detail.clone())
        },
        first_seen_ms: 0,
        last_seen_ms: 0,
        extras: Extras::default(),
    }
}

/// Fold the mesh peers into `Peer` units: self first (lock #23), then the other
/// peers by name.
fn fold_peers(mesh: &MeshSnapshot) -> Vec<Unit> {
    let leader = mesh.leader.as_deref();
    let self_row = mesh.peers.iter().find(|p| p.hostname == mesh.self_host);
    let self_unit = self_row.map_or_else(
        || self_unit_synthetic(&mesh.self_host, leader),
        |row| peer_unit(row, leader),
    );
    let mut units = vec![self_unit];
    // The directory read is already hostname-sorted; keep that order for the
    // non-self peers.
    units.extend(
        mesh.peers
            .iter()
            .filter(|p| p.hostname != mesh.self_host)
            .map(|p| peer_unit(p, leader)),
    );
    units
}

/// Dedup the cloud union by unit id (lock #20): one record per object id, keeping
/// the one whose node sorts first (deterministic). Shared by the unit fold and
/// EXPLORER-7's edge derivation so both see the SAME deduped object set (a
/// cross-node duplicate never double-lists a unit nor doubles its edges).
pub(super) fn dedup_cloud_records(cloud: &[CloudObjectRecord]) -> Vec<&CloudObjectRecord> {
    let mut records: Vec<&CloudObjectRecord> = cloud.iter().collect();
    records.sort_by(|a, b| {
        a.kind
            .unit_id(&a.id)
            .cmp(&b.kind.unit_id(&b.id))
            .then_with(|| a.node.cmp(&b.node))
    });
    let mut ids = BTreeSet::new();
    records
        .into_iter()
        .filter(|r| ids.insert(r.kind.unit_id(&r.id)))
        .collect()
}

/// Fold the cloud union into deduped units (lock #20): one per object id
/// (keeping the record whose node sorts first — deterministic), then ordered by
/// kind then name for display.
fn fold_cloud(cloud: &[CloudObjectRecord]) -> Vec<Unit> {
    let mut units: Vec<Unit> = dedup_cloud_records(cloud)
        .into_iter()
        .map(cloud_unit)
        .collect();
    units.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    units
}

/// Fold the LAN hosts into units, ordered by name then id.
fn fold_lan(lan: &[LanHostRecord]) -> Vec<Unit> {
    let mut units: Vec<Unit> = lan.iter().map(lan_unit).collect();
    units.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    units
}

/// The whole fold: union the three sources into one ordered, deduped,
/// time-stamped `Unit` stream. Deterministic and I/O-free.
#[must_use]
pub fn aggregate(
    mesh: &MeshSnapshot,
    cloud: &[CloudObjectRecord],
    lan: &[LanHostRecord],
    seen: &mut SeenTracker,
    now_ms: u64,
) -> Vec<Unit> {
    let mut units = fold_peers(mesh);
    units.extend(fold_lan(lan));
    units.extend(fold_cloud(cloud));
    for u in &mut units {
        seen.stamp(u, now_ms);
    }
    units
}

#[cfg(test)]
mod tests {
    use super::super::sources::CloudKind;
    use super::*;

    fn peer_rec(host: &str, health: &str, ip: Option<&str>) -> PeerRecord {
        let mut r = PeerRecord::now(host, Some("12.0.0".into()), health);
        r.overlay_ip = ip.map(ToString::to_string);
        r
    }

    fn cloud_rec(node: &str, id: &str, kind: CloudKind, name: &str) -> CloudObjectRecord {
        CloudObjectRecord {
            node: node.to_string(),
            id: id.to_string(),
            kind,
            name: name.to_string(),
            address: None,
            links: super::super::sources::CloudLinks::default(),
            detail: super::super::unit::CloudDetail::default(),
        }
    }

    fn ids(units: &[Unit]) -> Vec<String> {
        units.iter().map(|u| u.id.clone()).collect()
    }

    #[test]
    fn self_is_always_the_first_unit_even_with_no_directory_rows() {
        // Empty mesh (no directory rows) → self is still present, first (#23).
        let mesh = MeshSnapshot {
            self_host: "me".into(),
            leader: None,
            peers: vec![],
        };
        let mut seen = SeenTracker::new();
        let units = aggregate(&mesh, &[], &[], &mut seen, 1000);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].id, peer_unit_id("me"));
        assert_eq!(units[0].kind, UnitKind::Peer);
        // Synthesized self carries honest unknowns (no directory row yet).
        assert!(units[0].health.is_none());
        assert!(units[0].address.is_none());
    }

    #[test]
    fn peers_fold_self_first_then_by_name_with_health_and_leader() {
        let mesh = MeshSnapshot {
            self_host: "me".into(),
            leader: Some("anvil".into()),
            peers: vec![
                peer_rec("anvil", "healthy", Some("10.42.0.2")),
                peer_rec("me", "degraded", Some("10.42.0.9")),
                peer_rec("zed", "critical", None),
            ],
        };
        let mut seen = SeenTracker::new();
        let units = aggregate(&mesh, &[], &[], &mut seen, 500);
        // self first, then the other peers by name (anvil, zed).
        assert_eq!(
            ids(&units),
            vec![
                peer_unit_id("me"),
                peer_unit_id("anvil"),
                peer_unit_id("zed"),
            ]
        );
        // self came from its directory row → real health + overlay address.
        assert_eq!(units[0].health, Some(Health::Degraded));
        assert_eq!(units[0].address.as_deref(), Some("10.42.0.9"));
        // The leader flag is set on the leader peer only.
        let anvil = &units[1];
        assert_eq!(anvil.mesh.as_ref().map(|m| m.leader), Some(true));
        assert_eq!(units[0].mesh.as_ref().map(|m| m.leader), Some(false));
        assert_eq!(anvil.health, Some(Health::Healthy));
    }

    #[test]
    fn cloud_union_dedups_by_object_id_across_nodes() {
        // The same image id published by two nodes → one unit, deterministically
        // tagged with the first-sorting node (lock #20).
        let cloud = vec![
            cloud_rec("node-b", "img-1", CloudKind::Image, "cirros"),
            cloud_rec("node-a", "img-1", CloudKind::Image, "cirros"),
            cloud_rec("node-a", "vol-1", CloudKind::Volume, "data"),
        ];
        let mesh = MeshSnapshot {
            self_host: "me".into(),
            leader: None,
            peers: vec![],
        };
        let mut seen = SeenTracker::new();
        let units = aggregate(&mesh, &cloud, &[], &mut seen, 1);
        // self + volume + image = 3 (the duplicate image collapsed to one).
        let cloud_units: Vec<&Unit> = units
            .iter()
            .filter(|u| matches!(u.kind, UnitKind::Image | UnitKind::Volume))
            .collect();
        assert_eq!(cloud_units.len(), 2);
        let image = cloud_units
            .iter()
            .find(|u| u.kind == UnitKind::Image)
            .expect("image unit");
        assert_eq!(image.id, "cloud:image:img-1");
        // Deduped to the first-sorting node (node-a < node-b).
        assert_eq!(
            image.reachability,
            Reachability::CloudObject {
                node: "node-a".into()
            }
        );
        // Cloud objects carry honest unknowns for unprobed fields (§7).
        assert!(image.health.is_none());
        assert!(image.telemetry.is_none());
    }

    #[test]
    fn ordering_is_mesh_then_lan_then_cloud() {
        let mesh = MeshSnapshot {
            self_host: "me".into(),
            leader: None,
            peers: vec![peer_rec("me", "healthy", None)],
        };
        let lan = vec![LanHostRecord {
            key: "aa:bb".into(),
            name: "printer".into(),
            address: Some("172.20.0.50".into()),
            ..Default::default()
        }];
        let cloud = vec![cloud_rec("node-a", "i1", CloudKind::Instance, "web")];
        let mut seen = SeenTracker::new();
        let units = aggregate(&mesh, &cloud, &lan, &mut seen, 1);
        assert_eq!(units[0].kind, UnitKind::Peer);
        assert_eq!(units[1].kind, UnitKind::LanHost);
        assert_eq!(units[2].kind, UnitKind::Instance);
    }

    #[test]
    fn lan_host_folds_the_scan_fingerprint_into_extras() {
        // A fingerprinted LAN host (EXPLORER-2): its service labels + type guess
        // + rDNS name fold into the open Extras block (E5).
        let fingerprinted = LanHostRecord {
            key: "aa:bb:cc:dd:ee:ff".into(),
            name: "desk.local".into(),
            address: Some("192.168.1.40".into()),
            services: vec!["rdp".into(), "vnc".into()],
            open_ports: vec![3389, 5900],
            type_guess: Some("computer".into()),
            rdns: Some("desk.local".into()),
        };
        // An un-fingerprinted silent host (only ARP-known): honest-empty extras (§7).
        let bare = LanHostRecord {
            key: "192.168.1.41".into(),
            name: "192.168.1.41".into(),
            address: Some("192.168.1.41".into()),
            ..Default::default()
        };
        let mesh = MeshSnapshot {
            self_host: "me".into(),
            leader: None,
            peers: vec![],
        };
        let mut seen = SeenTracker::new();
        let units = aggregate(&mesh, &[], &[fingerprinted, bare], &mut seen, 1);
        let desk = units
            .iter()
            .find(|u| u.id == lan_unit_id("aa:bb:cc:dd:ee:ff"))
            .expect("fingerprinted lan unit");
        assert_eq!(desk.kind, UnitKind::LanHost);
        assert_eq!(desk.reachability, Reachability::OnLan);
        assert_eq!(desk.extras.fingerprint.as_deref(), Some("rdp, vnc"));
        assert_eq!(desk.extras.rdns.as_deref(), Some("desk.local"));
        assert_eq!(
            desk.extras.extra.get("type_guess").map(String::as_str),
            Some("computer")
        );
        assert_eq!(
            desk.extras.extra.get("open_ports").map(String::as_str),
            Some("3389,5900")
        );
        // The bare host carries no fabricated fingerprint/type — honest unknown.
        let bare_unit = units
            .iter()
            .find(|u| u.id == lan_unit_id("192.168.1.41"))
            .expect("bare lan unit");
        assert!(bare_unit.extras.fingerprint.is_none());
        assert!(bare_unit.extras.rdns.is_none());
        assert!(bare_unit.extras.extra.is_empty());
        assert!(bare_unit.health.is_none());
    }

    #[test]
    fn instance_detail_folds_onto_the_cloud_unit_and_bare_objects_stay_none() {
        // EXPLORER-9 (E4): a cloud instance carrying the deep detail sheet folds it
        // onto the unit's `cloud` block; a bare object (no detail) stays None (§7).
        use super::super::unit::CloudDetail;
        let mut inst = cloud_rec("node-a", "i1", CloudKind::Instance, "web");
        inst.detail = CloudDetail {
            flavor: Some("m1.small".into()),
            vcpus: Some(2),
            ram_mb: Some(2048),
            disk_gb: Some(20),
            power_state: Some("running".into()),
            status: Some("ACTIVE".into()),
            fixed_ips: vec!["10.0.0.5".into()],
            floating_ips: vec!["172.24.4.7".into()],
            ports: vec!["p1".into()],
            keypair: Some("mesh-key".into()),
            security_groups: vec!["default".into()],
            created: Some("2026-07-04T12:00:00Z".into()),
            uptime_s: Some(3600),
            ..CloudDetail::default()
        };
        let bare = cloud_rec("node-a", "img1", CloudKind::Image, "cirros");
        let mesh = MeshSnapshot {
            self_host: "me".into(),
            leader: None,
            peers: vec![],
        };
        let mut seen = SeenTracker::new();
        let units = aggregate(&mesh, &[inst, bare], &[], &mut seen, 1);
        let web = units
            .iter()
            .find(|u| u.kind == UnitKind::Instance)
            .expect("instance unit");
        let detail = web.cloud.as_ref().expect("instance carries E4 detail");
        assert_eq!(detail.flavor.as_deref(), Some("m1.small"));
        assert_eq!(detail.vcpus, Some(2));
        assert_eq!(detail.power_state.as_deref(), Some("running"));
        assert_eq!(detail.floating_ips, vec!["172.24.4.7".to_string()]);
        assert_eq!(detail.keypair.as_deref(), Some("mesh-key"));
        assert_eq!(detail.security_groups, vec!["default".to_string()]);
        // The bare image carries no fabricated detail — honest unknown (§7).
        let img = units
            .iter()
            .find(|u| u.kind == UnitKind::Image)
            .expect("image unit");
        assert!(img.cloud.is_none());
    }

    #[test]
    fn lan_host_enriches_oui_vendor_and_openable_actions() {
        // EXPLORER-9 (E5): a MAC-keyed LAN host gets an offline OUI vendor + the
        // openable actions its fingerprint implies; an IP-keyed host honestly can't
        // be vendored (§7).
        let qemu = LanHostRecord {
            key: "52:54:00:ab:cd:ef".into(), // a QEMU/KVM guest NIC
            name: "guest.local".into(),
            address: Some("172.20.0.60".into()),
            services: vec!["ssh".into(), "vnc".into()],
            open_ports: vec![22, 5900],
            type_guess: Some("computer".into()),
            rdns: Some("guest.local".into()),
        };
        let ip_keyed = LanHostRecord {
            key: "172.20.0.61".into(), // no MAC → not vendorable
            name: "172.20.0.61".into(),
            address: Some("172.20.0.61".into()),
            ..Default::default()
        };
        let mesh = MeshSnapshot {
            self_host: "me".into(),
            leader: None,
            peers: vec![],
        };
        let mut seen = SeenTracker::new();
        let units = aggregate(&mesh, &[], &[qemu, ip_keyed], &mut seen, 1);
        let guest = units
            .iter()
            .find(|u| u.id == lan_unit_id("52:54:00:ab:cd:ef"))
            .expect("qemu lan unit");
        assert_eq!(guest.extras.oui_vendor.as_deref(), Some("QEMU/KVM"));
        assert_eq!(
            guest.extras.extra.get("actions").map(String::as_str),
            Some("open-ssh,open-vnc")
        );
        // The IP-keyed host is honestly un-vendored + carries no actions (§7).
        let bare = units
            .iter()
            .find(|u| u.id == lan_unit_id("172.20.0.61"))
            .expect("ip-keyed lan unit");
        assert!(bare.extras.oui_vendor.is_none());
        assert!(!bare.extras.extra.contains_key("actions"));
    }

    #[test]
    fn peer_enriches_role_mac_vendor_and_remote_access_from_the_mirror() {
        // EXPLORER-9 (E5): a peer's directory row enriches cert_role (pinned role),
        // its own-reported MAC → OUI vendor, and its remote-access listeners → the
        // fingerprint + openable actions + a type hint — no new probe (§7).
        use mackes_mesh_types::peers::{RemoteAccess, ServiceDescriptors};
        let mut rec = PeerRecord::now("anvil", Some("12.0.0".into()), "healthy");
        rec.role = Some("workstation".into());
        rec.media = true;
        rec.descriptors = Some(ServiceDescriptors {
            remote_access: RemoteAccess {
                ssh: true,
                rdp: true,
                vnc: false,
            },
            lan_macs: vec!["b8:27:eb:00:11:22".into()], // a Raspberry Pi
            ..Default::default()
        });
        let mesh = MeshSnapshot {
            self_host: "me".into(),
            leader: None,
            peers: vec![PeerRecord::now("me", None, "healthy"), rec],
        };
        let mut seen = SeenTracker::new();
        let units = aggregate(&mesh, &[], &[], &mut seen, 1);
        let anvil = units
            .iter()
            .find(|u| u.id == peer_unit_id("anvil"))
            .expect("peer unit");
        assert_eq!(anvil.extras.cert_role.as_deref(), Some("workstation"));
        assert_eq!(anvil.extras.oui_vendor.as_deref(), Some("Raspberry Pi"));
        assert_eq!(anvil.extras.fingerprint.as_deref(), Some("ssh, rdp"));
        assert_eq!(
            anvil.extras.extra.get("actions").map(String::as_str),
            Some("open-ssh,open-rdp")
        );
        // RDP present → the fingerprint types it a desktop computer.
        assert_eq!(
            anvil.extras.extra.get("type_guess").map(String::as_str),
            Some("computer")
        );
        assert_eq!(
            anvil.extras.extra.get("media").map(String::as_str),
            Some("true")
        );
        // Self (no directory descriptors) carries honest-empty enrichment (§7).
        let me = &units[0];
        assert_eq!(me.id, peer_unit_id("me"));
        assert!(me.extras.oui_vendor.is_none());
        assert!(me.extras.fingerprint.is_none());
        assert!(me.extras.cert_role.is_none());
    }

    #[test]
    fn first_seen_is_monotonic_across_ticks_and_survives_a_vanish() {
        let mut seen = SeenTracker::new();
        let with_peer = MeshSnapshot {
            self_host: "me".into(),
            leader: None,
            peers: vec![
                peer_rec("me", "healthy", None),
                peer_rec("gone", "healthy", None),
            ],
        };
        // Tick 1 @100: both seen first at 100.
        let t1 = aggregate(&with_peer, &[], &[], &mut seen, 100);
        let gone1 = t1.iter().find(|u| u.id == peer_unit_id("gone")).unwrap();
        assert_eq!(gone1.first_seen_ms, 100);
        assert_eq!(gone1.last_seen_ms, 100);

        // Tick 2 @200: 'gone' vanished (only self). last_seen for self advances.
        let only_self = MeshSnapshot {
            self_host: "me".into(),
            leader: None,
            peers: vec![peer_rec("me", "healthy", None)],
        };
        let t2 = aggregate(&only_self, &[], &[], &mut seen, 200);
        assert!(t2.iter().all(|u| u.id != peer_unit_id("gone")));
        assert_eq!(
            t2[0].first_seen_ms, 100,
            "self keeps its original first_seen"
        );
        assert_eq!(t2[0].last_seen_ms, 200);

        // Tick 3 @300: 'gone' returns → its ORIGINAL first_seen (100) is restored.
        let t3 = aggregate(&with_peer, &[], &[], &mut seen, 300);
        let gone3 = t3.iter().find(|u| u.id == peer_unit_id("gone")).unwrap();
        assert_eq!(gone3.first_seen_ms, 100, "vanish→return keeps first_seen");
        assert_eq!(gone3.last_seen_ms, 300);
    }
}
