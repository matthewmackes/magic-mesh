//! Shell-owned unified search/omnibox front door.
//!
//! This is the runtime UI slice of the `SEARCH-omnibox` epic. It stays deliberately
//! thin: ranking is the shared `mde_egui::search_omnibox` core, and activation is
//! handed back to the owning shell surface so apps, Files, Explorer, and Browser
//! keep their existing local command paths.

use mde_egui::egui;
use mde_egui::search_omnibox::{ranked_hits, SearchDomain, SearchHit, SearchItem};
use mde_egui::Style;
use mde_files_egui::model::FileSearchTarget;

use crate::dock::Surface;

const AREA_ID: &str = "shell-front-door-omnibox";
const INPUT_ID: &str = "shell-front-door-omnibox-input";
const MAX_HITS: usize = 12;
const PANEL_W: f32 = 720.0;
const ROW_H: f32 = 42.0;
const INPUT_H: f32 = 38.0;
const PANEL_RADIUS: f32 = 8.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrontDoorTarget {
    App(Surface),
    File(FileSearchTarget),
    Mesh(String),
    Browser(String),
}

#[derive(Debug, Default)]
pub(crate) struct FrontDoorState {
    open: bool,
    query: String,
    selected: usize,
    focus_pending: bool,
}

impl FrontDoorState {
    #[cfg(test)]
    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn open(&mut self) {
        self.open = true;
        self.focus_pending = true;
        self.selected = 0;
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
        self.focus_pending = false;
    }

    pub(crate) fn query(&self) -> &str {
        &self.query
    }
}

pub(crate) fn app_search_items() -> Vec<SearchItem<FrontDoorTarget>> {
    Surface::ALL
        .iter()
        .copied()
        .enumerate()
        .map(|(idx, surface)| {
            SearchItem::new(
                SearchDomain::App,
                surface.label(),
                format!("surface:{surface:?}"),
                FrontDoorTarget::App(surface),
            )
            .with_terms([format!("{surface:?}")])
            .with_source_rank(idx)
        })
        .collect()
}

pub(crate) fn ranked_front_door_hits(
    query: &str,
    items: Vec<SearchItem<FrontDoorTarget>>,
) -> Vec<SearchHit<FrontDoorTarget>> {
    ranked_hits(query, items, MAX_HITS)
}

pub(crate) fn front_door_panel(
    ctx: &egui::Context,
    state: &mut FrontDoorState,
    items: Vec<SearchItem<FrontDoorTarget>>,
) -> Option<FrontDoorTarget> {
    if !state.open {
        return None;
    }

    let screen = ctx.screen_rect();
    let width = PANEL_W.min(screen.width() - Style::SP_L * 2.0).max(320.0);
    let top = screen.top() + (screen.height() * 0.14).max(Style::SP_XL);
    let pos = egui::pos2(screen.center().x - width / 2.0, top);
    let mut action = None;

    let area = egui::Area::new(egui::Id::new(AREA_ID))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .constrain(false)
        .show(ctx, |ui| {
            ui.set_width(width);
            let frame = egui::Frame::new()
                .fill(Style::SURFACE)
                .stroke(egui::Stroke::new(1.0, Style::BORDER))
                .corner_radius(PANEL_RADIUS)
                .inner_margin(egui::Margin::same(10));
            frame.show(ui, |ui| {
                let input_id = egui::Id::new(INPUT_ID);
                let response = ui.add_sized(
                    [ui.available_width(), INPUT_H],
                    egui::TextEdit::singleline(&mut state.query)
                        .id(input_id)
                        .hint_text("Search apps, files, mesh, Browser")
                        .desired_width(f32::INFINITY),
                );
                if state.focus_pending {
                    response.request_focus();
                    state.focus_pending = false;
                }
                if response.changed() {
                    state.selected = 0;
                }

                let hits = ranked_front_door_hits(&state.query, items);
                if !hits.is_empty() {
                    state.selected = state.selected.min(hits.len().saturating_sub(1));
                } else {
                    state.selected = 0;
                }

                let (escape, enter, up, down) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::Escape),
                        i.key_pressed(egui::Key::Enter),
                        i.key_pressed(egui::Key::ArrowUp),
                        i.key_pressed(egui::Key::ArrowDown),
                    )
                });
                if escape {
                    state.close();
                    return;
                }
                if !hits.is_empty() {
                    if down {
                        state.selected = (state.selected + 1) % hits.len();
                    }
                    if up {
                        state.selected = if state.selected == 0 {
                            hits.len() - 1
                        } else {
                            state.selected - 1
                        };
                    }
                    if enter {
                        action = hits.get(state.selected).map(|hit| hit.item.payload.clone());
                    }
                }

                ui.add_space(Style::SP_XS);
                if hits.is_empty() {
                    empty_note(ui, state.query.trim().is_empty());
                } else {
                    for (idx, hit) in hits.iter().enumerate() {
                        if option_row(ui, hit, idx == state.selected).clicked() {
                            action = Some(hit.item.payload.clone());
                        }
                    }
                }
            });
        });

    if area.response.clicked_elsewhere() {
        state.close();
    }
    if action.is_some() {
        state.close();
    }
    action
}

