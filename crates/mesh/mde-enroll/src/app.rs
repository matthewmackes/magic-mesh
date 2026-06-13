//! ONBOARD-5 — the enrollment TUI state machine.
//!
//! Pure, I/O-free model: input fields, focus, the enroll-progress step
//! list, and the terminal/failure states. The crossterm event loop +
//! ratatui render in `main.rs` drive this; keeping it free of any
//! terminal or network calls makes the whole interaction unit-testable.

use mackesd_core::nebula_enroll::{parse_join_token, JoinToken};

/// Which input field has focus while editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Optional lighthouse-IP override (blank → use the token's).
    Lighthouse,
    /// The join token (paste target).
    Token,
}

/// One observable enroll step + its state. The TUI advances these as
/// the real stages complete, so a stuck step is visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    /// Not started.
    Pending,
    /// In flight.
    Active,
    /// Completed OK.
    Ok,
    /// Failed (the error strip carries the reason).
    Failed,
}

/// The ordered progress steps the network enroll walks through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Parse + validate the join token (must carry a fingerprint).
    Validate,
    /// Open the fingerprint-pinned TLS connection + POST the CSR.
    Connect,
    /// Receive the CA-signed nebula bundle.
    Receive,
    /// Write `/etc/nebula` from the bundle.
    Materialize,
    /// Start the overlay (nebula up).
    Overlay,
}

impl Step {
    /// The full ordered list, for initializing the tracker.
    #[must_use]
    pub fn all() -> [Step; 5] {
        [
            Step::Validate,
            Step::Connect,
            Step::Receive,
            Step::Materialize,
            Step::Overlay,
        ]
    }

    /// Human label for the row.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Step::Validate => "Validate join token",
            Step::Connect => "Pin lighthouse cert + send CSR",
            Step::Receive => "Receive signed bundle",
            Step::Materialize => "Write /etc/nebula",
            Step::Overlay => "Bring up overlay",
        }
    }
}

/// Top-level screen phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Operator is entering the IP + token.
    Editing,
    /// Enroll is running; the step tracker is live.
    Enrolling,
    /// Enroll completed — overlay IP in `outcome`.
    Done,
    /// Enroll failed — `error` carries the reason.
    Failed,
}

/// The full TUI model.
#[derive(Debug, Clone)]
pub struct App {
    /// Optional lighthouse-IP override field contents.
    pub lighthouse: String,
    /// Join-token field contents.
    pub token: String,
    /// Which field has focus while [`Phase::Editing`].
    pub focus: Field,
    /// Current screen phase.
    pub phase: Phase,
    /// Per-step state, in [`Step::all`] order.
    pub steps: Vec<(Step, StepState)>,
    /// Failure reason (set in [`Phase::Failed`]).
    pub error: Option<String>,
    /// Success summary (overlay IP / mesh id), set in [`Phase::Done`].
    pub outcome: Option<String>,
    /// Set when the operator asks to quit.
    pub should_quit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            lighthouse: String::new(),
            token: String::new(),
            focus: Field::Token,
            phase: Phase::Editing,
            steps: Step::all().map(|s| (s, StepState::Pending)).to_vec(),
            error: None,
            outcome: None,
            should_quit: false,
        }
    }
}

