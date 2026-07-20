//! MENUBAR-ALL (Infra as Code) — the shared bar over the `OpenStack` control
//! plane. Every item is a real seam (§6): the **Catalog** spine (Refresh now /
//! Auto-refresh) + **View** (Show endpoint URLs), then the **catalog-driven
//! per-service menus**, then **Help**. IAC-5 makes the per-service menus **one
//! per advertised Keystone service**, so the bar grows + shrinks with the live
//! catalog (design #17): each carries that service's full landed verb set (Drill
//! / Refresh, Compute's armed lifecycle, the Orchestration menu's folded-in Heat
//! set), the governing-principle headline — comprehensive, yet every item maps to
//! a landed Bus seam; an absent verb is omitted, a context-gated one disabled, a
//! verb-less service an honest caption, never a dead entry (§8). The status
//! cluster reads the live catalog.

use std::collections::BTreeSet;

use super::{
    service_bucket, service_display_name, Arming, CatalogOutcome, HeatOp, IacTab, InfraCodeState,
    BUCKETS, CLOUD_PRODUCT_LABEL, DOT, HEAT_SERVICE,
};
use mackes_mesh_types::cloud::default_collection;
use mde_egui::egui::Ui;
use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
use mde_egui::{ChipTone, StatusChip, Style};

/// One menu action — each routes to a real Infra-as-Code seam in [`apply`].
/// The catalog-driven per-service verbs carry their target (service type /
/// instance id), so this is `Clone`, not `Copy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MenuAction {
    /// Force an immediate catalog re-poll (`Catalog → Refresh now`).
    Refresh,
    /// Toggle the ~15 s auto-poll (`Catalog → Auto-refresh`).
    ToggleAuto,
    /// Toggle full endpoint URLs on the tiles (`View → Show endpoint URLs`).
    ToggleUrls,
    /// Open the Resources tab focused on a service (`<Service> → Drill`).
    Drill(String),
    /// Force a re-poll of one service's resource pane (`<Service> → Refresh`).
    RefreshResources(String),
    /// A non-destructive Nova lifecycle op on the selected instance (Start /
    /// Stop) — issues the armed Bus request directly.
    Lifecycle {
        /// The lifecycle verb (`instance-start` / `instance-stop`).
        verb: &'static str,
        /// The target Nova instance id.
        instance_id: String,
        /// Its display name (for the honest action note).
        name: String,
    },
    /// A destructive Nova lifecycle op (Reboot / Delete) — opens the typed-
    /// arming confirm before anything publishes (#22).
    ArmLifecycle {
        /// The destructive verb (`instance-reboot` / `instance-delete`).
        verb: &'static str,
        /// The target Nova instance id.
        instance_id: String,
        /// Its display name — the typed-arming echo.
        name: String,
    },
    /// Heat: refresh the stack list (and re-fetch the open detail).
    HeatRefresh,
    /// Heat: (re)load the selected stack's detail.
    HeatShow,
    /// Heat: preview-update the selected stack with the edited template.
    HeatPreview,
    /// Heat: stack-check (drift) the selected stack.
    HeatCheck,
    /// Heat: reverse-generate a HOT template from live infra.
    HeatReverse,
    /// Heat: open the create-stack form.
    HeatNewStack,
    /// Heat: open the typed-arming confirm for a create / update / delete
    /// (#22).
    HeatArm(HeatOp),
    /// Help → surface the audit + notify posture in the action note (a real
    /// seam — the `note` renders; IAC-5's honest one-liner).
    HelpAbout,
}

/// Render the INFRA AS CODE bar and return the action picked this frame
/// (IAC-5).
///
/// The bar is the **Catalog / View** spine, then the **catalog-driven
/// per-service menus** (one menu per advertised Keystone service, growing +
/// shrinking with the live catalog, design #17), then the **Help** menu. The
/// governing principle (MENUBAR-ALL): every real control — incl. the armed
/// Nova lifecycle + the full Heat verb set (folded into the Orchestration
/// service's menu, lock 9 — no separate ad-hoc menu) — is here; a verb with no
/// landed seam is omitted and a context-gated one disabled, never a dead entry
/// (§8).
pub(super) fn show(ui: &mut Ui, state: &InfraCodeState) -> Option<MenuAction> {
    let mut menus = build_menus(state.auto_refresh, state.show_urls);
    menus.extend(build_service_menus(state));
    menus.push(build_help_menu());
    let status = build_status(state);
    let model = MenuBarModel {
        // The dock groups Infra as Code under **Workloads** (purple), so the
        // title wears that categorical accent (design lock #17 / §4).
        title: "Infra as Code",
        accent: Style::ACCENT_WORKLOADS,
        menus: &menus,
        status: &status,
    };
    MenuBar::show(ui, &model)
}

