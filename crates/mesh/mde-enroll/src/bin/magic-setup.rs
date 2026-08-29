//! SETUP-1/2/3/5 — `magic-setup`, the full-lifecycle mesh wizard.
//!
//! A full-screen ratatui app that takes a freshly-installed node from zero to a
//! running mesh member: Create a mesh, Join one, Manage peers, or check Status —
//! narrating each step in a live-log pane. Headless over SSH (lighthouses/
//! servers have no display). The pure model is [`mde_enroll::setup`]; the verb
//! actions are [`mde_enroll::setup_action`]; this file is the terminal shell.
//!
//! Each action screen uses one input field + the shared live-log pane: type the
//! value (mesh-id / token), press Enter to run the verb (output streams into the
//! log), Esc returns to the menu. The verbs already provision everything
//! (the substrate — etcd + Syncthing — via setup-etcd/setup-syncthing, the
//! ONBOARD-9 service manager), so the wizard is a narrated UX layer, not a
//! reimplementation.

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use mde_enroll::commissioning_view::JoinTokenView;
use mde_enroll::setup::{Screen, Wizard};
use mde_enroll::setup_action::{
    add_peer_argv, found_argv, is_active_argv, join_argv, peers_argv, remove_peer_argv,
    run_streaming, self_test_argv, SetupRole,
};
use mde_enroll::wizard_status::{grouped_plane_installed, status_units};

/// The action screens that collect one input value before running a verb.
fn screen_prompt(screen: Screen) -> Option<&'static str> {
    match screen {
        Screen::Create => Some("Mesh id (e.g. home-mesh), then Enter to found:"),
        Screen::Join => Some("Paste join token (mesh:…@ip:port#bearer?fp=…), then Enter:"),
        _ => None,
    }
}

/// Plain-language help shown above the input field on the Create screen so a
/// first-time operator knows what a mesh-id is and what founding does (§46: a
/// mesh-of-one is already a complete network).
const CREATE_HELP: &[&str] = &[
    "Create a brand-new private mesh. This machine becomes the founder — it",
    "mints the mesh CA and signs every node that joins later. The mesh-id is a",
    "short name for your network (e.g. home-mesh). Just this one node is already",
    "a complete, working mesh; grow it by sharing a join token (Manage → add",
    "peer). Tab switches the founding role: Workstation founds + holds the CA.",
];

/// Plain-language help shown above the input field on the Join screen: where the
/// token comes from and its shape (design §7/§9 — any enrolled node mints one).
const JOIN_HELP: &[&str] = &[
    "Join an existing mesh. Paste a join token minted on any enrolled node via",
    "its Manage → \"add peer\" action. Format:",
    "  mesh:<id>@<ip>:<port>#<bearer>?fp=<fingerprint>",
    "This node enrolls behind that token, brings the overlay up, and mounts Mesh",
    "Sync. Tab switches this node's role (Workstation · Lighthouse).",
];

/// Add-peer can mint from any enrolled node, not only a founded lighthouse.
const ADD_PEER_FAILED: &str = "✗ add-peer failed — is this node enrolled?";

