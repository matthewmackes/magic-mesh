use super::*;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex as StdMutex;
use tempfile::tempdir;
use tokio_rustls::TlsAcceptor;

static MDE_HOME_LOCK: StdMutex<()> = StdMutex::new(());

#[test]
fn worker_name_matches_module() {
    let w = KdcHostWorker::new(PathBuf::from("/tmp"));
    assert_eq!(w.name(), "kdc-host");
}

#[test]
fn fanout_classify_maps_only_follow_everywhere_packets() {
    // A clipboard copy → a Clipboard fanout action.
    assert_eq!(
        fanout_action_for_packet("kdeconnect.clipboard", &json!({ "content": "hi" })),
        Some(FanoutAction::Clipboard {
            content: "hi".into()
        })
    );
    // The connection-time clipboard push is fanned out too.
    assert_eq!(
        fanout_action_for_packet("kdeconnect.clipboard.connect", &json!({ "content": "x" })),
        Some(FanoutAction::Clipboard {
            content: "x".into()
        })
    );
    // An empty clipboard push isn't worth fanning out.
    assert_eq!(
        fanout_action_for_packet("kdeconnect.clipboard", &json!({ "content": "" })),
        None
    );
    // A find-my-device ring → a Ring fanout action.
    assert_eq!(
        fanout_action_for_packet("kdeconnect.findmyphone.request", &json!({})),
        Some(FanoutAction::Ring)
    );
    // Everything else is NOT a follow-everywhere action (node-specific / not
    // fanned out) — e.g. a run-command or a notification.
    assert_eq!(
        fanout_action_for_packet("kdeconnect.runcommand.request", &json!({})),
        None
    );
    assert_eq!(
        fanout_action_for_packet("kdeconnect.notification", &json!({ "ticker": "x" })),
        None
    );
}

#[derive(Default)]
struct RecordingMediaControl {
    calls: StdMutex<Vec<Vec<&'static str>>>,
    queries: StdMutex<Vec<Vec<String>>>,
    outputs: StdMutex<HashMap<Vec<String>, String>>,
}

impl MediaControl for RecordingMediaControl {
    fn run_playerctl(&self, invocation: PlayerctlInvocation) -> bool {
        self.calls.lock().unwrap().push(invocation.args.to_vec());
        true
    }

    fn query_playerctl(&self, args: &[String]) -> Option<String> {
        self.queries.lock().unwrap().push(args.to_vec());
        self.outputs.lock().unwrap().get(args).cloned()
    }
}

#[test]
fn local_announce_advertises_mpris_only_after_the_handler_exists() {
    let announce = local_announce();
    assert!(
        announce
            .incoming_capabilities
            .iter()
            .any(|cap| cap == "kdeconnect.mpris"),
        "phone media transport keys must be advertised once handled"
    );
    assert!(
        announce
            .incoming_capabilities
            .iter()
            .any(|cap| cap == "kdeconnect.mpris.request"),
        "phone MPRIS state pulls must be advertised once player state reports are implemented"
    );
    assert!(
        announce
            .outgoing_capabilities
            .iter()
            .any(|cap| cap == "kdeconnect.mpris"),
        "player state reports are sent as kdeconnect.mpris packets"
    );
}

#[test]
fn local_announce_advertises_remote_input_handoff_once_parser_exists() {
    let announce = local_announce();
    assert!(
        announce
            .incoming_capabilities
            .iter()
            .any(|cap| cap == "kdeconnect.mousepad.request"),
        "phone touchpad/keyboard packets must be advertised once parsed into the Bus handoff"
    );
}

#[test]
fn kdc_remote_input_body_maps_motion_click_and_text() {
    let peer = PeerId::from("moto");
    let move_body = kdc_remote_input_body(&peer, &MousepadEvent::Move { dx: 7.5, dy: -2.0 }, 123);
    assert_eq!(
        move_body,
        json!({
            "op": "kdc_remote_input",
            "source": "kdc_host",
            "phone": "moto",
            "ts_unix_ms": 123,
            "kind": "move",
            "dx": 7.5,
            "dy": -2.0,
        })
    );

    let click_body = kdc_remote_input_body(
        &peer,
        &MousepadEvent::Button {
            button: mde_kdc_proto::plugins::mousepad::MouseButton::Secondary,
            clicks: 1,
        },
        124,
    );
    assert_eq!(click_body["kind"], "button");
    assert_eq!(click_body["button"], "secondary");
    assert_eq!(click_body["clicks"], 1);

    let text_body = kdc_remote_input_body(
        &peer,
        &MousepadEvent::Text {
            text: "A".into(),
            modifiers: MouseModifiers {
                shift: true,
                ctrl: true,
                ..Default::default()
            },
        },
        125,
    );
    assert_eq!(text_body["kind"], "text");
    assert_eq!(text_body["text"], "A");
    assert_eq!(
        text_body["modifiers"],
        json!({
            "shift": true,
            "ctrl": true,
            "alt": false,
            "super": false,
        })
    );
}

#[test]
fn mpris_media_command_maps_only_allowlisted_transport_actions() {
    let control = RecordingMediaControl::default();
    let body = MprisBody {
        action: "Next".into(),
        ..Default::default()
    };

    assert_eq!(apply_mpris_media_command(&control, &body), Some("next"));
    assert_eq!(*control.calls.lock().unwrap(), vec![vec!["next"]]);

    let state_report = MprisBody {
        player: "mde-music".into(),
        is_playing: true,
        ..Default::default()
    };
    assert_eq!(apply_mpris_media_command(&control, &state_report), None);
    assert_eq!(
        *control.calls.lock().unwrap(),
        vec![vec!["next"]],
        "state reports must not execute media commands"
    );

    let raw_shell = MprisBody {
        action: "next; rm -rf /".into(),
        ..Default::default()
    };
    assert_eq!(apply_mpris_media_command(&control, &raw_shell), None);
    assert_eq!(
        *control.calls.lock().unwrap(),
        vec![vec!["next"]],
        "unknown/raw actions must be dropped rather than executed"
    );
}

#[test]
fn mpris_media_command_maps_volume_to_bounded_playerctl_steps() {
    let control = RecordingMediaControl::default();

    assert_eq!(
        apply_mpris_media_command(
            &control,
            &MprisBody {
                action: "VolumeUp".into(),
                ..Default::default()
            }
        ),
        Some("volume-up")
    );
    assert_eq!(
        apply_mpris_media_command(
            &control,
            &MprisBody {
                action: "volume-down".into(),
                ..Default::default()
            }
        ),
        Some("volume-down")
    );
    assert_eq!(
        *control.calls.lock().unwrap(),
        vec![vec!["volume", "0.05+"], vec!["volume", "0.05-"]],
        "phone volume controls are fixed playerctl steps, not raw action strings"
    );
}

#[test]
fn mpris_request_player_list_maps_playerctl_list_to_report() {
    let control = RecordingMediaControl::default();
    control
        .outputs
        .lock()
        .unwrap()
        .insert(vec!["-l".into()], "mde-music\nvlc\n".into());

    let reports = mpris_response_bodies_for_request(
        &control,
        &MprisRequestBody {
            request_player_list: true,
            ..Default::default()
        },
    );

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].kind(), MprisKind::PlayerList);
    assert_eq!(reports[0].player_list, vec!["mde-music", "vlc"]);
    assert_eq!(reports[0].support_album_art_payload, Some(false));
}

