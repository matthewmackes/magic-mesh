//! Bounded local-controller read model for shared Surface fleet summaries.
//!
//! The mackesd verifier publishes one compact summary on
//! `state/hardware/surface/<node>`. This module discovers those lanes from the
//! local replicated Bus spool, admits only the shared wire contract, and folds
//! them into a read-only rollup. It owns no action topic and exposes no control
//! callback: remote Surface state is visibility only.

use std::collections::BTreeMap;
use std::path::Path;

use mackes_mesh_types::surface_hardware::{
    SurfaceAvailability, SurfaceFleetSummary, SurfaceProGeneration, SurfaceSubsystem,
    MAX_SURFACE_ID_BYTES, MAX_SURFACE_STATE_AGE_MS,
};
use mde_egui::egui::{self, RichText};
use mde_egui::{muted_note, status_dot, Style};

use crate::bus_reader::BusReader;

const SUMMARY_PREFIX: &str = "state/hardware/surface/";
const MAX_FLEET_NODES: usize = 64;
const MAX_FUTURE_SKEW_MS: u64 = 5_000;

/// Freshness of one admitted Surface summary at the local controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SurfaceRowFreshness {
    Fresh,
    Stale,
    Unavailable,
}

/// One compact, non-actionable Surface node row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SurfaceFleetRow {
    pub(super) node: String,
    pub(super) model: Option<String>,
    pub(super) enablement_pct: Option<u8>,
    pub(super) red_subsystems: Vec<SurfaceSubsystem>,
    pub(super) freshness: SurfaceRowFreshness,
    pub(super) reason: Option<String>,
}

/// Whole bounded rollup. Rejected lanes are counted, never rendered as facts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct SurfaceFleetReadModel {
    pub(super) rows: Vec<SurfaceFleetRow>,
    pub(super) rejected_lanes: usize,
    pub(super) overflow: bool,
    pub(super) bus_available: bool,
}

/// Read the latest summary from each direct Surface state lane.
pub(super) fn read_surface_fleet(bus_root: Option<&Path>, now_ms: u64) -> SurfaceFleetReadModel {
    let Some(persist) = BusReader::new(bus_root.map(Path::to_path_buf)).open() else {
        return SurfaceFleetReadModel::default();
    };
    let Ok(topics) = persist.list_topics() else {
        return SurfaceFleetReadModel::default();
    };
    let mut admitted = BTreeMap::<String, SurfaceFleetRow>::new();
    let mut rejected_lanes = 0usize;
    let mut matching = 0usize;
    let mut overflow = false;
    for topic in topics {
        let Some(node) = direct_topic_node(&topic) else {
            continue;
        };
        matching = matching.saturating_add(1);
        if matching > MAX_FLEET_NODES {
            overflow = true;
            rejected_lanes = rejected_lanes.saturating_add(1);
            continue;
        }
        let Ok(Some(message)) = persist.read_latest(&topic) else {
            rejected_lanes = rejected_lanes.saturating_add(1);
            continue;
        };
        let Some(body) = message.body else {
            rejected_lanes = rejected_lanes.saturating_add(1);
            continue;
        };
        let Ok(summary) = SurfaceFleetSummary::from_json(body.as_bytes()) else {
            rejected_lanes = rejected_lanes.saturating_add(1);
            continue;
        };
        if summary.publication.node != node || !supported_model(&summary) {
            rejected_lanes = rejected_lanes.saturating_add(1);
            continue;
        }
        let Some(row) = row_from_summary(summary, now_ms) else {
            rejected_lanes = rejected_lanes.saturating_add(1);
            continue;
        };
        admitted.insert(node.to_string(), row);
    }

    SurfaceFleetReadModel {
        rows: admitted.into_values().collect(),
        rejected_lanes,
        overflow,
        bus_available: true,
    }
}

fn direct_topic_node(topic: &str) -> Option<&str> {
    let node = topic.strip_prefix(SUMMARY_PREFIX)?;
    (valid_node_id(node) && !node.contains('/')).then_some(node)
}

fn valid_node_id(node: &str) -> bool {
    !node.is_empty()
        && node.len() <= MAX_SURFACE_ID_BYTES
        && node
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
}

fn supported_model(summary: &SurfaceFleetSummary) -> bool {
    matches!(
        (
            summary.publication.model.generation,
            summary.publication.model.product.as_str()
        ),
        (SurfaceProGeneration::Pro5, "Surface Pro 5")
            | (SurfaceProGeneration::Pro6, "Surface Pro 6")
    )
}

