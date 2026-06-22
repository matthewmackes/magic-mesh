//! FARM-AUTO-5 — the **Build Farm** panel (Provisioning plane).
//!
//! A read-only view over build-farm activity: it reads the `event/farm/<jobid>`
//! events the mackesd `farm_orchestrator` worker (FARM-AUTO-1) publishes onto the
//! Bus and projects them into per-job rows (queued / passed / failed). Same
//! established pattern as the other Bus-reading panels (home/hub read their
//! topics the same way) — no new cross-crate dependency.
//!
//! Build-now-defer-visual: the load + projection are pure and unit-tested; the
//! on-Cosmic `/preview` render is the deferred tail (per the lifted visual gate).

use cosmic::iced::widget::{column, container, row, scrollable, text};
use cosmic::iced::{Length, Task};
use cosmic::Element;

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

/// Parse an `event/test/<tier>` nightly-test summary into a row (BUILD-PLATFORM-7:
/// L1 install / L2 feature / L3 stability / nightly). The body carries
/// `outcome`/`overall` (pass|fail|green|RED) rather than a job phase. Pure.
#[must_use]
pub fn parse_test_event(topic: &str, body: &str) -> Option<FarmJobRow> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let jobid = topic.strip_prefix("event/").unwrap_or(topic).to_string(); // e.g. "test/install"
    let raw = v
        .get("outcome")
        .or_else(|| v.get("overall"))
        .and_then(|o| o.as_str())
        .unwrap_or("");
    let outcome = match raw {
        "pass" | "green" => "pass",
        "" => "",
        _ => "fail",
    }
    .to_string();
    Some(FarmJobRow {
        jobid,
        phase: "done".into(),
        outcome,
    })
}

/// Project a set of `(topic, latest-body)` Bus reads into sorted rows — farm jobs
/// (`event/farm/*`) AND nightly test summaries (`event/test/*`), failures first.
#[must_use]
pub fn project_rows(events: &[(String, String)]) -> Vec<FarmJobRow> {
    let mut rows: Vec<FarmJobRow> = events
        .iter()
        .filter_map(|(topic, body)| {
            if topic.starts_with("event/farm/") {
                parse_farm_event(body)
            } else if topic.starts_with("event/test/") {
                parse_test_event(topic, body)
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

#[derive(Debug, Clone, Default)]
pub struct BuildFarmPanel {
    pub jobs: Vec<FarmJobRow>,
    pub status: String,
    pub busy: bool,
    /// Set when the load failed (vs legitimately empty) — render the error, not
    /// a misleading "no farm activity" empty state.
    pub load_error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<FarmJobRow>, String>),
    RefreshClicked,
}

impl BuildFarmPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read the `event/farm/*` topics off the Bus + project them into rows.
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move { Message::Loaded(read_farm_events()) },
            crate::Message::BuildFarm,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(Ok(jobs)) => {
                self.jobs = jobs;
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
        if self.jobs.is_empty() {
            return container(
                column![
                    text("No build-farm activity yet").size(18),
                    text(
                        "Queued and finished @farm jobs appear here as the orchestrator \
                         publishes them.",
                    ),
                ]
                .spacing(8),
            )
            .padding(16)
            .into();
        }
        let mut col = column![text(format!("Build farm — {} job(s)", self.jobs.len())).size(18)]
            .spacing(8)
            .padding(16);
        for j in &self.jobs {
            col = col.push(
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
        scrollable(col).into()
    }
}

/// Bus read: every `event/farm/*` topic's latest body. Best-effort — a missing
/// Bus yields an empty list (the panel shows the empty state, not an error).
fn read_farm_events() -> Result<Vec<FarmJobRow>, String> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Ok(Vec::new());
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
    Ok(project_rows(&events))
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
    fn parse_test_event_maps_outcome_to_pass_fail() {
        let ok = parse_test_event("event/test/install", r#"{"outcome":"pass"}"#).unwrap();
        assert_eq!(ok.jobid, "test/install");
        assert_eq!(ok.status_label(), "✓ passed");
        let red = parse_test_event("event/test/nightly", r#"{"overall":"RED"}"#).unwrap();
        assert_eq!(red.status_label(), "✗ failed");
    }

    #[test]
    fn project_rows_filters_and_orders_failed_first() {
        let events = vec![
            ("event/firewall/host".into(), r#"{"jobid":"nope"}"#.into()), // not farm → dropped
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
        assert_eq!(rows.len(), 3); // the non-farm event is excluded
        assert_eq!(rows[0].jobid, "f"); // failed first
        assert_eq!(rows[1].jobid, "q"); // then queued
        assert_eq!(rows[2].jobid, "p"); // then passed
    }
}