fn main() -> anyhow::Result<()> {
    let configured = mde_role::load().is_ok();
    let mut wiz = Wizard::new(configured);
    // Default role Workstation: the founder is a Workstation that becomes the
    // mesh CA (design §5/§6), and most nodes that Join are Workstations too. The
    // operator cycles with Tab (e.g. to found or add a Lighthouse).
    let mut role = SetupRole::Workstation;
    let mut input = String::new();

    let mut terminal = setup_terminal()?;
    let res = run(&mut terminal, &mut wiz, &mut role, &mut input);
    restore_terminal(&mut terminal)?;
    res
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    wiz: &mut Wizard,
    role: &mut SetupRole,
    input: &mut String,
) -> anyhow::Result<()> {
    // Manage screen "type a node-id to remove" sub-mode.
    let mut manage_removing = false;
    // Welcome/disclaimer scroll offset (the disclaimer is longer than one pane).
    let mut welcome_scroll: u16 = 0;
    loop {
        terminal.draw(|f| draw(f, wiz, *role, input, manage_removing, welcome_scroll))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match wiz.screen {
            Screen::Welcome => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    welcome_scroll = welcome_scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    welcome_scroll = welcome_scroll.saturating_add(1);
                }
                KeyCode::PageUp => welcome_scroll = welcome_scroll.saturating_sub(10),
                KeyCode::PageDown => welcome_scroll = welcome_scroll.saturating_add(10),
                KeyCode::Enter | KeyCode::Char(' ') => {
                    // §43: acknowledge the disclaimer, then open the menu. Record
                    // acceptance best-effort so the shell's other consumers see a
                    // consistent consent marker (harmless if $HOME is unwritable).
                    let _ = mde_disclaimer::record_acceptance();
                    wiz.acknowledge_welcome();
                }
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                _ => {}
            },
            Screen::Menu => match key.code {
                KeyCode::Up | KeyCode::Char('k') => wiz.menu_up(),
                KeyCode::Down | KeyCode::Char('j') => wiz.menu_down(),
                KeyCode::Enter => {
                    wiz.activate();
                    input.clear();
                    // Status/Manage run immediately on open (read-only).
                    match wiz.screen {
                        Screen::Status => run_status(wiz),
                        Screen::Manage => run_peers(wiz),
                        _ => {}
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                _ => {}
            },
            Screen::Create | Screen::Join => match key.code {
                KeyCode::Esc => {
                    wiz.back_to_menu();
                    input.clear();
                }
                KeyCode::Tab => *role = cycle_role(*role),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => input.push(c),
                KeyCode::Enter => {
                    let value = input.trim().to_string();
                    if value.is_empty() {
                        wiz.push_log("(enter a value first)".to_string());
                    } else if wiz.screen == Screen::Create {
                        run_create(wiz, &value, *role);
                    } else {
                        run_join(wiz, &value, *role);
                    }
                }
                _ => {}
            },
            Screen::Status => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => wiz.back_to_menu(),
                KeyCode::Char('r') => run_status(wiz),
                _ => {}
            },
            Screen::Lifecycle => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => wiz.back_to_menu(),
                _ => {}
            },
            Screen::Manage if manage_removing => match key.code {
                KeyCode::Esc => {
                    manage_removing = false;
                    input.clear();
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Enter => {
                    let target = input.trim().to_string();
                    if target.is_empty() {
                        wiz.push_log("(enter a node-id, e.g. peer:anvil)".to_string());
                    } else {
                        run_remove_peer(wiz, &target);
                    }
                    manage_removing = false;
                    input.clear();
                }
                KeyCode::Char(c) => input.push(c),
                _ => {}
            },
            Screen::Manage => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => wiz.back_to_menu(),
                KeyCode::Char('r') => run_peers(wiz),
                KeyCode::Char('a') => run_add_peer(wiz, *role),
                KeyCode::Char('l') => run_add_peer(wiz, SetupRole::Lighthouse),
                KeyCode::Tab => *role = cycle_role(*role),
                KeyCode::Char('d') => {
                    manage_removing = true;
                    input.clear();
                }
                _ => {}
            },
        }
        if wiz.should_quit {
            return Ok(());
        }
    }
}

fn cycle_role(r: SetupRole) -> SetupRole {
    match r {
        SetupRole::Lighthouse => SetupRole::Workstation,
        SetupRole::Workstation => SetupRole::Lighthouse,
    }
}

fn run_create(wiz: &mut Wizard, mesh_id: &str, role: SetupRole) {
    wiz.push_log(format!("founding mesh `{mesh_id}` as {}…", role.as_arg()));
    let argv = found_argv(mesh_id, "auto", role);
    let mut lines = Vec::new();
    let ok = run_streaming(&argv, |l| lines.push(l));
    for l in lines {
        wiz.push_log(l);
    }
    if ok {
        wiz.push_log(
            "✓ mesh founded — this node is a complete mesh-of-one; services enabled + Mesh Sync up."
                .to_string(),
        );
        run_self_test(wiz, role);
        wiz.push_log(
            "→ Next: share a join token from Manage → \"add peer\" to grow the mesh,".to_string(),
        );
        wiz.push_log("  then open the Mesh view to watch nodes appear.".to_string());
    } else {
        wiz.push_log("✗ found failed — see the log above.".to_string());
    }
}