#[test]
fn mpris_request_now_playing_and_volume_maps_playerctl_state_to_report() {
    let control = RecordingMediaControl::default();
    let mut outputs = control.outputs.lock().unwrap();
    outputs.insert(
        vec!["-p".into(), "mde-music".into(), "status".into()],
        "Playing\n".into(),
    );
    outputs.insert(
        vec!["-p".into(), "mde-music".into(), "position".into()],
        "83.25\n".into(),
    );
    outputs.insert(
        vec!["-p".into(), "mde-music".into(), "volume".into()],
        "0.42\n".into(),
    );
    outputs.insert(
        vec![
            "-p".into(),
            "mde-music".into(),
            "metadata".into(),
            "--format".into(),
            "{{artist}}\n{{title}}\n{{album}}\n{{mpris:length}}\n{{mpris:artUrl}}".into(),
        ],
        "Test Artist\nTest Title\nTest Album\n245000000\nfile:///tmp/art.png\n".into(),
    );
    drop(outputs);

    let reports = mpris_response_bodies_for_request(
        &control,
        &MprisRequestBody {
            player: "mde-music".into(),
            request_now_playing: true,
            request_volume: true,
            ..Default::default()
        },
    );

    assert_eq!(reports.len(), 1);
    let report = &reports[0];
    assert_eq!(report.kind(), MprisKind::State);
    assert_eq!(report.player, "mde-music");
    assert!(report.is_playing);
    assert_eq!(report.pos, 83_250);
    assert_eq!(report.length, 245_000);
    assert_eq!(report.volume, Some(42));
    assert_eq!(report.artist, "Test Artist");
    assert_eq!(report.title, "Test Title");
    assert_eq!(report.album, "Test Album");
    assert_eq!(report.album_art_url, "file:///tmp/art.png");
    assert_eq!(report.now_playing, "Test Artist - Test Title");
    assert_eq!(report.can_play, Some(true));
    assert_eq!(report.can_pause, Some(true));
}

#[test]
fn mpris_request_command_reuses_the_transport_allowlist() {
    let control = RecordingMediaControl::default();
    assert_eq!(
        apply_mpris_request_command(
            &control,
            &MprisRequestBody {
                action: "PlayPause".into(),
                ..Default::default()
            }
        ),
        Some("play-pause")
    );
    assert_eq!(
        apply_mpris_request_command(
            &control,
            &MprisRequestBody {
                action: "next; rm -rf /".into(),
                ..Default::default()
            }
        ),
        None
    );
    assert_eq!(*control.calls.lock().unwrap(), vec![vec!["play-pause"]]);
}

#[test]
fn fanout_drain_applies_a_peer_request_once_and_responds() {
    // The relay/aggregate substrate is exercised end-to-end via the worker glue:
    // a peer's request is drained (applied once, then de-duped) and a response
    // row is written the endpoint can aggregate. `apply_fanout_action` shells out
    // to wl-copy/canberra which are absent in CI — it must still no-op cleanly.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    // An endpoint (eagle) relayed a ring; a peer (oak) drains it.
    fanout::publish_request(
        root,
        "eagle",
        &FanoutRequest {
            id: "eagle:1000:ring".into(),
            action: FanoutAction::Ring,
            origin_host: "eagle".into(),
            ts_ms: now_ms(),
        },
        FANOUT_ROW_CAP,
    )
    .unwrap();
    let mut seen = NotifySeen::default();
    drain_fanout_requests(root, "oak", &mut seen);
    // oak wrote a response the endpoint can aggregate.
    let agg = fanout::aggregate_responses(root, "eagle:1000:ring", now_ms(), FANOUT_STALE_MS);
    assert_eq!(agg.responders, vec!["oak".to_string()]);
    assert_eq!(agg.applied, 1);
    // A second drain is a no-op (apply-once seen-set) — still one responder.
    drain_fanout_requests(root, "oak", &mut seen);
    let agg2 = fanout::aggregate_responses(root, "eagle:1000:ring", now_ms(), FANOUT_STALE_MS);
    assert_eq!(agg2.applied, 1, "de-duped: applied exactly once per node");
}

#[test]
fn read_power_supply_u8_handles_missing_and_garbage() {
    // A non-existent sysfs node is None (the desktop case).
    assert_eq!(
        read_power_supply_u8("/sys/class/power_supply/__nope__/capacity"),
        None
    );
    // A bogus path never panics.
    assert_eq!(read_power_supply_u8("/definitely/not/a/file"), None);
}

#[test]
fn local_battery_body_is_serializable_and_sane() {
    // Whatever this host is (laptop or desktop), the reply body must be a
    // valid `kdeconnect.battery` JSON object. A desktop yields the -1
    // sentinel; a laptop yields a 0..=100 charge — both are valid.
    let body = local_battery_body();
    let v = serde_json::to_value(&body).expect("battery body serializes");
    assert!(v.get("currentCharge").is_some());
    assert!(v.get("isCharging").is_some());
    // charge_pct() is None (sentinel) or Some(0..=100); never out of range.
    if let Some(p) = body.charge_pct() {
        assert!(p <= 100);
    }
}

#[test]
fn ring_and_clipboard_helpers_never_panic_when_tools_absent() {
    // Best-effort host actions: with no audio player / wl-copy present (CI),
    // these spawn-or-skip without panicking or blocking.
    ring_local_device();
    apply_clipboard("test clipboard content");
}

#[test]
fn default_runcommands_are_the_mesh_ops_bundle() {
    let defaults = default_runcommands();
    let keys: Vec<&str> = defaults.iter().map(|c| c.key.as_str()).collect();
    // The operator-selected Mesh-ops set.
    for k in [
        "mesh-health",
        "mesh-status",
        "disk-headroom",
        "restart-mesh",
        "presenter-next",
        "presenter-previous",
        "presenter-start",
        "presenter-exit",
    ] {
        assert!(keys.contains(&k), "missing default runcommand {k}");
    }
}

#[test]
fn default_runcommands_include_presenter_seat_helper_actions() {
    let defaults = default_runcommands();
    let command_for = |key: &str| {
        defaults
            .iter()
            .find(|cmd| cmd.key == key)
            .unwrap_or_else(|| panic!("missing default runcommand {key}"))
            .command
            .as_str()
    };

    for (key, special_key) in [
        ("presenter-next", 9),
        ("presenter-previous", 8),
        ("presenter-start", 25),
        ("presenter-exit", 14),
    ] {
        let command = command_for(key);
        assert!(command.contains("/usr/libexec/mackesd/seat-remote-input"));
        assert!(command.contains(&format!("\"special_key\":{special_key}")));
        assert!(command.contains("\"kind\":\"special_key\""));
        assert!(!command.contains('$'));
        assert!(!command.contains('`'));
    }
}

#[test]
fn load_runcommands_falls_back_to_defaults_without_a_toml() {
    let tmp = tempdir().unwrap();
    let cmds = load_runcommands(tmp.path());
    assert_eq!(cmds.len(), default_runcommands().len());
}

#[test]
fn load_runcommands_reads_a_custom_toml() {
    let tmp = tempdir().unwrap();
    std::fs::write(
        tmp.path().join("runcommands.toml"),
        "[[command]]\nkey=\"lock\"\nname=\"Lock screen\"\ncommand=\"loginctl lock-session\"\n",
    )
    .unwrap();
    let cmds = load_runcommands(tmp.path());
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].key, "lock");
    assert_eq!(cmds[0].name, "Lock screen");
}

#[test]
fn command_list_json_is_a_keyed_name_command_map() {
    let cmds = vec![RunCmd {
        key: "k1".into(),
        name: "N1".into(),
        command: "echo hi".into(),
    }];
    let s = command_list_json(&cmds);
    let v: Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["k1"]["name"], "N1");
    assert_eq!(v["k1"]["command"], "echo hi");
}