/// Build the catalog-driven per-service menus (design #17) — **one menu per
/// advertised Keystone service**, so the bar auto-grows + shrinks as the live
/// catalog changes (a newly-advertised service automatically gets its menu).
///
/// The known families (Compute / Network / Image / Volume / Orchestration /
/// Identity / DNS / Object Storage / Placement) render in canonical order,
/// collapsing their type variants (`volume`/`volumev3` → one **Volume** menu);
/// the `Other` catch-all fans out **one menu per distinct unknown type**, so a
/// brand-new service still surfaces its own menu. Each menu carries that
/// service's full landed verb set ([`service_menu`]) — the governing principle
/// (surface ALL controls), with honest omit/disable (§8). Empty until the
/// catalog is [`CatalogOutcome::Ready`].
fn build_service_menus(state: &InfraCodeState) -> Vec<Menu<MenuAction>> {
    let CatalogOutcome::Ready(view) = &state.outcome else {
        return Vec::new();
    };
    let selected = state.single_selected_instance();
    let has_stack = state.heat.selected.is_some();
    let mut menus = Vec::new();
    for bucket in BUCKETS {
        if bucket == "Other" {
            // Fan out one menu per distinct unknown service type (deduped) — the
            // auto-grow showcase: a service Keystone advertises that isn't a
            // known family still gets its own menu, titled by its identity.
            let mut seen: BTreeSet<&str> = BTreeSet::new();
            for svc in &view.catalog.services {
                if service_bucket(&svc.service_type) == "Other"
                    && seen.insert(svc.service_type.as_str())
                {
                    menus.push(service_menu(
                        &svc.service_type,
                        service_display_name(svc),
                        selected.as_ref(),
                        has_stack,
                    ));
                }
            }
        } else if let Some(svc) = view
            .catalog
            .services
            .iter()
            .find(|s| service_bucket(&s.service_type) == bucket)
        {
            // One menu per known family present in the catalog, titled by the
            // family, targeting the first advertised type in it (they share a
            // resource collection, so any is a valid drill target).
            menus.push(service_menu(
                &svc.service_type,
                bucket.to_string(),
                selected.as_ref(),
                has_stack,
            ));
        }
    }
    menus
}

/// Build one advertised service's menu (design #17) — its **full landed verb
/// set**, honestly gated (§8):
///
/// - the read seam **Drill into resources** + **Refresh resources** when the
///   service has a resource collection (a landed `list-resources` seam);
/// - for **Compute**, the armed Nova lifecycle verbs on the selected instance
///   (Start / Stop direct; Reboot / Delete typed-armed, #22), disabled until
///   exactly one instance is selected (context-gated, §7);
/// - for **Orchestration**, the folded-in full **Heat** verb set (Refresh /
///   Reverse-generate / New stack / Show / Preview / Stack-check / Update /
///   Delete — lock 9, no separate ad-hoc menu);
/// - and, for a service with **no** landed management verb (Identity / DNS /
///   Placement / Object Storage / an unknown type), a single honest
///   [`Entry::Caption`] — the service is advertised, but nothing is wired to
///   fake (§8, never a dead/greyed actionable entry).
fn service_menu(
    ty: &str,
    title: impl Into<String>,
    selected: Option<&(String, String)>,
    has_stack: bool,
) -> Menu<MenuAction> {
    let bucket = service_bucket(ty);
    let mut entries = Vec::new();
    // The read seam — omitted (§8) when the service has no resource collection.
    if default_collection(ty).is_some() {
        entries.push(Entry::Item(Item::new(
            MenuAction::Drill(ty.to_string()),
            "Drill into resources",
        )));
        // Orchestration's own Heat set carries "Refresh stacks", so we don't
        // duplicate a generic Refresh there (lock 9).
        if bucket != "Orchestration" {
            entries.push(Entry::Item(Item::new(
                MenuAction::RefreshResources(ty.to_string()),
                "Refresh resources",
            )));
        }
    }
    match bucket {
        "Compute" => {
            if !entries.is_empty() {
                entries.push(Entry::Separator);
            }
            push_lifecycle_verbs(&mut entries, selected);
        }
        "Orchestration" => {
            if !entries.is_empty() {
                entries.push(Entry::Separator);
            }
            entries.extend(heat_entries(has_stack));
        }
        _ => {}
    }
    if entries.is_empty() {
        // Honest §8 — advertised, but no management verb is wired for its type.
        entries.push(Entry::Caption(
            "No management verbs are wired for this service yet.".to_string(),
        ));
    }
    Menu::new(title, entries)
}

