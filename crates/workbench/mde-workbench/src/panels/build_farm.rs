//! FARM-AUTO-5 — the **Build Farm** panel (Provisioning plane).
//!
//! A read-only view over build-farm activity: it reads the `event/farm/<jobid>`
//! events the mackesd `farm_orchestrator` worker (FARM-AUTO-1) publishes onto the
//! Bus and projects them into per-job rows (queued / passed / failed). Same
//! established pattern as the other Bus-reading panels (home/hub read their
//! topics the same way) — no new cross-crate dependency.
//!
//! BUILD-PLATFORM-7: it ALSO reads `event/test/{install,feature,stability}` (the
//! L1/L2/L3 nightly tiers) off the same Bus and surfaces a pass/fail badge per
//! tier, so a rotting safety net is visible without asking an AI. A tier that has
//! never run renders a clean "no runs yet" badge (not an error, not a missing row).
//!
//! Build-now-defer-visual: the load + projection are pure and unit-tested; the
//! on-Cosmic `/preview` render is the deferred tail (per the lifted visual gate).

use crate::cosmic_compat::prelude::*;
use cosmic::iced::widget::{column, container, row, scrollable, text};
use cosmic::iced::{Length, Task};
use cosmic::Element;

/// The three internal test tiers the nightly orchestrator (BUILD-PLATFORM-4..7)
/// publishes to `event/test/{install,feature,stability}` — L1/L2/L3. Rendered as
/// one always-present badge each so a rotting safety net is obvious even when a
/// tier has *never* run (clean "no runs yet" state, not a missing row).
const TEST_TIERS: [(&str, &str); 3] = [
    ("install", "L1 · install"),
    ("feature", "L2 · feature"),
    ("stability", "L3 · stability"),
];

/// One farm job as last seen on the Bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FarmJobRow {
    pub jobid: String,
    /// "queued" | "done"
    pub phase: String,
    /// "pass" | "fail" for done jobs; empty while queued.
    pub outcome: String,
}

impl FarmJobRow {
    /// A short status glyph + label for the row (Carbon: text/icon, no raw color).
    #[must_use]
    pub fn status_label(&self) -> &'static str {
        match (self.phase.as_str(), self.outcome.as_str()) {
            ("done", "pass") => "✓ passed",
            ("done", "fail") => "✗ failed",
            ("done", _) => "• done",
            _ => "… queued",
        }
    }
}

/// Parse one `event/farm/<jobid>` message body into a row. Pure + testable.
#[must_use]
pub fn parse_farm_event(body: &str) -> Option<FarmJobRow> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let jobid = v.get("jobid")?.as_str()?.to_string();
    let phase = v
        .get("phase")
        .and_then(|p| p.as_str())
        .unwrap_or("queued")
        .to_string();
    let outcome = v
        .get("outcome")
        .and_then(|o| o.as_str())
        .unwrap_or("")
        .to_string();
    Some(FarmJobRow {
        jobid,
        phase,
        outcome,
    })
}

/// The latest pass/fail verdict for one internal test tier (L1/L2/L3), as last
/// seen on `event/test/<tier>`. `Unknown` is the clean "no runs yet" state — the
/// tier badge always renders, even before the nightly orchestrator has published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierOutcome {
    Pass,
    Fail,
    /// Published, but neither pass nor fail (e.g. running / unrecognised verdict).
    Unknown,
    /// No `event/test/<tier>` body on the Bus at all.
    NoRuns,
}

/// One nightly test-tier badge (BUILD-PLATFORM-7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestTierRow {
    /// Tier key, e.g. "install" / "feature" / "stability".
    pub tier: String,
    /// Human label, e.g. "L1 · install".
    pub label: String,
    pub outcome: TierOutcome,
}

impl TestTierRow {
    /// Short status label for the badge (Carbon: text/glyph, color applied via token).
    #[must_use]
    pub fn status_label(&self) -> &'static str {
        match self.outcome {
            TierOutcome::Pass => "✓ passed",
            TierOutcome::Fail => "✗ failed",
            TierOutcome::Unknown => "• running",
            TierOutcome::NoRuns => "— no runs yet",
        }
    }
}

/// Map an `event/test/<tier>` body's verdict to a [`TierOutcome`]. The body carries
/// `outcome`/`overall` (pass|fail|green|RED). Pure.
#[must_use]
pub fn parse_tier_outcome(body: &str) -> TierOutcome {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return TierOutcome::Unknown;
    };
    let raw = v
        .get("outcome")
        .or_else(|| v.get("overall"))
        .and_then(|o| o.as_str())
        .unwrap_or("");
    match raw {
        "pass" | "green" => TierOutcome::Pass,
        "" => TierOutcome::Unknown,
        "fail" | "red" | "RED" => TierOutcome::Fail,
        _ => TierOutcome::Fail,
    }
}

