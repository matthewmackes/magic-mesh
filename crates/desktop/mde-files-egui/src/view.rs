//! The egui rendering of the Files surface (E12-11).
//!
//! Every widget reads the render-agnostic [`FileBrowser`] and draws through the
//! shared [`Style`] — no raw colours or spacing (governance §4). The view never
//! mutates the model mid-render: a frame collects the user's intents as
//! [`Action`]s while it holds a shared `&FileBrowser`, then applies them once the
//! borrow is released. That keeps the borrow checker happy and the data flow
//! one-directional (render → intents → apply).

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;

use mde_files::model::{FileRow, Mime, PeerStatus};

use crate::model::{FileBrowser, Pane, SendOutcome, LOCAL_SPOTS};

/// A user intent captured during a render, applied to the model after the frame
/// has released its shared borrow.
enum Action {
    /// Browse a local directory (backend path).
    OpenLocal(String),
    /// Browse a mesh peer's folder (peer id).
    OpenPeer(String),
    /// Select the listing row at this index.
    Select(usize),
    /// Choose this peer id as the Send-To destination.
    SetDestination(String),
    /// Re-probe the mesh + reload the listing.
    Refresh,
    /// Fire the Send-To for the current selection.
    Send,
}

/// Render the whole Files surface for one frame.
pub fn show(ctx: &egui::Context, browser: &mut FileBrowser) {
    let mut actions: Vec<Action> = Vec::new();
    top_bar(ctx, browser, &mut actions);
    sidebar(ctx, browser, &mut actions);
    listing(ctx, browser, &mut actions);
    for action in actions {
        apply(browser, action);
    }
}

/// Apply a captured intent to the model.
fn apply(browser: &mut FileBrowser, action: Action) {
    match action {
        Action::OpenLocal(path) => browser.open_local(path),
        Action::OpenPeer(id) => browser.open_peer(id),
        Action::Select(idx) => browser.select(idx),
        Action::SetDestination(id) => browser.set_destination(id),
        Action::Refresh => {
            browser.refresh_roster();
            browser.reload();
        }
        Action::Send => {
            browser.send();
        }
    }
}

// ── Top bar ─────────────────────────────────────────────────────────────────

fn top_bar(ctx: &egui::Context, b: &FileBrowser, actions: &mut Vec<Action>) {
    egui::TopBottomPanel::top("files-top").show(ctx, |ui| {
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            ui.heading(
                RichText::new("Files")
                    .color(Style::TEXT)
                    .size(Style::HEADING),
            );
            ui.add_space(Style::SP_M);
            ui.colored_label(Style::TEXT_DIM, pane_title(b.pane()));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Primary action: Send the selected local file to the chosen peer.
                let can = b.can_send();
                let label = match b.destination() {
                    Some(peer) => format!("Send to {peer}"),
                    None => "Send to…".to_string(),
                };
                let button = egui::Button::new(RichText::new(label).color(Style::BG).strong())
                    .fill(Style::ACCENT);
                if ui.add_enabled(can, button).clicked() {
                    actions.push(Action::Send);
                }
                ui.add_space(Style::SP_S);
                if ui.button("Refresh").clicked() {
                    actions.push(Action::Refresh);
                }
            });
        });
        ui.add_space(Style::SP_XS);
        status_line(ui, b);
        ui.add_space(Style::SP_XS);
    });
}

fn status_line(ui: &mut egui::Ui, b: &FileBrowser) {
    match b.last_send() {
        SendOutcome::Idle => {
            ui.colored_label(
                Style::TEXT_DIM,
                "Pick a local file, choose a reachable peer, then Send.",
            );
        }
        SendOutcome::Sent { op_id, file, peer } => {
            ui.colored_label(Style::OK, format!("Sent {file} → {peer}  (op #{op_id})"));
        }
        SendOutcome::Failed(err) => {
            ui.colored_label(Style::DANGER, format!("Send failed: {err}"));
        }
    }
}