fn run_join(wiz: &mut Wizard, token: &str, role: SetupRole) {
    // Attach withheld identity before the verb runs so the live-log header
    // fills from commissioning_lines(); the bearer never enters wizard state.
    present_join_paste(wiz, token);
    wiz.push_log(format!("joining as {}…", role.as_arg()));
    let argv = join_argv(token, role);
    let mut lines = Vec::new();
    let ok = run_streaming(&argv, |l| lines.push(l));
    for l in lines {
        wiz.push_log(redact_issued_line(&l));
    }
    if ok {
        wiz.push_log("✓ joined — overlay up, services enabled, Mesh Sync mounted.".to_string());
        run_self_test(wiz, role);
        wiz.push_log(
            "→ Next: open the Mesh view to see the network — this node is reachable".to_string(),
        );
        wiz.push_log("  at <host>.<mesh> over the overlay.".to_string());
    } else {
        wiz.push_log("✗ join failed — see the log above.".to_string());
    }
}

/// Post-Create/Join confirmation (§47): report each guaranteed service's state
/// green/red, then run the node self-diagnostic and narrate its verdict. A
/// mesh-of-one with no lighthouse is success, not a failure — the self-test
/// classifies the missing lighthouse as skipped, never red.
fn run_self_test(wiz: &mut Wizard, role: SetupRole) {
    wiz.push_log("— self-test: mesh services —".to_string());
    for unit in status_units(role, grouped_plane_installed()) {
        let mut state = String::from("unknown");
        run_streaming(&is_active_argv(&unit), |l| state = l);
        let glyph = if state == "active" { "✓" } else { "✗" };
        wiz.push_log(format!("{glyph} {unit:<22} {state}"));
    }
    wiz.push_log("— self-test: node diagnostic —".to_string());
    let mut lines = Vec::new();
    let ran = run_streaming(&self_test_argv(), |l| lines.push(l));
    if lines.is_empty() {
        wiz.push_log(if ran {
            "(self-test produced no output)".to_string()
        } else {
            "(node self-test unavailable — is mackesd installed + on PATH?)".to_string()
        });
    }
    for l in lines {
        wiz.push_log(l);
    }
}

fn run_peers(wiz: &mut Wizard) {
    wiz.push_log("— enrolled peers —".to_string());
    let mut lines = Vec::new();
    let ok = run_streaming(&peers_argv(), |l| lines.push(l));
    if lines.is_empty() {
        wiz.push_log("(no peers / directory empty)".to_string());
    }
    for l in lines {
        wiz.push_log(l);
    }
    if !ok {
        wiz.push_log("(could not read the directory — is mackesd running?)".to_string());
    }
}

fn run_add_peer(wiz: &mut Wizard, role: SetupRole) {
    wiz.push_log(format!(
        "minting a single-use join token for a {}…",
        role.as_arg()
    ));
    let mut lines = Vec::new();
    let ok = run_streaming(&add_peer_argv(role), |l| lines.push(l));
    present_issued_material(wiz, &lines, unix_now_ms());
    for l in lines {
        wiz.push_log(redact_issued_line(&l));
    }
    if ok {
        wiz.push_log(
            "↑ paste that token into the new node's `magic-setup` Join screen.".to_string(),
        );
    } else {
        wiz.push_log(ADD_PEER_FAILED.to_string());
    }
}

fn run_remove_peer(wiz: &mut Wizard, node_id: &str) {
    wiz.push_log(format!("removing {node_id}…"));
    let mut lines = Vec::new();
    let ok = run_streaming(&remove_peer_argv(node_id), |l| lines.push(l));
    for l in lines {
        wiz.push_log(l);
    }
    wiz.push_log(if ok {
        format!("✓ {node_id} removed (decommissioned + cert revoked + banned)")
    } else {
        format!("✗ remove {node_id} failed — see the log above")
    });
}

