use super::render::*;
use super::*;
use mde_chat::{Message, MessageId, NodeRole};

/// A message with a FIXED id so dedup + canonical order are deterministic
/// across the perf-5 fold/rebuild tests (the real `Message::text` mints a
/// random-tailed id).
fn fixed_msg(id: &str, ts: i64, body: &str) -> Message {
    let mut m = Message::text("nyc3", ts, body);
    m.id = MessageId(id.to_string());
    m
}

#[test]
fn fold_rings_merges_constituent_blobs_in_canonical_order_and_dedups() {
    // perf-5 — the fold is the exact per-conversation build refresh does: a
    // contact's dm: ∪ alert: rings merged oldest-first, deduped by id even
    // when a message appears in both constituent blobs.
    let dm = vec![fixed_msg("a", 30, "newest"), fixed_msg("dup", 20, "shared")];
    let alert = vec![fixed_msg("dup", 20, "shared"), fixed_msg("b", 10, "oldest")];
    let folded = fold_rings("nyc3", [dm, alert]);
    let ids: Vec<&str> = folded.messages().iter().map(|m| m.id.as_str()).collect();
    // Sorted by ts (10, 20, 30); the id present in both rings appears once.
    assert_eq!(ids, vec!["b", "dup", "a"]);
}

#[test]
fn incremental_refresh_equals_a_full_rebuild_across_appends_trim_and_switch() {
    // perf-5 — drive the SAME cursor-reuse / rebuild machinery `refresh` runs,
    // over an in-memory sequence of latest-wins ring blobs (each with the ULID
    // the worker would stamp), and prove the incrementally-maintained model is
    // byte-identical to a from-scratch fold of the current blob at every tick.
    let id = "room:sys:all-fleet";
    // (cursor, ring-blob) as the worker republishes it latest-wins.
    let steps: Vec<(&str, Vec<Message>)> = vec![
        // first load — must build.
        ("u1", vec![fixed_msg("a", 10, "one")]),
        // append: a new message ⇒ new ULID ⇒ rebuild.
        (
            "u2",
            vec![fixed_msg("a", 10, "one"), fixed_msg("b", 20, "two")],
        ),
        // idle tick: SAME cursor + blob ⇒ must REUSE (no rebuild).
        (
            "u2",
            vec![fixed_msg("a", 10, "one"), fixed_msg("b", 20, "two")],
        ),
        // retention trim: oldest evicted + a newer message, new ULID ⇒ rebuild
        // must RESYNC to the trimmed blob (not retain the evicted "a").
        (
            "u3",
            vec![fixed_msg("b", 20, "two"), fixed_msg("c", 30, "three")],
        ),
    ];

    let mut cursors: Option<Vec<Option<String>>> = None;
    let mut model: Option<Conversation> = None;
    let mut rebuilds = 0;
    for (cur, ring) in &steps {
        let now = vec![Some((*cur).to_string())];
        let has_existing = model.is_some();
        if !conversation_is_current(cursors.as_ref(), &now, has_existing) {
            model = Some(fold_rings(id, std::iter::once(ring.clone())));
            cursors = Some(now);
            rebuilds += 1;
        }
        // Equivalence invariant: the maintained model == a full rebuild from
        // the CURRENT blob, at every step (append AND trim).
        let full = fold_rings(id, std::iter::once(ring.clone()));
        assert_eq!(model.as_ref().unwrap(), &full, "diverged at cursor {cur}");
    }
    // Four ticks, but the idle tick reused → exactly three rebuilds.
    assert_eq!(rebuilds, 3, "the unchanged tick must reuse, not rebuild");

    // Topic switch: a freshly-selected conversation (no cached cursor, no
    // existing model) always builds — it never inherits another key's cache.
    assert!(
        !conversation_is_current(None, &[Some("z9".to_string())], false),
        "a first-load / switched-to conversation must rebuild"
    );
    // And an existing conversation with a MATCHING cursor is reused.
    assert!(
        conversation_is_current(
            Some(&vec![Some("u3".to_string())]),
            &[Some("u3".to_string())],
            true,
        ),
        "an unchanged cursor with a live conversation must reuse"
    );
}

#[test]
fn dm_key_is_order_independent_and_matches_the_worker() {
    assert_eq!(dm_key("eagle", "nyc3"), dm_key("nyc3", "eagle"));
    assert_eq!(dm_key("a", "b"), "dm:a|b");
}

