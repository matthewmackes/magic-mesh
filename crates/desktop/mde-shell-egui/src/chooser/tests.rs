use super::*;
use mde_egui::egui::{pos2, vec2, Rect};
use mde_egui::{Density, StyleColorScheme};

/// A fixture body in the exact `CHOOSER-1` wire shape (the worker's
/// `DesktopSourcesState` serde output — `snake_case` tags, optional ports
/// skipped when unknown, `thumbnail_ref` always present + honestly null).
const FIXTURE: &str = r#"{
        "node": "elm",
        "sources": [
            {
                "id": "peer:oak", "name": "oak", "node": "oak", "host": "10.42.0.7",
                "protocols": [
                    {"protocol": "rdp", "port": 3389},
                    {"protocol": "vnc", "port": 5900}
                ],
                "origin": "mesh_peer", "reachability": "reachable",
                "os_hint": "linux", "thumbnail_ref": null
            },
            {
                "id": "peer-vm:oak:win11", "name": "win11", "node": "oak", "host": "10.42.0.7",
                "protocols": [{"protocol": "spice"}],
                "origin": "mesh_peer", "reachability": "unreachable",
                "reason": "vm shut off", "power_state": "shut off", "thumbnail_ref": null
            },
            {
                "id": "mdns:192.168.1.60:3389:rdp", "name": "OfficePC",
                "node": "192.168.1.60", "host": "192.168.1.60",
                "protocols": [{"protocol": "rdp", "port": 3389}],
                "origin": "mdns", "reachability": "reachable", "thumbnail_ref": null
            }
        ],
        "lanes": [
            {"lane": "mesh-registry", "status": "ok"},
            {"lane": "mdns", "status": "ok (3 types)"},
            {"lane": "local-kvm", "status": "gated: virsh not found"},
            {"lane": "manual", "status": "ok (0 sources)"}
        ],
        "published_at_ms": 1720000000000
    }"#;

/// An in-memory [`DesktopSourcesClient`] with a canned roster.
struct FakeSources(Option<DesktopSourcesState>);

impl DesktopSourcesClient for FakeSources {
    fn latest(&self) -> Option<DesktopSourcesState> {
        self.0.clone()
    }

    fn has_bus(&self) -> bool {
        true
    }
}

/// A CHOOSER-6 credential store the integration tests share (via `Rc`) to
/// seed + assert seals through a live [`ChooserState`]. It does a REAL
/// seal→store→read round-trip so "prompt once then remember" is exercised
/// end to end.
#[derive(Clone, Default)]
struct RecordingStore {
    inner: std::rc::Rc<std::cell::RefCell<HashMap<String, crate::auth::Credential>>>,
}

impl RecordingStore {
    /// The credential sealed under `store_ref`, if any (the round-trip proof).
    fn get_ref(&self, store_ref: &str) -> Option<crate::auth::Credential> {
        self.inner.borrow().get(store_ref).cloned()
    }

    /// Number of remembered credentials in the fake store.
    fn seal_count(&self) -> usize {
        self.inner.borrow().len()
    }
}

impl CredentialStore for RecordingStore {
    fn get(&self, store_ref: &str) -> Result<Option<crate::auth::Credential>, String> {
        Ok(self.inner.borrow().get(store_ref).cloned())
    }

    fn seal(&self, store_ref: &str, credential: &crate::auth::Credential) -> SealOutcome {
        self.inner
            .borrow_mut()
            .insert(store_ref.to_string(), credential.clone());
        SealOutcome::Sealed
    }
}

/// An inert CHOOSER-9 prefs session (its workgroup root is unprovisioned, so it
/// is a silent no-op) — favorites/recents still track session-locally, exactly
/// as an offline seat behaves, so the pre-CHOOSER-9 tests are unaffected.
fn inert_prefs() -> ChooserPrefs {
    chooser_prefs::ChooserPrefs::new(
        chooser_prefs::ChooserPrefsStore::new(PathBuf::from("/no/such/mesh/root")),
        "matthew",
        "seat-test",
    )
}

/// A CHOOSER-9 prefs session over an explicit workgroup root + seat (the
/// two-seat sync tests point two of these at one shared tempdir).
fn prefs_at(root: PathBuf, seat: &str) -> ChooserPrefs {
    chooser_prefs::ChooserPrefs::new(chooser_prefs::ChooserPrefsStore::new(root), "matthew", seat)
}

/// A `ChooserState` over a canned roster, with no publish root (the
/// broker publish then records its honest error) and a fixed peer name. Most
/// tests exercise mesh-peer sources (SSO), so the honest-gated production
/// credential store is fine; the external-cred tests inject their own store.
fn state_with(state: Option<DesktopSourcesState>) -> ChooserState {
    state_with_store(state, Box::new(MeshCredentialStore))
}

/// [`state_with`] over an explicit credential store (the CHOOSER-6 seam).
fn state_with_store(
    state: Option<DesktopSourcesState>,
    creds: Box<dyn CredentialStore>,
) -> ChooserState {
    let mut s = ChooserState::with_client(
        Box::new(FakeSources(state)),
        None,
        "client-node".to_string(),
        creds,
        inert_prefs(),
    );
    s.refresh();
    s
}

fn fixture_state() -> DesktopSourcesState {
    parse_sources(FIXTURE).expect("the fixture decodes")
}

/// A minimal source row for fold/connect tests.
fn source(id: &str, node: &str, protocols: &[Protocol]) -> DesktopSource {
    DesktopSource {
        id: id.to_string(),
        name: id.rsplit(':').next().unwrap_or(id).to_string(),
        node: node.to_string(),
        host: node.to_string(),
        protocols: protocols
            .iter()
            .map(|p| ProtocolOffer {
                protocol: *p,
                port: None,
            })
            .collect(),
        origin: SourceOrigin::MeshPeer,
        reachability: Reachability::Reachable,
        reason: None,
        os_hint: None,
        power_state: None,
        thumbnail_ref: None,
    }
}

fn roster(sources: Vec<DesktopSource>) -> DesktopSourcesState {
    DesktopSourcesState {
        sources,
        lanes: vec![],
    }
}

/// Encode a `w×h` opaque-grey RGBA PNG with the same `png` crate the shell
/// decoder uses, so the thumbnail plumbing is driven end to end by a REAL
/// snapshot (no opaque fixture blob).
fn tiny_png(w: u32, h: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut bytes, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().expect("png header");
        let px = vec![200u8; w as usize * h as usize * 4];
        writer.write_image_data(&px).expect("png data");
    }
    bytes
}

/// Wrap PNG bytes as the `data:image/png;base64,…` ref the worker inlines on
/// the state plane — the exact shape [`decode_thumbnail_ref`] resolves.
fn png_data_uri(png: &[u8]) -> String {
    use base64::Engine;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    )
}

// ── a11y-05: the desktop-card accessible name/state seam ──

#[test]
fn card_a11y_name_is_the_display_identity() {
    let state = parse_sources(FIXTURE).expect("fixture parses");
    // The accessible name is exactly the bold display name the card paints.
    assert_eq!(card_a11y_name(&state.sources[0]), "oak");
    assert_eq!(card_a11y_name(&state.sources[2]), "OfficePC");
}

#[test]
fn card_a11y_state_mirrors_the_visible_caption_and_carries_markers() {
    let state = parse_sources(FIXTURE).expect("fixture parses");
    let oak = &state.sources[0]; // reachable mesh peer, no power
    assert_eq!(
        card_a11y_state(oak, false, false, false),
        "reachable \u{00B7} mesh peer",
        "the base value mirrors the painted status \u{00B7} origin caption",
    );
    assert_eq!(
        card_a11y_state(oak, false, false, true),
        "reachable \u{00B7} mesh peer \u{00B7} recently used",
        "the recents marker matches the caption's CHOOSER-9 tail",
    );
    assert_eq!(
        card_a11y_state(oak, true, true, false),
        "reachable \u{00B7} mesh peer \u{00B7} pinned \u{00B7} connecting",
        "the pin dot + pending border ride the value as trailing markers",
    );
}

#[test]
fn card_a11y_state_uses_the_offline_reason_and_vm_power() {
    let state = parse_sources(FIXTURE).expect("fixture parses");
    // win11 is unreachable → the worker reason replaces the status/origin
    // caption (lock 14), and the VM power reading is appended — exactly the
    // two lines the greyed card paints.
    let win11 = &state.sources[1];
    assert_eq!(
        card_a11y_state(win11, false, false, false),
        "vm shut off \u{00B7} vm shut off",
    );
}

// ── the wire mirror ──

#[test]
fn topic_matches_the_worker_contract() {
    // Cross-check: MUST equal mackesd::workers::desktop_sources::SOURCES_TOPIC.
    assert_eq!(SOURCES_TOPIC, "state/desktops/sources");
}

#[test]
fn needs_broker_resolution_flags_a_portless_brokered_vm() {
    // VDI-VM-1 — a mesh-brokered VM with a port-less Spice offer resolves its
    // endpoint from the serving peer's broker record, not a discovery port.
    let vm = source("peer-vm:oak:win11", "oak", &[Protocol::Spice]);
    assert!(vm.endpoint_for(VdiProtocol::Spice).is_none());
    assert!(vm.needs_broker_resolution(VdiProtocol::Spice));

    // A mesh source that DID advertise a port resolves directly — no broker read.
    let mut seat = source("peer:oak", "oak", &[Protocol::Vnc]);
    seat.protocols = vec![ProtocolOffer {
        protocol: Protocol::Vnc,
        port: Some(5900),
    }];
    assert!(!seat.needs_broker_resolution(VdiProtocol::Vnc));

    // An off-mesh (manual) endpoint is never broker-resolved.
    let mut manual = source("manual:10.0.0.5:5900:vnc", "10.0.0.5", &[Protocol::Vnc]);
    manual.origin = SourceOrigin::Manual;
    manual.protocols = vec![ProtocolOffer {
        protocol: Protocol::Vnc,
        port: Some(5900),
    }];
    assert!(!manual.needs_broker_resolution(VdiProtocol::Vnc));
}

#[test]
fn the_chooser1_fixture_parses_to_the_projected_shape() {
    let state = fixture_state();
    assert_eq!(state.sources.len(), 3);

    // The peer seat: two offers with their well-known ports, an OS hint.
    let seat = &state.sources[0];
    assert_eq!(seat.id, "peer:oak");
    assert_eq!(seat.node, "oak");
    assert_eq!(seat.origin, SourceOrigin::MeshPeer);
    assert_eq!(seat.reachability, Reachability::Reachable);
    assert_eq!(seat.protocols.len(), 2);
    assert_eq!(seat.protocols[0].protocol, Protocol::Rdp);
    assert_eq!(seat.protocols[0].port, Some(3389));
    assert_eq!(seat.os_hint.as_deref(), Some("linux"));
    assert!(seat.thumbnail_ref.is_none(), "honestly null (CHOOSER-3)");
    assert!(seat.connectable());

    // The stopped VM: a brokered Spice offer (no port on the wire), the
    // worker's grey reason + power state, NOT connectable.
    let vm = &state.sources[1];
    assert_eq!(
        vm.protocols,
        vec![ProtocolOffer {
            protocol: Protocol::Spice,
            port: None
        }]
    );
    assert_eq!(vm.reachability, Reachability::Unreachable);
    assert_eq!(vm.reason.as_deref(), Some("vm shut off"));
    assert_eq!(vm.power_state.as_deref(), Some("shut off"));
    assert!(!vm.connectable());

    // The LAN endpoint.
    assert_eq!(state.sources[2].origin, SourceOrigin::Mdns);

    // The lanes, with the degraded one detectable.
    assert_eq!(state.lanes.len(), 4);
    let degraded: Vec<&str> = state
        .lanes
        .iter()
        .filter(|l| l.is_degraded())
        .map(|l| l.lane.as_str())
        .collect();
    assert_eq!(degraded, vec!["local-kvm"]);
}