fn row_from_summary(summary: SurfaceFleetSummary, now_ms: u64) -> Option<SurfaceFleetRow> {
    let publication = &summary.publication;
    if publication.published_at_ms > now_ms.saturating_add(MAX_FUTURE_SKEW_MS) {
        return None;
    }
    let model = Some(publication.model.product.clone());
    let age_ms = now_ms.saturating_sub(publication.published_at_ms);
    match &publication.availability {
        SurfaceAvailability::Fresh if age_ms <= MAX_SURFACE_STATE_AGE_MS => Some(SurfaceFleetRow {
            node: publication.node.clone(),
            model,
            enablement_pct: Some(summary.enablement_pct),
            red_subsystems: summary.red_subsystems,
            freshness: SurfaceRowFreshness::Fresh,
            reason: None,
        }),
        SurfaceAvailability::Fresh => Some(SurfaceFleetRow {
            node: publication.node.clone(),
            model,
            enablement_pct: None,
            red_subsystems: Vec::new(),
            freshness: SurfaceRowFreshness::Stale,
            reason: Some("Surface summary is older than 90 seconds".to_string()),
        }),
        SurfaceAvailability::Stale { reason } => Some(SurfaceFleetRow {
            node: publication.node.clone(),
            model,
            enablement_pct: None,
            red_subsystems: Vec::new(),
            freshness: SurfaceRowFreshness::Stale,
            reason: Some(reason.clone()),
        }),
        SurfaceAvailability::Unavailable { reason } => Some(SurfaceFleetRow {
            node: publication.node.clone(),
            model,
            enablement_pct: None,
            red_subsystems: Vec::new(),
            freshness: SurfaceRowFreshness::Unavailable,
            reason: Some(reason.clone()),
        }),
    }
}

pub(super) fn render(ui: &mut egui::Ui, model: &SurfaceFleetReadModel) {
    mde_egui::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Surface health")
                    .color(Style::TEXT_STRONG)
                    .size(Style::BODY)
                    .strong(),
            );
            ui.add_space(Style::SP_S);
            muted_note(
                ui,
                format!(
                    "{} {} · read-only fleet summaries",
                    model.rows.len(),
                    if model.rows.len() == 1 {
                        "node"
                    } else {
                        "nodes"
                    }
                ),
            );
        });
        if model.rows.is_empty() {
            muted_note(ui, "No admitted Surface summaries are available yet.");
        }
        for row in &model.rows {
            ui.horizontal_wrapped(|ui| {
                status_dot(
                    ui,
                    match row.freshness {
                        SurfaceRowFreshness::Fresh if row.red_subsystems.is_empty() => Style::OK,
                        SurfaceRowFreshness::Fresh => Style::DANGER,
                        SurfaceRowFreshness::Stale => Style::WARN,
                        SurfaceRowFreshness::Unavailable => Style::TEXT_DIM,
                    },
                );
                ui.label(RichText::new(&row.node).color(Style::TEXT).strong());
                muted_note(
                    ui,
                    row.model
                        .as_deref()
                        .unwrap_or("Surface summary unavailable"),
                );
                match row.enablement_pct {
                    Some(percent) => muted_note(ui, format!("{percent}% enabled")),
                    None => muted_note(
                        ui,
                        match row.freshness {
                            SurfaceRowFreshness::Stale => "stale",
                            _ => "unavailable",
                        },
                    ),
                };
                if !row.red_subsystems.is_empty() {
                    muted_note(
                        ui,
                        format!(
                            "red: {}",
                            row.red_subsystems
                                .iter()
                                .map(|subsystem| subsystem_label(*subsystem))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    );
                }
                if let Some(reason) = &row.reason {
                    muted_note(ui, reason);
                }
            });
        }
        if model.rejected_lanes > 0 || model.overflow {
            muted_note(
                ui,
                format!(
                    "{} rejected Surface {}{}",
                    model.rejected_lanes,
                    if model.rejected_lanes == 1 {
                        "lane"
                    } else {
                        "lanes"
                    },
                    if model.overflow {
                        " · fleet bound exceeded"
                    } else {
                        ""
                    }
                ),
            );
        } else if !model.bus_available {
            muted_note(
                ui,
                "Surface Bus unavailable; node rows remain read-only and unavailable.",
            );
        }
    });
}