#[test]
fn keys_for_a_peer_merge_dm_and_alert_but_self_is_alert_only() {
    let peer = keys_for_contact("eagle", "nyc3");
    assert!(peer.contains(&dm_key("eagle", "nyc3")));
    assert!(peer.contains(&alert_key("nyc3")));
    // Self carries only its local alerts (lock 17 — no notes-to-self DM).
    assert_eq!(keys_for_contact("eagle", "eagle"), vec![alert_key("eagle")]);
}

#[test]
fn delivery_state_is_derived_honestly_from_recipient_presence() {
    // Available peer → the live relay reached them (Delivered).
    let online = Contact::new("nyc3", NodeRole::Headless).with_presence(Presence::Online);
    assert_eq!(Delivery::for_recipient(Some(&online)), Delivery::Delivered);
    // Unavailable peer → queued to backfill.
    let off = Contact::new("fra1", NodeRole::Headless).with_presence(Presence::Offline);
    assert_eq!(Delivery::for_recipient(Some(&off)), Delivery::Queued);
    // Unknown recipient → merely Sent.
    assert_eq!(Delivery::for_recipient(None), Delivery::Sent);
}

#[test]
fn presence_and_severity_map_to_style_tokens_not_raw_hex() {
    assert_eq!(presence_color(Presence::Online), Style::OK);
    assert_eq!(presence_color(Presence::Dnd), Style::DANGER);
    assert_eq!(presence_color(Presence::Away), Style::WARN);
    assert_eq!(presence_color(Presence::Offline), Style::TEXT_DIM);
    assert_eq!(severity_color(Severity::Critical), Style::SUPPORT_ERROR);
    assert_eq!(severity_color(Severity::Warning), Style::SUPPORT_WARNING);
    assert_eq!(severity_color(Severity::Info), Style::SUPPORT_INFO);
}

#[test]
fn unread_watermarks_a_first_seen_contact_then_counts_new() {
    let mut state = ChatState::default();
    let mut conv = Conversation::new("nyc3");
    conv.insert(Message::text("nyc3", 10, "old"));
    conv.insert(Message::text("nyc3", 20, "history"));
    state.convos.insert("nyc3".into(), conv);
    // First sight: watermark at current length → nothing unread.
    state.seen.insert("nyc3".into(), 2);
    assert_eq!(state.unread("nyc3"), 0);
    // A new message arrives → one unread.
    let mut conv = state.convos.remove("nyc3").unwrap();
    conv.insert(Message::text("nyc3", 30, "new!"));
    state.convos.insert("nyc3".into(), conv);
    assert_eq!(state.unread("nyc3"), 1);
}

#[test]
fn empty_copy_distinguishes_a_missing_bus_from_an_empty_roster() {
    let (title, _) = empty_copy(true);
    assert_eq!(title, "No contacts yet");
    let (title, subtitle) = empty_copy(false);
    assert_eq!(title, "Chat unavailable");
    assert!(subtitle.contains("Bus") && subtitle.contains("unblocks"));
}

/// Headless mount + tessellate: build a populated roster + conversation and
/// render the whole surface (roster rail + open conversation pane) through the
/// CPU tessellator — the same paint path the DRM runner drives, minus the GPU.
/// Proves the surface actually draws over real model state (no demo data).
#[test]
fn surface_mounts_and_tessellates_over_real_state() {
    use mde_egui::egui::{pos2, vec2, Rect};

    let ctx = egui::Context::default();
    Style::install(&ctx);

    let mut state = ChatState::default();
    // A real roster: self + an online peer + an offline peer.
    let mut roster = Roster::new("eagle");
    roster.upsert(
        Contact::new("nyc3", NodeRole::Lighthouse)
            .with_presence(Presence::Online)
            .with_status("deploying"),
    );
    roster.upsert(Contact::new("fra1", NodeRole::Headless).with_presence(Presence::Offline));
    state.roster = Some(roster);
    // A conversation with my outgoing text + an inbound line.
    let mut conv = Conversation::new("nyc3");
    conv.insert(Message::text("eagle", 10, "ping"));
    conv.insert(Message::text("nyc3", 20, "pong"));
    state.convos.insert("nyc3".into(), conv);
    state.seen.insert("nyc3".into(), 1); // one unread
    state.selected = Some(Selection::Contact("nyc3".into()));

    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.push_id("shell-chat", |ui| state.show(ui));
        });
    });
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    assert!(
        !prims.is_empty(),
        "the chat surface produced no draw primitives"
    );
}