#[test]
fn execute_runcommand_runs_known_key_and_rejects_unknown() {
    let cmds = vec![RunCmd {
        key: "say".into(),
        name: "Say hi".into(),
        command: "echo mesh-ok".into(),
    }];
    let out = execute_runcommand(&cmds, "say");
    assert!(out.contains("mesh-ok"), "output should carry stdout: {out}");
    assert!(execute_runcommand(&cmds, "nope").contains("unknown command"));
}

#[test]
fn open_pairing_creates_the_identity() {
    // E2.2 — the worker holds the canonical pairing store now.
    // open_pairing opens it, creating identity.pkcs8 on first run.
    let tmp = tempdir().unwrap();
    let w = KdcHostWorker::new(tmp.path().to_path_buf());
    let store = w.open_pairing().unwrap();
    assert!(Arc::strong_count(&store) >= 1);
    assert!(tmp.path().join("identity.pkcs8").exists());
}

#[test]
fn mesh_enroll_token_mint_records_a_short_ttl_invite() {
    let tmp = tempdir().unwrap();
    let state = mint_mesh_enroll_token_state(tmp.path(), "peer:anvil", Duration::from_secs(60))
        .expect("mint token");

    assert!(state.token.starts_with("mde-invite:"));
    assert!(state.recorded);
    assert_eq!(state.source, MESH_ENROLL_TOKEN_SOURCE);
    assert!(crate::onboard::invite::is_recorded(
        tmp.path(),
        &state.token
    ));
    let invite = crate::onboard::invite::Invite::decode(&state.token).expect("invite");
    assert_eq!(invite.mesh_id, "mesh-peer:anvil");
    assert_eq!(state.expires_at_ms, i64::try_from(invite.exp_ms).unwrap());
}

#[test]
fn mesh_enroll_token_bus_handler_publishes_state_and_reply() {
    let bus = tempdir().unwrap();
    let workgroup = tempdir().unwrap();
    let persist = Persist::open(bus.path().to_path_buf()).expect("bus");

    let reply = serve_mesh_enroll_token(&persist, workgroup.path(), "node-a");
    let reply: Value = serde_json::from_str(&reply).expect("reply json");

    assert_eq!(reply["ok"], true);
    let token = reply["token"].as_str().expect("reply token");
    assert!(crate::onboard::invite::is_recorded(workgroup.path(), token));
    let state_msg = persist
        .list_since(MESH_ENROLL_TOKEN_TOPIC, None)
        .expect("state messages")
        .pop()
        .expect("published state");
    let state: Value =
        serde_json::from_str(state_msg.body.as_deref().expect("state body")).unwrap();
    assert_eq!(state["token"], token);
    assert_eq!(state["source"], MESH_ENROLL_TOKEN_SOURCE);
    assert_eq!(state["recorded"], true);
}

fn test_store(dir: &std::path::Path) -> PairingStore {
    PairingStore::open(dir).unwrap()
}

fn pair_body(id: &str, name: &str) -> Value {
    json!({
        "id": id, "name": name, "kind": "phone",
        "fingerprint": "AB:CD", "public_key_b64": "", "capabilities": [],
        "paired_at": 123,
    })
}

#[test]
fn connect_verb_version_and_empty_list() {
    let tmp = tempdir().unwrap();
    let store = test_store(tmp.path());
    let outbound = PendingSends::new();
    let v: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "version",
        &Value::Null,
    ))
    .unwrap();
    assert_eq!(v["ok"], true);
    assert!(v["version"].is_string());
    let l: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "list",
        &Value::Null,
    ))
    .unwrap();
    assert_eq!(l["devices"].as_array().unwrap().len(), 0);
}

#[test]
fn connect_verb_pair_get_unpair_roundtrip() {
    let tmp = tempdir().unwrap();
    let store = test_store(tmp.path());
    let outbound = PendingSends::new();
    // pair
    let r: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "pair",
        &pair_body("d1", "Pixel"),
    ))
    .unwrap();
    assert_eq!(r["ok"], true);
    // get
    let g: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "get",
        &json!({ "device_id": "d1" }),
    ))
    .unwrap();
    assert_eq!(g["device"]["name"], "Pixel");
    assert_eq!(g["device"]["fingerprint"], "AB:CD");
    // get unknown
    let gx: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "get",
        &json!({ "device_id": "nope" }),
    ))
    .unwrap();
    assert_eq!(gx["error"], "NoSuchDevice");
    // unpair, then unpair-again
    let u: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "unpair",
        &json!({ "device_id": "d1" }),
    ))
    .unwrap();
    assert_eq!(u["ok"], true);
    let u2: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "unpair",
        &json!({ "device_id": "d1" }),
    ))
    .unwrap();
    assert_eq!(u2["error"], "NoSuchDevice");
}

#[test]
fn connect_verb_pair_persists_across_reopen() {
    // E2.2 — the pair verb writes through to the canonical store's
    // devices.toml; a fresh store opened on the same dir sees it.
    let tmp = tempdir().unwrap();
    {
        let store = test_store(tmp.path());
        let outbound = PendingSends::new();
        handle_connect_verb(&store, &outbound, "pair", &pair_body("d1", "Pixel"));
    }
    let reopened = PairingStore::open(tmp.path()).unwrap();
    assert!(reopened.is_paired("d1"));
    assert_eq!(reopened.get("d1").unwrap().device_name, "Pixel");
}

fn spawn_loopback_pair_device() -> (std::thread::JoinHandle<()>, SocketAddr, String) {
    let pkcs8 = mde_kdc_host::keygen::generate_pkcs8().expect("keygen");
    let cert = mde_kdc_host::keygen::issue_identity_cert(&pkcs8, "device-qr").expect("cert");
    let fingerprint = mde_kdc_host::compute_fingerprint(&cert);
    let config = mde_kdc_host::build_server_config(&cert, &pkcs8).expect("server config");
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind pair device");
    let addr = std_listener.local_addr().expect("loopback addr");
    std_listener
        .set_nonblocking(true)
        .expect("pair device nonblocking listener");
    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("pair-device test runtime");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(std_listener)
                .expect("tokio pair device listener");
            if let Ok((tcp, _)) = listener.accept().await {
                let acceptor = TlsAcceptor::from(Arc::new(config));
                if let Ok(stream) = acceptor.accept(tcp).await {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    drop(stream);
                }
            }
        });
    });
    (handle, addr, fingerprint)
}

#[test]
fn connect_verb_pair_device_tofu_pins_and_persists_the_pairing_record() {
    let tmp = tempdir().unwrap();
    let store = test_store(tmp.path());
    let outbound = PendingSends::new();
    let (server, addr, expected_fingerprint) = spawn_loopback_pair_device();

    let reply: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "pair-device",
        &json!({
            "id": "device-qr",
            "name": "QR Phone",
            "addr": addr.to_string(),
        }),
    ))
    .expect("pair-device reply");
    server.join().expect("loopback pair device exits");

    assert_eq!(reply["ok"], true);
    assert_eq!(reply["device_id"], "device-qr");
    assert_eq!(reply["fingerprint"], expected_fingerprint);
    let reopened = PairingStore::open(tmp.path()).unwrap();
    let record = reopened.get("device-qr").expect("persisted pair record");
    assert_eq!(record.device_name, "QR Phone");
    assert_eq!(record.fingerprint, expected_fingerprint);
    assert!(tmp.path().join("sessions.enc").exists());
}

