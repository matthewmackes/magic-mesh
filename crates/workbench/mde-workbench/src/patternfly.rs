//! Page chrome helpers — breadcrumb + page_title + page_subtitle.
//!
//! Ported from `mackes/workbench/_common.py`. The v1.x helpers
//! returned GTK widgets; the v2.0.0 surface returns plain typed
//! data so the Iced view layer can render however it wants
//! (PatternFly v6 tokens via `cosmic-theme`).
//!
//! CB-1.2 lock: "Breadcrumb + page_title + page_subtitle helpers
//! ported from `_common.py` to a new `crates/mde-workbench/src/
//! carbon.rs` (renamed to `patternfly.rs` once 0.7 CSS namespace
//! rename lands)." Per the v2.0.0 PatternFly lock (memory:
//! `project_v2_0_patternfly.md`), the file ships as
//! `patternfly.rs` from day one — `carbon` was the v1.x token
//! family.

use crate::model::{nav_model, Group, View};

/// Breadcrumb crumb. `slug` is the deep-link target (empty
/// string for the root "Workbench" crumb); `label` is what the
/// user sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    pub slug: String,
    pub label: String,
}

impl Crumb {
    fn new(slug: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            slug: slug.into(),
            label: label.into(),
        }
    }
}

/// Build the breadcrumb chain for a [`View`]. Always starts with
/// the "Workbench" root crumb; ends with the active group (and
/// optional panel) so the rightmost crumb mirrors the page title.
#[must_use]
pub fn breadcrumb(view: View) -> Vec<Crumb> {
    let mut out = vec![Crumb::new("", "Workbench")];
    let group = view.group();
    out.push(Crumb::new(group.slug(), group.label()));
    if let Some(panel_slug) = view.panel_slug() {
        if let Some(label) = panel_label(group, panel_slug) {
            out.push(Crumb::new(
                format!("{}.{}", group.slug(), panel_slug),
                label,
            ));
        }
    }
    out
}

/// User-visible label for a `(group, panel_slug)` pair, or
/// `None` if the panel isn't in the locked nav model.
#[must_use]
pub fn panel_label(group: Group, panel_slug: &str) -> Option<&'static str> {
    nav_model()
        .into_iter()
        .find(|e| e.group == group)?
        .panels
        .iter()
        .find(|p| p.slug() == panel_slug)
        .map(|p| p.label())
}

/// Big H1 above the right pane. For a group view, that's the
/// group label ("Network"); for a panel view, the panel label
/// ("Mesh SSH").
#[must_use]
pub fn page_title(view: View) -> String {
    match view {
        View::Group(g) => g.label().to_string(),
        View::Panel { group, panel } => {
            panel_label(group, panel).map_or_else(|| panel.to_string(), str::to_string)
        }
    }
}

/// Subtitle one line below [`page_title`]. For group views,
/// surfaces the count of contained panels ("12 panels"); for
/// panel views, surfaces the parent group ("in Network").
#[must_use]
pub fn page_subtitle(view: View) -> String {
    match view {
        View::Group(g) => {
            let count = nav_model()
                .into_iter()
                .find(|e| e.group == g)
                .map_or(0, |e| e.panels.len());
            match count {
                0 => "no panels".to_string(),
                1 => "1 panel".to_string(),
                n => format!("{n} panels"),
            }
        }
        View::Panel { group, .. } => format!("in {}", group.label()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breadcrumb_for_group_has_two_crumbs() {
        let crumbs = breadcrumb(View::Group(Group::Dashboard));
        assert_eq!(crumbs.len(), 2);
        assert_eq!(crumbs[0].label, "Workbench");
        assert_eq!(crumbs[1].label, "Overview");
        assert_eq!(crumbs[1].slug, "dashboard");
    }

    #[test]
    fn breadcrumb_for_panel_has_three_crumbs_with_full_slug() {
        let crumbs = breadcrumb(View::Panel {
            group: Group::ThisNode,
            panel: "remote_desktop",
        });
        assert_eq!(crumbs.len(), 3);
        assert_eq!(crumbs[0].slug, "");
        assert_eq!(crumbs[1].slug, "node");
        assert_eq!(crumbs[2].slug, "node.remote_desktop");
        assert_eq!(crumbs[2].label, "Remote Access");
    }

    #[test]
    fn breadcrumb_for_unknown_panel_collapses_to_group_chain() {
        let crumbs = breadcrumb(View::Panel {
            group: Group::System,
            panel: "ghost",
        });
        assert_eq!(crumbs.len(), 2, "unknown panel drops the leaf crumb");
        assert_eq!(crumbs[1].slug, "system");
    }

    #[test]
    fn page_title_uses_group_label_for_group_view() {
        assert_eq!(page_title(View::Group(Group::Mesh)), "Mesh");
    }

    #[test]
    fn page_title_uses_panel_label_for_panel_view() {
        // DATACENTER-25 — snapshots folded into Datacenter; `repair` is a
        // still-standalone System panel with a curated label.
        assert_eq!(
            page_title(View::Panel {
                group: Group::System,
                panel: "repair"
            }),
            "Repair"
        );
    }

    #[test]
    fn page_title_falls_back_to_slug_for_unknown_panel() {
        assert_eq!(
            page_title(View::Panel {
                group: Group::System,
                panel: "ghost"
            }),
            "ghost"
        );
    }

    #[test]
    fn page_subtitle_counts_panels_for_group_view() {
        // NAV-1.2 — This Node carries 9 panels: hardware, mesh_services,
        // interfaces, wifi, vpn, firewall, remote_desktop, plus the two
        // mesh-specific panels relocated from the retired Desktop group
        // (wallpaper, notifications).
        // See `crates/mde-workbench/src/model.rs` for the lock.
        assert_eq!(page_subtitle(View::Group(Group::ThisNode)), "9 panels");
    }

    #[test]
    fn page_subtitle_singular_for_dashboard() {
        // Dashboard ships exactly one panel ("home").
        assert_eq!(page_subtitle(View::Group(Group::Dashboard)), "1 panel");
    }

    #[test]
    fn page_subtitle_names_parent_group_for_panel_view() {
        assert_eq!(
            page_subtitle(View::Panel {
                group: Group::Mesh,
                panel: "peers"
            }),
            "in Mesh"
        );
    }
}