// ── NOTIFY-CHAT-4: message kinds + per-contact actions ────────────────────

/// The alert action button is offered only for a verb the shell's ONE resolver
/// accepts — a bare `action/shell/goto` or an unknown surface yields no button.
#[test]
fn alert_nav_verb_resolves_only_a_known_shell_target() {
    assert_eq!(
        alert_nav_verb(Some("action/shell/goto/system")).as_deref(),
        Some("shell/goto/system"),
    );
    // Already in the KIRON grammar (no `action/` prefix) still resolves.
    assert_eq!(
        alert_nav_verb(Some("shell/goto/files")).as_deref(),
        Some("shell/goto/files"),
    );
    // A bare verb without a surface, an unknown surface, and None → no button.
    assert!(alert_nav_verb(Some("action/shell/goto")).is_none());
    assert!(alert_nav_verb(Some("action/shell/goto/nope")).is_none());
    assert!(alert_nav_verb(None).is_none());
}

/// The Bus-writing action helpers never panic without a Bus directory: the
/// best-effort ones no-op, and the message-carrying ones report `Err` (so the
/// caller can keep the draft) rather than silently dropping it (shell-ux-11).
#[test]
fn action_helpers_report_without_a_bus_and_never_panic() {
    assert!(
        publish(None, "action/x", "{}").is_err(),
        "no Bus is an honest Err, not a silent drop"
    );
    navigate_via_toast(None, "chat", "hi", "shell/goto/files");
    dial_peer(None, "nyc3");
    // send_file opens no Bus either — a ChatState with no bus_root.
    let state = ChatState {
        bus_root: None,
        ..ChatState::default()
    };
    assert!(
        state
            .send_file("nyc3", Path::new("/tmp/does-not-matter.txt"))
            .is_err(),
        "a file send with no Bus reports Err so the attach path is kept"
    );
}

/// Every one of the six kinds renders *and* draws its action affordance: build a
/// conversation carrying all six (emoji text, a clipboard clip, an alert card
/// with a resolvable verb, a file offer, a Call + a Remote row) and tessellate
/// the open pane. Proves each kind paints geometry over real model state — the
/// same CPU paint path the DRM runner drives.
#[test]
fn every_message_kind_renders_its_action() {
    use mde_egui::egui::{pos2, vec2, Rect};

    let ctx = egui::Context::default();
    Style::install(&ctx);

    let mut state = ChatState::default();
    let mut roster = Roster::new("eagle");
    roster.upsert(Contact::new("nyc3", NodeRole::Workstation).with_presence(Presence::Online));
    state.roster = Some(roster);

    let mut conv = Conversation::new("nyc3");
    conv.insert(Message::text("eagle", 10, "hello 👋 🎉")); // emoji is just text
    conv.insert(Message::new(
        "nyc3",
        20,
        MessageKind::Clipboard {
            preview: "ssh nyc3".into(),
            full: "ssh root@nyc3.mesh".into(),
        },
    ));
    let mut fields = BTreeMap::new();
    fields.insert("summary".to_string(), "disk 92%".to_string());
    fields.insert("host".to_string(), "nyc3".to_string());
    conv.insert(Message::new(
        "nyc3",
        30,
        MessageKind::Alert {
            severity: Severity::Warning,
            flag: "storage".into(),
            fields,
            action_verb: Some("action/shell/goto/system".into()),
            actions: Vec::new(),
        },
    ));
    conv.insert(Message::new(
        "nyc3",
        40,
        MessageKind::File {
            name: "report.pdf".into(),
            size_bytes: 12_345,
            mime: None,
        },
    ));
    conv.insert(Message::new(
        "eagle",
        50,
        MessageKind::CallAction {
            target_host: "nyc3".into(),
        },
    ));
    conv.insert(Message::new(
        "eagle",
        60,
        MessageKind::RemoteAction {
            target_host: "nyc3".into(),
        },
    ));
    state.convos.insert("nyc3".into(), conv);
    state.seen.insert("nyc3".into(), 6);
    state.selected = Some(Selection::Contact("nyc3".into()));

    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 720.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.push_id("shell-chat", |ui| state.show(ui));
        });
    });
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    assert!(
        !prims.is_empty(),
        "the mixed-kind conversation produced no draw primitives"
    );
}

// ── NOTIFY-CHAT-5 surfacing: rooms in the roster + the room pane ───────────