#[test]
fn unknown_tags_degrade_honestly_instead_of_failing_the_parse() {
    // A future worker minting a new protocol / lane / reachability tag
    // must not blank the whole roster: the mirrors degrade per-field.
    let raw = r#"{
            "sources": [{
                "id": "x", "name": "x", "node": "n", "host": "n",
                "protocols": [{"protocol": "quic-desktop"}],
                "origin": "carrier-pigeon", "reachability": "flaky",
                "thumbnail_ref": null
            }],
            "lanes": []
        }"#;
    let state = parse_sources(raw).expect("degrades, not fails");
    let s = &state.sources[0];
    assert_eq!(s.protocols[0].protocol, Protocol::Unknown);
    assert_eq!(s.origin, SourceOrigin::Unknown);
    assert_eq!(s.reachability, Reachability::Unknown);
    assert!(s.connectable(), "an honest Unknown may try");
}

#[test]
fn malformed_state_is_an_honest_none() {
    assert!(parse_sources("not json").is_none());
}

#[test]
fn bus_client_without_a_root_reads_none_and_reports_no_bus() {
    let client = BusDesktopSources::with_root(None);
    assert!(client.latest().is_none(), "no Bus dir → an honest None");
    assert!(!client.has_bus());
}

// ── grouping ──

#[test]
fn group_by_node_folds_consecutive_runs_in_published_order() {
    let state = fixture_state();
    let groups = group_by_node(&state.sources);
    let shape: Vec<(&str, usize)> = groups.iter().map(|(n, m)| (*n, m.len())).collect();
    // The worker sorts by node: 192.168.1.60 < oak — but the fixture is
    // in oak-first order, and grouping preserves the PUBLISHED order
    // (the worker owns the sort; the surface must not re-order it).
    assert_eq!(shape, vec![("oak", 2), ("192.168.1.60", 1)]);
}

// ── the seen-set / auto-popup fold (design lock 1) ──

#[test]
fn first_fold_seeds_silently_then_a_new_source_pops_once() {
    let mut state = state_with(Some(roster(vec![source(
        "peer:oak",
        "oak",
        &[Protocol::Rdp],
    )])));
    // The pre-existing world seeds the seen set without a popup.
    assert!(!state.take_popup(), "startup must not pop the Chooser");

    // The same roster again: nothing new, no popup.
    state.fold_sources(roster(vec![source("peer:oak", "oak", &[Protocol::Rdp])]));
    assert!(!state.take_popup());

    // A genuinely new source pops — once.
    state.fold_sources(roster(vec![
        source("peer:oak", "oak", &[Protocol::Rdp]),
        source("vm:elm:dev", "elm", &[Protocol::Spice]),
    ]));
    assert!(state.take_popup(), "a new source raises the popup");
    assert!(!state.take_popup(), "the popup drains once");
}

#[test]
fn a_source_that_left_and_returned_does_not_repop() {
    let mut state = state_with(Some(roster(vec![source(
        "peer:oak",
        "oak",
        &[Protocol::Rdp],
    )])));
    let _ = state.take_popup();
    // oak flaps away and back: the operator already saw it — no re-pop.
    state.fold_sources(roster(vec![]));
    state.fold_sources(roster(vec![source("peer:oak", "oak", &[Protocol::Rdp])]));
    assert!(!state.take_popup(), "a seen source must not re-pop");
}

// ── the connect flow (CHOOSER-4) ──

#[test]
fn the_protocol_route_maps_wire_tags_to_vdi_routes() {
    // The routing fold: each renderable wire tag maps to its VDI route; an
    // unknown tag has none (badged, never connected blind — §7).
    assert_eq!(Protocol::Rdp.route(), Some(VdiProtocol::Rdp));
    assert_eq!(Protocol::Vnc.route(), Some(VdiProtocol::Vnc));
    assert_eq!(Protocol::Spice.route(), Some(VdiProtocol::Spice));
    assert_eq!(Protocol::Unknown.route(), None);
}

#[test]
fn a_single_protocol_source_still_asks_display_options_then_hands_off_once() {
    let mut state = state_with(Some(roster(vec![source(
        "peer-vm:oak:web1",
        "oak",
        &[Protocol::Spice],
    )])));
    let sources = state.sources_snapshot();

    // Even a single protocol opens the picker: fullscreen/windowed + the
    // monitor span are per-connection choices (locks 9/12), so activate must
    // NOT connect — it seeds the draft to the one offer.
    state.activate(&sources, "peer-vm:oak:web1");
    assert!(
        state.take_connect().is_none(),
        "activate opens the picker, not a connect"
    );
    assert_eq!(
        state.pending.as_ref().map(|d| d.protocol),
        Some(VdiProtocol::Spice)
    );

    state.confirm_connect(&sources);
    // The broker publish had no Bus root → the honest inline error (the same
    // discipline as the E12-5b picker), but the Desktop hand-off still
    // happens so the surface reflects the pending connect.
    assert!(state
        .last_error
        .as_deref()
        .is_some_and(|e| e.contains("Bus")));
    let req = state.take_connect().expect("a request was handed off");
    assert_eq!(req.target.serving_peer, "oak");
    assert_eq!(req.target.name, "web1");
    assert_eq!(req.protocol, VdiProtocol::Spice);
    assert_eq!(req.display, DisplayMode::Fullscreen, "seeded to fullscreen");
    assert_eq!(
        req.monitors,
        MonitorSpan::Single,
        "seeded to single display"
    );
    assert!(state.take_connect().is_none(), "the hand-off drains once");
    // The Spice route is live-client-capable now, so the note should describe
    // the brokered request without a stale CHOOSER-5 gate.
    assert!(state
        .note
        .as_deref()
        .is_some_and(|n| n.contains("brokering over the mesh") && !n.contains("CHOOSER-5")));
}

/// Fold a single external (mDNS) RDP endpoint into a fresh `ChooserState`
/// backed by `creds`, and return it + the source id.
fn external_state(creds: Box<dyn CredentialStore>) -> (ChooserState, String) {
    let mut state = state_with_store(None, creds);
    let mut lan = source(
        "mdns:192.168.1.60:3389:rdp",
        "192.168.1.60",
        &[Protocol::Rdp],
    );
    lan.origin = SourceOrigin::Mdns;
    lan.name = "OfficePC".to_string();
    // The dial address the credential ref is derived from (host:port).
    lan.host = "192.168.1.60:3389".to_string();
    state.fold_sources(roster(vec![lan]));
    (state, "mdns:192.168.1.60:3389:rdp".to_string())
}

#[test]
fn an_external_endpoint_prompts_once_seals_then_connects_without_a_broker_open() {
    // CHOOSER-6 — the full external fold: activate → the first Connect resolves
    // no sealed credential and raises a one-time prompt (does NOT connect) →
    // the operator fills it → the next Connect seals it + connects.
    let store = RecordingStore::default();
    let (mut state, id) = external_state(Box::new(store.clone()));
    let sources = state.sources_snapshot();

    // Phase 1: activate + Connect → the prompt is raised, nothing connects.
    state.activate(&sources, &id);
    state.confirm_connect(&sources);
    assert!(
        state.take_connect().is_none(),
        "an external endpoint with no sealed credential must not connect blind"
    );
    assert!(
        state
            .pending
            .as_ref()
            .is_some_and(|d| d.cred_prompt.is_some()),
        "the one-time credential prompt is raised"
    );

    // The operator fills the prompt once.
    {
        let prompt = state
            .pending
            .as_mut()
            .and_then(|d| d.cred_prompt.as_mut())
            .expect("the prompt is open");
        prompt.username = "administrator".to_string();
        prompt.password = "s3cr3t-pw".to_string();
    }

    // Phase 2: Connect → seals the credential + connects (no broker verb, no
    // Bus error — an external endpoint has no broker `Open`).
    state.confirm_connect(&sources);
    assert!(
        state.last_error.is_none(),
        "no broker verb for an off-mesh endpoint"
    );
    assert!(state.pending.is_none(), "the picker closes on connect");

    // The credential really round-tripped into the store (sealed), under the
    // derived `desktop/<host>/<proto>` ref.
    let sealed = store
        .get_ref("desktop/192.168.1.60:3389/rdp")
        .expect("the credential was sealed");
    assert_eq!(sealed.username, "administrator");
    assert_eq!(sealed.secret.expose(), "s3cr3t-pw");

    // The note names the gated direct-transport leg + that the credential is
    // sealed (remembered), and never leaks the secret.
    let note = state.note.clone().expect("a connect note");
    assert!(note.contains("RDP") && note.contains("E12-4"));
    assert!(note.contains("sealed") && note.contains("remembered"));
    assert!(!note.contains("s3cr3t-pw"), "the note leaked the secret");

    // The request carries the resolved sealed auth (secret redacted from Debug).
    let req = state.take_connect().expect("hand-off");
    assert_eq!(req.target.name, "OfficePC");
    assert_eq!(req.protocol, VdiProtocol::Rdp);
    assert_eq!(
        req.target
            .endpoint
            .as_ref()
            .map(|e| (e.host.as_str(), e.port)),
        Some(("192.168.1.60", 3389))
    );
    assert!(matches!(req.auth, DesktopAuth::Sealed { .. }));
    assert!(!format!("{req:?}").contains("s3cr3t-pw"));
}

#[test]
fn a_remembered_external_credential_connects_without_a_second_prompt() {
    // A store that already holds the sealed credential: activate + one Connect
    // connects straight through, no prompt (the "then remembered" half).
    let store = RecordingStore::default();
    assert!(matches!(
        store.seal(
            "desktop/192.168.1.60:3389/rdp",
            &crate::auth::Credential::new("administrator", "s3cr3t-pw"),
        ),
        SealOutcome::Sealed
    ));
    let (mut state, id) = external_state(Box::new(store));
    let sources = state.sources_snapshot();

    state.activate(&sources, &id);
    state.confirm_connect(&sources);
    // No prompt was raised (the credential was remembered) and it connected.
    assert!(state.pending.is_none(), "a remembered cred needs no prompt");
    let req = state
        .take_connect()
        .expect("connects with the remembered cred");
    let DesktopAuth::Sealed { credential, .. } = req.auth else {
        unreachable!("expected the remembered sealed cred")
    };
    assert_eq!(credential.username, "administrator");
    assert_eq!(credential.secret.expose(), "s3cr3t-pw");
}

#[test]
fn a_mesh_peer_connects_with_no_credential_prompt_via_sso() {
    // The SSO path: a mesh-brokered peer connects with the node's mesh identity
    // and no prompt is raised when there is no remembered guest credential.
    let store = RecordingStore::default();
    let mut state = state_with_store(
        Some(roster(vec![source("peer:oak", "oak", &[Protocol::Rdp])])),
        Box::new(store.clone()),
    );
    let sources = state.sources_snapshot();
    state.activate(&sources, "peer:oak");
    state.confirm_connect(&sources);
    assert!(state.pending.is_none(), "SSO needs no credential prompt");
    let req = state.take_connect().expect("SSO connects straight through");
    let DesktopAuth::MeshIdentity { node, guest } = req.auth else {
        unreachable!("expected mesh-identity SSO")
    };
    assert_eq!(node, "client-node");
    assert!(
        guest.is_none(),
        "no remembered guest credential was present"
    );
    assert_eq!(store.seal_count(), 0, "SSO resolution must not seal");
    // The broker publish had no Bus root → the honest inline error (mesh peer),
    // and the note names SSO, never a credential.
    assert!(
        req.broker_session.is_none(),
        "no lifecycle handle is attached when the Open publish had no Bus"
    );
    assert!(state.note.as_deref().is_some_and(|n| n.contains("SSO")));
}