/// Push the armed Nova lifecycle verbs (Start / Stop direct; Reboot / Delete
/// typed-armed, #22) onto a Compute menu. They act on the single selected
/// instance, so they are disabled (context-gated, §7 — not omitted) when the
/// selection isn't exactly one.
fn push_lifecycle_verbs(entries: &mut Vec<Entry<MenuAction>>, selected: Option<&(String, String)>) {
    let (enabled, id, name) = selected.map_or_else(
        || (false, String::new(), String::new()),
        |(id, name)| (true, id.clone(), name.clone()),
    );
    for (verb, label) in [
        ("instance-start", "Start instance"),
        ("instance-stop", "Stop instance"),
    ] {
        entries.push(Entry::Item(
            Item::new(
                MenuAction::Lifecycle {
                    verb,
                    instance_id: id.clone(),
                    name: name.clone(),
                },
                label,
            )
            .enabled(enabled),
        ));
    }
    for (verb, label) in [
        ("instance-reboot", "Reboot instance\u{2026}"),
        ("instance-delete", "Delete instance\u{2026}"),
    ] {
        entries.push(Entry::Item(
            Item::new(
                MenuAction::ArmLifecycle {
                    verb,
                    instance_id: id.clone(),
                    name: name.clone(),
                },
                label,
            )
            .enabled(enabled),
        ));
    }
}

/// The full native-IaC **Heat** verb set (IAC-4), folded into the
/// Orchestration service's menu (lock 9): Refresh stacks / Reverse-generate /
/// New stack, plus the selection-gated Show / Preview / Stack-check / Update /
/// Delete. The mutating Update / Delete are typed-armed (#22); the reads + the
/// non-destructive stack-check act directly; Create rides the New-stack form
/// (armed there). `has_sel` disables the selection-gated verbs (§7 — not
/// omitted) until a stack is selected.
fn heat_entries(has_sel: bool) -> Vec<Entry<MenuAction>> {
    vec![
        Entry::Item(Item::new(MenuAction::HeatRefresh, "Refresh stacks")),
        Entry::Item(Item::new(
            MenuAction::HeatReverse,
            "Reverse-generate template",
        )),
        Entry::Item(Item::new(MenuAction::HeatNewStack, "New stack\u{2026}")),
        Entry::Separator,
        Entry::Item(Item::new(MenuAction::HeatShow, "Show / refresh detail").enabled(has_sel)),
        Entry::Item(
            Item::new(MenuAction::HeatPreview, "Preview update (dry-run)\u{2026}").enabled(has_sel),
        ),
        Entry::Item(Item::new(MenuAction::HeatCheck, "Stack-check (drift)").enabled(has_sel)),
        Entry::Item(
            Item::new(MenuAction::HeatArm(HeatOp::Update), "Update stack\u{2026}").enabled(has_sel),
        ),
        Entry::Item(
            Item::new(MenuAction::HeatArm(HeatOp::Delete), "Delete stack\u{2026}").enabled(has_sel),
        ),
    ]
}

/// The **Help** menu (the MENUBAR-ALL spine) — an honest surface identity
/// caption + a real seam: **Audit + notify posture** writes IAC-5's one-line
/// note (every mutating op audits; notify fires only on failure/service-down),
/// so even Help carries no dead entry (§8).
fn build_help_menu() -> Menu<MenuAction> {
    Menu::new(
        "Help",
        vec![
            Entry::Caption(format!(
                "Infra as Code \u{2014} the {CLOUD_PRODUCT_LABEL} control plane."
            )),
            Entry::Item(Item::new(
                MenuAction::HelpAbout,
                "Audit + notify posture\u{2026}",
            )),
        ],
    )
}