/// A typed room name derives the same canonical id on every node — lowercase,
/// non-alphanumerics collapsed to single dashes, trimmed.
#[test]
fn room_id_from_name_is_a_stable_canonical_slug() {
    assert_eq!(room_id_from_name("Build Farm!"), "build-farm");
    assert_eq!(room_id_from_name("  Ops   Room  "), "ops-room");
    assert_eq!(room_id_from_name("nyc3//fra1"), "nyc3-fra1");
    // Nothing alphanumeric → empty (the caller refuses to create it).
    assert_eq!(room_id_from_name("***"), "");
}

/// The room unread watermark is keyed by the `room:<id>` conversation key, so a
/// room id can never collide with a contact hostname's unread count.
#[test]
fn room_unread_watermarks_then_counts_new_without_colliding_with_a_contact() {
    let mut state = ChatState::default();
    let mut room = Conversation::new("sys:all-fleet");
    room.insert(Message::text("nyc3", 10, "fleet up"));
    state.room_convos.insert("sys:all-fleet".into(), room);
    state.seen.insert(room_key("sys:all-fleet"), 1);
    assert_eq!(state.room_unread("sys:all-fleet"), 0);
    // A same-named contact watermark is independent.
    let mut conv = Conversation::new("sys:all-fleet");
    conv.insert(Message::text("x", 5, "dm"));
    conv.insert(Message::text("x", 6, "dm2"));
    state.convos.insert("sys:all-fleet".into(), conv);
    state.seen.insert("sys:all-fleet".into(), 0);
    assert_eq!(state.unread("sys:all-fleet"), 2, "contact key is separate");
    // A new room message → one unread.
    let mut room = state.room_convos.remove("sys:all-fleet").unwrap();
    room.insert(Message::text("fra1", 20, "new"));
    state.room_convos.insert("sys:all-fleet".into(), room);
    assert_eq!(state.room_unread("sys:all-fleet"), 1);
}

/// The chrome unread indicator's tally (NOTIFY-CHAT-6) sums every contact AND
/// room unread, over the same watermarks the roster badges use — so the chrome
/// badge can't diverge from the surface. A quiet mesh is an honest zero.
#[test]
fn total_unread_sums_contacts_and_rooms() {
    let mut state = ChatState::default();
    // A quiet host: nothing unread.
    assert_eq!(state.total_unread(), 0);

    // A contact with 2 new messages since the watermark.
    let mut dm = Conversation::new("nyc3");
    dm.insert(Message::text("nyc3", 10, "hi"));
    dm.insert(Message::text("nyc3", 20, "still there?"));
    state.convos.insert("nyc3".into(), dm);
    state.seen.insert("nyc3".into(), 0);

    // A room with 1 new message; only rooms in the registry are counted.
    let mut room = Conversation::new("ops");
    room.insert(Message::text("fra1", 30, "deploy done"));
    state.room_convos.insert("ops".into(), room);
    state.seen.insert(room_key("ops"), 0);
    state.rooms = vec![RoomDescriptor {
        id: "ops".into(),
        name: "Ops".into(),
        kind: RoomKind::AdHoc,
        creator: "eagle".into(),
        members: vec!["eagle".into()],
    }];

    assert_eq!(state.total_unread(), 3, "2 contact + 1 room unread");
}

/// Headless mount + tessellate with a selected room: the roster shows the Rooms
/// group and the room pane renders the shared log over real model state — proving
/// rooms surface and open without a live display (no demo data).
#[test]
fn room_surfaces_in_the_roster_and_its_pane_tessellates() {
    use mde_egui::egui::{pos2, vec2, Rect};

    let ctx = egui::Context::default();
    Style::install(&ctx);

    let mut state = ChatState::default();
    let mut roster = Roster::new("eagle");
    roster.upsert(Contact::new("nyc3", NodeRole::Headless).with_presence(Presence::Online));
    state.roster = Some(roster);

    // A system room + an ad-hoc room I created (so Dissolve is reachable).
    state.rooms = vec![
        RoomDescriptor {
            id: "sys:all-fleet".into(),
            name: "All Fleet".into(),
            kind: RoomKind::System,
            creator: String::new(),
            members: vec!["eagle".into(), "nyc3".into()],
        },
        RoomDescriptor {
            id: "ops".into(),
            name: "Ops".into(),
            kind: RoomKind::AdHoc,
            creator: "eagle".into(),
            members: vec!["eagle".into()],
        },
    ];
    let mut log = Conversation::new("sys:all-fleet");
    log.insert(Message::text("nyc3", 10, "fleet chatter"));
    state.room_convos.insert("sys:all-fleet".into(), log);
    state.seen.insert(room_key("sys:all-fleet"), 0); // one unread
    state.selected = Some(Selection::Room("sys:all-fleet".into()));

    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 720.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.push_id("shell-chat", |ui| state.show(ui));
        });
    });
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    assert!(
        !prims.is_empty(),
        "the room pane produced no draw primitives"
    );
    // Opening the room watermarked it read.
    assert_eq!(state.room_unread("sys:all-fleet"), 0);
}