#[test]
fn a_mesh_peer_connect_keeps_the_published_broker_session_id() {
    let dir = temp_bus_dir("vdi-open");
    let mut state = state_with_bus(
        Some(roster(vec![source("peer:oak", "oak", &[Protocol::Rdp])])),
        Some(dir.clone()),
    );
    let sources = state.sources_snapshot();
    state.activate(&sources, "peer:oak");
    state.confirm_connect(&sources);
    let req = state.take_connect().expect("SSO connects straight through");
    let broker = req
        .broker_session
        .as_ref()
        .expect("successful broker Open attaches lifecycle metadata");
    let persist = mde_bus::persist::Persist::open(dir.clone()).expect("open bus");
    let msgs = persist
        .list_since("action/vdi/session", None)
        .expect("list");
    assert_eq!(msgs.len(), 1);
    let body = msgs[0].body.as_deref().expect("body");
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(v["op"], "open");
    assert_eq!(v["id"], broker.id);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mesh_peer_carries_the_overlay_endpoint_into_the_vdi_request() {
    let mut oak = source("peer:oak", "oak", &[Protocol::Rdp]);
    oak.host = "10.42.0.7".to_string();
    oak.protocols = vec![ProtocolOffer {
        protocol: Protocol::Rdp,
        port: Some(3389),
    }];
    let mut state = state_with_store(Some(roster(vec![oak])), Box::new(RecordingStore::default()));
    let sources = state.sources_snapshot();
    state.activate(&sources, "peer:oak");
    state.confirm_connect(&sources);
    let req = state.take_connect().expect("SSO connects straight through");
    assert_eq!(
        req.target
            .endpoint
            .as_ref()
            .map(|endpoint| (endpoint.host.as_str(), endpoint.port)),
        Some(("10.42.0.7", 3389)),
        "worker-published overlay host + RDP port must reach live-vdi"
    );
}

#[test]
fn a_mesh_peer_can_carry_a_remembered_guest_credential_for_live_rdp() {
    let dir = temp_bus_dir("vdi-open-guest");
    let store = RecordingStore::default();
    assert_eq!(
        store.seal(
            "desktop/oak/rdp",
            &crate::auth::Credential::new("administrator", "mesh-rdp-pw"),
        ),
        SealOutcome::Sealed
    );
    let mut state = ChooserState::with_client(
        Box::new(FakeSources(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp],
        )])))),
        Some(dir.clone()),
        "client-node".to_string(),
        Box::new(store),
        inert_prefs(),
    );
    state.refresh();
    let sources = state.sources_snapshot();
    state.activate(&sources, "peer:oak");
    state.confirm_connect(&sources);
    let req = state.take_connect().expect("SSO connects straight through");
    assert!(
        req.broker_session.is_some(),
        "mesh guest login still keeps broker lifecycle tracking"
    );
    assert!(!format!("{req:?}").contains("mesh-rdp-pw"));
    let DesktopAuth::MeshIdentity {
        node,
        guest: Some(guest),
    } = &req.auth
    else {
        unreachable!("expected mesh identity plus remembered guest credential")
    };
    assert_eq!(node, "client-node");
    assert_eq!(guest.store_ref, "desktop/oak/rdp");
    assert_eq!(guest.credential.username, "administrator");
    assert_eq!(guest.credential.secret.expose(), "mesh-rdp-pw");
    assert!(
        state
            .note
            .as_deref()
            .is_some_and(|n| n.contains("sealed guest credential")),
        "the note names guest auth without leaking the secret"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_production_credential_store_gate_is_honest_on_an_external_connect() {
    // On the live fleet the seal is gated: an external connect still hands off
    // (the entered credential drives the session in-memory) but the note says
    // it isn't remembered — never a faked "sealed" (§7).
    let (mut state, id) = external_state(Box::new(MeshCredentialStore));
    let sources = state.sources_snapshot();
    state.activate(&sources, &id);
    state.confirm_connect(&sources); // phase 1 → prompt
    {
        let prompt = state
            .pending
            .as_mut()
            .and_then(|d| d.cred_prompt.as_mut())
            .expect("prompt open");
        prompt.password = "in-memory-only".to_string();
    }
    state.confirm_connect(&sources); // phase 2 → gated seal + connect
    let note = state.note.clone().expect("note");
    assert!(
        note.contains("isn't remembered"),
        "a gated seal is honest, not faked as remembered: {note}"
    );
    assert!(
        !note.contains("in-memory-only"),
        "the note leaked the secret"
    );
    assert!(
        state.take_connect().is_some(),
        "the session still hands off"
    );
}

#[test]
fn an_offline_source_never_connects() {
    let mut off = source("peer:ash", "ash", &[Protocol::Rdp]);
    off.reachability = Reachability::Unreachable;
    off.reason = Some("peer unreachable".to_string());
    let mut state = state_with(Some(roster(vec![off])));
    let sources = state.sources_snapshot();
    state.activate(&sources, "peer:ash");
    assert!(state.take_connect().is_none(), "greyed cards don't connect");
    assert!(
        state.pending.is_none(),
        "greyed cards don't open the picker"
    );
}

#[test]
fn an_unknown_only_source_offers_no_connectable_protocol() {
    // A source advertising only a tag this build can't route: activation opens
    // no picker and says so honestly — never a blind connect (§7).
    let mut state = state_with(Some(roster(vec![source(
        "peer:oak",
        "oak",
        &[Protocol::Unknown],
    )])));
    let sources = state.sources_snapshot();
    state.activate(&sources, "peer:oak");
    assert!(state.pending.is_none(), "no routable protocol → no picker");
    assert!(state.take_connect().is_none());
    assert!(state
        .note
        .as_deref()
        .is_some_and(|n| n.contains("no connectable protocol")));
}

#[test]
fn the_picker_seeds_the_first_routable_offer_skipping_unknown() {
    // [Unknown, Rdp]: the unknown tag is badged but never routed — the picker
    // seeds to RDP (the first routable offer).
    let mut state = state_with(Some(roster(vec![source(
        "peer:oak",
        "oak",
        &[Protocol::Unknown, Protocol::Rdp],
    )])));
    let sources = state.sources_snapshot();
    state.activate(&sources, "peer:oak");
    assert_eq!(
        state.pending.as_ref().map(|d| d.protocol),
        Some(VdiProtocol::Rdp)
    );
}

#[test]
fn a_multi_protocol_source_asks_the_protocol_and_connects_only_on_confirm() {
    let mut state = state_with(Some(roster(vec![source(
        "peer:oak",
        "oak",
        &[Protocol::Rdp, Protocol::Vnc],
    )])));
    let sources = state.sources_snapshot();

    // Activation raises the CHOOSER-4 picker seeded to the first offer — it
    // must NOT connect (lock 6 — always-ask, never a silent first-pick).
    state.activate(&sources, "peer:oak");
    assert_eq!(
        state.pending.as_ref().map(|d| d.source_id.as_str()),
        Some("peer:oak")
    );
    assert_eq!(
        state.pending.as_ref().map(|d| d.protocol),
        Some(VdiProtocol::Rdp)
    );
    assert!(state.take_connect().is_none(), "no silent first-pick");

    // Cancel backs out.
    state.cancel_connect();
    assert!(state.pending.is_none());

    // Ask again, pick VNC + windowed + span-all, then confirm — the request
    // is built from exactly those choices (the CHOOSER-4 construction fold).
    state.activate(&sources, "peer:oak");
    {
        let draft = state.pending.as_mut().expect("the picker is open");
        draft.protocol = VdiProtocol::Vnc;
        draft.display = DisplayMode::Windowed;
        draft.monitors = MonitorSpan::All;
    }
    state.confirm_connect(&sources);
    assert!(state.pending.is_none());
    let req = state.take_connect().expect("confirm connects");
    assert_eq!(req.target.serving_peer, "oak");
    assert_eq!(req.protocol, VdiProtocol::Vnc);
    assert_eq!(req.display, DisplayMode::Windowed);
    assert_eq!(req.monitors, MonitorSpan::All);
}

// ── headless mount renders (the DRM runner's path, minus the GPU) ──

/// Drive one headless 960×640 frame of `chooser_panel` and tessellate it
/// on the CPU — the same `Context::run` → `tessellate` path the DRM
/// runner drives. Returns whether it produced draw primitives.
fn run_panel(state: &mut ChooserState) -> bool {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| chooser_panel(ui, state));
    });
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    !prims.is_empty()
}

fn panel_text_positions(state: &mut ChooserState) -> Vec<(String, egui::Pos2)> {
    fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
        match shape {
            egui::Shape::Text(text) => out.push((text.galley.text().to_owned(), text.pos)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }

    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| chooser_panel(ui, state));
    });
    let mut texts = Vec::new();
    for clipped in out.shapes {
        walk(&clipped.shape, &mut texts);
    }
    texts
}

fn painted_text_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Color32)> {
    fn text_color(text: &egui::epaint::TextShape) -> egui::Color32 {
        if let Some(color) = text.override_text_color {
            return color;
        }
        text.galley
            .job
            .sections
            .iter()
            .find_map(|section| {
                (section.format.color != egui::Color32::PLACEHOLDER).then_some(section.format.color)
            })
            .unwrap_or(text.fallback_color)
    }

    fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
        match shape {
            egui::Shape::Text(text) => out.push((text.galley.text().to_owned(), text_color(text))),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut texts = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut texts);
    }
    texts
}

fn painted_fills(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
        match shape {
            egui::Shape::Rect(rect) if rect.fill != egui::Color32::TRANSPARENT => {
                out.push(rect.fill);
            }
            egui::Shape::Path(path) if path.fill != egui::Color32::TRANSPARENT => {
                out.push(path.fill);
            }
            egui::Shape::Mesh(mesh) => {
                out.extend(mesh.vertices.iter().map(|vertex| vertex.color));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut fills = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut fills);
    }
    fills
}

#[test]
fn chooser_hover_tooltip_uses_themed_text_and_surface() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 120.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                chooser_tooltip(ui, "Remote Sessions endpoint");
            });
    });

    let texts = painted_text_colors(&out.shapes);
    assert!(
        texts
            .iter()
            .any(|(text, color)| text == "Remote Sessions endpoint" && *color == Style::TEXT),
        "Remote Sessions tooltip should paint themed text: {texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|(text, color)| text == "Remote Sessions endpoint"
                && *color == egui::Color32::BLACK),
        "Remote Sessions tooltip leaked raw black popup text: {texts:?}"
    );

    let fills = painted_fills(&out.shapes);
    assert!(
        fills.contains(&Style::SURFACE),
        "Remote Sessions tooltip should paint its own themed surface: {fills:?}"
    );
}