fn run_status(wiz: &mut Wizard) {
    let role = mde_role::load().unwrap_or(mde_role::Role::Workstation);
    let setup_role = match role {
        mde_role::Role::Lighthouse => SetupRole::Lighthouse,
        mde_role::Role::Workstation => SetupRole::Workstation,
    };
    wiz.push_log(format!("— status — role: {} —", role));
    for unit in status_units(setup_role, grouped_plane_installed()) {
        let mut state = String::from("unknown");
        run_streaming(&is_active_argv(&unit), |l| state = l);
        let glyph = if state == "active" { "✓" } else { "✗" };
        wiz.push_log(format!("{glyph} {unit:<22} {state}"));
    }
}

/// Join-paste path used by [`run_join`]: attach the withheld token view so
/// [`Wizard::commissioning_lines`] fills the live-log header. Failed present
/// leaves any existing view unchanged and never logs the raw paste.
fn present_join_paste(wiz: &mut Wizard, pasted: &str) {
    if wiz.present_join_token(pasted).is_err() {
        wiz.push_log("(join token identity not attached)".to_string());
    }
}

/// Add-peer / issue path used by [`run_add_peer`]: scan verb output for a
/// minted join token or commissioning capsule and attach the withheld views.
/// The bearer and capsule signature never enter wizard state.
fn present_issued_material(wiz: &mut Wizard, lines: &[String], now_ms: i64) {
    for line in lines {
        if let Some(token) = join_token_in_line(line) {
            let _ = wiz.present_join_token(token);
        }
        if let Some(json) = capsule_json_in_line(line) {
            let _ = wiz.present_capsule(json, now_ms);
        }
    }
}

fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// First parseable `mesh:…` wire form on `line`, if any.
fn join_token_in_line(line: &str) -> Option<&str> {
    let start = line.find("mesh:")?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '\'' || c == '"')
        .unwrap_or(rest.len());
    let candidate = rest.get(..end)?.trim_end_matches(['.', ',', ';', ')']);
    JoinTokenView::from_wire(candidate).ok()?;
    Some(candidate)
}

/// Single-line commissioning-capsule JSON on `line`, if any. Shape-only so an
/// expired envelope is still redacted; [`Wizard::present_capsule`] applies the
/// real `now_ms` bound.
fn capsule_json_in_line(line: &str) -> Option<&str> {
    let start = line.find('{')?;
    let candidate = line[start..].trim();
    if !(candidate.contains("capsule_id") && candidate.contains("signature_hex")) {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(candidate).ok()?;
    Some(candidate)
}

/// Operator-log copy of an issued line with the bearer / capsule signature
/// withheld. Identity belongs in [`Wizard::commissioning_lines`], not here.
fn redact_issued_line(line: &str) -> String {
    if let Some(token) = join_token_in_line(line) {
        return line.replacen(token, "mesh:… (bearer withheld)", 1);
    }
    if capsule_json_in_line(line).is_some() {
        return "capsule issued (signature withheld)".to_string();
    }
    line.to_string()
}

// ── render ──────────────────────────────────────────────────────────────────

fn draw(
    f: &mut Frame,
    wiz: &Wizard,
    role: SetupRole,
    input: &str,
    manage_removing: bool,
    welcome_scroll: u16,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Min(8),    // body (menu or screen)
            Constraint::Length(3), // footer/help
        ])
        .split(f.area());

    let configured = if wiz.configured {
        "configured"
    } else {
        "unconfigured"
    };
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            "MCNF — Setup",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("   [{configured}]")),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    match wiz.screen {
        Screen::Welcome => draw_welcome(f, welcome_scroll, chunks[1]),
        Screen::Menu => draw_menu(f, wiz, chunks[1]),
        _ => draw_screen(f, wiz, role, input, manage_removing, chunks[1]),
    }

    let help = match wiz.screen {
        Screen::Welcome => "↑/↓ scroll · Enter acknowledge & continue · q/Esc quit",
        Screen::Menu => "↑/↓ (or j/k) move · Enter open · q quit",
        Screen::Create | Screen::Join => "type value · Tab switch role · Enter run · Esc back",
        Screen::Status => "r refresh · Esc back",
        Screen::Lifecycle => "read-only session · Esc back",
        Screen::Manage if manage_removing => "type node-id · Enter remove · Esc cancel",
        Screen::Manage => {
            "a add peer · l add lighthouse · d remove · Tab role · r refresh · Esc back"
        }
    };
    f.render_widget(
        Paragraph::new(help).block(Block::default().borders(Borders::ALL)),
        chunks[2],
    );
}