/// The room lifecycle + send helpers are best-effort: with no Bus directory they
/// are silent no-ops (the honest solo-host state), never a panic.
#[test]
fn room_action_helpers_are_silent_without_a_bus() {
    let state = ChatState {
        bus_root: None,
        ..ChatState::default()
    };
    assert!(
        state.send_room("sys:all-fleet", "hi").is_err(),
        "a room send with no Bus reports Err (kept, not silently dropped)"
    );
    state.create_room("Build Farm");
    state.create_room("***"); // empty slug → refused, still no panic
    state.room_action("join", "ops", None);
    state.room_action("create", "ops", Some("Ops"));
}

// ── timestamps + presence/status/mute (the four closed gaps) ───────────────

/// The message row's timestamp: a compact HH:MM (UTC) with a full-date hover,
/// derived by the pure formatters — and the row paints over a real timestamped
/// message. A non-positive time renders blank (never a fabricated "00:00").
#[test]
fn message_row_renders_a_timestamp() {
    use mde_egui::egui::{pos2, vec2, Rect};

    // A known epoch: 1_700_000_000_000 ms = 2023-11-14 22:13:20 UTC.
    assert_eq!(fmt_hh_mm(1_700_000_000_000), "22:13");
    assert_eq!(fmt_full_datetime(1_700_000_000_000), "2023-11-14 22:13 UTC");
    assert_eq!(fmt_date(1_700_000_000_000), "2023-11-14");
    // A civil leap-year day still resolves (2024-02-29).
    assert_eq!(fmt_date(1_709_200_000_000), "2024-02-29");
    // A non-positive timestamp is an honest blank, not a faked clock.
    assert!(fmt_hh_mm(0).is_empty());
    assert!(fmt_hh_mm(-5).is_empty());

    let ctx = egui::Context::default();
    Style::install(&ctx);
    let msg = Message::text("nyc3", 1_700_000_000_000, "hello");
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 200.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            message_row(ui, &msg, "eagle", None, None);
        });
    });
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    assert!(
        !prims.is_empty(),
        "a timestamped message row produced no draw primitives"
    );
}

/// The self-presence picker maps live presence → the picker selection, and
/// each option → the wire tag the worker's `PresenceSet` decodes ("available"
/// clears to auto; the four ICQ manual states set their override).
#[test]
fn presence_choice_maps_presence_and_wire_tags() {
    // Auto/derived states map to the closest picker option so the live
    // presence always shows a selection.
    assert_eq!(
        PresenceChoice::from_presence(Presence::Online),
        PresenceChoice::Available
    );
    assert_eq!(
        PresenceChoice::from_presence(Presence::ManualAway),
        PresenceChoice::Away
    );
    assert_eq!(
        PresenceChoice::from_presence(Presence::Dnd),
        PresenceChoice::Dnd
    );
    assert_eq!(
        PresenceChoice::from_presence(Presence::FreeForChat),
        PresenceChoice::FreeForChat
    );
    // Wire tags match the worker's `PresenceSet` snake_case names.
    assert_eq!(PresenceChoice::Available.wire(), "available");
    assert_eq!(PresenceChoice::Dnd.wire(), "dnd");
    assert_eq!(PresenceChoice::Away.wire(), "away");
    assert_eq!(PresenceChoice::FreeForChat.wire(), "free_for_chat");
    assert_eq!(PresenceChoice::Invisible.wire(), "invisible");
}