#[test]
fn chooser_popup_surfaces_use_themed_text_and_compact_spacing() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut style = (*ctx.style()).clone();
    apply_chooser_popup_style(&ctx, &mut style);

    assert_eq!(style.visuals.window_fill, Style::SURFACE);
    assert_eq!(style.visuals.panel_fill, Style::SURFACE);
    assert_eq!(style.visuals.window_stroke.color, Style::BORDER);
    assert_eq!(style.visuals.override_text_color, Some(Style::TEXT));
    assert_eq!(style.visuals.widgets.inactive.fg_stroke.color, Style::TEXT);
    assert_eq!(style.visuals.widgets.hovered.bg_fill, Style::SURFACE_HI);
    assert_eq!(style.visuals.widgets.hovered.fg_stroke.color, Style::TEXT);
    assert_eq!(style.visuals.widgets.open.bg_fill, Style::SURFACE_HI);
    assert_eq!(style.spacing.button_padding.y, Style::CONTROL_PAD_Y);
    assert_eq!(style.spacing.item_spacing.y, Style::TOOLBAR_INSET_Y);

    let light = egui::Context::default();
    Style::install_color_scheme_with_density(&light, StyleColorScheme::Light, Density::Mouse);
    let palette = Style::palette_for(StyleColorScheme::Light);
    let mut light_style = (*light.style()).clone();
    apply_chooser_popup_style(&light, &mut light_style);

    assert_eq!(light_style.visuals.window_fill, palette.surface);
    assert_eq!(light_style.visuals.panel_fill, palette.surface);
    assert_eq!(light_style.visuals.window_stroke.color, palette.border);
    assert_eq!(light_style.visuals.override_text_color, Some(palette.text));
    assert_eq!(
        light_style.visuals.widgets.inactive.fg_stroke.color,
        palette.text
    );
    assert_ne!(
        light_style.visuals.widgets.inactive.fg_stroke.color,
        Style::TEXT,
        "Remote Sessions chooser popups must resolve light text instead of raw dark text"
    );
}

#[test]
fn an_empty_roster_renders_the_backdrop_with_the_honest_reason() {
    // A published-but-quiet roster: the BRAND-1 hero + the quiet-lane copy.
    let mut state = state_with(Some(DesktopSourcesState {
        sources: vec![],
        lanes: vec![LaneStatus {
            lane: "local-kvm".to_string(),
            status: "gated: virsh not found".to_string(),
        }],
    }));
    let (title, detail) = state.empty_copy();
    assert_eq!(title, "No desktops discovered");
    assert!(
        detail.contains("local-kvm") && detail.contains("gated"),
        "the quiet lane is named: {detail}"
    );
    assert!(
        run_panel(&mut state),
        "the empty Chooser backdrop produced no draw primitives"
    );
    assert!(state.take_connect().is_none());

    // No published record yet is a DIFFERENT honest truth.
    let mut unreported = state_with(None);
    let (title, _) = unreported.empty_copy();
    assert_eq!(title, "Desktop discovery hasn't reported yet");
    assert!(run_panel(&mut unreported));
}

#[test]
fn empty_roster_title_renders_near_the_workspace_center() {
    let mut state = state_with(Some(DesktopSourcesState {
        sources: vec![],
        lanes: vec![],
    }));
    let texts = panel_text_positions(&mut state);
    let y = texts
        .iter()
        .find_map(|(text, pos)| (text == "No desktops discovered").then_some(pos.y))
        .expect("empty title should paint");

    assert!(
        y > 250.0 && y < 360.0,
        "No desktops discovered should be centered in the workspace, painted at y={y}; texts={texts:?}"
    );
}

#[test]
fn a_missing_bus_reads_as_gated_not_as_a_quiet_mesh() {
    // §7 — a gated read must not render as a live-looking "no desktops".
    let state = ChooserState::with_client(
        Box::new(BusDesktopSources::with_root(None)),
        None,
        "client-node".to_string(),
        Box::new(MeshCredentialStore),
        inert_prefs(),
    );
    let (title, detail) = state.empty_copy();
    assert_eq!(title, "Desktop discovery unavailable");
    assert!(detail.contains("Bus") && detail.contains("unblocks"));
}

#[test]
fn a_populated_roster_renders_the_grouped_card_grid() {
    let mut state = state_with(Some(fixture_state()));
    assert!(
        run_panel(&mut state),
        "the card grid produced no draw primitives"
    );
}

#[test]
fn an_offline_source_renders_greyed_with_its_reason() {
    // The fixture's stopped VM is the greyed card; the render must
    // tessellate (the grey path draws real geometry + the reason).
    let mut state = state_with(Some(roster(vec![{
        let mut vm = source("peer-vm:oak:win11", "oak", &[Protocol::Spice]);
        vm.reachability = Reachability::Unreachable;
        vm.reason = Some("vm shut off".to_string());
        vm.power_state = Some("shut off".to_string());
        vm
    }])));
    assert!(
        run_panel(&mut state),
        "the offline-greyed card produced no draw primitives"
    );
}

#[test]
fn the_raised_connect_picker_renders_the_chooser4_affordance() {
    let mut state = state_with(Some(roster(vec![source(
        "peer:oak",
        "oak",
        &[Protocol::Rdp, Protocol::Vnc],
    )])));
    let sources = state.sources_snapshot();
    state.activate(&sources, "peer:oak");
    assert!(
        run_panel(&mut state),
        "the connect-picker affordance produced no draw primitives"
    );
    // Rendering the picker is not a connect.
    assert!(state.take_connect().is_none());
}

#[test]
fn the_external_credential_prompt_renders_with_masked_fields() {
    // CHOOSER-6 — an external endpoint whose first Connect found no sealed
    // credential renders the one-time username/password prompt (§4 tokens); it
    // tessellates and still hasn't connected (nothing connects blind).
    let store = RecordingStore::default();
    let (mut state, id) = external_state(Box::new(store));
    let sources = state.sources_snapshot();
    state.activate(&sources, &id);
    state.confirm_connect(&sources); // raise the prompt
    assert!(
        state
            .pending
            .as_ref()
            .is_some_and(|d| d.cred_prompt.is_some()),
        "the credential prompt is raised"
    );
    assert!(
        run_panel(&mut state),
        "the credential-prompt picker produced no draw primitives"
    );
    assert!(
        state.take_connect().is_none(),
        "rendering the prompt is not a connect"
    );
}

#[test]
fn a_spice_picker_renders_without_a_stale_chooser5_gate() {
    // A Spice-only source: the picker renders through the normal live-client
    // path and does not show the retired CHOOSER-5 gate.
    let mut state = state_with(Some(roster(vec![source(
        "peer-vm:oak:win11",
        "oak",
        &[Protocol::Spice],
    )])));
    let sources = state.sources_snapshot();
    state.activate(&sources, "peer-vm:oak:win11");
    assert!(
        run_panel(&mut state),
        "the Spice picker produced no draw primitives"
    );
    assert!(state
        .pending
        .as_ref()
        .is_some_and(|draft| draft.protocol == VdiProtocol::Spice));
    assert!(state.take_connect().is_none(), "rendering is not a connect");
}

// ── CHOOSER-3: the thumbnail decode + bounded/throttled cache ──

#[test]
fn a_png_data_uri_ref_decodes_to_an_image_of_the_right_size() {
    let img = decode_data_uri_png(&png_data_uri(&tiny_png(4, 3)))
        .expect("a valid base64 PNG data URI decodes");
    assert_eq!(img.size, [4, 3], "the decode keeps the snapshot dimensions");
}

#[test]
fn a_malformed_or_unsupported_ref_is_an_honest_none() {
    // Not a data URI at all.
    assert!(decode_data_uri_png("not a data uri").is_none());
    // A data URI, but not base64-encoded.
    assert!(decode_data_uri_png("data:image/png,QUJD").is_none());
    // A mediatype the shell doesn't decode (only PNG snapshots).
    assert!(decode_data_uri_png("data:image/jpeg;base64,QUJD").is_none());
    // Well-formed base64 whose bytes are not a PNG (`QUJD` == "ABC").
    assert!(decode_data_uri_png("data:image/png;base64,QUJD").is_none());
    // Garbage base64 payload.
    assert!(decode_data_uri_png("data:image/png;base64,%%%not-base64%%%").is_none());
}

#[test]
fn source_to_thumbnail_plumbing_decodes_a_ref_and_falls_back_without_one() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut cache = ThumbnailCache::default();

    // A source carrying a real snapshot ref → a decoded, uploaded texture.
    let mut with_thumb = source("peer:oak", "oak", &[Protocol::Rdp]);
    with_thumb.thumbnail_ref = Some(png_data_uri(&tiny_png(6, 4)));
    let tex = cache
        .texture_for(&ctx, &with_thumb)
        .expect("a real snapshot ref resolves to a texture");
    assert_eq!(tex.size(), [6, 4], "the well shows the decoded snapshot");

    // A second frame with the SAME ref must NOT re-decode — the cached
    // handle (same texture id) is returned (Q7: never decode per frame).
    let again = cache
        .texture_for(&ctx, &with_thumb)
        .expect("the cached texture is returned");
    assert_eq!(again.id(), tex.id(), "an unchanged ref reuses the cache");

    // A source WITHOUT a ref → no texture → the honest monitor-icon fallback.
    let bare = source("peer:elm", "elm", &[Protocol::Rdp]);
    assert!(
        cache.texture_for(&ctx, &bare).is_none(),
        "no ref → the icon fallback, never a fake preview (§7)"
    );
}

#[test]
fn the_decode_gate_is_first_sight_then_change_plus_throttle() {
    let t0 = Instant::now();
    let slot = ThumbSlot {
        ref_key: Some("snap-a".to_string()),
        texture: None,
        decoded_at: t0,
        used: 0,
    };
    // Never-seen source: decode now.
    assert!(ThumbnailCache::needs_decode(None, Some("snap-a"), t0));
    assert!(
        ThumbnailCache::needs_decode(None, None, t0),
        "a no-ref miss is cached too"
    );
    // Same ref: never re-decode (this is the per-frame no-op).
    assert!(!ThumbnailCache::needs_decode(
        Some(&slot),
        Some("snap-a"),
        t0
    ));
    // Changed ref but within the throttle window: keep the (stale) cache.
    assert!(!ThumbnailCache::needs_decode(
        Some(&slot),
        Some("snap-b"),
        t0
    ));
    // Changed ref AND the throttle window elapsed: re-decode the new snapshot.
    let later = t0 + THUMB_MIN_DECODE_INTERVAL + Duration::from_secs(1);
    assert!(ThumbnailCache::needs_decode(
        Some(&slot),
        Some("snap-b"),
        later
    ));
    // …but an unchanged ref stays a no-op even past the window.
    assert!(!ThumbnailCache::needs_decode(
        Some(&slot),
        Some("snap-a"),
        later
    ));
}

#[test]
fn the_cache_is_lru_bounded() {
    let ctx = egui::Context::default();
    let mut cache = ThumbnailCache::default();
    // Touch more distinct sources than the cap; each first sight inserts a
    // slot (a no-ref miss is enough to exercise the eviction).
    for i in 0..(THUMB_CACHE_CAP + 6) {
        let s = source(&format!("peer:n{i}"), "n", &[Protocol::Rdp]);
        let _ = cache.texture_for(&ctx, &s);
    }
    assert_eq!(
        cache.slots.len(),
        THUMB_CACHE_CAP,
        "the live texture set is bounded (Q7)"
    );
    // The earliest-touched ids were evicted; the most-recent survive.
    assert!(
        !cache.slots.contains_key("peer:n0"),
        "the LRU slot was evicted"
    );
    assert!(
        cache
            .slots
            .contains_key(&format!("peer:n{}", THUMB_CACHE_CAP + 5)),
        "the most-recently shown card is retained"
    );
}

