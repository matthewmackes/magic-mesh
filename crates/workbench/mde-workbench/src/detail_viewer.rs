//! NOTIFY-REDESIGN-C — the reusable hub **detail viewer** contract.
//!
//! One render-agnostic model, [`DetailContent`], that EVERY Notification-Hub
//! item type maps itself to — a notification, a clipboard row, a lighthouse
//! beacon, a voice/music snapshot. The bin's shared center-modal shell renders
//! any `DetailContent` identically, so the detail surface is built ONCE and
//! reused for every item kind (the operator's REUSE directive, 2026-06-30) —
//! not a notification-specific viewer.
//!
//! Pure: no iced/cosmic dependency, unit-testable in isolation. It mirrors the
//! `mde-notify` / `notify_clipboard` split — the render-agnostic model lives
//! here, and the themed iced glue (the modal shell + the per-item providers)
//! lives in the `mde-notify-center` bin, which alone crosses the World-2
//! layer-shell boundary.

use mde_notify::Severity;

/// The visual tone of a detail action button.
///
/// The bin's renderer maps each tone to an `mde-theme` token (§4): `Neutral` →
/// the subtle Carbon button, `Primary` → the accent fill, `Danger` → the danger
/// token for destructive actions (Dismiss / delete).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionTone {
    /// A secondary action drawn in the subtle Carbon button style.
    Neutral,
    /// The primary / affirmative action — accent fill.
    Primary,
    /// A destructive action — danger tone.
    Danger,
}

/// One footer action button in a detail view.
///
/// Generic over the surface's message type so the model names no bin-specific
/// verb (the "generic, reusable" directive): each consumer supplies the exact
/// message the shell dispatches when the button is pressed.
#[derive(Debug, Clone)]
pub struct DetailAction<Msg> {
    /// The button label.
    pub label: String,
    /// The button's visual tone.
    pub tone: ActionTone,
    /// The message dispatched on press.
    pub on_press: Msg,
}

impl<Msg> DetailAction<Msg> {
    /// A labeled action of `tone` that dispatches `on_press` when pressed.
    pub fn new(label: impl Into<String>, tone: ActionTone, on_press: Msg) -> Self {
        Self {
            label: label.into(),
            tone,
            on_press,
        }
    }
}

/// The render-agnostic content of one hub detail view.
///
/// A consumer fills a [`DetailContent`] from its own item (see the bin's
/// `notif_detail` / `clip_detail` / `beacon_detail` / `voice_detail`); the
/// shared shell then renders the hero band (tinted by [`Self::severity`]), the
/// field table, the free-text body, the mono raw block, and the action buttons
/// identically for every kind. Copy-all / Copy-raw are provided by the shell
/// (from [`Self::copy_all_text`] + [`Self::raw`]), so [`Self::actions`] carries
/// only the domain verbs.
#[derive(Debug, Clone)]
pub struct DetailContent<Msg> {
    /// The prominent title shown in the hero band.
    pub title: String,
    /// Optional severity — tints the hero band + picks its status icon. `None`
    /// for non-severity items (clipboard / lighthouse / voice) → a neutral
    /// accent-tinted band.
    pub severity: Option<Severity>,
    /// Label / value rows rendered as the structured field table.
    pub fields: Vec<(String, String)>,
    /// The full free-text body (Carbon sans). Empty → omitted.
    pub body: String,
    /// The verbatim raw / structured block (mono). Empty → omitted.
    pub raw: String,
    /// The footer action buttons (Open source · Mark read · Dismiss · Mute …).
    pub actions: Vec<DetailAction<Msg>>,
}