#[test]
fn connect_verb_ring_requires_paired_and_enqueues() {
    let tmp = tempdir().unwrap();
    let store = test_store(tmp.path());
    let outbound = PendingSends::new();
    // ring an unpaired device -> NoSuchDevice, nothing queued.
    let r: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "ring",
        &json!({ "device_id": "d1" }),
    ))
    .unwrap();
    assert_eq!(r["error"], "NoSuchDevice");
    assert_eq!(outbound.len(), 0);
    // pair then ring -> ok + one queued packet.
    handle_connect_verb(&store, &outbound, "pair", &pair_body("d1", "Pixel"));
    let r2: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "ring",
        &json!({ "device_id": "d1" }),
    ))
    .unwrap();
    assert_eq!(r2["ok"], true);
    assert_eq!(outbound.len(), 1);
}

#[test]
fn outbound_take_all_drains_the_queue() {
    // AUD-2 — the kdc_outbound drainer takes the whole backlog each tick.
    let q = PendingSends::new();
    assert_eq!(q.len(), 0);
    q.push(OutboundSend {
        device_id: "d1".into(),
        packet: build_packet("kdeconnect.findmyphone.request", json!({})),
    });
    q.push(OutboundSend {
        device_id: "d2".into(),
        packet: build_packet("kdeconnect.findmyphone.request", json!({})),
    });
    assert_eq!(q.len(), 2);
    let drained = q.take_all();
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].device_id, "d1");
    assert_eq!(q.len(), 0, "queue is empty after a drain");
    // A second drain on the empty queue is a no-op.
    assert!(q.take_all().is_empty());
}

