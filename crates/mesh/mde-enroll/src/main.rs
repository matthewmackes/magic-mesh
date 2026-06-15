//! ONBOARD-5 — `mde-enroll`, the Magic enrollment TUI.
//!
//! A full-screen ratatui app where the operator pastes the join token
//! printed by `mackesd found` (optionally overriding the lighthouse IP)
//! and watches the fingerprint-pinned network enroll run, step by step.
//! Works headless over SSH — lighthouses/servers have no display, so a
//! terminal UI is the right surface (not a libcosmic window).
//!
//! The state machine lives in [`app`]; this file is the terminal I/O
//! shell: raw-mode setup, the crossterm event loop, the ratatui render,
//! and a worker thread that drives the real enroll stages
//! ([`mackesd_core::nebula_enroll_client`]) and reports progress over a
//! channel.

mod app;

use std::io::{self, Stdout};
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Frame, Terminal};

use app::{App, Field, Phase, Step, StepState};
use mackesd_core::nebula_enroll::JoinToken;

/// Messages the enroll worker sends back to the UI loop.
enum EnrollMsg {
    Step(Step),
    Done(String),
    Failed(String),
}

fn main() -> anyhow::Result<()> {
    let mut terminal = setup_terminal()?;
    let res = run(&mut terminal);
    restore_terminal(&mut terminal)?;
    res
}

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn setup_terminal() -> anyhow::Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Tui) -> anyhow::Result<()> {
    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run(terminal: &mut Tui) -> anyhow::Result<()> {
    let mut app = App::new();
    // Channel is live only while a worker is running.
    let mut rx: Option<mpsc::Receiver<EnrollMsg>> = None;

    loop {
        terminal.draw(|f| render(f, &app))?;

        // Drain any worker progress (non-blocking).
        if let Some(receiver) = &rx {
            while let Ok(msg) = receiver.try_recv() {
                match msg {
                    EnrollMsg::Step(s) => app.complete_step(s),
                    EnrollMsg::Done(summary) => {
                        app.finish_ok(summary);
                        rx = None;
                        break;
                    }
                    EnrollMsg::Failed(e) => {
                        app.fail(e);
                        rx = None;
                        break;
                    }
                }
            }
        }

        // Poll input with a short timeout so progress keeps flowing.
        if event::poll(Duration::from_millis(120))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match (app.phase, key.code) {
                    // Quit from any non-running phase.
                    (Phase::Editing | Phase::Done | Phase::Failed, KeyCode::Esc)
                    | (Phase::Editing | Phase::Done | Phase::Failed, KeyCode::Char('q')) => {
                        app.should_quit = true;
                    }
                    (Phase::Editing, KeyCode::Tab) => app.toggle_focus(),
                    (Phase::Editing, KeyCode::Backspace) => app.backspace(),
                    (Phase::Editing, KeyCode::Char(c)) => app.push_char(c),
                    (Phase::Editing, KeyCode::Enter) => match app.validated_token() {
                        Ok(token) => {
                            app.begin_enroll();
                            rx = Some(spawn_enroll(token));
                        }
                        Err(e) => app.error = Some(e),
                    },
                    // Retry from a terminal state.
                    (Phase::Done | Phase::Failed, KeyCode::Char('r')) => app.reset_to_editing(),
                    _ => {}
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Spawn the worker thread that runs the real enroll stages and reports
/// progress. Returns the receiver the UI loop polls.
fn spawn_enroll(token: JoinToken) -> mpsc::Receiver<EnrollMsg> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Token already validated by the caller.
        let _ = tx.send(EnrollMsg::Step(Step::Validate));

        let node_id = node_id();
        let display_name = node_id
            .strip_prefix("peer:")
            .unwrap_or(&node_id)
            .to_string();
        let root = mackesd_core::default_qnm_shared_root();
        let config_dir = std::path::PathBuf::from("/etc/nebula");

        let fp = match &token.fp {
            Some(fp) => fp.clone(),
            None => {
                let _ = tx.send(EnrollMsg::Failed("token lost its fingerprint".into()));
                return;
            }
        };
        let lighthouse = token.lighthouse.clone();
        let port = token.port;

        let identity = mackesd_core::enrollment::build_identity();
        let pending =
            mackesd_core::nebula_enroll::build_pending(&identity, &node_id, &display_name, token);
        let csr_json = match serde_json::to_vec(&pending) {
            Ok(b) => b,
            Err(e) => {
                let _ = tx.send(EnrollMsg::Failed(format!("encode CSR: {e}")));
                return;
            }
        };

        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(EnrollMsg::Failed(format!("runtime: {e}")));
                return;
            }
        };

        let _ = tx.send(EnrollMsg::Step(Step::Connect));
        let bundle =
            match runtime.block_on(mackesd_core::nebula_enroll_client::enroll_over_network(
                &lighthouse,
                port,
                &fp,
                &csr_json,
            )) {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx.send(EnrollMsg::Failed(e.to_string()));
                    return;
                }
            };
        let _ = tx.send(EnrollMsg::Step(Step::Receive));

        if let Err(e) = mackesd_core::nebula_enroll_client::persist_bundle(
            &root,
            &config_dir,
            &node_id,
            &bundle,
        ) {
            let _ = tx.send(EnrollMsg::Failed(e.to_string()));
            return;
        }
        let _ = tx.send(EnrollMsg::Step(Step::Materialize));

        // Best-effort overlay bring-up.
        let _ = std::process::Command::new("systemctl")
            .args(["start", "nebula.service"])
            .status();
        let _ = tx.send(EnrollMsg::Step(Step::Overlay));

        let _ = tx.send(EnrollMsg::Done(format!(
            "joined `{}` as {} (overlay {})",
            bundle.mesh_id, node_id, bundle.overlay_ip
        )));
    });
    rx
}