#[test]
fn a_thumbnailed_card_renders_the_decoded_preview_end_to_end() {
    let mut thumbnailed = source("peer:oak", "oak", &[Protocol::Rdp]);
    thumbnailed.thumbnail_ref = Some(png_data_uri(&tiny_png(8, 6)));
    let mut state = state_with(Some(roster(vec![thumbnailed])));
    assert!(
        run_panel(&mut state),
        "the thumbnailed card produced no draw primitives"
    );
    // The render ran the full source→texture path into the bounded cache.
    assert_eq!(state.thumbs.slots.len(), 1);
    assert!(
        state.thumbs.slots.values().all(|s| s.texture.is_some()),
        "the card's snapshot decoded to a live texture"
    );
}

// ── CHOOSER-7: local-VM power controls ──

/// A local-KVM source row in the exact `source_from_vm` shape (Spice console,
/// reachability + reason derived from the power state) — the aggregator's
/// `local-kvm` lane projection this surface renders.
fn local_vm(name: &str, node: &str, power: &str) -> DesktopSource {
    let live = matches!(power.trim(), "running" | "paused");
    DesktopSource {
        id: format!("vm:{node}:{name}"),
        name: name.to_string(),
        node: node.to_string(),
        host: node.to_string(),
        protocols: vec![ProtocolOffer {
            protocol: Protocol::Spice,
            port: None,
        }],
        origin: SourceOrigin::LocalVm,
        reachability: if live {
            Reachability::Reachable
        } else {
            Reachability::Unreachable
        },
        reason: (!live).then(|| format!("vm {power}")),
        os_hint: None,
        power_state: Some(power.to_string()),
        thumbnail_ref: None,
    }
}

fn lane(name: &str, status: &str) -> LaneStatus {
    LaneStatus {
        lane: name.to_string(),
        status: status.to_string(),
    }
}

#[test]
fn power_state_reflection_offers_state_appropriate_actions() {
    // State reflection: the card offers only the ops valid for the published
    // power state — a stopped VM starts (one click away), a running one
    // stops/pauses, a paused one resumes/stops, an unmapped state nothing.
    assert_eq!(
        PowerState::from_wire("shut off").actions().to_vec(),
        vec![PowerOp::Start]
    );
    assert_eq!(
        PowerState::from_wire("crashed").actions().to_vec(),
        vec![PowerOp::Start],
        "a crashed VM can be re-Started"
    );
    assert_eq!(
        PowerState::from_wire("running").actions().to_vec(),
        vec![PowerOp::Stop, PowerOp::Pause]
    );
    assert_eq!(
        PowerState::from_wire("paused").actions().to_vec(),
        vec![PowerOp::Resume, PowerOp::Stop]
    );
    assert!(
        PowerState::from_wire("pmsuspended").actions().is_empty(),
        "an unmapped state offers no blind action (§7)"
    );
}

#[test]
fn the_lifecycle_topic_matches_the_worker_contract() {
    // Cross-check: MUST equal mackesd::workers::vm_lifecycle::ACTION_TOPIC.
    assert_eq!(LIFECYCLE_TOPIC, "action/vm/lifecycle");
}

#[test]
fn power_publish_mints_an_exact_body_bound_direct_libvirt_capability() {
    let tmp = tempfile::tempdir().unwrap();
    let mut error = None;
    let request = PowerOp::Pause.to_request("elm", "dev");
    publish_power(Some(tmp.path()), &mut error, &request);
    assert!(error.is_none(), "{error:?}");
    let persist = mde_bus::persist::Persist::open(tmp.path().to_path_buf()).unwrap();
    let messages = persist.list_since(LIFECYCLE_TOPIC, None).unwrap();
    assert_eq!(messages.len(), 1);
    let body = messages[0].body.as_deref().unwrap();
    let value: serde_json::Value = serde_json::from_str(body).unwrap();
    let token =
        mackes_mesh_types::cloud::CloudArmedToken::parse(value["armed_token"].as_str().unwrap())
            .unwrap();
    assert_eq!(token.verb, "vm-pause");
    assert_eq!(token.node, "elm");
    assert_eq!(token.target, "dev");
    assert_eq!(
        token.request_sha256,
        mackes_mesh_types::cloud::cloud_request_digest(body).unwrap()
    );
}

#[test]
fn power_ops_map_to_the_host_targeted_vm_lifecycle_verbs() {
    // Action dispatch (wire): each op serialises to the worker's LifecycleAction
    // shape, host-targeted so it can only act on the named node.
    let body = |op: PowerOp| op.to_request("elm", "dev").to_body();
    let start: serde_json::Value = serde_json::from_str(&body(PowerOp::Start)).unwrap();
    assert_eq!(start["op"], "start");
    assert_eq!(start["schema_version"], 1);
    let stop: serde_json::Value = serde_json::from_str(&body(PowerOp::Stop)).unwrap();
    assert_eq!(stop["op"], "stop");
    assert_eq!(stop["force"], false, "the card issues a graceful stop");
    let pause: serde_json::Value = serde_json::from_str(&body(PowerOp::Pause)).unwrap();
    assert_eq!(pause["op"], "pause");
    let resume: serde_json::Value = serde_json::from_str(&body(PowerOp::Resume)).unwrap();
    assert_eq!(resume["op"], "resume");
    for op in [
        PowerOp::Start,
        PowerOp::Stop,
        PowerOp::Pause,
        PowerOp::Resume,
    ] {
        let v: serde_json::Value = serde_json::from_str(&body(op)).unwrap();
        assert_eq!(v["schema_version"], 1, "versioned action contract");
        assert_eq!(v["host"], "elm", "host-targeted");
        assert_eq!(v["name"], "dev");
    }
}

#[test]
fn build_power_request_targets_local_vms_and_skips_peers() {
    // Action dispatch (source→request): a local VM maps to a Start for its own
    // node + name; a peer VM/seat is powered from ITS node, never from here.
    let sources = vec![
        local_vm("dev", "elm", "shut off"),
        source("peer:oak", "oak", &[Protocol::Rdp]),
    ];
    let req = build_power_request(&sources, "vm:elm:dev", PowerOp::Start).expect("local maps");
    let v: serde_json::Value = serde_json::from_str(&req.to_body()).unwrap();
    assert_eq!(v["op"], "start");
    assert_eq!(v["host"], "elm");
    assert_eq!(v["name"], "dev");
    assert!(
        build_power_request(&sources, "peer:oak", PowerOp::Stop).is_none(),
        "a peer source is not driven from here"
    );
    assert!(
        build_power_request(&sources, "vm:elm:ghost", PowerOp::Start).is_none(),
        "a vanished id maps to nothing"
    );
}

#[test]
fn the_no_hypervisor_gate_reads_the_local_kvm_lane_status() {
    // The honest-gate fold: a gated/errored local-kvm lane surfaces its reason
    // (power controls disable); a live "ok" lane does not gate.
    assert_eq!(
        local_hypervisor_gate(&[lane("local-kvm", "gated: virsh not found")]).as_deref(),
        Some("gated: virsh not found")
    );
    assert!(
        local_hypervisor_gate(&[lane("local-kvm", "error: libvirt refused")]).is_some(),
        "a backend error also gates"
    );
    assert!(
        local_hypervisor_gate(&[lane("local-kvm", "ok (2 vms)")]).is_none(),
        "a live hypervisor does not gate"
    );
    assert!(
        local_hypervisor_gate(&[lane("mdns", "ok")]).is_none(),
        "no local-kvm lane → no gate"
    );
    assert!(local_hypervisor_gate(&[]).is_none());
}

#[test]
fn a_local_vm_power_click_routes_through_the_lifecycle_emitter() {
    // Driving a card power op with no Bus root records the honest publish error
    // (never a panic) — proving the click reaches the shared vm_lifecycle
    // emitter rather than faking a local state flip (§7).
    let mut state = state_with(Some(roster(vec![local_vm("dev", "elm", "shut off")])));
    let sources = state.sources_snapshot();
    state.power_action(&sources, "vm:elm:dev", PowerOp::Start);
    assert!(
        state
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("Bus")),
        "no Bus dir surfaces an honest error, not a panic: {:?}",
        state.last_error
    );

    // A peer/non-local source is a no-op here (no error, no note).
    let mut peer = state_with(Some(roster(vec![source(
        "peer:oak",
        "oak",
        &[Protocol::Rdp],
    )])));
    let peers = peer.sources_snapshot();
    peer.power_action(&peers, "peer:oak", PowerOp::Start);
    assert!(
        peer.last_error.is_none() && peer.note.is_none(),
        "a non-local source is never driven from the Chooser"
    );
}

#[test]
fn a_stopped_local_vm_card_renders_the_power_controls() {
    // The shut-off local VM greys (offline) but its Start button draws at full
    // strength — the "one click away" affordance tessellates, and rendering it
    // is not a connect.
    let mut state = state_with(Some(roster(vec![local_vm("dev", "elm", "shut off")])));
    assert!(
        run_panel(&mut state),
        "the local-VM power row produced no draw primitives"
    );
    assert!(state.take_connect().is_none());
}

#[test]
fn a_gated_local_kvm_lane_renders_disabled_power_controls_with_the_reason() {
    // A LocalVm card while the local-kvm lane reports no hypervisor: the buttons
    // render disabled and the honest reason draws (§7 — never a control that
    // pretends to act).
    let state = DesktopSourcesState {
        sources: vec![local_vm("dev", "elm", "running")],
        lanes: vec![lane("local-kvm", "gated: virsh not found")],
    };
    let mut cs = state_with(Some(state));
    assert!(
        run_panel(&mut cs),
        "the gated power row produced no draw primitives"
    );
}

// ── CHOOSER-8: card actions + find + non-blocking offline states ──

/// A manual (operator-added) source row in the aggregator's `source_from_manual`
/// shape — origin `Manual`, never probed (an honest `Unknown` reachability).
fn manual_source(host: &str, port: u16, proto: Protocol) -> DesktopSource {
    DesktopSource {
        id: format!("manual:{host}:{port}:{}", proto.wire_tag().unwrap_or("?")),
        name: format!("{host}:{port}"),
        node: host.to_string(),
        host: host.to_string(),
        protocols: vec![ProtocolOffer {
            protocol: proto,
            port: Some(port),
        }],
        origin: SourceOrigin::Manual,
        reachability: Reachability::Unknown,
        reason: None,
        os_hint: None,
        power_state: None,
        thumbnail_ref: None,
    }
}

/// [`state_with`] over an explicit publish Bus root (a real temp spool) so the
/// CHOOSER-8 verbs can be read back off the topic they land on.
fn state_with_bus(state: Option<DesktopSourcesState>, bus_root: Option<PathBuf>) -> ChooserState {
    let mut s = ChooserState::with_client(
        Box::new(FakeSources(state)),
        bus_root,
        "client-node".to_string(),
        Box::new(MeshCredentialStore),
        inert_prefs(),
    );
    s.refresh();
    s
}

/// [`state_with_bus`] with a CHOOSER-9 prefs session over an explicit workgroup
/// root + seat — so the two-seat sync tests can pin at one seat and read the
/// roamed record at another over one shared mesh dir.
fn state_with_prefs(
    state: Option<DesktopSourcesState>,
    bus_root: Option<PathBuf>,
    prefs_root: PathBuf,
    seat: &str,
) -> ChooserState {
    let mut s = ChooserState::with_client(
        Box::new(FakeSources(state)),
        bus_root,
        "client-node".to_string(),
        Box::new(MeshCredentialStore),
        prefs_at(prefs_root, seat),
    );
    s.refresh();
    s
}

/// A unique temp Bus dir (the crate's `std::env::temp_dir()` idiom — no
/// `tempfile` dep), cleaned up by each test that uses it.
fn temp_bus_dir(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("mde-chooser8-{tag}-{n}"))
}