impl App {
    /// New empty app focused on the token field.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Move focus between the two fields (Tab).
    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Field::Lighthouse => Field::Token,
            Field::Token => Field::Lighthouse,
        };
    }

    /// Append a typed character to the focused field (editing only).
    pub fn push_char(&mut self, c: char) {
        if self.phase != Phase::Editing {
            return;
        }
        match self.focus {
            Field::Lighthouse => self.lighthouse.push(c),
            Field::Token => self.token.push(c),
        }
    }

    /// Backspace the focused field (editing only).
    pub fn backspace(&mut self) {
        if self.phase != Phase::Editing {
            return;
        }
        match self.focus {
            Field::Lighthouse => {
                self.lighthouse.pop();
            }
            Field::Token => {
                self.token.pop();
            }
        }
    }

    /// Validate the current inputs and produce the parsed token to
    /// enroll with — applying the lighthouse-IP override if the
    /// operator filled that field. Returns the error message to show
    /// in the strip on invalid input. Does NOT change phase (the caller
    /// flips to [`Phase::Enrolling`] once it kicks off the work).
    ///
    /// # Errors
    /// A human message when the token is unparseable or lacks the
    /// pinned fingerprint the network path requires.
    pub fn validated_token(&self) -> Result<JoinToken, String> {
        let mut token = parse_join_token(self.token.trim()).ok_or_else(|| {
            "invalid join token — expected mesh:<id>@<ip>:<port>#<bearer>?fp=<sha256>".to_string()
        })?;
        let lh = self.lighthouse.trim();
        if !lh.is_empty() {
            token.lighthouse = lh.to_string();
        }
        if token.fp.is_none() {
            return Err(
                "this token has no ?fp= fingerprint — it can't be used over the network \
                 (re-run `mackesd found` to mint a v3 token)"
                    .to_string(),
            );
        }
        Ok(token)
    }

    /// Transition into the enrolling phase, marking the first step
    /// active. Clears any prior error.
    pub fn begin_enroll(&mut self) {
        self.phase = Phase::Enrolling;
        self.error = None;
        self.steps = Step::all().map(|s| (s, StepState::Pending)).to_vec();
        self.set_step(Step::Validate, StepState::Active);
    }

    /// Mark `step` complete and the next step active (if any).
    pub fn complete_step(&mut self, step: Step) {
        self.set_step(step, StepState::Ok);
        if let Some(next) = Self::next_step(step) {
            self.set_step(next, StepState::Active);
        }
    }

    /// Mark the run done (success), recording the summary line.
    pub fn finish_ok(&mut self, summary: impl Into<String>) {
        // Whatever step was active becomes Ok; phase → Done.
        for (_, st) in &mut self.steps {
            if *st == StepState::Active {
                *st = StepState::Ok;
            }
        }
        self.outcome = Some(summary.into());
        self.phase = Phase::Done;
    }

    /// Mark the run failed at the currently-active step.
    pub fn fail(&mut self, error: impl Into<String>) {
        for (_, st) in &mut self.steps {
            if *st == StepState::Active {
                *st = StepState::Failed;
            }
        }
        self.error = Some(error.into());
        self.phase = Phase::Failed;
    }

    /// Reset to the editing phase (e.g. after a failure, to retry).
    pub fn reset_to_editing(&mut self) {
        self.phase = Phase::Editing;
        self.steps = Step::all().map(|s| (s, StepState::Pending)).to_vec();
        self.error = None;
        self.outcome = None;
    }

    fn set_step(&mut self, step: Step, state: StepState) {
        if let Some(entry) = self.steps.iter_mut().find(|(s, _)| *s == step) {
            entry.1 = state;
        }
    }

    fn next_step(step: Step) -> Option<Step> {
        let all = Step::all();
        let idx = all.iter().position(|s| *s == step)?;
        all.get(idx + 1).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD_TOKEN: &str = "mesh:home@10.0.0.5:4243#bearer?fp=\
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn editing_typing_and_focus() {
        let mut app = App::new();
        assert_eq!(app.focus, Field::Token);
        app.push_char('m');
        app.push_char('x');
        assert_eq!(app.token, "mx");
        app.backspace();
        assert_eq!(app.token, "m");
        app.toggle_focus();
        assert_eq!(app.focus, Field::Lighthouse);
        app.push_char('1');
        assert_eq!(app.lighthouse, "1");
        assert_eq!(app.token, "m");
    }

    #[test]
    fn validate_rejects_garbage_and_fpless_tokens() {
        let mut app = App::new();
        app.token = "garbage".into();
        assert!(app.validated_token().is_err());
        // fp-less token rejected for the network path.
        app.token = "mesh:home@10.0.0.5:4243#bearer".into();
        let err = app.validated_token().unwrap_err();
        assert!(err.contains("fp"), "got: {err}");
    }

    #[test]
    fn validate_accepts_good_token_and_applies_ip_override() {
        let mut app = App::new();
        app.token = GOOD_TOKEN.into();
        let t = app.validated_token().expect("valid");
        assert_eq!(t.lighthouse, "10.0.0.5");
        assert!(t.fp.is_some());
        // Override the IP via the lighthouse field.
        app.lighthouse = "203.0.113.9".into();
        let t = app.validated_token().expect("valid");
        assert_eq!(t.lighthouse, "203.0.113.9", "override applied");
        assert_eq!(t.port, 4243, "port preserved from token");
    }

    #[test]
    fn step_progression_marks_active_then_ok() {
        let mut app = App::new();
        app.begin_enroll();
        assert_eq!(app.phase, Phase::Enrolling);
        assert_eq!(app.steps[0], (Step::Validate, StepState::Active));
        app.complete_step(Step::Validate);
        assert_eq!(app.steps[0].1, StepState::Ok);
        assert_eq!(app.steps[1], (Step::Connect, StepState::Active));
        app.complete_step(Step::Connect);
        app.complete_step(Step::Receive);
        app.complete_step(Step::Materialize);
        app.complete_step(Step::Overlay);
        // Last step has no successor; finish.
        app.finish_ok("overlay 10.42.0.2");
        assert_eq!(app.phase, Phase::Done);
        assert!(app.steps.iter().all(|(_, s)| *s == StepState::Ok));
        assert_eq!(app.outcome.as_deref(), Some("overlay 10.42.0.2"));
    }

    #[test]
    fn failure_marks_active_step_failed_and_can_retry() {
        let mut app = App::new();
        app.begin_enroll();
        app.complete_step(Step::Validate); // Connect now active
        app.fail("tls handshake: fingerprint-mismatch");
        assert_eq!(app.phase, Phase::Failed);
        assert_eq!(app.steps[1].1, StepState::Failed);
        assert!(app.error.as_deref().unwrap().contains("mismatch"));
        // Retry returns to editing with a clean tracker.
        app.reset_to_editing();
        assert_eq!(app.phase, Phase::Editing);
        assert!(app.steps.iter().all(|(_, s)| *s == StepState::Pending));
        assert!(app.error.is_none());
    }

    #[test]
    fn no_typing_once_enrolling() {
        let mut app = App::new();
        app.token = GOOD_TOKEN.into();
        app.begin_enroll();
        app.push_char('z');
        assert_eq!(app.token, GOOD_TOKEN, "fields are frozen during enroll");
    }
}