/// Project the Bus reads into one badge per known tier — always all three, in
/// L1→L3 order, so a tier that has never run shows a clean "no runs yet" badge
/// rather than vanishing. Mirrors `project_rows`' Bus-read idiom.
#[must_use]
pub fn project_tiers(events: &[(String, String)]) -> Vec<TestTierRow> {
    TEST_TIERS
        .iter()
        .map(|(tier, label)| {
            let topic = format!("event/test/{tier}");
            let outcome = events
                .iter()
                .find(|(t, _)| *t == topic)
                .map_or(TierOutcome::NoRuns, |(_, body)| parse_tier_outcome(body));
            TestTierRow {
                tier: (*tier).to_string(),
                label: (*label).to_string(),
                outcome,
            }
        })
        .collect()
}

/// Project a set of `(topic, latest-body)` Bus reads into sorted farm-job rows
/// (`event/farm/*` only — test tiers are projected separately by
/// [`project_tiers`]), failures first.
#[must_use]
pub fn project_rows(events: &[(String, String)]) -> Vec<FarmJobRow> {
    let mut rows: Vec<FarmJobRow> = events
        .iter()
        .filter_map(|(topic, body)| {
            if topic.starts_with("event/farm/") {
                parse_farm_event(body)
            } else {
                None
            }
        })
        .collect();
    rows.sort_by_key(|r| match (r.phase.as_str(), r.outcome.as_str()) {
        ("done", "fail") => 0,
        (_, _) if r.phase != "done" => 1,
        _ => 2,
    });
    rows
}

/// What one Bus read yields: the farm-job rows and the per-tier test badges.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FarmSnapshot {
    pub jobs: Vec<FarmJobRow>,
    pub tiers: Vec<TestTierRow>,
}

#[derive(Debug, Clone, Default)]
pub struct BuildFarmPanel {
    pub jobs: Vec<FarmJobRow>,
    /// Per-tier nightly test badges (BUILD-PLATFORM-7) — always the three known
    /// tiers; an unseen tier renders as "no runs yet".
    pub tiers: Vec<TestTierRow>,
    pub status: String,
    pub busy: bool,
    /// Set when the load failed (vs legitimately empty) — render the error, not
    /// a misleading "no farm activity" empty state.
    pub load_error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<FarmSnapshot, String>),
    RefreshClicked,
}

impl BuildFarmPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read the `event/farm/*` + `event/test/*` topics off the Bus + project them.
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move { Message::Loaded(read_farm_events()) },
            crate::Message::BuildFarm,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(Ok(snapshot)) => {
                self.jobs = snapshot.jobs;
                self.tiers = snapshot.tiers;
                self.busy = false;
                self.load_error = None;
                self.status.clear();
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.load_error = Some(e);
                self.busy = false;
                Task::none()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Refreshing…".into();
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        if let Some(err) = &self.load_error {
            return container(text(format!("Couldn't read farm activity: {err}")))
                .padding(16)
                .into();
        }
        let palette = crate::live_theme::palette();

        // BUILD-PLATFORM-7: nightly test tiers — always rendered (an unseen tier
        // shows a clean "no runs yet" badge), so a rotting safety net is obvious.
        let tiers = if self.tiers.is_empty() {
            project_tiers(&[])
        } else {
            self.tiers.clone()
        };
        let mut tier_section = column![text("Nightly tests").size(16)].spacing(6);
        for t in &tiers {
            let badge_color = match t.outcome {
                TierOutcome::Pass => palette.success,
                TierOutcome::Fail => palette.danger,
                TierOutcome::Unknown => palette.warning,
                TierOutcome::NoRuns => palette.text_muted,
            };
            tier_section = tier_section.push(
                container(
                    row![
                        text(t.label.clone()).width(Length::FillPortion(2)),
                        text(t.status_label())
                            .width(Length::FillPortion(2))
                            .colr(badge_color.into_cosmic_color()),
                    ]
                    .spacing(12),
                )
                .padding(10)
                .width(Length::Fill),
            );
        }

        let mut col = column![tier_section].spacing(12).padding(16);

        // Farm jobs (FARM-AUTO-5).
        if self.jobs.is_empty() {
            col = col.push(
                column![
                    text("Build farm").size(16),
                    text("No @farm jobs yet — queued and finished jobs appear here as the \
                          orchestrator publishes them.")
                    .colr(palette.text_muted.into_cosmic_color()),
                ]
                .spacing(6),
            );
        } else {
            let mut jobs_section =
                column![text(format!("Build farm — {} job(s)", self.jobs.len())).size(16)]
                    .spacing(6);
            for j in &self.jobs {
                jobs_section = jobs_section.push(
                    container(
                        row![
                            text(j.jobid.clone()).width(Length::FillPortion(2)),
                            text(j.phase.clone()).width(Length::FillPortion(1)),
                            text(j.status_label()).width(Length::FillPortion(1)),
                        ]
                        .spacing(12),
                    )
                    .padding(10)
                    .width(Length::Fill),
                );
            }
            col = col.push(jobs_section);
        }
        scrollable(col).into()
    }
}