/// Build the Catalog + View menus, reflecting the two live toggles.
fn build_menus(auto_refresh: bool, show_urls: bool) -> Vec<Menu<MenuAction>> {
    vec![
        Menu::new(
            "Catalog",
            vec![
                Entry::Item(Item::new(MenuAction::Refresh, "Refresh now")),
                Entry::Separator,
                Entry::Item(
                    Item::new(MenuAction::ToggleAuto, "Auto-refresh (15\u{202F}s)")
                        .checked(auto_refresh),
                ),
            ],
        ),
        Menu::new(
            "View",
            vec![Entry::Item(
                Item::new(MenuAction::ToggleUrls, "Show endpoint URLs").checked(show_urls),
            )],
        ),
    ]
}

/// The live status cluster: N services · M healthy · the region — or the
/// honest not-configured / unreachable / querying read when there's no
/// catalog yet (§7).
fn build_status(state: &InfraCodeState) -> Vec<StatusChip> {
    match &state.outcome {
        CatalogOutcome::Ready(view) => {
            let total = view.catalog.services.len();
            let healthy = view.healthy_count();
            let mut chips = vec![StatusChip::new(
                format!("{total} service{}", if total == 1 { "" } else { "s" }),
                ChipTone::Neutral,
            )];
            if total > 0 {
                let tone = if healthy == total {
                    ChipTone::Ok
                } else {
                    ChipTone::Warn
                };
                chips.push(StatusChip::with_icon(
                    DOT,
                    format!("{healthy} healthy"),
                    tone,
                ));
            }
            if let Some(region) = &view.catalog.region {
                chips.push(StatusChip::new(region.clone(), ChipTone::Info));
            }
            chips
        }
        CatalogOutcome::Querying => {
            vec![StatusChip::new("querying\u{2026}", ChipTone::Neutral)]
        }
        CatalogOutcome::NotConfigured(_) => {
            vec![StatusChip::with_icon(DOT, "not configured", ChipTone::Warn)]
        }
        CatalogOutcome::Failed(_) => {
            vec![StatusChip::with_icon(DOT, "unreachable", ChipTone::Danger)]
        }
    }
}