/// Resolve this box's stable node id (mirrors the daemon's resolution).
fn node_id() -> String {
    if let Ok(v) = std::env::var("MACKESD_NODE_ID") {
        return v;
    }
    let host = std::env::var("HOSTNAME").ok().or_else(|| {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_owned())
    });
    match host {
        Some(h) if !h.is_empty() => format!("peer:{h}"),
        _ => "peer:unknown".to_owned(),
    }
}

fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Length(3), // lighthouse field
            Constraint::Length(3), // token field
            Constraint::Min(7),    // steps
            Constraint::Length(3), // status strip
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    render_title(f, chunks[0]);
    render_field(
        f,
        chunks[1],
        "Lighthouse IP (optional — overrides token)",
        &app.lighthouse,
        app.phase == Phase::Editing && app.focus == Field::Lighthouse,
    );
    render_field(
        f,
        chunks[2],
        "Join token  (paste from `mackesd found`)",
        &app.token,
        app.phase == Phase::Editing && app.focus == Field::Token,
    );
    render_steps(f, chunks[3], app);
    render_status(f, chunks[4], app);
    render_footer(f, chunks[5], app);
}

fn render_title(f: &mut Frame, area: Rect) {
    let p = Paragraph::new(Line::from(vec![Span::styled(
        " Magic Mesh — join a mesh ",
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(p, area);
}

fn render_field(f: &mut Frame, area: Rect, label: &str, value: &str, focused: bool) {
    let border = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let shown = if focused {
        format!("{value}\u{2588}") // block cursor
    } else {
        value.to_string()
    };
    let p = Paragraph::new(shown).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .title(label),
    );
    f.render_widget(p, area);
}

fn render_steps(f: &mut Frame, area: Rect, app: &App) {
    let mut lines = Vec::new();
    for (step, state) in &app.steps {
        let (glyph, color) = match state {
            StepState::Pending => ("[ ]", Color::DarkGray),
            StepState::Active => ("[>]", Color::Yellow),
            StepState::Ok => ("[\u{2713}]", Color::Green),
            StepState::Failed => ("[\u{2717}]", Color::Red),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{glyph} "), Style::default().fg(color)),
            Span::styled(step.label(), Style::default().fg(color)),
        ]));
    }
    let p = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Progress"));
    f.render_widget(p, area);
}

fn render_status(f: &mut Frame, area: Rect, app: &App) {
    let (text, style) = match app.phase {
        Phase::Editing => (
            app.error
                .clone()
                .unwrap_or_else(|| "Paste the token, then press Enter to join.".to_string()),
            if app.error.is_some() {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
        ),
        Phase::Enrolling => ("Enrolling…".to_string(), Style::default().fg(Color::Yellow)),
        Phase::Done => (
            app.outcome.clone().unwrap_or_else(|| "Done.".to_string()),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Phase::Failed => (
            app.error
                .clone()
                .unwrap_or_else(|| "Enrollment failed.".to_string()),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    };
    let p = Paragraph::new(text)
        .style(style)
        .block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(p, area);
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    let hint = match app.phase {
        Phase::Editing => "Tab: switch field   Enter: join   q/Esc: quit",
        Phase::Enrolling => "Enrolling… please wait",
        Phase::Done | Phase::Failed => "r: retry   q/Esc: quit",
    };
    let p = Paragraph::new(Line::from(Span::styled(
        hint,
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(p, area);
}