/// The first-run Welcome + disclaimer gate (§43): a friendly intro over the
/// canonical `mde-disclaimer` text, scrollable, acknowledged with Enter.
fn draw_welcome(f: &mut Frame, scroll: u16, area: ratatui::layout::Rect) {
    let heading = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);

    let (disc_title, disc_body) = mde_disclaimer::split();

    let mut lines = vec![
        Line::styled("Welcome to the Mesh", heading),
        Line::raw(""),
        Line::raw("This wizard takes this machine from zero to a working private mesh."),
        Line::raw("You can Create a new mesh (this node founds it) or Join an existing"),
        Line::raw("one with a token another node shares with you."),
        Line::raw(""),
        Line::styled("Before you begin, please read and acknowledge:", heading),
        Line::raw(""),
        Line::styled(disc_title, Style::default().add_modifier(Modifier::BOLD)),
        Line::raw(""),
    ];
    lines.extend(disc_body.lines().map(|l| Line::raw(l.to_string())));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Press Enter to acknowledge and continue · ↑/↓ to scroll · q to quit.",
        dim,
    ));

    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Welcome & Disclaimer"),
            ),
        area,
    );
}

fn draw_menu(f: &mut Frame, wiz: &Wizard, area: ratatui::layout::Rect) {
    let dim = Style::default().fg(Color::DarkGray);
    let items: Vec<ListItem> = wiz
        .menu_items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let selected = i == wiz.menu_index;
            let marker = if selected { "▶ " } else { "  " };
            let label_style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            // Two lines per entry: the bold label, then a dim one-line
            // description so a first-time operator can tell the actions apart.
            ListItem::new(vec![
                Line::from(vec![
                    Span::raw(marker),
                    Span::styled(item.label(), label_style),
                ]),
                Line::styled(format!("    {}", item.description()), dim),
            ])
        })
        .collect();
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Menu")),
        area,
    );
}

fn draw_screen(
    f: &mut Frame,
    wiz: &Wizard,
    role: SetupRole,
    input: &str,
    manage_removing: bool,
    area: ratatui::layout::Rect,
) {
    // Create/Join carry a few lines of guidance above the field, so their top
    // block is taller; the read-only screens keep the compact one-line header.
    let top_h = match wiz.screen {
        Screen::Create | Screen::Join => 10,
        _ => 3,
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(top_h), Constraint::Min(4)])
        .split(area);

    let dim = Style::default().fg(Color::DarkGray);
    let role_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    // Input prompt (Create/Join, or Manage remove-mode) or a screen heading.
    if let Some(prompt) = screen_prompt(wiz.screen) {
        let help: &[&str] = if wiz.screen == Screen::Create {
            CREATE_HELP
        } else {
            JOIN_HELP
        };
        let mut lines: Vec<Line> = help.iter().map(|h| Line::styled(*h, dim)).collect();
        lines.push(Line::raw(""));
        lines.push(Line::raw(prompt));
        lines.push(Line::from(vec![
            Span::raw(format!("> {input}_")),
            Span::styled(format!("   (role: {})", role.as_arg()), role_style),
        ]));
        f.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Input")),
            rows[0],
        );
    } else if wiz.screen == Screen::Manage && manage_removing {
        let line = format!("Remove which peer? node-id, then Enter:\n> {input}_");
        f.render_widget(
            Paragraph::new(line).block(Block::default().borders(Borders::ALL).title("Remove peer")),
            rows[0],
        );
    } else {
        let (title, hint) = match wiz.screen {
            Screen::Status => ("Status & services", "press r to refresh".to_string()),
            Screen::Lifecycle => ("Lifecycle session", wiz.lifecycle_lines().join(" · ")),
            Screen::Manage => (
                "Peers & lighthouses",
                format!(
                    "a add peer · l add lighthouse · d remove (role: {})",
                    role.as_arg()
                ),
            ),
            _ => ("", String::new()),
        };
        f.render_widget(
            Paragraph::new(hint).block(Block::default().borders(Borders::ALL).title(title)),
            rows[0],
        );
    }

    // Live-log pane (newest lines, bounded to the visible height). Lines are
    // tinted by their leading glyph / self-test tag so the green/red verdict
    // reads at a glance. Token/capsule facts sit in the log header via
    // Wizard::commissioning_lines — the bearer and capsule signature stay
    // withheld. Empty lines mean nothing is attached; do not invent ready.
    let height = rows[1].height.saturating_sub(2) as usize;
    let header_len = wiz.commissioning_lines().len();
    let log_lines: Vec<Line> = live_log_lines(wiz, height)
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            let style = if i < header_len {
                Style::default().fg(Color::Cyan)
            } else {
                log_line_style(&l)
            };
            Line::styled(l, style)
        })
        .collect();
    f.render_widget(
        Paragraph::new(log_lines).block(Block::default().borders(Borders::ALL).title("Log")),
        rows[1],
    );
}