/// Apply a picked action to its real seam (§6). Refresh queues one immediate
/// request (clearing any in-flight one so it fires on the next poll); the two
/// toggles flip the matching view/poll state.
pub(super) fn apply(state: &mut InfraCodeState, action: MenuAction) {
    match action {
        MenuAction::Refresh => {
            state.forced = true;
            state.pending = None;
        }
        MenuAction::ToggleAuto => state.auto_refresh = !state.auto_refresh,
        MenuAction::ToggleUrls => state.show_urls = !state.show_urls,
        MenuAction::Drill(ty) => {
            state.tab = IacTab::Resources;
            state.linked_focus = Some(ty);
        }
        MenuAction::RefreshResources(ty) => {
            let pane = state.resources.entry(ty).or_default();
            pane.forced = true;
            pane.pending = None;
        }
        // Start / Stop are non-destructive — issue the armed request directly.
        MenuAction::Lifecycle {
            verb,
            instance_id,
            name,
        } => state.issue_lifecycle(verb, &instance_id, &name),
        // Reboot / Delete open the typed-arming confirm before anything
        // publishes (#22) — nothing reaches the Bus until the name is typed.
        MenuAction::ArmLifecycle {
            verb,
            instance_id,
            name,
        } => {
            state.arming = Some(Arming {
                verb,
                instance_id,
                target_name: name,
                typed: String::new(),
            });
        }
        // ── Heat (IAC-4) — every item maps to a real seam (§6). ──
        MenuAction::HeatRefresh => {
            if let Some(pane) = state.resources.get_mut(HEAT_SERVICE) {
                pane.forced = true;
                pane.pending = None;
            }
            state.heat.show_for = None;
            state.tab = IacTab::Heat;
        }
        MenuAction::HeatShow => {
            state.heat.show_for = None;
            state.tab = IacTab::Heat;
        }
        MenuAction::HeatPreview => {
            state.tab = IacTab::Heat;
            state.send_heat_preview();
        }
        MenuAction::HeatCheck => {
            state.tab = IacTab::Heat;
            state.issue_heat_check();
        }
        MenuAction::HeatReverse => {
            state.tab = IacTab::Heat;
            state.send_heat_reverse();
        }
        MenuAction::HeatNewStack => {
            state.tab = IacTab::Heat;
            state.heat.show_create = true;
        }
        // The armed Heat mutations open the typed-arming confirm (#22).
        MenuAction::HeatArm(HeatOp::Update) => state.arm_heat_update(),
        MenuAction::HeatArm(HeatOp::Delete) => state.arm_heat_delete(),
        MenuAction::HeatArm(HeatOp::Create) => state.arm_heat_create(),
        // Help → the honest IAC-5 audit + notify posture, surfaced in the note.
        MenuAction::HelpAbout => {
            state.note = Some(
                "Every mutating op is audited to the KDC hash-chained log; the mesh notify \
                     feed fires only on a failure or a service going down."
                    .to_string(),
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::super::tests::{fixture_view, heat_view};
    use super::super::{CatalogOutcome, HeatOp, InfraCodeState, CLOUD_PRODUCT_LABEL, HEAT_SERVICE};
    use super::{
        apply, build_help_menu, build_menus, build_service_menus, build_status, MenuAction,
    };
    use mde_egui::menubar::{Entry, Item, Menu};
    use mde_egui::ChipTone;

    /// The service menu titled `title`, if the generator built one.
    fn menu<'a>(menus: &'a [Menu<MenuAction>], title: &str) -> Option<&'a Menu<MenuAction>> {
        menus.iter().find(|m| m.title == title)
    }

    /// The item ids of a menu, in order.
    fn menu_action_ids(menu: &super::Menu<MenuAction>) -> Vec<MenuAction> {
        menu.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(i) => Some(i.id.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn orchestration_menu_folds_in_the_full_heat_verb_set_and_is_absent_without_it() {
        // §8 / lock 9 — the Heat verb set lives in the Orchestration service
        // menu, not a separate ad-hoc menu. No orchestration service ⇒ no
        // Orchestration menu at all (omitted, not dead).
        let no_heat = InfraCodeState {
            outcome: CatalogOutcome::Ready(fixture_view()),
            ..InfraCodeState::default()
        };
        assert!(
            menu(&build_service_menus(&no_heat), "Orchestration").is_none(),
            "no orchestration service ⇒ no Orchestration menu"
        );

        // Orchestration cataloged ⇒ its menu carries Drill + the full Heat set.
        let mut state = InfraCodeState {
            outcome: CatalogOutcome::Ready(heat_view()),
            ..InfraCodeState::default()
        };
        let menus = build_service_menus(&state);
        let orch = menu(&menus, "Orchestration")
            .expect("Orchestration menu")
            .clone();
        let ids = menu_action_ids(&orch);
        for want in [
            MenuAction::Drill(HEAT_SERVICE.to_string()),
            MenuAction::HeatRefresh,
            MenuAction::HeatReverse,
            MenuAction::HeatNewStack,
            MenuAction::HeatShow,
            MenuAction::HeatPreview,
            MenuAction::HeatCheck,
            MenuAction::HeatArm(HeatOp::Update),
            MenuAction::HeatArm(HeatOp::Delete),
        ] {
            assert!(
                ids.contains(&want),
                "the Orchestration menu is missing {want:?}"
            );
        }

        // The selection-gated verbs are disabled (context-gated, §7) — not
        // omitted — until a stack is selected.
        let show = orch
            .entries
            .iter()
            .find_map(|e| match e {
                Entry::Item(i) if i.id == MenuAction::HeatShow => Some(i),
                _ => None,
            })
            .expect("Show item");
        assert!(!show.enabled, "Show is disabled with no selection");

        // With a selection they enable, and the armed verbs open the confirm.
        state.heat.selected = Some(("s-1".to_string(), "mesh-net".to_string()));
        let menus2 = build_service_menus(&state);
        let orch2 = menu(&menus2, "Orchestration").expect("Orchestration menu");
        let show2 = orch2
            .entries
            .iter()
            .find_map(|e| match e {
                Entry::Item(i) if i.id == MenuAction::HeatShow => Some(i),
                _ => None,
            })
            .expect("Show item");
        assert!(show2.enabled, "Show enables once a stack is selected");
        apply(&mut state, MenuAction::HeatArm(HeatOp::Delete));
        assert!(
            state.heat.arming.is_some(),
            "the Delete menu verb opens the typed-arming confirm"
        );
    }

    #[test]
    fn service_menus_are_catalog_driven_and_carry_the_verb_set() {
        // The fixture catalog = compute + identity + image; compute & image are
        // drillable, identity is not (no resource collection).
        let state = InfraCodeState {
            outcome: CatalogOutcome::Ready(fixture_view()),
            ..InfraCodeState::default()
        };
        let menus = build_service_menus(&state);
        let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
        // #17 — one menu per advertised service, incl. the verb-less Identity.
        assert!(
            titles.contains(&"Compute")
                && titles.contains(&"Image")
                && titles.contains(&"Identity"),
            "every advertised service gets a menu (auto-grow); got {titles:?}"
        );

        // A non-compute drillable service carries just the two read verbs.
        let image = menu(&menus, "Image").expect("Image menu");
        assert_eq!(image.entries.len(), 2, "Drill + Refresh only");

        // A verb-less service (identity) gets an honest caption, not a dead
        // actionable entry (§8) — no activatable items at all.
        let identity = menu(&menus, "Identity").expect("Identity menu");
        assert!(
            matches!(identity.entries.as_slice(), [Entry::Caption(_)]),
            "a verb-less service shows one honest caption, no items"
        );

        // Compute carries the read verbs + the four armed lifecycle verbs,
        // disabled while nothing is selected (context-gated, §7 — not omitted).
        let compute = menus
            .iter()
            .find(|m| m.title == "Compute")
            .expect("Compute menu");
        let items: Vec<&Item<MenuAction>> = compute
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(i) => Some(i),
                _ => None,
            })
            .collect();
        assert_eq!(items.len(), 6, "Drill + Refresh + Start/Stop/Reboot/Delete");
        let is_lifecycle = |a: &MenuAction| {
            matches!(
                a,
                MenuAction::Lifecycle { .. } | MenuAction::ArmLifecycle { .. }
            )
        };
        assert_eq!(items.iter().filter(|i| is_lifecycle(&i.id)).count(), 4);
        assert!(
            items
                .iter()
                .filter(|i| is_lifecycle(&i.id))
                .all(|i| !i.enabled),
            "the lifecycle verbs are disabled until exactly one instance is selected"
        );
        // Delete is a typed-armed verb (ArmLifecycle), present in the menu.
        assert!(items.iter().any(|i| i.id
            == MenuAction::ArmLifecycle {
                verb: "instance-delete",
                instance_id: String::new(),
                name: String::new(),
            }));
    }

    /// A `CatalogView` over a Keystone token, no health rows.
    fn view_from(token: &str) -> super::super::CatalogView {
        super::super::CatalogView {
            catalog: mackes_mesh_types::cloud::ServiceCatalog::from_keystone_token_json(token)
                .expect("fixture catalog"),
            health: Vec::new(),
        }
    }

    #[test]
    fn the_bar_auto_grows_one_menu_per_advertised_service() {
        // #17 — the bar carries exactly one menu per advertised service and
        // grows as the catalog advertises more.
        let two = view_from(
            r#"{"token":{"catalog":[
                    {"type":"compute","name":"nova","endpoints":[{"interface":"public","url":"http://n:8774/v2.1","region":"R"}]},
                    {"type":"network","name":"neutron","endpoints":[{"interface":"public","url":"http://n:9696","region":"R"}]}
                ]}}"#,
        );
        let pair = InfraCodeState {
            outcome: CatalogOutcome::Ready(two),
            ..InfraCodeState::default()
        };
        let pair_menus = build_service_menus(&pair);
        let titles: Vec<&str> = pair_menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, vec!["Compute", "Network"], "one menu per service");

        // Advertise a third service → the bar grows a menu for it automatically.
        let three = view_from(
            r#"{"token":{"catalog":[
                    {"type":"compute","name":"nova","endpoints":[{"interface":"public","url":"http://n:8774/v2.1","region":"R"}]},
                    {"type":"network","name":"neutron","endpoints":[{"interface":"public","url":"http://n:9696","region":"R"}]},
                    {"type":"image","name":"glance","endpoints":[{"interface":"public","url":"http://g:9292","region":"R"}]}
                ]}}"#,
        );
        let grown = InfraCodeState {
            outcome: CatalogOutcome::Ready(three),
            ..InfraCodeState::default()
        };
        let grown_menus = build_service_menus(&grown);
        assert_eq!(
            grown_menus.len(),
            3,
            "the bar grew a menu for the new service"
        );
        assert!(
            menu(&grown_menus, "Image").is_some(),
            "the newly-advertised Image service got its own menu"
        );

        // An unknown service type still fans out its own menu (the Other bucket
        // is per-type, not a single collapsed catch-all).
        let unknown = view_from(
            r#"{"token":{"catalog":[
                    {"type":"metering","name":"ceilometer","endpoints":[{"interface":"public","url":"http://m:8777","region":"R"}]},
                    {"type":"sharev2","name":"manila","endpoints":[{"interface":"public","url":"http://s:8786","region":"R"}]}
                ]}}"#,
        );
        let exotic = InfraCodeState {
            outcome: CatalogOutcome::Ready(unknown),
            ..InfraCodeState::default()
        };
        assert_eq!(
            build_service_menus(&exotic).len(),
            2,
            "two unknown types ⇒ two distinct menus"
        );
    }

    #[test]
    fn help_menu_carries_a_real_seam_not_a_dead_entry() {
        // The Help spine (§8) — a caption + one real action; no dead entry.
        let help = build_help_menu();
        assert_eq!(help.title, "Help");
        assert_eq!(menu_action_ids(&help), vec![MenuAction::HelpAbout]);
        let caption = help
            .entries
            .iter()
            .find_map(|entry| match entry {
                Entry::Caption(text) => Some(text.as_str()),
                _ => None,
            })
            .expect("help caption");
        assert!(caption.contains(CLOUD_PRODUCT_LABEL), "{caption}");
        assert!(
            !caption.contains("OpenStack"),
            "the Help caption is user-facing and must stay provider-neutral"
        );
        // The action drives a real seam — the audit/notify posture note.
        let mut state = InfraCodeState::default();
        assert!(state.note.is_none());
        apply(&mut state, MenuAction::HelpAbout);
        assert!(
            state.note.as_deref().is_some_and(|n| n.contains("audited")),
            "Help surfaces the audit + notify posture (a real seam)"
        );
    }

    #[test]
    fn the_two_toggles_track_state() {
        // The Auto-refresh + Show-URLs items are checkable and mirror state.
        let checked = |auto: bool, urls: bool| {
            let menus = build_menus(auto, urls);
            let auto_item = match &menus[0].entries[2] {
                Entry::Item(i) => i.checked,
                _ => panic!("Catalog[2] is the auto-refresh toggle"),
            };
            let url_item = match &menus[1].entries[0] {
                Entry::Item(i) => i.checked,
                _ => panic!("View[0] is the show-URLs toggle"),
            };
            (auto_item, url_item)
        };
        assert_eq!(checked(true, false), (Some(true), Some(false)));
        assert_eq!(checked(false, true), (Some(false), Some(true)));
    }

    #[test]
    fn apply_flips_the_real_seams() {
        let mut state = InfraCodeState::default();
        assert!(state.auto_refresh && !state.show_urls);
        apply(&mut state, MenuAction::ToggleAuto);
        apply(&mut state, MenuAction::ToggleUrls);
        assert!(!state.auto_refresh && state.show_urls);
        // Refresh queues an immediate request + drops any in-flight one.
        apply(&mut state, MenuAction::Refresh);
        assert!(state.forced, "Refresh queues a re-poll");
        assert!(state.pending.is_none());
    }

    #[test]
    fn status_counts_services_and_healthy_from_the_live_catalog() {
        let state = InfraCodeState {
            outcome: CatalogOutcome::Ready(fixture_view()),
            ..InfraCodeState::default()
        };
        let chips = build_status(&state);
        // The fixture catalogs three services; compute + identity probe up.
        assert!(chips.iter().any(|c| c.text == "3 services"));
        assert!(chips
            .iter()
            .any(|c| c.text == "2 healthy" && c.tone == ChipTone::Warn));
        assert!(chips.iter().any(|c| c.text == "RegionOne"));
    }

    #[test]
    fn status_reads_honestly_when_not_configured_or_unreachable() {
        let not_configured = InfraCodeState {
            outcome: CatalogOutcome::NotConfigured("no clouds.yaml on node-a".to_string()),
            ..InfraCodeState::default()
        };
        let chips = build_status(&not_configured);
        assert!(chips
            .iter()
            .any(|c| c.text == "not configured" && c.tone == ChipTone::Warn));

        let failed = InfraCodeState {
            outcome: CatalogOutcome::Failed("keystone auth failed".to_string()),
            ..InfraCodeState::default()
        };
        assert!(build_status(&failed)
            .iter()
            .any(|c| c.text == "unreachable" && c.tone == ChipTone::Danger));
    }
}