/// Bus read: every `event/farm/*` + `event/test/*` topic's latest body.
/// Best-effort — a missing Bus yields an empty snapshot (the panel renders the
/// clean "no runs yet" state, not an error).
fn read_farm_events() -> Result<FarmSnapshot, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Ok(FarmSnapshot {
            jobs: Vec::new(),
            tiers: project_tiers(&[]),
        });
    };
    let persist = mde_bus::persist::Persist::open(dir).map_err(|e| e.to_string())?;
    let topics = persist.list_topics().map_err(|e| e.to_string())?;
    let mut events = Vec::new();
    for topic in topics
        .into_iter()
        .filter(|t| t.starts_with("event/farm/") || t.starts_with("event/test/"))
    {
        if let Ok(msgs) = persist.list_since(&topic, None) {
            if let Some(body) = msgs.last().and_then(|m| m.body.clone()) {
                events.push((topic, body));
            }
        }
    }
    Ok(FarmSnapshot {
        jobs: project_rows(&events),
        tiers: project_tiers(&events),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_farm_event_reads_jobid_phase_outcome() {
        let r = parse_farm_event(r#"{"jobid":"abc","phase":"done","outcome":"pass"}"#).unwrap();
        assert_eq!(r.jobid, "abc");
        assert_eq!(r.phase, "done");
        assert_eq!(r.outcome, "pass");
        assert_eq!(r.status_label(), "✓ passed");

        let q = parse_farm_event(r#"{"jobid":"x","phase":"queued"}"#).unwrap();
        assert_eq!(q.status_label(), "… queued");
        assert!(parse_farm_event("not json").is_none());
    }

    #[test]
    fn parse_tier_outcome_maps_verdicts() {
        assert_eq!(parse_tier_outcome(r#"{"outcome":"pass"}"#), TierOutcome::Pass);
        assert_eq!(parse_tier_outcome(r#"{"overall":"green"}"#), TierOutcome::Pass);
        assert_eq!(parse_tier_outcome(r#"{"overall":"RED"}"#), TierOutcome::Fail);
        assert_eq!(parse_tier_outcome(r#"{"outcome":"fail"}"#), TierOutcome::Fail);
        // empty / running → Unknown; malformed JSON → Unknown (never an error)
        assert_eq!(parse_tier_outcome(r#"{"outcome":""}"#), TierOutcome::Unknown);
        assert_eq!(parse_tier_outcome("not json"), TierOutcome::Unknown);
    }

    #[test]
    fn project_tiers_always_renders_three_with_no_runs_default() {
        // No test events at all → all three tiers show the clean "no runs yet" state.
        let tiers = project_tiers(&[]);
        assert_eq!(tiers.len(), 3);
        assert_eq!(tiers[0].tier, "install");
        assert_eq!(tiers[1].tier, "feature");
        assert_eq!(tiers[2].tier, "stability");
        assert!(tiers.iter().all(|t| t.outcome == TierOutcome::NoRuns));
        assert_eq!(tiers[0].status_label(), "— no runs yet");
    }

    #[test]
    fn project_tiers_reads_latest_per_tier() {
        let events = vec![
            (
                "event/test/install".into(),
                r#"{"outcome":"pass"}"#.into(),
            ),
            (
                "event/test/stability".into(),
                r#"{"overall":"RED"}"#.into(),
            ),
            // feature absent → stays "no runs yet"
            ("event/farm/x".into(), r#"{"jobid":"x"}"#.into()), // ignored here
        ];
        let tiers = project_tiers(&events);
        assert_eq!(tiers[0].outcome, TierOutcome::Pass); // install
        assert_eq!(tiers[1].outcome, TierOutcome::NoRuns); // feature
        assert_eq!(tiers[2].outcome, TierOutcome::Fail); // stability
        assert_eq!(tiers[0].status_label(), "✓ passed");
        assert_eq!(tiers[2].status_label(), "✗ failed");
    }

    #[test]
    fn project_rows_filters_to_farm_only_and_orders_failed_first() {
        let events = vec![
            ("event/firewall/host".into(), r#"{"jobid":"nope"}"#.into()), // not farm → dropped
            ("event/test/install".into(), r#"{"outcome":"pass"}"#.into()), // test → not a job row
            (
                "event/farm/p".into(),
                r#"{"jobid":"p","phase":"done","outcome":"pass"}"#.into(),
            ),
            (
                "event/farm/f".into(),
                r#"{"jobid":"f","phase":"done","outcome":"fail"}"#.into(),
            ),
            (
                "event/farm/q".into(),
                r#"{"jobid":"q","phase":"queued"}"#.into(),
            ),
        ];
        let rows = project_rows(&events);
        assert_eq!(rows.len(), 3); // only the three farm events
        assert_eq!(rows[0].jobid, "f"); // failed first
        assert_eq!(rows[1].jobid, "q"); // then queued
        assert_eq!(rows[2].jobid, "p"); // then passed
    }
}