#[test]
fn the_desktop_action_topics_match_the_worker_contract() {
    // Cross-check: MUST equal the mackesd desktop_sources worker's verb topics.
    assert_eq!(ADD_SOURCE_TOPIC, "action/desktops/add-source");
    assert_eq!(REMOVE_SOURCE_TOPIC, "action/desktops/remove-source");
    assert_eq!(REFRESH_TOPIC, "action/desktops/refresh");
}

#[test]
fn add_and_remove_source_bodies_match_the_worker_shape() {
    // The add-source body carries host + port + protocol (§9 — never a command
    // string); an absent name is skipped so the worker defaults it to host:port.
    let add = AddSourceRequest {
        name: Some("OfficePC".to_string()),
        host: "10.0.0.5".to_string(),
        port: 3389,
        protocol: "rdp",
    };
    let v: serde_json::Value = serde_json::from_str(&add.to_body()).unwrap();
    assert_eq!(v["name"], "OfficePC");
    assert_eq!(v["host"], "10.0.0.5");
    assert_eq!(v["port"], 3389);
    assert_eq!(v["protocol"], "rdp");
    let bare = AddSourceRequest {
        name: None,
        host: "h".to_string(),
        port: 1,
        protocol: "vnc",
    };
    let v2: serde_json::Value = serde_json::from_str(&bare.to_body()).unwrap();
    assert!(
        v2.get("name").is_none(),
        "a None name is skipped on the wire"
    );
    // The remove-source body is just the manual id.
    let rm = RemoveSourceRequest {
        id: "manual:h:1:vnc".to_string(),
    };
    let vr: serde_json::Value = serde_json::from_str(&rm.to_body()).unwrap();
    assert_eq!(vr["id"], "manual:h:1:vnc");
}

#[test]
fn the_search_matches_name_node_host_and_os() {
    let state = fixture_state();
    let mut f = FilterSort::default();
    let hits = |f: &FilterSort| -> Vec<String> {
        state
            .sources
            .iter()
            .filter(|s| f.matches(s))
            .map(|s| s.id.clone())
            .collect()
    };
    // Name substring (case-insensitive).
    f.search = "office".to_string();
    assert_eq!(hits(&f), vec!["mdns:192.168.1.60:3389:rdp"]);
    // OS hint — only the peer seat carries "linux".
    f.search = "LINUX".to_string();
    assert_eq!(hits(&f), vec!["peer:oak"]);
    // Node/host substring.
    f.search = "192.168".to_string();
    assert_eq!(hits(&f), vec!["mdns:192.168.1.60:3389:rdp"]);
    // A blank/whitespace query matches the whole roster.
    f.search = "   ".to_string();
    assert_eq!(hits(&f).len(), 3);
}

#[test]
fn filters_narrow_by_node_protocol_status_and_os() {
    let state = fixture_state();
    let count = |f: &FilterSort| state.sources.iter().filter(|s| f.matches(s)).count();

    assert_eq!(
        count(&FilterSort {
            node: Some("oak".to_string()),
            ..Default::default()
        }),
        2,
        "oak groups the seat + its VM"
    );
    assert_eq!(
        count(&FilterSort {
            protocol: Some(Protocol::Spice),
            ..Default::default()
        }),
        1,
        "only the VM offers Spice"
    );
    assert_eq!(
        count(&FilterSort {
            status: Some(Reachability::Reachable),
            ..Default::default()
        }),
        2
    );
    assert_eq!(
        count(&FilterSort {
            status: Some(Reachability::Unreachable),
            ..Default::default()
        }),
        1,
        "the offline VM"
    );
    assert_eq!(
        count(&FilterSort {
            os: Some("linux".to_string()),
            ..Default::default()
        }),
        1
    );
}

#[test]
fn is_active_and_clear_reset_the_narrowing_but_keep_the_sort() {
    let mut f = FilterSort::default();
    assert!(!f.is_active(), "a default filter narrows nothing");
    f.search = " win ".to_string();
    assert!(f.is_active());
    f.search.clear();
    f.protocol = Some(Protocol::Rdp);
    f.node = Some("oak".to_string());
    assert!(f.is_active());
    f.sort = SortKey::Name;
    f.clear();
    assert!(!f.is_active(), "clear drops every filter + the search");
    assert_eq!(f.sort, SortKey::Name, "clear keeps the sort preference");
}

#[test]
fn distinct_nodes_and_os_feed_the_filter_combos() {
    let state = fixture_state();
    // First-seen (published) order, deduped case-insensitively.
    assert_eq!(distinct_nodes(&state.sources), vec!["oak", "192.168.1.60"]);
    assert_eq!(distinct_os(&state.sources), vec!["linux"]);
}

#[test]
fn order_members_floats_favorites_then_applies_the_sort_key() {
    let zeta = source("peer:zeta", "n", &[Protocol::Rdp]);
    let alpha = source("peer:alpha", "n", &[Protocol::Rdp]);
    let mid = source("peer:mid", "n", &[Protocol::Rdp]);
    let ids = |m: &[&DesktopSource]| m.iter().map(|s| s.id.clone()).collect::<Vec<_>>();

    // `Discovered` is a stable no-op — the published order is preserved.
    let mut m = vec![&zeta, &alpha, &mid];
    order_members(&mut m, SortKey::Discovered, &HashSet::new());
    assert_eq!(ids(&m), vec!["peer:zeta", "peer:alpha", "peer:mid"]);

    // `Name` sorts A→Z within the group.
    let mut m = vec![&zeta, &alpha, &mid];
    order_members(&mut m, SortKey::Name, &HashSet::new());
    assert_eq!(ids(&m), vec!["peer:alpha", "peer:mid", "peer:zeta"]);

    // A favorite floats first, ahead of the sort key.
    let favs: HashSet<String> = std::iter::once("peer:zeta".to_string()).collect();
    let mut m = vec![&zeta, &alpha, &mid];
    order_members(&mut m, SortKey::Name, &favs);
    assert_eq!(ids(&m), vec!["peer:zeta", "peer:alpha", "peer:mid"]);
}

#[test]
fn the_status_sort_floats_reachable_before_offline() {
    let mut up = source("peer:up", "n", &[Protocol::Rdp]);
    up.reachability = Reachability::Reachable;
    let mut down = source("peer:down", "n", &[Protocol::Rdp]);
    down.reachability = Reachability::Unreachable;
    let mut unk = source("peer:unk", "n", &[Protocol::Rdp]);
    unk.reachability = Reachability::Unknown;
    let mut m = vec![&down, &unk, &up];
    order_members(&mut m, SortKey::Status, &HashSet::new());
    assert_eq!(
        m.iter().map(|s| s.reachability).collect::<Vec<_>>(),
        vec![
            Reachability::Reachable,
            Reachability::Unknown,
            Reachability::Unreachable
        ]
    );
}

#[test]
fn toggle_favorite_pins_then_unpins() {
    let mut state = state_with(Some(roster(vec![source(
        "peer:oak",
        "oak",
        &[Protocol::Rdp],
    )])));
    assert!(!state.favorites.contains("peer:oak"));
    state.toggle_favorite("peer:oak");
    assert!(state.favorites.contains("peer:oak"), "a pin adds it");
    state.toggle_favorite("peer:oak");
    assert!(
        !state.favorites.contains("peer:oak"),
        "a second toggle unpins"
    );
}

#[test]
fn an_offline_card_offers_retry_not_a_blind_connect() {
    // The non-blocking offline model: a click on the greyed card never connects
    // nor opens the picker (lock 14); Retry drives a discovery re-enumerate.
    let mut off = source("peer:ash", "ash", &[Protocol::Rdp]);
    off.reachability = Reachability::Unreachable;
    off.reason = Some("peer unreachable".to_string());
    let mut state = state_with(Some(roster(vec![off])));
    let sources = state.sources_snapshot();
    state.activate(&sources, "peer:ash");
    assert!(
        state.pending.is_none() && state.take_connect().is_none(),
        "a greyed card never connects"
    );
    // Retry reaches the refresh emitter (honest no-Bus error), returning at once
    // — never a probe, never a block.
    state.retry_discovery(&sources, "peer:ash");
    assert!(state
        .last_error
        .as_deref()
        .is_some_and(|e| e.contains("Bus")));
}