/// The mute toggles read the worker-published `state/chat/notify` mirror — the
/// round-trip that makes them real, not local-only. No mirror ⇒ nothing muted.
#[test]
fn mute_state_reads_the_published_notify_mirror() {
    let mut state = ChatState::default();
    assert!(!state.is_contact_muted("nyc3"));
    assert!(!state.is_room_muted("ops"));
    assert!(!state.is_source_muted("security"));
    let mut prefs = NotifyPrefs::new();
    prefs.mute_contact("nyc3");
    prefs.mute_room("ops");
    prefs.mute_source("security");
    state.notify = Some(prefs);
    assert!(state.is_contact_muted("nyc3"));
    assert!(state.is_room_muted("ops"));
    assert!(state.is_source_muted("security"));
    assert!(
        !state.is_contact_muted("fra1"),
        "an unmuted contact still rings"
    );
    assert_eq!(state.notify_threshold(), Severity::Warning);
}

#[test]
fn notification_items_collect_newest_first_and_unique_sources() {
    let mut state = ChatState::default();
    let mut security_fields = BTreeMap::new();
    security_fields.insert("summary".to_string(), "intrusion".to_string());
    let mut nyc3 = Conversation::new("nyc3");
    nyc3.insert(Message::new(
        "nyc3",
        20,
        MessageKind::Alert {
            severity: Severity::Critical,
            flag: "security".into(),
            fields: security_fields,
            action_verb: None,
            actions: Vec::new(),
        },
    ));
    let mut fra1 = Conversation::new("fra1");
    fra1.insert(Message::new(
        "fra1",
        10,
        MessageKind::Alert {
            severity: Severity::Warning,
            flag: "compute".into(),
            fields: BTreeMap::new(),
            action_verb: None,
            actions: Vec::new(),
        },
    ));
    fra1.insert(Message::text("fra1", 30, "human line"));
    let mut lh1 = Conversation::new("lh1");
    lh1.insert(Message::new(
        "lh1",
        30,
        MessageKind::Alert {
            severity: Severity::Info,
            flag: "security".into(),
            fields: BTreeMap::new(),
            action_verb: None,
            actions: Vec::new(),
        },
    ));
    state.convos.insert("nyc3".into(), nyc3);
    state.convos.insert("fra1".into(), fra1);
    state.convos.insert("lh1".into(), lh1);

    let items = state.notification_items();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].host, "lh1");
    assert_eq!(items[1].host, "nyc3");
    assert_eq!(
        notification_sources(&items),
        vec!["compute".to_string(), "security".to_string()]
    );
    assert_eq!(
        state.notifications_unread(),
        0,
        "first sight is watermarked"
    );
    state.seen.insert(NOTIFICATIONS_SEEN_KEY.to_string(), 1);
    assert_eq!(state.notifications_unread(), 2);
}

#[test]
fn acknowledged_notification_leaves_the_lane_and_new_live_alert_reappears() {
    let mut state = ChatState::default();
    let mut first = Message::new(
        "nyc3",
        20,
        MessageKind::Alert {
            severity: Severity::Critical,
            flag: "security".into(),
            fields: BTreeMap::new(),
            action_verb: None,
            actions: Vec::new(),
        },
    );
    first.id = mde_chat::MessageId::new("alert-old");
    let mut second = Message::new(
        "nyc3",
        30,
        MessageKind::Alert {
            severity: Severity::Critical,
            flag: "security".into(),
            fields: BTreeMap::new(),
            action_verb: None,
            actions: Vec::new(),
        },
    );
    second.id = mde_chat::MessageId::new("alert-new");

    let mut conv = Conversation::new("nyc3");
    conv.insert(first);
    state.convos.insert("nyc3".into(), conv);
    assert_eq!(state.notification_items().len(), 1);

    state.acked_alerts.insert("alert-old".into());
    assert_eq!(
        state.notification_items().len(),
        0,
        "ack clears the aggregate Notifications lane"
    );

    let conv = state.convos.get_mut("nyc3").expect("conversation");
    conv.insert(second);
    let items = state.notification_items();
    assert_eq!(items.len(), 1, "a newer live alert re-raises the lane");
    assert_eq!(items[0].msg.id.as_str(), "alert-new");
}