impl<Msg> DetailContent<Msg> {
    /// A minimal detail carrying only `title`; the builders fill the rest.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            severity: None,
            fields: Vec::new(),
            body: String::new(),
            raw: String::new(),
            actions: Vec::new(),
        }
    }

    /// Tint the hero band by `severity` (and pick its status icon).
    #[must_use]
    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = Some(severity);
        self
    }

    /// Push a `label: value` field, SKIPPING an empty value so the table shows
    /// only real data (§7 — never a blank "Host:" row for a hostless alert).
    #[must_use]
    pub fn field(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        let value = value.into();
        if !value.is_empty() {
            self.fields.push((label.into(), value));
        }
        self
    }

    /// Set the free-text body.
    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    /// Set the verbatim raw / structured block.
    #[must_use]
    pub fn with_raw(mut self, raw: impl Into<String>) -> Self {
        self.raw = raw.into();
        self
    }

    /// Append one action button.
    #[must_use]
    pub fn action(mut self, action: DetailAction<Msg>) -> Self {
        self.actions.push(action);
        self
    }

    /// `true` when there is a verbatim raw block to show (and to offer Copy-raw
    /// for). Whitespace-only counts as empty.
    #[must_use]
    pub fn has_raw(&self) -> bool {
        !self.raw.trim().is_empty()
    }

    /// The whole detail rendered as plain text for the shell's **Copy-all**
    /// button.
    ///
    /// The title, then each `label: value` field, then the body, then the raw
    /// block, separated by blank lines where it reads best. Pure + testable so
    /// the Copy-all payload is verified without a live clipboard.
    #[must_use]
    pub fn copy_all_text(&self) -> String {
        let mut out = String::new();
        out.push_str(self.title.trim());
        for (label, value) in &self.fields {
            out.push('\n');
            out.push_str(label);
            out.push_str(": ");
            out.push_str(value);
        }
        if !self.body.trim().is_empty() {
            out.push_str("\n\n");
            out.push_str(self.body.trim());
        }
        if self.has_raw() {
            out.push_str("\n\n");
            out.push_str(self.raw.trim());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway message type — the model is generic, so the tests need any
    // `Msg`; a unit-like enum keeps the intent (an action carries SOME verb).
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestMsg {
        Dismiss,
        Open,
    }

    #[test]
    fn field_skips_empty_values() {
        // §7 — a hostless alert must not render a blank "Host:" row.
        let c = DetailContent::<TestMsg>::new("t")
            .field("Source", "System")
            .field("Host", "") // dropped
            .field("Topic", "fleet/sec");
        let labels: Vec<&str> = c.fields.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["Source", "Topic"]);
    }

    #[test]
    fn has_raw_ignores_whitespace() {
        assert!(!DetailContent::<TestMsg>::new("t").has_raw());
        assert!(!DetailContent::<TestMsg>::new("t")
            .with_raw("   \n ")
            .has_raw());
        assert!(DetailContent::<TestMsg>::new("t").with_raw("{}").has_raw());
    }

    #[test]
    fn copy_all_text_composes_title_fields_body_and_raw() {
        let c = DetailContent::<TestMsg>::new("Disk almost full")
            .with_severity(Severity::Warning)
            .field("Source", "System")
            .field("Host", "node-a")
            .with_body("The root volume is at 92%.")
            .with_raw("{\"topic\":\"mackesd::alert\"}");
        let txt = c.copy_all_text();
        assert_eq!(
            txt,
            "Disk almost full\nSource: System\nHost: node-a\n\nThe root volume is at 92%.\n\n{\"topic\":\"mackesd::alert\"}"
        );
    }

    #[test]
    fn copy_all_text_omits_absent_sections() {
        // A title-only detail copies to just the title (no stray blank lines).
        let c = DetailContent::<TestMsg>::new("Just a title");
        assert_eq!(c.copy_all_text(), "Just a title");
    }

    #[test]
    fn builders_carry_severity_and_actions() {
        let c = DetailContent::new("t")
            .with_severity(Severity::Critical)
            .action(DetailAction::new(
                "Open",
                ActionTone::Primary,
                TestMsg::Open,
            ))
            .action(DetailAction::new(
                "Dismiss",
                ActionTone::Danger,
                TestMsg::Dismiss,
            ));
        assert_eq!(c.severity, Some(Severity::Critical));
        assert_eq!(c.actions.len(), 2);
        assert_eq!(c.actions[0].tone, ActionTone::Primary);
        assert_eq!(c.actions[1].on_press, TestMsg::Dismiss);
    }
}