fn subsystem_label(subsystem: SurfaceSubsystem) -> &'static str {
    match subsystem {
        SurfaceSubsystem::Touch => "touch",
        SurfaceSubsystem::Pen => "pen",
        SurfaceSubsystem::TypeCover => "Cover",
        SurfaceSubsystem::Sam => "SAM",
        SurfaceSubsystem::RotationAccel => "rotation",
        SurfaceSubsystem::Cameras => "cameras",
        SurfaceSubsystem::WifiBt => "Wi-Fi/Bluetooth",
        SurfaceSubsystem::S0ix => "S0ix",
        SurfaceSubsystem::Fingerprint => "fingerprint",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::surface_hardware::{
        SurfaceModelIdentity, SurfaceObservationSource, SurfacePublication, MAX_SURFACE_WIRE_BYTES,
        SURFACE_HARDWARE_SCHEMA_VERSION,
    };
    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;

    const NOW: u64 = 1_800_000_000_000;

    fn summary(node: &str) -> SurfaceFleetSummary {
        SurfaceFleetSummary {
            publication: SurfacePublication {
                schema_version: SURFACE_HARDWARE_SCHEMA_VERSION,
                node: node.to_string(),
                model: SurfaceModelIdentity {
                    product: "Surface Pro 6".to_string(),
                    generation: SurfaceProGeneration::Pro6,
                },
                source: SurfaceObservationSource::Kernel,
                published_at_ms: NOW,
                availability: SurfaceAvailability::Fresh,
            },
            enablement_pct: 88,
            red_count: 1,
            red_subsystems: vec![SurfaceSubsystem::Cameras],
        }
    }

    fn pro5_summary(node: &str) -> SurfaceFleetSummary {
        let mut value = summary(node);
        value.publication.model = SurfaceModelIdentity {
            // `surface::identify` normalizes the raw Pro 5 DMI aliases to this
            // producer wire identity before `shared_summary` publishes it.
            product: "Surface Pro 5".to_string(),
            generation: SurfaceProGeneration::Pro5,
        };
        value
    }

    fn write(persist: &Persist, topic: &str, body: &str) {
        persist
            .write(topic, Priority::Default, None, Some(body))
            .expect("write summary");
    }

    #[test]
    fn reads_every_admitted_surface_summary_and_excludes_non_surface_topics() {
        let temp = tempfile::tempdir().unwrap();
        let persist = Persist::open(temp.path().to_path_buf()).unwrap();
        write(
            &persist,
            "state/hardware/surface/surface-a",
            &serde_json::to_string(&summary("surface-a")).unwrap(),
        );
        write(
            &persist,
            "state/hardware/surface/surface-b",
            &serde_json::to_string(&pro5_summary("surface-b")).unwrap(),
        );
        write(&persist, "state/units/healthy-non-surface", "{}");
        let model = read_surface_fleet(Some(temp.path()), NOW);
        assert_eq!(model.rows.len(), 2);
        assert_eq!(model.rows[0].enablement_pct, Some(88));
        assert_eq!(
            model.rows[0].red_subsystems,
            vec![SurfaceSubsystem::Cameras]
        );
        assert_eq!(model.rows[1].node, "surface-b");
        assert_eq!(model.rows[1].model.as_deref(), Some("Surface Pro 5"));
        assert!(model
            .rows
            .iter()
            .all(|row| row.node != "healthy-non-surface"));
    }

    #[test]
    fn rejects_foreign_duplicate_oversize_future_and_foreign_model_claims() {
        let temp = tempfile::tempdir().unwrap();
        let persist = Persist::open(temp.path().to_path_buf()).unwrap();
        let cases = [
            (
                "foreign",
                serde_json::to_string(&summary("other-node")).unwrap(),
            ),
            (
                "duplicate",
                serde_json::to_string(&summary("duplicate"))
                    .unwrap()
                    .replacen(
                        "\"enablement_pct\":88",
                        "\"enablement_pct\":88,\"enablement_pct\":88",
                        1,
                    ),
            ),
            ("oversize", " ".repeat(MAX_SURFACE_WIRE_BYTES + 1)),
            (
                "unknown-field",
                serde_json::to_string(&summary("unknown-field"))
                    .unwrap()
                    .replacen(
                        "\"enablement_pct\":88",
                        "\"unknown\":true,\"enablement_pct\":88",
                        1,
                    ),
            ),
            ("future", {
                let mut value = summary("future");
                value.publication.published_at_ms = NOW + MAX_FUTURE_SKEW_MS + 1;
                serde_json::to_string(&value).unwrap()
            }),
            ("foreign-model", {
                let mut value = summary("foreign-model");
                value.publication.model.product = "Surface Pro 7".to_string();
                serde_json::to_string(&value).unwrap()
            }),
        ];
        for (node, body) in cases {
            write(&persist, &format!("{SUMMARY_PREFIX}{node}"), &body);
        }
        let model = read_surface_fleet(Some(temp.path()), NOW);
        assert_eq!(model.rejected_lanes, 6);
        assert!(model.rows.is_empty());
    }

    #[test]
    fn stale_fresh_publication_hides_percentage_and_red_facts() {
        let temp = tempfile::tempdir().unwrap();
        let persist = Persist::open(temp.path().to_path_buf()).unwrap();
        write(
            &persist,
            "state/hardware/surface/surface-a",
            &serde_json::to_string(&summary("surface-a")).unwrap(),
        );
        let model = read_surface_fleet(Some(temp.path()), NOW + MAX_SURFACE_STATE_AGE_MS + 1);
        assert_eq!(model.rows[0].freshness, SurfaceRowFreshness::Stale);
        assert_eq!(model.rows[0].enablement_pct, None);
        assert!(model.rows[0].red_subsystems.is_empty());
    }

    #[test]
    fn compact_rollup_renders_headlessly() {
        let model = SurfaceFleetReadModel {
            rows: vec![row_from_summary(summary("surface-a"), NOW).unwrap()],
            bus_available: true,
            ..Default::default()
        };
        let context = egui::Context::default();
        Style::install(&context);
        let output = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| render(ui, &model));
        });
        assert!(!output.shapes.is_empty());
    }
}