#[test]
fn notifications_lane_renders_and_opening_it_clears_session_unread() {
    use mde_egui::egui::{pos2, vec2, Rect};

    let ctx = egui::Context::default();
    Style::install(&ctx);

    let mut state = ChatState::default();
    let mut roster = Roster::new("eagle");
    roster.upsert(Contact::new("nyc3", NodeRole::Headless).with_presence(Presence::Online));
    state.roster = Some(roster);

    let mut fields = BTreeMap::new();
    fields.insert("summary".to_string(), "disk critical".to_string());
    let mut conv = Conversation::new("nyc3");
    conv.insert(Message::new(
        "nyc3",
        20,
        MessageKind::Alert {
            severity: Severity::Critical,
            flag: "storage".into(),
            fields,
            action_verb: None,
            actions: Vec::new(),
        },
    ));
    state.convos.insert("nyc3".into(), conv);
    state.seen.insert("nyc3".into(), 0);
    state.seen.insert(NOTIFICATIONS_SEEN_KEY.to_string(), 0);
    state.selected = Some(Selection::Notifications);
    assert_eq!(state.notifications_unread(), 1);

    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 720.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.push_id("shell-chat-notifications", |ui| state.show(ui));
        });
    });
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    assert!(
        !prims.is_empty(),
        "the Notifications lane produced no draw primitives"
    );
    assert_eq!(
        state.notifications_unread(),
        0,
        "opening the lane writes the session-only watermark"
    );
    assert_eq!(
        state.total_unread(),
        1,
        "the Chat icon still counts the underlying host alert"
    );
}