#[test]
fn connect_verb_share_requires_paired_and_enqueues_file_or_url() {
    // PD-3/L6 — Send-file from the Peers Devices group enqueues a
    // share packet; an unpaired device is refused with nothing queued,
    // and a share with neither url nor filename is rejected.
    let tmp = tempdir().unwrap();
    let store = test_store(tmp.path());
    let outbound = PendingSends::new();
    // share to an unpaired device -> NoSuchDevice, nothing queued.
    let r: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "share",
        &json!({ "device_id": "d1", "filename": "report.pdf" }),
    ))
    .unwrap();
    assert_eq!(r["error"], "NoSuchDevice");
    assert_eq!(outbound.len(), 0);
    handle_connect_verb(&store, &outbound, "pair", &pair_body("d1", "Pixel"));
    // empty share (no url, no filename) is rejected, still nothing queued.
    let empty: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "share",
        &json!({ "device_id": "d1" }),
    ))
    .unwrap();
    assert_eq!(empty["ok"], false);
    assert_eq!(outbound.len(), 0);
    // file share -> ok + one queued packet.
    let f: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "share",
        &json!({ "device_id": "d1", "filename": "report.pdf", "payload_size": 2048 }),
    ))
    .unwrap();
    assert_eq!(f["ok"], true);
    assert_eq!(outbound.len(), 1);
    // url share -> ok + a second queued packet.
    let u: Value = serde_json::from_str(&handle_connect_verb(
        &store,
        &outbound,
        "share",
        &json!({ "device_id": "d1", "url": "https://example.com" }),
    ))
    .unwrap();
    assert_eq!(u["ok"], true);
    assert_eq!(outbound.len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn worker_exits_on_shutdown_request() {
    let tmp = tempdir().unwrap();
    let mut w = KdcHostWorker::new(tmp.path().to_path_buf());
    let (tx, rx) = tokio::sync::watch::channel(false);
    let token = super::super::ShutdownToken::from_receiver(rx);

    let handle = tokio::spawn(async move { w.run(token).await });
    tx.send(true).expect("shutdown channel intact");
    let result = handle.await.expect("worker join");
    assert!(result.is_ok(), "worker must exit Ok on shutdown");
    // identity.pkcs8 was created during init.
    assert!(tmp.path().join("identity.pkcs8").exists());
}

// ── E2.3 live-roster folding (the host that moved off the shell daemon) ──

use mde_kdc_host::PeerId;
use mde_kdc_proto::plugins::battery::battery_packet;

fn announce(id: &str, name: &str) -> Announce {
    Announce {
        device_id: id.into(),
        device_name: name.into(),
        device_type: DeviceType::Phone,
        protocol_version: 7,
        incoming_capabilities: vec![],
        outgoing_capabilities: vec![],
    }
}

// ── KDC-MESH-5: bidirectional mesh notifications ─────────────────────────

fn notif_pkt(id: &str, app: &str, ticker: &str) -> mde_kdc_proto::wire::Packet {
    build_packet(
        "kdeconnect.notification",
        json!({ "id": id, "appName": app, "ticker": ticker }),
    )
}

fn test_ctx(host: &str, tmp: &std::path::Path) -> NotifyCtx {
    NotifyCtx {
        hostname: host.to_string(),
        bus_root: Some(tmp.join(format!("bus-{host}"))),
        db_path: tmp.join(format!("mded-{host}.db")),
    }
}

fn phone_lane_len(bus_root: &std::path::Path) -> usize {
    Persist::open(bus_root.to_path_buf())
        .unwrap()
        .list_since(NOTIFY_TOPIC_PHONE, None)
        .map(|v| v.len())
        .unwrap_or(0)
}

fn audit_row_count(db_path: &std::path::Path) -> usize {
    let conn = crate::store::open(db_path).unwrap();
    crate::store::load_audit_rows(&conn).unwrap().len()
}

#[test]
fn phone_notify_summary_collapses_empty_parts() {
    assert_eq!(phone_notify_summary("Signal", "hi"), "Signal: hi");
    assert_eq!(phone_notify_summary("", "hi"), "hi");
    assert_eq!(phone_notify_summary("Signal", ""), "Signal");
    assert_eq!(phone_notify_summary("  ", "  "), "");
}

#[test]
fn parse_inbound_notification_parses_skips_cancel_and_keys_distinctly() {
    let peer = PeerId::from("moto");
    let n = parse_inbound_notification(&peer, &notif_pkt("m1", "Signal", "new message"), "Moto")
        .expect("a content notification parses");
    assert_eq!(n.summary, "Signal: new message");
    assert_eq!(n.phone_id, "moto");
    assert_eq!(n.severity, "info");
    assert!(n.key.contains("moto") && n.key.contains("m1"));
    // A cancel (dismissal) is skipped — no desktop toast.
    let cancel = build_packet(
        "kdeconnect.notification",
        json!({ "id": "m1", "isCancel": true }),
    );
    assert!(parse_inbound_notification(&peer, &cancel, "Moto").is_none());
    // A content-less notification is skipped.
    assert!(parse_inbound_notification(&peer, &notif_pkt("m2", "", ""), "Moto").is_none());
    // Distinct notification ids ⇒ distinct de-dup keys.
    let a = parse_inbound_notification(&peer, &notif_pkt("m1", "S", "x"), "Moto").unwrap();
    let b = parse_inbound_notification(&peer, &notif_pkt("m2", "S", "x"), "Moto").unwrap();
    assert_ne!(a.key, b.key);
    // A wrong packet kind isn't a notification.
    assert!(
        parse_inbound_notification(&peer, &build_packet("kdeconnect.ping", json!({})), "Moto")
            .is_none()
    );
}

#[test]
fn notify_seen_dedups_and_is_bounded() {
    let mut seen = NotifySeen::default();
    assert!(seen.admit("k1"), "first sight acts");
    assert!(!seen.admit("k1"), "second sight suppressed");
    // Flood past the cap; the ring stays bounded and old keys are evicted.
    for i in 0..(NOTIFY_SEEN_CAP * 2) {
        seen.admit(&format!("f{i}"));
    }
    assert!(seen.recent.len() <= NOTIFY_SEEN_CAP, "seen ring is bounded");
}

#[test]
fn mesh_notify_packet_builds_a_kdeconnect_notification() {
    let n = MeshNotify {
        id: "01ULID".into(),
        source: "service".into(),
        host: "nyc3".into(),
        summary: "service sshd.service failed".into(),
    };
    let pkt = mesh_notify_packet(&n, 42);
    assert_eq!(pkt.kind, "kdeconnect.notification");
    let body: NotificationBody = mde_kdc_proto::plugins::from_packet_body(&pkt).expect("decodes");
    assert_eq!(body.app_name, "Quasar Mesh");
    assert_eq!(body.id, "01ULID");
    assert_eq!(body.text, "service sshd.service failed");
    assert!(body.title.contains("nyc3") && body.title.contains("service"));
    assert!(!body.is_cancel);
}

#[test]
fn phone_notification_fans_out_and_dedups_across_nodes() {
    // Two nodes (A + B) share ONE replicated relay root (the substrate) but each
    // has its own local bus + audit store. A phone notification received on A
    // must appear on B's desktop feed exactly once — and never twice even if B
    // also receives it directly or drains the relay again.
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("shared");
    let ctx_a = test_ctx("nodeA", tmp.path());
    let ctx_b = test_ctx("nodeB", tmp.path());
    let mut seen_a = NotifySeen::default();
    let mut seen_b = NotifySeen::default();

    let peer = PeerId::from("moto");
    let n = parse_inbound_notification(&peer, &notif_pkt("m1", "Signal", "ping!"), "Moto")
        .expect("parses");

    // Node A receives it: republishes to A's feed + relays it + audits.
    ctx_a.fanout_inbound(&root, &mut seen_a, &n, 1_000);
    assert_eq!(
        phone_lane_len(ctx_a.bus_root.as_deref().unwrap()),
        1,
        "the notification is on node A's desktop feed"
    );
    assert!(
        audit_row_count(&ctx_a.db_path) >= 1,
        "the fan-out is audited (#16)"
    );
    // It's on the replicated substrate for peers to pick up.
    assert_eq!(
        crate::workers::mesh_shunt::collect_notify_relay(
            &root,
            "nodeB",
            1_200,
            NOTIFY_RELAY_STALE_MS
        )
        .len(),
        1,
        "node B sees A's relayed notification on the substrate"
    );

    // Node B drains the relay: the notification appears on B's feed (fan-out).
    assert_eq!(ctx_b.drain_relayed(&root, &mut seen_b, 1_200), 1);
    assert_eq!(
        phone_lane_len(ctx_b.bus_root.as_deref().unwrap()),
        1,
        "the phone notification fanned out to node B's desktop feed"
    );

    // De-dup: draining again surfaces nothing (already seen).
    assert_eq!(ctx_b.drain_relayed(&root, &mut seen_b, 1_300), 0);
    // De-dup across paths: if B ALSO receives it directly from the phone, it's a
    // no-op — one phone notification is never N toasts on a single desktop.
    ctx_b.fanout_inbound(&root, &mut seen_b, &n, 1_400);
    assert_eq!(
        phone_lane_len(ctx_b.bus_root.as_deref().unwrap()),
        1,
        "still exactly one toast on node B after a duplicate direct receipt"
    );
}

#[test]
fn mesh_notify_forward_is_forward_only_and_skips_the_prime() {
    // The mesh→phone drainer forwards only notifications produced AFTER it first
    // saw a lane (no backlog replay), and never forwards the benign prime.
    let tmp = tempdir().unwrap();
    let bus = tmp.path().join("bus");
    let persist = Persist::open(bus.clone()).unwrap();
    let service = format!("{}service", crate::workers::notify::NOTIFY_TOPIC_PREFIX);
    let body = |summary: &str| {
        json!({ "severity": "warning", "source": "service", "summary": summary, "host": "nyc3" })
            .to_string()
    };
    // A backlog message exists before the first drain.
    persist
        .write(
            &service,
            Priority::Default,
            None,
            Some(&body("old failure")),
        )
        .unwrap();
    let mut cursors: HashMap<String, String> = HashMap::new();
    // First drain: forward-only — seeds the cursor, replays nothing.
    assert!(collect_local_notifies(&persist, &mut cursors).is_empty());
    // Now a prime + a real notification land.
    persist
        .write(
            &service,
            Priority::Default,
            None,
            Some(&body("notify monitor online")),
        )
        .unwrap();
    persist
        .write(
            &service,
            Priority::Default,
            None,
            Some(&body("nginx.service failed")),
        )
        .unwrap();
    let got = collect_local_notifies(&persist, &mut cursors);
    assert_eq!(
        got.len(),
        1,
        "only the real, post-cursor notification is forwarded"
    );
    assert_eq!(got[0].summary, "nginx.service failed");
    assert_eq!(got[0].source, "service");
}

#[tokio::test]
async fn mesh_forward_is_honest_noop_when_unpaired() {
    // No paired phone: the forwarder drains nothing (a later pairing seeds
    // forward-only — no backlog dump) and never fakes a delivery or an audit.
    let tmp = tempdir().unwrap();
    let ctx = test_ctx("nodeA", tmp.path());
    let store = Arc::new(PairingStore::open(tmp.path().join("pair-unpaired")).unwrap());
    let transport = OverlayTransport::new(announce("nodeA", "Node A"), Arc::clone(&store));
    // A notification exists on the bus, but with no phone paired nothing forwards.
    let bus = ctx.bus_root.clone().unwrap();
    let persist = Persist::open(bus.clone()).unwrap();
    let service = format!("{}service", crate::workers::notify::NOTIFY_TOPIC_PREFIX);
    persist
            .write(&service, Priority::Default, None, Some(
                &json!({"severity":"warning","source":"service","summary":"x failed","host":"nodeA"}).to_string(),
            ))
            .unwrap();
    let mut cursors: HashMap<String, String> = HashMap::new();
    ctx.forward_to_phones(&transport, &store, &mut cursors)
        .await;
    assert!(
        cursors.is_empty(),
        "no draining occurs with no paired phone"
    );
    assert_eq!(audit_row_count(&ctx.db_path), 0, "no fake-delivery audit");
}

#[tokio::test]
async fn mesh_forward_to_a_paired_but_unreachable_phone_is_an_honest_noop() {
    // A paired phone with no live link and no known overlay IP: the forwarder
    // tries the overlay, fails honestly, and audits NOTHING (no fake delivery).
    let tmp = tempdir().unwrap();
    let ctx = test_ctx("nodeA", tmp.path());
    let store = Arc::new(PairingStore::open(tmp.path().join("pair-unreach")).unwrap());
    store
        .pair(DeviceRecord {
            device_id: "moto".into(),
            device_name: "Moto".into(),
            paired_at_ms: 1,
            fingerprint: String::new(),
        })
        .unwrap();
    let transport = OverlayTransport::new(announce("nodeA", "Node A"), Arc::clone(&store));
    let bus = ctx.bus_root.clone().unwrap();
    let persist = Persist::open(bus.clone()).unwrap();
    let service = format!("{}service", crate::workers::notify::NOTIFY_TOPIC_PREFIX);
    let write_notify = |summary: &str| {
        persist
                .write(
                    &service,
                    Priority::Default,
                    None,
                    Some(
                        &json!({"severity":"warning","source":"service","summary":summary,"host":"nodeA"})
                            .to_string(),
                    ),
                )
                .unwrap();
    };
    let mut cursors: HashMap<String, String> = HashMap::new();
    // A first notification seeds the cursor forward-only (the first drain skips
    // it); a second, post-cursor notification IS drained + delivery attempted —
    // but the phone is unreachable, so nothing is delivered and nothing audited.
    write_notify("old failure");
    ctx.forward_to_phones(&transport, &store, &mut cursors)
        .await;
    write_notify("nginx failed");
    ctx.forward_to_phones(&transport, &store, &mut cursors)
        .await;
    assert_eq!(
        audit_row_count(&ctx.db_path),
        0,
        "an unreachable phone is delivered nothing → nothing audited"
    );
}

#[test]
fn kdc_event_alert_classifies_notable_events() {
    use mde_kdc_proto::wire::Packet;
    let pkt = |kind: &str, body: serde_json::Value| HostEvent::Packet {
        peer: PeerId::from("moto"),
        packet: serde_json::from_value::<Packet>(json!({"id":0,"type":kind,"body":body}))
            .expect("packet"),
    };
    // KDC-NOISE-1 — connect/disconnect presence churn + bare pings are NOT
    // surfaced to the Alert Center (too noisy; presence lives in the roster).
    assert!(kdc_event_alert(&HostEvent::Connected(PeerId::from("moto"))).is_none());
    assert!(kdc_event_alert(&HostEvent::Disconnected(PeerId::from("moto"))).is_none());
    assert!(kdc_event_alert(&pkt("kdeconnect.ping", json!({}))).is_none());
    // a phone notification mirrors app + text.
    let (s, sev) = kdc_event_alert(&pkt(
        "kdeconnect.notification",
        json!({"appName":"Signal","ticker":"new message"}),
    ))
    .expect("notification alert");
    assert_eq!(sev, "info");
    assert!(s.contains("Signal") && s.contains("new message"));
    // a cancel is skipped.
    assert!(kdc_event_alert(&pkt("kdeconnect.notification", json!({"isCancel":true}))).is_none());
    // low battery warns; healthy battery is silent.
    assert_eq!(
        kdc_event_alert(&pkt("kdeconnect.battery", json!({"currentCharge":9}))).map(|(_, s)| s),
        Some("warn")
    );
    assert!(kdc_event_alert(&pkt("kdeconnect.battery", json!({"currentCharge":80}))).is_none());
    // noisy discovery refreshes are skipped.
    assert!(kdc_event_alert(&HostEvent::PeerLost(PeerId::from("moto"))).is_none());
}

#[test]
fn apply_event_connected_then_disconnected_flips_online() {
    let mut m = HashMap::new();
    apply_event(&mut m, HostEvent::Connected(PeerId::from("p1")));
    assert!(m["p1"].online, "Connected brings the peer online");
    apply_event(&mut m, HostEvent::Disconnected(PeerId::from("p1")));
    assert!(
        !m["p1"].online,
        "Disconnected takes it offline (kept in roster)"
    );
    assert!(m.contains_key("p1"));
}

#[test]
fn apply_event_discovery_refreshes_the_display_name() {
    let mut m = HashMap::new();
    m.insert("p1".to_string(), DeviceInfo::unknown("p1"));
    apply_event(&mut m, HostEvent::PeerDiscovered(announce("p1", "Pixel 8")));
    assert_eq!(m["p1"].name, "Pixel 8");
}

#[test]
fn apply_event_battery_updates_charge_and_clamps_unknown() {
    let mut m = HashMap::new();
    m.insert("p1".to_string(), DeviceInfo::unknown("p1"));
    apply_event(
        &mut m,
        HostEvent::Packet {
            peer: PeerId::from("p1"),
            packet: battery_packet(
                1,
                BatteryBody {
                    current_charge: 73,
                    is_charging: false,
                    threshold_event: String::new(),
                },
            ),
        },
    );
    assert_eq!(m["p1"].battery, Some(73));
    // Upstream's -1 "unknown" sentinel sanitizes to None.
    apply_event(
        &mut m,
        HostEvent::Packet {
            peer: PeerId::from("p1"),
            packet: battery_packet(
                2,
                BatteryBody {
                    current_charge: -1,
                    is_charging: false,
                    threshold_event: String::new(),
                },
            ),
        },
    );
    assert_eq!(m["p1"].battery, None);
}

#[test]
fn roster_json_round_trips_sorted_with_optional_battery() {
    let mut map = HashMap::new();
    map.insert(
        "zeta".to_string(),
        DeviceInfo {
            id: "zeta".into(),
            name: "Zeta".into(),
            online: true,
            battery: Some(80),
        },
    );
    map.insert(
        "alpha".to_string(),
        DeviceInfo {
            id: "alpha".into(),
            name: "Alpha".into(),
            online: false,
            battery: None,
        },
    );
    let roster: Roster = Arc::new(Mutex::new(map));
    let wires: Vec<WireDevice> =
        serde_json::from_str(&roster_json(&roster)).expect("decode roster json");
    assert_eq!(wires.len(), 2);
    assert_eq!(wires[0].id, "alpha");
    assert_eq!(wires[0].battery, None);
    assert_eq!(wires[1].id, "zeta");
    assert!(wires[1].online);
    assert_eq!(wires[1].battery, Some(80));
}

// ── KDC-MESH-2: directed discovery over the mesh roster ──────────────────

#[test]
fn kdc_mesh2_roster_feeds_directory_and_selects_the_phone() {
    use std::net::{IpAddr, Ipv4Addr};
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    let pairing = Arc::new(PairingStore::open(&cfg).unwrap());
    // Locally pair phone-1 (known mesh-wide) + phone-ghost (no overlay IP).
    for (id, name) in [("phone-1", "Pixel"), ("phone-ghost", "Ghost")] {
        pairing
            .pair(DeviceRecord {
                device_id: id.into(),
                device_name: name.into(),
                paired_at_ms: 1,
                fingerprint: "AB:CD".into(),
            })
            .unwrap();
    }
    let transport = OverlayTransport::new(local_announce(), Arc::clone(&pairing))
        .with_overlay_ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
        .with_listen_port(0);
    let roster: Roster = Arc::new(Mutex::new(HashMap::new()));
    let registry = std::sync::Mutex::new(mde_kdc_proto::discovery::DiscoveryRegistry::new());

    // A neighbor ("hostA") publishes its overlay identity + phone-1's IP.
    let shared = tmp.path().join("shared");
    super::super::mesh_shunt::publish_roster(
        &shared,
        "hostA",
        "hostA-devid",
        Some("10.42.0.9".into()),
        &[super::super::mesh_shunt::PublishedDevice {
            device_id: "phone-1".into(),
            device_name: "Pixel".into(),
            overlay_ip: Some("10.42.0.77".into()),
            ..Default::default()
        }],
    )
    .unwrap();

    // THIS host's shunt tick: relay + fold the overlay directory + republish.
    let host_ip = IpAddr::V4(Ipv4Addr::new(10, 42, 0, 5));
    let phone_ids = run_shunt_tick(
        &pairing,
        &roster,
        &shared,
        "hostB",
        &registry,
        &transport,
        HostOverlay {
            device_id: "hostB-devid",
            overlay_ip: Some(host_ip),
        },
    );

    // (1) The roster fed the directory: phone-1 + the neighbor host resolve.
    let dir = transport.peer_directory();
    let guard = dir.lock().unwrap();
    assert_eq!(
        guard.get("phone-1"),
        Some(&IpAddr::V4(Ipv4Addr::new(10, 42, 0, 77))),
        "the phone's overlay IP flowed roster→directory"
    );
    assert_eq!(
        guard.get("hostA-devid"),
        Some(&IpAddr::V4(Ipv4Addr::new(10, 42, 0, 9))),
        "the neighbor host's overlay IP flowed roster→directory"
    );

    // (2) Directed-announce target selection: phone-1 at its overlay IP;
    // phone-ghost (no roster IP) is honestly excluded (not_discovered); a
    // host is never a directed-announce target.
    let targets = directed_announce_targets(&phone_ids, &guard);
    assert_eq!(
        targets,
        vec![(
            PeerId::from("phone-1"),
            IpAddr::V4(Ipv4Addr::new(10, 42, 0, 77))
        )],
        "only the known-IP phone is a directed-announce target"
    );
    assert!(
        !targets.iter().any(|(p, _)| p.as_str() == "phone-ghost"),
        "an unknown-IP phone is not_discovered, never a target (no broadcast)"
    );
    assert!(
        !targets.iter().any(|(p, _)| p.as_str() == "hostA-devid"),
        "a host is never a directed-announce target"
    );
    drop(guard);

    // (3) We republished our own overlay identity so a third host learns us.
    let raw =
        std::fs::read_to_string(super::super::mesh_shunt::phones_dir(&shared).join("hostB.json"))
            .unwrap();
    let back = super::super::mesh_shunt::parse_roster(&raw).expect("our roster parses");
    assert_eq!(back.host_device_id, "hostB-devid");
    assert_eq!(back.host_overlay_ip.as_deref(), Some("10.42.0.5"));
}

// ── KDC-MESH-3 (#5): mesh-wide pairing replicates through the shunt tick ──

#[test]
fn kdc_mesh3_shunt_tick_replicates_a_neighbor_pairing_into_the_store() {
    use std::net::{IpAddr, Ipv4Addr};
    let tmp = tempdir().unwrap();
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    // THIS node has paired NOTHING locally (an honest, unsynced start).
    let pairing = Arc::new(PairingStore::open(&cfg).unwrap());
    assert!(
        !pairing.is_paired("phone-1"),
        "unsynced: honest gate, no trust"
    );

    let transport = OverlayTransport::new(local_announce(), Arc::clone(&pairing))
        .with_overlay_ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
        .with_listen_port(0);
    let roster: Roster = Arc::new(Mutex::new(HashMap::new()));
    let registry = std::sync::Mutex::new(mde_kdc_proto::discovery::DiscoveryRegistry::new());

    // A neighbor (hostA) publishes phone-1 as a PAIRED device (carrying the pin
    // from ITS TOFU) plus a pin-less name-relay phone-ghost.
    let shared = tmp.path().join("shared");
    super::super::mesh_shunt::publish_phones(
        &shared,
        "hostA",
        &[
            super::super::mesh_shunt::PublishedDevice {
                device_id: "phone-1".into(),
                device_name: "Pixel".into(),
                fingerprint: "AA:BB:CC".into(),
                paired_at_ms: 77,
                ..Default::default()
            },
            super::super::mesh_shunt::PublishedDevice {
                device_id: "phone-ghost".into(),
                device_name: "Ghost".into(),
                ..Default::default()
            },
        ],
    )
    .unwrap();

    // THIS host's shunt tick folds neighbors' pairings into the local store.
    let _ = run_shunt_tick(
        &pairing,
        &roster,
        &shared,
        "hostB",
        &registry,
        &transport,
        HostOverlay {
            device_id: "hostB-devid",
            overlay_ip: Some(IpAddr::V4(Ipv4Addr::new(10, 42, 0, 5))),
        },
    );

    // phone-1 is now recognized mesh-wide WITHOUT a local pairing, carrying
    // hostA's pin (so the transport enforces the same cert). phone-ghost (no
    // pin) is NOT trusted — the honest gate.
    assert!(
        pairing.is_paired("phone-1"),
        "the neighbor's pairing replicated in"
    );
    assert!(pairing.is_synced("phone-1"));
    assert!(
        !pairing.is_locally_paired("phone-1"),
        "recognized via mesh, not own-row"
    );
    assert_eq!(pairing.get("phone-1").unwrap().fingerprint, "AA:BB:CC");
    assert_eq!(
        pairing.synced_pairing("phone-1").unwrap().origin_host,
        "hostA"
    );
    assert!(
        !pairing.is_paired("phone-ghost"),
        "a pin-less relay stays untrusted"
    );
    // Own-row authority: we never republished the synced pairing as our own.
    let raw =
        std::fs::read_to_string(super::super::mesh_shunt::phones_dir(&shared).join("hostB.json"))
            .unwrap();
    let back = super::super::mesh_shunt::parse_roster(&raw).expect("our roster parses");
    assert!(
        back.devices.is_empty(),
        "synced pairings are not republished"
    );
}

#[test]
fn seed_roster_lists_paired_peers_offline() {
    // A device paired through the store seeds into the roster offline,
    // with no battery, so the worker answers `devices` before any link.
    let tmp = tempdir().unwrap();
    let store = test_store(tmp.path());
    handle_connect_verb(
        &store,
        &PendingSends::new(),
        "pair",
        &pair_body("d1", "Pixel"),
    );
    let seeded = seed_roster(&store);
    assert_eq!(seeded.len(), 1);
    let d = &seeded["d1"];
    assert_eq!(d.name, "Pixel");
    assert!(!d.online);
    assert_eq!(d.battery, None);
}

// ── KDC-MESH-7: two-way any-node files + the mesh service directory ───────

#[test]
fn default_shared_roots_reads_toml_and_drops_phantom_paths() {
    let tmp = tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("shared-roots.toml"),
        format!(
            "[[root]]\nlabel = \"Real\"\npath = {real:?}\n\
                 [[root]]\nlabel = \"Ghost\"\npath = \"/no/such/dir\"\n",
        ),
    )
    .unwrap();
    let roots = default_shared_roots(&cfg);
    // The phantom (non-existent) root is dropped — an honest, non-phantom share.
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].label, "Real");
}