#[test]
fn retry_discovery_writes_the_bodyless_refresh_verb() {
    let dir = temp_bus_dir("retry");
    let mut off = source("peer:ash", "ash", &[Protocol::Rdp]);
    off.reachability = Reachability::Unreachable;
    let mut state = state_with_bus(Some(roster(vec![off])), Some(dir.clone()));
    let sources = state.sources_snapshot();
    state.retry_discovery(&sources, "peer:ash");
    assert!(
        state.last_error.is_none(),
        "the refresh publish succeeded: {:?}",
        state.last_error
    );
    let persist = mde_bus::persist::Persist::open(dir.clone()).expect("open bus");
    let msgs = persist.list_since(REFRESH_TOPIC, None).expect("list");
    assert_eq!(msgs.len(), 1, "one Retry ⇒ one refresh nudge");
    assert!(state
        .note
        .as_deref()
        .is_some_and(|n| n.contains("Re-checking")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_source_targets_only_manual_origins() {
    let dir = temp_bus_dir("remove");
    let manual = manual_source("10.0.0.5", 3389, Protocol::Rdp);
    let manual_id = manual.id.clone();
    let mut state = state_with_bus(
        Some(roster(vec![
            manual,
            source("peer:oak", "oak", &[Protocol::Rdp]),
        ])),
        Some(dir.clone()),
    );
    let sources = state.sources_snapshot();

    // A discovered peer is never removed from here (no verb published).
    state.remove_source(&sources, "peer:oak");
    assert!(state.last_error.is_none() && state.note.is_none());

    // A manual source publishes the remove-source verb keyed on its id.
    state.remove_source(&sources, &manual_id);
    let persist = mde_bus::persist::Persist::open(dir.clone()).expect("open bus");
    let msgs = persist.list_since(REMOVE_SOURCE_TOPIC, None).expect("list");
    assert_eq!(
        msgs.len(),
        1,
        "only the manual source published a remove; the peer was a no-op"
    );
    let v: serde_json::Value = serde_json::from_str(msgs[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(v["id"], manual_id);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn begin_edit_seeds_from_the_manual_source_and_ignores_non_manual() {
    let mut named = manual_source("10.0.0.5", 3389, Protocol::Rdp);
    named.name = "OfficePC".to_string();
    let manual_id = named.id.clone();
    let mut state = state_with(Some(roster(vec![
        named,
        source("peer:oak", "oak", &[Protocol::Rdp]),
    ])));
    let sources = state.sources_snapshot();

    // A discovered source never opens the edit form.
    state.begin_edit(&sources, "peer:oak");
    assert!(
        state.manual_edit.is_none(),
        "only a manual source is editable"
    );

    // The manual source seeds the form from its current fields.
    state.begin_edit(&sources, &manual_id);
    let edit = state.manual_edit.as_ref().expect("the edit form opened");
    assert_eq!(edit.original_id, manual_id);
    assert_eq!(edit.name, "OfficePC");
    assert_eq!(edit.host, "10.0.0.5");
    assert_eq!(edit.port, "3389");
    assert_eq!(edit.protocol, Protocol::Rdp);
}

#[test]
fn begin_edit_blanks_a_default_host_port_name() {
    // A manual source whose name is the `host:port` default seeds an EMPTY name
    // field, so an unchanged Save lets the worker re-default it.
    let m = manual_source("h", 5900, Protocol::Vnc);
    let id = m.id.clone();
    let mut state = state_with(Some(roster(vec![m])));
    let sources = state.sources_snapshot();
    state.begin_edit(&sources, &id);
    let edit = state.manual_edit.as_ref().expect("form open");
    assert!(
        edit.name.is_empty(),
        "a default host:port name seeds an empty field"
    );
    assert_eq!(edit.protocol, Protocol::Vnc);
}

#[test]
fn save_manual_edit_validates_host_and_port_without_publishing() {
    let m = manual_source("10.0.0.5", 3389, Protocol::Rdp);
    let id = m.id.clone();
    let mut state = state_with(Some(roster(vec![m])));
    let sources = state.sources_snapshot();
    state.begin_edit(&sources, &id);

    // Empty host → an inline error; the publish is never reached (no Bus error).
    state.manual_edit.as_mut().unwrap().host = "   ".to_string();
    state.save_manual_edit(&sources);
    assert!(state
        .manual_edit
        .as_ref()
        .and_then(|e| e.error.as_deref())
        .is_some_and(|e| e.contains("Host")));
    assert!(
        state.last_error.is_none(),
        "a validation stop never reaches the publish"
    );

    // Non-numeric port → an inline error.
    {
        let e = state.manual_edit.as_mut().unwrap();
        e.host = "10.0.0.9".to_string();
        e.port = "not-a-port".to_string();
        e.error = None;
    }
    state.save_manual_edit(&sources);
    assert!(state
        .manual_edit
        .as_ref()
        .and_then(|e| e.error.as_deref())
        .is_some_and(|e| e.contains("Port")));
    assert!(state.last_error.is_none());
}

#[test]
fn save_manual_edit_republishes_via_remove_then_add() {
    let dir = temp_bus_dir("edit");
    let m = manual_source("10.0.0.5", 3389, Protocol::Rdp);
    let original_id = m.id.clone();
    let mut state = state_with_bus(Some(roster(vec![m])), Some(dir.clone()));
    let sources = state.sources_snapshot();
    state.begin_edit(&sources, &original_id);
    {
        let e = state.manual_edit.as_mut().unwrap();
        e.name = "Reception".to_string();
        e.host = "10.0.0.9".to_string();
        e.port = "5900".to_string();
        e.protocol = Protocol::Vnc;
    }
    state.save_manual_edit(&sources);
    assert!(
        state.last_error.is_none(),
        "both verbs published: {:?}",
        state.last_error
    );
    assert!(
        state.manual_edit.is_none(),
        "the form closes on a successful save"
    );

    let persist = mde_bus::persist::Persist::open(dir.clone()).expect("open bus");
    // The old id is removed …
    let rm = persist.list_since(REMOVE_SOURCE_TOPIC, None).expect("list");
    assert_eq!(rm.len(), 1);
    let rv: serde_json::Value = serde_json::from_str(rm[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(rv["id"], original_id);
    // … and the edited endpoint added over the worker's typed add-source verb.
    let add = persist.list_since(ADD_SOURCE_TOPIC, None).expect("list");
    assert_eq!(add.len(), 1);
    let av: serde_json::Value = serde_json::from_str(add[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(av["name"], "Reception");
    assert_eq!(av["host"], "10.0.0.9");
    assert_eq!(av["port"], 5900);
    assert_eq!(av["protocol"], "vnc");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── CHOOSER-8 headless renders ──

#[test]
fn the_filter_bar_and_grid_render_together() {
    let mut state = state_with(Some(fixture_state()));
    assert!(
        run_panel(&mut state),
        "the find bar + card grid produced no draw primitives"
    );
}

#[test]
fn a_fully_filtered_out_roster_renders_the_no_match_note() {
    let mut state = state_with(Some(fixture_state()));
    state.filter.search = "no-such-desktop".to_string();
    assert!(
        run_panel(&mut state),
        "the no-match note produced no draw primitives"
    );
    assert!(state.take_connect().is_none(), "rendering never connects");
}

#[test]
fn the_manual_edit_form_renders() {
    let m = manual_source("10.0.0.5", 3389, Protocol::Rdp);
    let id = m.id.clone();
    let mut state = state_with(Some(roster(vec![m])));
    let sources = state.sources_snapshot();
    state.begin_edit(&sources, &id);
    assert!(
        run_panel(&mut state),
        "the manual-source edit form produced no draw primitives"
    );
}

#[test]
fn a_favorited_card_renders_the_pin_marker() {
    let mut state = state_with(Some(roster(vec![source(
        "peer:oak",
        "oak",
        &[Protocol::Rdp],
    )])));
    state.toggle_favorite("peer:oak");
    assert!(
        run_panel(&mut state),
        "the pinned card produced no draw primitives"
    );
}

// ── CHOOSER-9: mesh-synced favorites / recents / manual sources ──

/// A unique temp workgroup root (the crate's `std::env::temp_dir()` idiom — no
/// `tempfile` dep), created + cleaned up by each sync test that uses it.
fn temp_prefs_root(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let root = std::env::temp_dir().join(format!("mde-chooser9-shell-{tag}-{n}"));
    std::fs::create_dir_all(&root).expect("mkroot");
    root
}

#[test]
fn a_pin_at_seat_a_syncs_and_appears_at_seat_b() {
    // THE ACCEPTANCE (two-seat): pin at seat A → the per-identity record syncs
    // over the shared workgroup root → seat B shows the pin. This drives the
    // sync mechanism directly (the live cross-seat is the gated leg).
    let root = temp_prefs_root("pin");
    let oak = source("peer:oak", "oak", &[Protocol::Rdp]);

    // ── Seat A pins the desktop. ──
    let mut seat_a = state_with_prefs(
        Some(roster(vec![oak.clone()])),
        None,
        root.clone(),
        "seat-a",
    );
    assert!(!seat_a.favorites.contains("peer:oak"), "not pinned yet");
    seat_a.toggle_favorite("peer:oak");
    assert!(seat_a.favorites.contains("peer:oak"), "pinned at seat A");

    // ── Seat B opens fresh over the SAME workgroup root. ──
    let seat_b = state_with_prefs(Some(roster(vec![oak])), None, root.clone(), "seat-b");
    assert!(
        seat_b.favorites.contains("peer:oak"),
        "seat A's pin roamed to seat B"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_recent_and_an_unpin_roam_between_seats() {
    let root = temp_prefs_root("recent");
    let oak = source("peer:oak", "oak", &[Protocol::Rdp]);

    // Seat A pins + connects (a genuine "recently used").
    let mut seat_a = state_with_prefs(
        Some(roster(vec![oak.clone()])),
        None,
        root.clone(),
        "seat-a",
    );
    let sources_a = seat_a.sources_snapshot();
    seat_a.toggle_favorite("peer:oak");
    seat_a.activate(&sources_a, "peer:oak");
    seat_a.confirm_connect(&sources_a); // records the recent
    assert!(seat_a.recents.contains("peer:oak"), "recorded at seat A");

    // Seat B sees both the pin and the recent.
    let mut seat_b = state_with_prefs(
        Some(roster(vec![oak.clone()])),
        None,
        root.clone(),
        "seat-b",
    );
    assert!(seat_b.favorites.contains("peer:oak"), "pin roamed");
    assert!(seat_b.recents.contains("peer:oak"), "recent roamed");

    // Seat B un-pins; seat A re-reads and the un-pin has converged (LWW, so the
    // newer un-pin beats the older pin — never a grow-only set).
    seat_b.toggle_favorite("peer:oak");
    assert!(
        !seat_b.favorites.contains("peer:oak"),
        "un-pinned at seat B"
    );
    let seat_a2 = state_with_prefs(Some(roster(vec![oak])), None, root.clone(), "seat-a");
    assert!(
        !seat_a2.favorites.contains("peer:oak"),
        "seat B's newer un-pin roamed back to seat A"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn compact_reconnect_uses_the_newest_recent_source() {
    let root = temp_prefs_root("compact-reconnect");
    let older = source("peer:ash", "ash", &[Protocol::Vnc]);
    let newer = source("peer:oak", "oak", &[Protocol::Rdp]);
    let mut state = state_with_prefs(
        Some(roster(vec![older.clone(), newer.clone()])),
        None,
        root.clone(),
        "seat-a",
    );

    state.prefs.record_recent("peer:ash", "ash", 10);
    state.prefs.record_recent("peer:oak", "oak", 20);
    state.refresh_prefs_cache();

    let request = state
        .connect_last_recent()
        .expect("newest recent reconnects");
    assert_eq!(request.target.name, "oak");
    assert_eq!(request.protocol, VdiProtocol::Rdp);
    assert!(
        state.take_connect().is_none(),
        "the compact reconnect returns and drains the request"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn compact_source_rows_reuse_chooser_state_and_selected_source_connects() {
    let root = temp_prefs_root("compact-source-rows");
    let plain = source("peer:ash", "ash", &[Protocol::Vnc]);
    let pinned = source("peer:oak", "oak", &[Protocol::Rdp]);
    let mut state = state_with_prefs(
        Some(roster(vec![plain.clone(), pinned.clone()])),
        None,
        root.clone(),
        "seat-a",
    );
    state.toggle_favorite("peer:oak");
    state.refresh_prefs_cache();

    let rows = state.rail_sources();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "peer:oak", "pinned source floats first");
    assert_eq!(rows[0].label, "oak");
    assert!(rows[0].connectable);

    let request = state
        .connect_source_id("peer:ash")
        .expect("selected compact row connects through chooser");
    assert_eq!(request.target.name, "ash");
    assert_eq!(request.protocol, VdiProtocol::Vnc);
    assert!(
        state.take_connect().is_none(),
        "compact source connect returns and drains the request"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn compact_selection_and_expanded_panel_share_the_same_pending_picker() {
    // NAVBAR-U4 — the compact rail face and the expanded chooser face are one
    // `ChooserState`: selecting an external source from compact mode raises the
    // credential picker in the same state the expanded panel renders.
    let (mut state, id) = external_state(Box::new(RecordingStore::default()));
    assert!(
        state.connect_source_id(&id).is_none(),
        "no sealed external credential means compact selection must not connect blind"
    );
    assert!(
        state
            .pending
            .as_ref()
            .is_some_and(|d| d.source_id == id && d.cred_prompt.is_some()),
        "compact selection raised the expanded chooser's pending credential prompt"
    );
    assert!(
        run_panel(&mut state),
        "the expanded chooser renders the pending prompt from the compact pick"
    );
    assert!(
        state
            .pending
            .as_ref()
            .is_some_and(|d| d.source_id == id && d.cred_prompt.is_some()),
        "rendering the expanded face keeps the same pending picker state"
    );
}

#[test]
fn a_manual_source_roams_and_rematerializes_at_a_new_seat() {
    // A manual desktop added on seat A is captured into the synced prefs; seat B
    // (whose worker doesn't know it yet) re-publishes it over the ONE existing
    // add-source verb, so it appears there too — reusing the CHOOSER-8 seam.
    let prefs_root = temp_prefs_root("manual");
    let bus_a = temp_bus_dir("manual-a");
    let bus_b = temp_bus_dir("manual-b");
    let manual = manual_source("10.0.0.5", 3389, Protocol::Rdp);
    let manual_id = manual.id.clone();

    // ── Seat A folds a roster carrying the manual source → captured into prefs.
    let seat_a = state_with_prefs(
        Some(roster(vec![manual])),
        Some(bus_a.clone()),
        prefs_root.clone(),
        "seat-a",
    );
    assert!(
        seat_a
            .prefs
            .merged()
            .manual
            .iter()
            .any(|m| m.id == manual_id),
        "the manual source was captured into the synced prefs"
    );

    // ── Seat B has an EMPTY roster (its worker hasn't heard of the endpoint) but
    // shares the workgroup root → it re-materializes the roamed manual source
    // onto its own worker via the add-source verb.
    let seat_b = state_with_prefs(
        Some(roster(vec![])),
        Some(bus_b.clone()),
        prefs_root.clone(),
        "seat-b",
    );
    assert!(
        seat_b.last_error.is_none(),
        "the re-materialize publish succeeded: {:?}",
        seat_b.last_error
    );
    let persist = mde_bus::persist::Persist::open(bus_b.clone()).expect("open bus");
    let adds = persist.list_since(ADD_SOURCE_TOPIC, None).expect("list");
    assert_eq!(
        adds.len(),
        1,
        "the roamed manual source is re-published once"
    );
    let v: serde_json::Value = serde_json::from_str(adds[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(v["host"], "10.0.0.5");
    assert_eq!(v["port"], 3389);
    assert_eq!(v["protocol"], "rdp");

    let _ = std::fs::remove_dir_all(&prefs_root);
    let _ = std::fs::remove_dir_all(&bus_a);
    let _ = std::fs::remove_dir_all(&bus_b);
}

#[test]
fn removing_a_manual_source_tombstones_it_so_it_does_not_reappear() {
    // Remove on seat A tombstones the synced register; a fresh seat B must NOT
    // re-materialize it (a grow-only set would resurrect a removed desktop).
    let prefs_root = temp_prefs_root("manual-rm");
    let bus_a = temp_bus_dir("manual-rm-a");
    let bus_b = temp_bus_dir("manual-rm-b");
    let manual = manual_source("10.0.0.5", 3389, Protocol::Rdp);
    let manual_id = manual.id.clone();

    let mut seat_a = state_with_prefs(
        Some(roster(vec![manual])),
        Some(bus_a.clone()),
        prefs_root.clone(),
        "seat-a",
    );
    let sources_a = seat_a.sources_snapshot();
    seat_a.remove_source(&sources_a, &manual_id);
    assert!(
        !seat_a
            .prefs
            .merged()
            .manual
            .iter()
            .any(|m| m.id == manual_id),
        "the removed manual source is tombstoned in the synced prefs"
    );

    // Seat B, empty roster, shared root: the tombstone means nothing to
    // re-materialize — no add-source verb published.
    let seat_b = state_with_prefs(
        Some(roster(vec![])),
        Some(bus_b.clone()),
        prefs_root.clone(),
        "seat-b",
    );
    let persist = mde_bus::persist::Persist::open(bus_b.clone()).expect("open bus");
    let adds = persist.list_since(ADD_SOURCE_TOPIC, None).expect("list");
    assert!(
        adds.is_empty(),
        "a removed manual source does not roam back to a new seat"
    );
    assert!(seat_b.favorites.is_empty());

    let _ = std::fs::remove_dir_all(&prefs_root);
    let _ = std::fs::remove_dir_all(&bus_a);
    let _ = std::fs::remove_dir_all(&bus_b);
}

// ── TESTVM-4: pinned endpoints — selectable with NO mesh discovery ──

#[test]
fn a_pinned_endpoint_is_added_rendered_and_counted_with_no_roster_and_no_bus() {
    let root = temp_prefs_root("pin-endpoint");
    // No roster ever published, no Bus root — the mesh-less seat.
    let mut state = state_with_prefs(None, None, root.clone(), "seat-a");
    assert!(state.sources_snapshot().is_empty(), "nothing pinned yet");

    // The operator pins the live VNC test endpoint through the ADD form.
    state.begin_add();
    {
        let e = state.manual_edit.as_mut().expect("the add form opened");
        assert!(e.original_id.is_empty(), "ADD mode has no original id");
        e.name = "testvm-lin".to_string();
        e.host = "172.20.146.144".to_string();
        e.port = "5900".to_string();
        e.protocol = Protocol::Vnc;
        e.password = "testvm".to_string();
    }
    state.save_manual_edit(&[]);
    assert!(state.manual_edit.is_none(), "the form closes on save");
    assert!(
        state.last_error.is_none(),
        "a mesh-less pin is not an error: {:?}",
        state.last_error
    );

    // The pin renders as a card from the prefs register alone (§7 honest
    // fields: manual origin, never-probed Unknown, still connectable).
    let sources = state.sources_snapshot();
    assert_eq!(
        sources.len(),
        1,
        "the pinned endpoint renders with no roster"
    );
    let card = &sources[0];
    assert_eq!(card.id, "manual:172.20.146.144:5900:vnc");
    assert_eq!(card.name, "testvm-lin");
    assert_eq!(
        (card.node.as_str(), card.host.as_str()),
        ("172.20.146.144", "172.20.146.144")
    );
    assert_eq!(card.origin, SourceOrigin::Manual);
    assert_eq!(card.reachability, Reachability::Unknown);
    assert_eq!(
        card.protocols,
        vec![ProtocolOffer {
            protocol: Protocol::Vnc,
            port: Some(5900)
        }]
    );
    assert!(card.connectable());
    assert_eq!(
        state.source_count(),
        1,
        "the menubar count matches the cards"
    );
    assert!(
        run_panel(&mut state),
        "the pinned card produced no draw primitives"
    );

    // …and the pin roams: a fresh seat over the same root shows it too.
    let seat_b = state_with_prefs(None, None, root.clone(), "seat-b");
    assert_eq!(
        seat_b.sources_snapshot().len(),
        1,
        "the pinned endpoint roamed to a second mesh-less seat"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_pinned_endpoint_with_a_stored_credential_connects_without_a_prompt() {
    // THE TESTVM-4 connect acceptance: the stored register credential drives
    // Connect straight through — no prompt, and no credential is sealed.
    let root = temp_prefs_root("pin-connect");
    let store = RecordingStore::default();
    let mut state = ChooserState::with_client(
        Box::new(FakeSources(None)),
        None,
        "client-node".to_string(),
        Box::new(store.clone()),
        prefs_at(root.clone(), "seat-a"),
    );
    state.prefs.set_manual(ManualEntry {
        id: "manual:172.20.146.54:3389:rdp".to_string(),
        present: true,
        host: "172.20.146.54".to_string(),
        port: 3389,
        protocol: "rdp".to_string(),
        name: Some("testvm-win".to_string()),
        username: Some("root".to_string()),
        password: Some("testvm".to_string()),
        updated_ms: 1,
    });
    state.refresh();

    let sources = state.sources_snapshot();
    state.activate(&sources, "manual:172.20.146.54:3389:rdp");
    assert!(
        state.pending.is_some(),
        "the always-ask picker still opens (lock 6)"
    );
    state.confirm_connect(&sources);

    let request = state
        .take_connect()
        .expect("the stored credential connects straight through");
    assert_eq!(
        store.seal_count(),
        0,
        "the stored register cred is not re-sealed"
    );
    assert_eq!(request.protocol, VdiProtocol::Rdp);
    assert_eq!(request.target.name, "testvm-win");
    assert_eq!(
        request
            .target
            .endpoint
            .as_ref()
            .map(|e| (e.host.as_str(), e.port)),
        Some(("172.20.146.54", 3389))
    );
    let DesktopAuth::Sealed {
        credential,
        store_ref,
    } = &request.auth
    else {
        unreachable!("expected the stored register credential")
    };
    assert_eq!(store_ref, "desktop/172.20.146.54/rdp");
    assert_eq!(credential.username, "root");
    assert_eq!(credential.secret.expose(), "testvm");
    assert!(state.pending.is_none(), "the picker closed on connect");
    assert!(
        state.recents.contains("manual:172.20.146.54:3389:rdp"),
        "a genuine connect records the recent"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_pinned_endpoint_without_a_stored_password_still_prompts_once() {
    // No stored password → the CHOOSER-6 fold is untouched: the one-time
    // credential prompt raises and nothing connects until it's filled.
    let root = temp_prefs_root("pin-prompt");
    let mut state = state_with_prefs(None, None, root.clone(), "seat-a");
    state.prefs.set_manual(ManualEntry {
        id: "manual:172.20.146.144:5900:vnc".to_string(),
        present: true,
        host: "172.20.146.144".to_string(),
        port: 5900,
        protocol: "vnc".to_string(),
        name: None,
        username: None,
        password: None,
        updated_ms: 1,
    });
    state.refresh();
    let sources = state.sources_snapshot();
    state.activate(&sources, "manual:172.20.146.144:5900:vnc");
    state.confirm_connect(&sources);
    assert!(
        state.take_connect().is_none(),
        "nothing connects before the prompt is filled"
    );
    assert!(
        state
            .pending
            .as_ref()
            .is_some_and(|d| d.cred_prompt.is_some()),
        "the one-time credential prompt raised"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_add_endpoint_form_renders_over_an_empty_grid() {
    // The ADD form must paint even with no roster at all (the empty branch),
    // or a mesh-less seat could never enter its first pin.
    let mut state = state_with(None);
    state.begin_add();
    assert!(
        run_panel(&mut state),
        "the add-endpoint form produced no draw primitives over the empty grid"
    );
}

#[test]
fn an_edit_round_trips_the_stored_credential_and_a_remove_tombstones_the_pin() {
    let root = temp_prefs_root("pin-edit");
    let mut state = state_with_prefs(None, None, root.clone(), "seat-a");
    state.prefs.set_manual(ManualEntry {
        id: "manual:172.20.146.144:5900:vnc".to_string(),
        present: true,
        host: "172.20.146.144".to_string(),
        port: 5900,
        protocol: "vnc".to_string(),
        name: Some("testvm-lin".to_string()),
        username: None,
        password: Some("testvm".to_string()),
        updated_ms: 1,
    });
    state.refresh();
    let sources = state.sources_snapshot();

    // Edit seeds the stored credential; an untouched Save keeps it.
    state.begin_edit(&sources, "manual:172.20.146.144:5900:vnc");
    assert_eq!(
        state.manual_edit.as_ref().map(|e| e.password.as_str()),
        Some("testvm"),
        "the edit form seeds the stored password"
    );
    state.save_manual_edit(&sources);
    assert!(state.manual_edit.is_none());
    assert_eq!(
        state
            .manual_cache
            .iter()
            .find(|m| m.id == "manual:172.20.146.144:5900:vnc")
            .and_then(|m| m.password.as_deref()),
        Some("testvm"),
        "an untouched Save keeps the stored credential"
    );

    // Remove tombstones the register — the card is gone with no Bus error.
    let sources = state.sources_snapshot();
    state.remove_source(&sources, "manual:172.20.146.144:5900:vnc");
    assert!(
        state.last_error.is_none(),
        "a mesh-less remove is not an error: {:?}",
        state.last_error
    );
    assert!(
        state.sources_snapshot().is_empty(),
        "the removed pin no longer renders"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_recently_used_card_renders_its_marker() {
    let root = temp_prefs_root("recent-render");
    let mut state = state_with_prefs(
        Some(roster(vec![source("peer:oak", "oak", &[Protocol::Rdp])])),
        None,
        root.clone(),
        "seat-a",
    );
    let sources = state.sources_snapshot();
    state.activate(&sources, "peer:oak");
    state.confirm_connect(&sources);
    assert!(state.recents.contains("peer:oak"));
    assert!(
        run_panel(&mut state),
        "the recently-used card produced no draw primitives"
    );
    let _ = std::fs::remove_dir_all(&root);
}