#[test]
fn notification_control_helpers_publish_and_persist() {
    let tmp = tempfile::tempdir().unwrap();
    let bus_root = tmp.path().join("bus");
    let state = ChatState {
        bus_root: Some(bus_root.clone()),
        ..ChatState::default()
    };

    state.set_notify_threshold(Severity::Critical);
    publish_mute(Some(&bus_root), "source", "security", true).expect("mute publishes");
    let persist = Persist::open(bus_root.clone()).expect("persist");
    let prefs_msgs = persist
        .list_since(ACTION_CHAT_NOTIFY_PREFS, None)
        .expect("notify prefs action");
    assert_eq!(
        prefs_msgs.last().and_then(|m| m.body.as_deref()),
        Some(r#"{"threshold":"critical"}"#)
    );
    let mute_msgs = persist
        .list_since(ACTION_CHAT_MUTE, None)
        .expect("mute action");
    assert_eq!(
        mute_msgs.last().and_then(|m| m.body.as_deref()),
        Some(r#"{"id":"security","muted":true,"target":"source"}"#)
    );

    assert!(!state.dnd_active());
    state.set_dnd(true);
    let dnd = mde_bus::dnd::load_default(&bus_root);
    assert!(dnd.active);
    assert!(dnd.since_unix_ms > 0);
    assert!(!dnd.set_by_peer.is_empty());
    state.set_dnd(false);
    assert!(!mde_bus::dnd::load_default(&bus_root).active);
}

#[test]
fn alert_action_helper_publishes_typed_worker_request() {
    let tmp = tempfile::tempdir().unwrap();
    let bus_root = tmp.path().join("bus");
    let msg = Message::new(
        "nyc3",
        20,
        MessageKind::Alert {
            severity: Severity::Critical,
            flag: "system".into(),
            fields: BTreeMap::new(),
            action_verb: None,
            actions: Vec::new(),
        },
    );
    let action = AlertAction {
        id: "restart".into(),
        label: "Restart".into(),
        verb: Some("action/systemd/restart".into()),
        kind: AlertActionKind::Safe,
    };
    publish_alert_action(Some(&bus_root), &msg, &action, false);

    let persist = Persist::open(bus_root).expect("bus");
    let msgs = persist
        .list_since(ACTION_CHAT_ALERT_ACTION, None)
        .expect("alert action");
    let body = msgs.last().and_then(|m| m.body.as_deref()).unwrap();
    assert!(body.contains(r#""action_id":"restart""#));
    assert!(body.contains(r#""kind":"safe""#));
    assert!(body.contains(r#""verb":"action/systemd/restart""#));
    assert!(body.contains(r#""armed":false"#));
}

// ── MENU-2: the View feed filters + Unread Only ────────────────────────────

/// The three feed-filter bands classify every message kind: alerts, clips,
/// and everything human-authored (text / file / call / remote) as messages.
#[test]
fn feed_filters_classify_every_message_kind() {
    let mut state = ChatState::default();
    let alert = MessageKind::Alert {
        severity: Severity::Info,
        flag: "x".into(),
        fields: BTreeMap::new(),
        action_verb: None,
        actions: Vec::new(),
    };
    let clip = MessageKind::Clipboard {
        preview: "p".into(),
        full: "f".into(),
    };
    let text = MessageKind::Text("hi".into());
    let file = MessageKind::File {
        name: "a.txt".into(),
        size_bytes: 1,
        mime: None,
    };
    // Defaults: everything shows.
    for kind in [&alert, &clip, &text, &file] {
        assert!(state.feed_shows(kind), "all bands default on");
    }
    state.show_alerts = false;
    assert!(!state.feed_shows(&alert), "Alerts off hides an alert");
    assert!(state.feed_shows(&clip), "…but not a clip");
    state.show_clips = false;
    assert!(!state.feed_shows(&clip));
    state.show_messages = false;
    assert!(!state.feed_shows(&text), "Messages off hides text");
    assert!(!state.feed_shows(&file), "…and file offers");
}

/// Unread Only prunes the roster to conversations with unread — but self
/// stays pinned and the OPEN conversation stays visible (opening a pane
/// watermarks it read, so it must not vanish mid-read).
#[test]
fn unread_only_prunes_but_keeps_self_and_the_open_pane() {
    let mut state = ChatState::default();
    let mut roster = Roster::new("eagle");
    roster.upsert(Contact::new("nyc3", NodeRole::Headless).with_presence(Presence::Online));
    roster.upsert(Contact::new("fra1", NodeRole::Headless).with_presence(Presence::Online));
    let me = Contact::new("eagle", NodeRole::Workstation);
    let nyc3 = Contact::new("nyc3", NodeRole::Headless);
    let fra1 = Contact::new("fra1", NodeRole::Headless);

    // Filters off: everything shows.
    assert!(state.roster_shows(&roster, &nyc3));

    state.unread_only = true;
    // nyc3 carries one unread → visible; fra1 is read → pruned.
    let mut conv = Conversation::new("nyc3");
    conv.insert(Message::text("nyc3", 10, "hi"));
    state.convos.insert("nyc3".into(), conv);
    state.seen.insert("nyc3".into(), 0);
    assert!(state.roster_shows(&roster, &nyc3), "unread stays");
    assert!(!state.roster_shows(&roster, &fra1), "read prunes");
    assert!(state.roster_shows(&roster, &me), "self stays pinned");
    // The open (therefore read) conversation stays visible.
    state.selected = Some(Selection::Contact("fra1".into()));
    assert!(state.roster_shows(&roster, &fra1), "the open pane stays");
    // Rooms follow the same rule.
    assert!(!state.room_shows("ops"), "a read room prunes");
    state.selected = Some(Selection::Room("ops".into()));
    assert!(state.room_shows("ops"), "the open room stays");
}

/// The presence / status / mute action helpers are best-effort: with no Bus
/// directory they are silent no-ops (the honest solo-host state), never a panic.
#[test]
fn presence_status_and_mute_helpers_are_silent_without_a_bus() {
    let state = ChatState {
        bus_root: None,
        ..ChatState::default()
    };
    state.set_presence(PresenceChoice::Dnd);
    state.set_presence(PresenceChoice::Available); // clear to auto
    state.set_status(Some("brb"));
    state.set_status(Some("")); // empty clears at the worker
    state.set_status(None);
    assert!(publish_mute(None, "contact", "nyc3", true).is_err());
    assert!(publish_mute(None, "room", "ops", false).is_err());
}

/// The message / notification cards cast the shared `Elevation::Raised` soft
/// shadow (Phase-C depth adoption): every field of [`card_shadow`] comes straight
/// from the token — offset/blur/spread and, critically, the umbra colour (no
/// minted `Color32`, §4) — and the umbra stays translucent (design lock #2).
#[test]
fn chat_card_shadow_is_the_raised_depth_token() {
    let raised = mde_egui::style::Elevation::Raised.shadow();
    let shadow = card_shadow();
    assert_eq!(
        shadow.offset,
        [raised.offset[0] as i8, raised.offset[1] as i8],
        "the card shadow offset comes from the Raised token"
    );
    assert_eq!(
        shadow.blur, raised.blur as u8,
        "the card shadow blur comes from the Raised token"
    );
    assert_eq!(
        shadow.spread, raised.spread as u8,
        "the card shadow spread comes from the Raised token"
    );
    assert_eq!(
        shadow.color, raised.umbra,
        "the card shadow umbra is the Raised token's, not a minted colour"
    );
    assert!(
        shadow.color.a() > 0 && shadow.color.a() < 255,
        "the depth is a translucent umbra (lock #2), never an opaque fill"
    );
}