#[test]
fn kdc_sftp_mountpoint_sanitizes_the_device_id_to_one_segment() {
    let mp = kdc_sftp_mountpoint("../../etc/moto id");
    // No path traversal survives: the id collapses to a single sanitized seg.
    assert!(mp.ends_with("kdc-sftp/______etc_moto_id"));
    assert!(mp.to_string_lossy().contains("kdc-sftp"));
}

#[test]
fn kdc_mesh7_service_directory_and_node_targeted_browse() {
    // `MDE_HOME` is process-global; serialize the few tests that redirect it
    // so the default parallel harness still writes audit rows hermetically.
    let _env_guard = MDE_HOME_LOCK.lock().unwrap();
    let tmp = tempdir().unwrap();
    std::env::set_var("MDE_HOME", tmp.path());

    // A shared root with a file + a subdir, and a SECRET dir outside it.
    let shared = tmp.path().join("share");
    std::fs::create_dir_all(shared.join("sub")).unwrap();
    std::fs::write(shared.join("a.txt"), b"hi").unwrap();
    let secret = tmp.path().join("secret");
    std::fs::create_dir_all(&secret).unwrap();

    let cfg = tmp.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("shared-roots.toml"),
        format!("[[root]]\nlabel = \"Share\"\npath = {shared:?}\n"),
    )
    .unwrap();

    // (c) The service directory: this node's snapshot advertises files + sftp
    // and carries a shallow snapshot of its shared root.
    let snap = node_services_snapshot(&cfg, "nodeA", "id-a", None);
    assert!(snap.offers(service_directory::service::FILES));
    assert!(snap.offers(service_directory::service::SFTP));
    assert_eq!(snap.shared_roots.len(), 1);
    assert!(snap.shared_roots[0]
        .entries
        .iter()
        .any(|e| e.name == "a.txt"));

    // Publish → collect → select any node (the directory round-trip, #7).
    let root = tmp.path().join("workgroup");
    service_directory::publish_services(&root, &snap).unwrap();
    service_directory::publish_services(
        &root,
        &NodeServices {
            node_host: "nodeB".into(),
            services: advertised_services(),
            ..Default::default()
        },
    )
    .unwrap();
    let all = service_directory::collect_all_services(&root);
    assert_eq!(all.len(), 2);
    let picked = service_directory::select_node(&all, "nodeA").expect("node targetable");
    assert_eq!(picked.node_host, "nodeA");

    // (b) A node-targeted file browse: serve the shared files, refuse an escape.
    let ok = serve_browse(&cfg, &json!({ "path": shared.to_string_lossy() }));
    let v: Value = serde_json::from_str(&ok).unwrap();
    assert_eq!(v["ok"], true);
    let names: Vec<String> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"a.txt".to_string()));
    assert!(names.contains(&"sub".to_string()));

    let refused = serve_browse(&cfg, &json!({ "path": secret.to_string_lossy() }));
    let rv: Value = serde_json::from_str(&refused).unwrap();
    assert_eq!(
        rv["ok"], false,
        "a path outside the shared roots is refused"
    );

    // The browse audited (design #16) into the hermetic MDE_HOME db.
    assert!(
        audit_row_count(&crate::default_db_path()) >= 1,
        "the node-targeted browse appended a KDC audit row",
    );

    std::env::remove_var("MDE_HOME");
}