fn empty_note(ui: &mut egui::Ui, blank: bool) {
    let text = if blank {
        "Type to search"
    } else {
        "No local matches"
    };
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::hover(),
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(13.0),
        Style::TEXT_DIM,
    );
}

fn option_row(
    ui: &mut egui::Ui,
    hit: &SearchHit<FrontDoorTarget>,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::click(),
    );
    let fill = if selected || response.hovered() {
        Style::ACCENT.linear_multiply(0.16)
    } else {
        Style::SURFACE_HI
    };
    let painter = ui.painter();
    painter.rect_filled(rect.shrink2(egui::vec2(0.0, 2.0)), 5.0, fill);

    let domain_rect = egui::Rect::from_min_size(
        rect.left_center() + egui::vec2(Style::SP_S, -10.0),
        egui::vec2(82.0, 20.0),
    );
    painter.rect_stroke(
        domain_rect,
        4.0,
        egui::Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    painter.text(
        domain_rect.center(),
        egui::Align2::CENTER_CENTER,
        domain_label(hit.item.domain),
        egui::FontId::proportional(11.0),
        Style::TEXT_DIM,
    );

    let title_pos = egui::pos2(domain_rect.right() + Style::SP_S, rect.center().y - 8.0);
    painter.text(
        title_pos,
        egui::Align2::LEFT_CENTER,
        &hit.item.title,
        egui::FontId::proportional(14.0),
        Style::TEXT,
    );
    let target_pos = egui::pos2(domain_rect.right() + Style::SP_S, rect.center().y + 10.0);
    painter.text(
        target_pos,
        egui::Align2::LEFT_CENTER,
        &hit.item.target,
        egui::FontId::proportional(11.0),
        Style::TEXT_DIM,
    );
    response
}

pub(crate) const fn domain_label(domain: SearchDomain) -> &'static str {
    match domain {
        SearchDomain::App => "App",
        SearchDomain::File => "File",
        SearchDomain::Mesh => "Mesh",
        SearchDomain::BrowserBookmark => "Bookmark",
        SearchDomain::BrowserHistory => "History",
        SearchDomain::WebSuggestion => "Web",
        SearchDomain::Assistant => "Assistant",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::Surface;

    #[test]
    fn app_search_items_cover_the_shell_surface_inventory() {
        let items = app_search_items();
        assert_eq!(items.len(), Surface::ALL.len());
        assert!(items.iter().any(|item| {
            item.domain == SearchDomain::App
                && item.title == Surface::Browser.label()
                && item.payload == FrontDoorTarget::App(Surface::Browser)
        }));
    }

    #[test]
    fn front_door_ranking_accepts_apps_files_mesh_browser_and_web() {
        let file_target = FileSearchTarget {
            pane: 0,
            row: 2,
            path: None,
        };
        let items = vec![
            SearchItem::new(
                SearchDomain::App,
                "Browser",
                "surface:Browser",
                FrontDoorTarget::App(Surface::Browser),
            ),
            SearchItem::new(
                SearchDomain::File,
                "browser-notes.md",
                "/home/mde/browser-notes.md",
                FrontDoorTarget::File(file_target),
            ),
            SearchItem::new(
                SearchDomain::Mesh,
                "browser-node",
                "peer:browser-node",
                FrontDoorTarget::Mesh("peer:browser-node".to_owned()),
            ),
            SearchItem::new(
                SearchDomain::BrowserHistory,
                "Browser status",
                "https://status.mesh/browser",
                FrontDoorTarget::Browser("https://status.mesh/browser".to_owned()),
            ),
            SearchItem::new(
                SearchDomain::WebSuggestion,
                "Search web for browser",
                "https://search.mesh/search?q=browser",
                FrontDoorTarget::Browser("browser".to_owned()),
            ),
        ];

        let hits = ranked_front_door_hits("browser", items);
        let domains: Vec<SearchDomain> = hits.iter().map(|hit| hit.item.domain).collect();
        assert!(domains.contains(&SearchDomain::App));
        assert!(domains.contains(&SearchDomain::File));
        assert!(domains.contains(&SearchDomain::Mesh));
        assert!(domains.contains(&SearchDomain::BrowserHistory));
        assert!(domains.contains(&SearchDomain::WebSuggestion));
    }

    #[test]
    fn front_door_domain_labels_cover_every_shared_domain() {
        for domain in [
            SearchDomain::App,
            SearchDomain::File,
            SearchDomain::Mesh,
            SearchDomain::BrowserBookmark,
            SearchDomain::BrowserHistory,
            SearchDomain::WebSuggestion,
            SearchDomain::Assistant,
        ] {
            assert!(!domain_label(domain).is_empty());
        }
    }
}