fn pane_title(pane: &Pane) -> String {
    match pane {
        Pane::Local(path) => {
            let shown = path.strip_prefix("local:").unwrap_or(path);
            format!("Local · {shown}")
        }
        Pane::Peer(id) => format!("Peer · {id}"),
    }
}

// ── Sidebar ─────────────────────────────────────────────────────────────────

fn sidebar(ctx: &egui::Context, b: &FileBrowser, actions: &mut Vec<Action>) {
    egui::SidePanel::left("files-side")
        .default_width(Style::SP_XL * 7.0)
        .show(ctx, |ui| {
            ui.add_space(Style::SP_S);
            let host = if b.self_node().host.is_empty() {
                "this node"
            } else {
                b.self_node().host.as_str()
            };
            ui.label(RichText::new(host).color(Style::TEXT).strong());
            ui.colored_label(Style::TEXT_DIM, node_role(b));
            mesh_badge(ui, b);
            ui.add_space(Style::SP_M);

            section_header(ui, "LOCAL");
            for spot in LOCAL_SPOTS {
                let active = matches!(b.pane(), Pane::Local(p) if p.as_str() == spot.path);
                if ui.selectable_label(active, spot.label).clicked() {
                    actions.push(Action::OpenLocal(spot.path.to_string()));
                }
            }
            ui.add_space(Style::SP_M);

            section_header(ui, "MESH PEERS");
            if b.peers().is_empty() {
                ui.colored_label(Style::TEXT_DIM, "No peers connected.");
            } else {
                ui.colored_label(
                    Style::TEXT_DIM,
                    format!(
                        "{} of {} reachable",
                        b.reachable_destinations().len(),
                        b.peers().len()
                    ),
                );
                for peer in b.peers() {
                    peer_row(ui, b, peer, actions);
                }
            }
        });
}

fn peer_row(
    ui: &mut egui::Ui,
    b: &FileBrowser,
    peer: &mde_files::model::Peer,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        status_dot(ui, peer_color(peer.status));
        let browsing = matches!(b.pane(), Pane::Peer(id) if id.as_str() == peer.id.as_str());
        if ui
            .selectable_label(browsing, peer.host.as_str())
            .on_hover_text("Browse this peer's shared folder")
            .clicked()
        {
            actions.push(Action::OpenPeer(peer.id.clone()));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if peer.status.is_reachable() {
                let is_dest = b.destination() == Some(peer.id.as_str());
                if ui
                    .selectable_label(is_dest, "dest")
                    .on_hover_text("Set as the Send-To destination")
                    .clicked()
                {
                    actions.push(Action::SetDestination(peer.id.clone()));
                }
            } else {
                ui.colored_label(Style::TEXT_DIM, "offline");
            }
        });
    });
}

// ── Central listing ─────────────────────────────────────────────────────────

fn listing(ctx: &egui::Context, b: &FileBrowser, actions: &mut Vec<Action>) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.add_space(Style::SP_S);
        ui.colored_label(Style::TEXT_DIM, format!("{} items", b.rows().len()));
        ui.add_space(Style::SP_XS);
        ui.separator();
        ui.add_space(Style::SP_XS);

        if b.rows().is_empty() {
            empty_state(ui, b);
            return;
        }

        // Snapshot the row display data while we hold the shared borrow, then
        // render; click intents are pushed as Actions (applied post-frame).
        let rows: Vec<(usize, String, bool)> = b
            .rows()
            .iter()
            .enumerate()
            .map(|(i, row)| (i, format_row(row), b.selected() == Some(i)))
            .collect();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (idx, label, selected) in rows {
                    if ui
                        .add(egui::SelectableLabel::new(
                            selected,
                            RichText::new(label).monospace().color(Style::TEXT),
                        ))
                        .clicked()
                    {
                        actions.push(Action::Select(idx));
                    }
                }
            });
    });
}