#[test]
fn connect_verb_sftp_requires_paired_and_enqueues_a_browse_request() {
    let tmp = tempdir().unwrap();
    let store = test_store(tmp.path());
    let outbound = PendingSends::new();
    // Unpaired → refused, nothing enqueued.
    let miss = handle_connect_verb(&store, &outbound, "sftp", &json!({ "device_id": "d1" }));
    assert!(miss.contains("NoSuchDevice"));
    assert_eq!(outbound.len(), 0);
    // Pair then request SFTP browse → a `kdeconnect.sftp.request` is queued.
    handle_connect_verb(&store, &outbound, "pair", &pair_body("d1", "Pixel"));
    let ok = handle_connect_verb(&store, &outbound, "sftp", &json!({ "device_id": "d1" }));
    assert!(ok.contains(r#""ok":true"#));
    let queued = outbound.take_all();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].device_id, "d1");
    assert_eq!(queued[0].packet.kind, "kdeconnect.sftp.request");
}

// ── KDC-MESH-8: run-commands (OpenStack lifecycle) + telephony + connectivity ─

fn instance(name: &str, status: &str) -> CloudInstance {
    CloudInstance {
        id: format!("id-{name}"),
        name: name.to_string(),
        status: status.to_string(),
        flavor: None,
        image: None,
        networks: None,
    }
}