/// Live-log rows: commissioning identity first, then the newest operator-log
/// lines that fit `visible`. No ready flag — an empty
/// [`Wizard::commissioning_lines`] list just leaves the pane as the log.
fn live_log_lines(wiz: &Wizard, visible: usize) -> Vec<String> {
    let header = wiz.commissioning_lines();
    let body_slots = visible.saturating_sub(header.len()).max(1);
    let start = wiz.log.len().saturating_sub(body_slots);
    let mut lines = header;
    lines.extend(wiz.log.iter().skip(start).cloned());
    lines
}

/// Tint a live-log line by its meaning: green for a pass (`✓` / self-test
/// `[ok]`), red for a failure (`✗` / `[FAIL]` / `FAILED`), cyan for a
/// step/next-step marker (`—` / `→` / `↑`), yellow for a soft status
/// (`[warn]` / `[gated]` / `[skip]`), default otherwise.
fn log_line_style(line: &str) -> Style {
    let t = line.trim_start();
    if t.starts_with('✓') || t.contains("[ok]") {
        Style::default().fg(Color::Green)
    } else if t.starts_with('✗') || t.contains("[FAIL]") || t.contains("FAILED") {
        Style::default().fg(Color::Red)
    } else if t.starts_with('—') || t.starts_with('→') || t.starts_with('↑') {
        Style::default().fg(Color::Cyan)
    } else if t.contains("[warn]") || t.contains("[gated]") || t.contains("[skip]") {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    }
}

// ── terminal lifecycle ────────────────────────────────────────────────────────

fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> anyhow::Result<()> {
    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_enroll::commissioning_view::{CapsuleView, JoinTokenView};

    #[test]
    fn live_log_binds_commissioning_lines_without_secrets() {
        let mut wiz = Wizard::new(false);
        let bearer = "single-use-bearer";
        let token = format!("mesh:home@10.0.0.5:4243#{bearer}?fp={}", "a".repeat(64));
        wiz.set_token_view(JoinTokenView::from_wire(&token).unwrap());
        let signature = "c".repeat(128);
        let capsule = serde_json::json!({
            "schema_version": 1,
            "capsule_id": "capsule-1",
            "target_id": "seat-15",
            "expires_at_ms": 2_000,
            "bootstrap_digest_hex": "b".repeat(64),
            "one_time": true,
            "key_id": "commissioning-v1",
            "signature_hex": signature,
        })
        .to_string();
        wiz.set_capsule_view(CapsuleView::from_wire(&capsule, 1_000).unwrap());
        wiz.push_log("→ Join an existing mesh".to_string());

        let lines = live_log_lines(&wiz, 8);
        assert!(
            lines.iter().any(|l| l.contains("bearer withheld")),
            "token identity missing from log header: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("signature withheld")),
            "capsule identity missing from log header: {lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("→ Join an existing mesh")));
        assert!(
            !lines.iter().any(|l| l.contains(bearer)),
            "wizard live-log leaked the bearer: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains(&signature)),
            "wizard live-log leaked the capsule signature: {lines:?}"
        );
    }

    #[test]
    fn join_paste_and_add_peer_issue_present_without_leaking_bearer() {
        let bearer = "single-use-bearer";
        let token = format!("mesh:home@10.0.0.5:4243#{bearer}?fp={}", "a".repeat(64));
        let signature = "c".repeat(128);
        let capsule = serde_json::json!({
            "schema_version": 1,
            "capsule_id": "capsule-1",
            "target_id": "seat-15",
            "expires_at_ms": 2_000,
            "bootstrap_digest_hex": "b".repeat(64),
            "one_time": true,
            "key_id": "commissioning-v1",
            "signature_hex": signature,
        })
        .to_string();

        let mut pasted = Wizard::new(false);
        assert!(pasted.commissioning_lines().is_empty());
        present_join_paste(&mut pasted, &token);
        pasted.push_log("joining as workstation…".to_string());
        let paste_lines = live_log_lines(&pasted, 8);
        assert!(
            paste_lines.iter().any(|l| l.contains("bearer withheld")),
            "Join paste must call present_join_token: {paste_lines:?}"
        );
        assert!(
            !paste_lines.iter().any(|l| l.contains(bearer)),
            "Join paste leaked the bearer: {paste_lines:?}"
        );
        assert!(
            !format!("{pasted:?}").contains(bearer),
            "wizard debug leaked the bearer after Join paste"
        );

        let mut refuse = Wizard::new(false);
        present_join_paste(&mut refuse, "{{JOIN_TOKEN}}");
        present_join_paste(&mut refuse, "garbage");
        assert!(
            refuse.commissioning_lines().is_empty(),
            "failed present must leave commissioning_lines empty: {:?}",
            refuse.commissioning_lines()
        );
        assert!(
            !refuse
                .log
                .iter()
                .any(|l| l.contains("{{JOIN_TOKEN}}") || l.contains("garbage")),
            "failed present logged the raw paste: {:?}",
            refuse.log
        );

        let mut issued = Wizard::new(true);
        assert!(issued.commissioning_lines().is_empty());
        let verb_out = vec![
            token.clone(),
            format!("or:  mackesd join '{token}' --role workstation"),
            capsule.clone(),
            "single-use v3 token minted (SETUP-5)".to_string(),
        ];
        present_issued_material(&mut issued, &verb_out, 1_000);
        let header = issued.commissioning_lines();
        assert!(
            header.iter().any(|l| l.contains("bearer withheld")),
            "add-peer issue must call present_join_token: {header:?}"
        );
        assert!(
            header.iter().any(|l| l.contains("signature withheld")),
            "add-peer issue must call present_capsule: {header:?}"
        );
        assert!(
            !header
                .iter()
                .any(|l| l.contains(bearer) || l.contains(&signature)),
            "issued commissioning_lines leaked a secret: {header:?}"
        );

        let redacted: Vec<String> = verb_out.iter().map(|l| redact_issued_line(l)).collect();
        for line in &redacted {
            issued.push_log(line.clone());
        }
        let live = live_log_lines(&issued, 12);
        assert!(
            !live.iter().any(|l| l.contains(bearer)),
            "add-peer live-log leaked the bearer: {live:?}"
        );
        assert!(
            !live.iter().any(|l| l.contains(&signature)),
            "add-peer live-log leaked the capsule signature: {live:?}"
        );
        assert!(redacted.iter().any(|l| l.contains("bearer withheld")));
        assert!(redacted.iter().any(|l| l.contains("signature withheld")));
    }

    #[test]
    fn add_peer_failure_copy_is_not_lighthouse_only() {
        assert!(ADD_PEER_FAILED.contains("enrolled"));
        assert!(
            !ADD_PEER_FAILED.to_ascii_lowercase().contains("lighthouse"),
            "add-peer must not imply only a founded lighthouse can mint: {ADD_PEER_FAILED}"
        );
    }
}