fn empty_state(ui: &mut egui::Ui, b: &FileBrowser) {
    ui.add_space(Style::SP_XL);
    ui.vertical_centered(|ui| {
        let (title, body) = match b.pane() {
            Pane::Local(_) => (
                "Nothing here",
                "This directory is empty, or it doesn't exist on this node.",
            ),
            // Distinguish a genuinely-empty share from "no mesh at all" using the
            // same live overlay the sidebar badge reads — an honest error state
            // instead of a hedged "sharing nothing, or the Bus is unavailable".
            Pane::Peer(_) if b.mesh_overlay().is_none() => (
                "No mesh connection",
                "This node isn't on a live mesh, so no peer files can be listed.",
            ),
            Pane::Peer(_) => ("No shared files", "This peer is sharing nothing right now."),
        };
        ui.label(
            RichText::new(title)
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        ui.colored_label(Style::TEXT_DIM, body);
    });
}

// ── Small render helpers ────────────────────────────────────────────────────

/// The node's sub-label under its hostname: its live mesh role (Lighthouse /
/// Workstation) when the Nebula overlay is up, else the neutral "this node".
fn node_role(b: &FileBrowser) -> &'static str {
    match b.mesh_overlay() {
        Some(m) if m.is_lighthouse => "this node · Lighthouse",
        Some(_) => "this node · Workstation",
        None => "this node",
    }
}

/// The live mesh badge under the node header: a status dot plus the mesh id and
/// active transport when this node is on a Nebula overlay, or an honest
/// "standalone" line when it isn't (the demo/local backends, or a node whose mesh
/// daemon isn't reachable). Reads the same cached overlay the model refreshes with
/// the roster — never a fabricated value.
fn mesh_badge(ui: &mut egui::Ui, b: &FileBrowser) {
    ui.horizontal(|ui| {
        if let Some(mesh) = b.mesh_overlay() {
            status_dot(ui, Style::OK);
            let mut label = if mesh.mesh_id.is_empty() {
                "on the mesh".to_string()
            } else {
                format!("mesh {}", mesh.mesh_id)
            };
            if !mesh.active_transport.is_empty() {
                label.push_str(" · via ");
                label.push_str(&mesh.active_transport);
            }
            ui.label(
                RichText::new(label)
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        } else {
            status_dot(ui, Style::WARN);
            ui.label(
                RichText::new("Standalone — no mesh")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        }
    });
}

fn section_header(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
}

/// A small filled circle used as a peer reachability indicator.
fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let diameter = Style::SP_S;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(diameter, diameter), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), diameter * 0.28, color);
}

/// One monospace-aligned listing line: a type tag, the name, the size + age, and
/// (for mesh rows) the peer the file came from. Fira Code is monospace in this
/// harness, so the padded columns line up.
fn format_row(row: &FileRow) -> String {
    let tag = mime_tag(row.mime);
    let origin = row
        .origin()
        .map(|o| format!("  from {o}"))
        .unwrap_or_default();
    format!(
        "{tag:<4} {name:<34.34} {size:>9}  {age:>5}{origin}",
        name = row.name,
        size = row.size,
        age = row.age,
    )
}

/// A short, fixed-width type tag for a row's MIME class.
const fn mime_tag(mime: Mime) -> &'static str {
    match mime {
        Mime::Folder => "DIR",
        Mime::Doc => "DOC",
        Mime::Image => "IMG",
        Mime::Pdf => "PDF",
        Mime::Archive => "ZIP",
        Mime::Disk => "DSK",
    }
}

/// The reachability colour for a peer's status dot.
const fn peer_color(status: PeerStatus) -> Color32 {
    match status {
        PeerStatus::Online | PeerStatus::Self_ => Style::OK,
        PeerStatus::Idle => Style::WARN,
        PeerStatus::Offline => Style::TEXT_DIM,
    }
}