#[test]
fn cloud_command_keys_map_and_the_list_includes_them() {
    assert_eq!(
        CloudCommand::from_key("cloud-list"),
        Some(CloudCommand::List)
    );
    assert_eq!(
        CloudCommand::from_key("cloud-reboot-all"),
        Some(CloudCommand::RebootAll)
    );
    // A shell key isn't a cloud command (so it takes the shell path).
    assert_eq!(CloudCommand::from_key("mesh-health"), None);
    // The phone-visible command list carries the cloud entries.
    let list = command_list_json(&cloud_command_entries());
    for c in [
        "cloud-list",
        "cloud-start-all",
        "cloud-stop-all",
        "cloud-reboot-all",
    ] {
        assert!(list.contains(c), "command list missing {c}");
    }
    // Delete is deliberately NOT phone-exposed (safety).
    assert!(!list.contains("cloud-delete"));
}

#[test]
fn plan_cloud_lifecycle_filters_by_nova_status() {
    let fleet = [
        instance("web", "ACTIVE"),
        instance("db", "SHUTOFF"),
        instance("cache", "ACTIVE"),
        instance("broken", "ERROR"),
    ];
    // start-all acts on the SHUTOFF instance only.
    assert_eq!(
        plan_cloud_lifecycle(LifecycleAction::Start, &fleet),
        vec!["db".to_string()]
    );
    // stop-all / reboot-all act on the ACTIVE instances only.
    assert_eq!(
        plan_cloud_lifecycle(LifecycleAction::Stop, &fleet),
        vec!["web".to_string(), "cache".to_string()]
    );
    assert_eq!(
        plan_cloud_lifecycle(LifecycleAction::Reboot, &fleet),
        vec!["web".to_string(), "cache".to_string()]
    );
    // Delete targets nothing (never phone-exposed).
    assert!(plan_cloud_lifecycle(LifecycleAction::Delete, &fleet).is_empty());
}

#[test]
fn lifecycle_bus_verb_maps_to_the_openstack_action_verb() {
    assert_eq!(lifecycle_bus_verb(LifecycleAction::Start), "instance-start");
    assert_eq!(
        lifecycle_bus_verb(LifecycleAction::Reboot),
        "instance-reboot"
    );
}

#[test]
fn cloud_summaries_are_phone_friendly() {
    let fleet = [instance("web", "ACTIVE"), instance("db", "SHUTOFF")];
    let list = summarize_instances(&fleet);
    assert!(list.contains("web [ACTIVE]") && list.contains("db [SHUTOFF]"));
    let status = summarize_status(&fleet);
    assert!(status.contains("1 active") && status.contains("1 shutoff"));
    assert_eq!(summarize_instances(&[]), "No cloud instances");
}

#[test]
fn telephony_alert_flags_ringing_and_missed_only() {
    // Ringing + missed are notable call events (warn); talking/disconnected
    // and a cancel are not surfaced.
    let ringing = json!({ "event": "ringing", "contactName": "Alice" });
    assert_eq!(
        telephony_alert(&ringing),
        Some(("Incoming call from Alice".to_string(), "warn"))
    );
    let missed = json!({ "event": "missed", "phoneNumber": "+15551234" });
    assert_eq!(
        telephony_alert(&missed),
        Some(("Missed call from +15551234".to_string(), "warn"))
    );
    assert!(telephony_alert(&json!({ "event": "talking" })).is_none());
    assert!(telephony_alert(&json!({ "event": "disconnected" })).is_none());
    assert!(
        telephony_alert(&json!({ "event": "ringing", "isCancel": true })).is_none(),
        "a cancel is not a new alert"
    );
    // It also feeds the Alert-Center classifier.
    let pkt = build_packet("kdeconnect.telephony", ringing);
    let ev = HostEvent::Packet {
        peer: PeerId::from("moto"),
        packet: pkt,
    };
    assert!(matches!(kdc_event_alert(&ev), Some((_, "warn"))));
}

#[test]
fn format_connectivity_reads_the_mesh_route_and_links() {
    let up = format_connectivity(true, true, 2);
    assert!(
        up.contains("on the mesh") && up.contains("internet routable") && up.contains("2 link")
    );
    let down = format_connectivity(false, false, 0);
    assert!(down.contains("OFF the mesh") && down.contains("no default route"));
}

#[test]
fn kdc_mesh8_a_phone_action_appends_a_hash_chained_audit_row() {
    // design #16 — pairing is the auth, but EVERY action is recorded.
    // `MDE_HOME` is process-global; serialize this with the browse-audit test.
    let _env_guard = MDE_HOME_LOCK.lock().unwrap();
    let tmp = tempdir().unwrap();
    std::env::set_var("MDE_HOME", tmp.path());
    let before = audit_row_count(&crate::default_db_path());
    audit_kdc_action(
        json!({ "action": "kdc_openstack", "verb": "instance-reboot", "instance": "web" }),
    );
    let after = audit_row_count(&crate::default_db_path());
    assert_eq!(
        after,
        before + 1,
        "the phone-triggered action appended one audit row"
    );
    std::env::remove_var("MDE_HOME");
}
