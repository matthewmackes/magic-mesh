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

use mde_enroll::setup::{Screen, Wizard};
use mde_enroll::setup_action::{
    add_peer_argv, found_argv, is_active_argv, join_argv, peers_argv, remove_peer_argv,
    run_streaming, self_test_argv, SetupRole, WIZARD_SERVICES,
};

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
        run_self_test(wiz);
        wiz.push_log(
            "→ Next: share a join token from Manage → \"add peer\" to grow the mesh,".to_string(),
        );
        wiz.push_log("  then open the Mesh view to watch nodes appear.".to_string());
    } else {
        wiz.push_log("✗ found failed — see the log above.".to_string());
    }
}

fn run_join(wiz: &mut Wizard, token: &str, role: SetupRole) {
    wiz.push_log(format!("joining as {}…", role.as_arg()));
    let argv = join_argv(token, role);
    let mut lines = Vec::new();
    let ok = run_streaming(&argv, |l| lines.push(l));
    for l in lines {
        wiz.push_log(l);
    }
    if ok {
        wiz.push_log("✓ joined — overlay up, services enabled, Mesh Sync mounted.".to_string());
        run_self_test(wiz);
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
fn run_self_test(wiz: &mut Wizard) {
    wiz.push_log("— self-test: mesh services —".to_string());
    for unit in WIZARD_SERVICES {
        let mut state = String::from("unknown");
        run_streaming(&is_active_argv(unit), |l| state = l);
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
    for l in lines {
        wiz.push_log(l);
    }
    if ok {
        wiz.push_log(
            "↑ paste that token into the new node's `magic-setup` Join screen.".to_string(),
        );
    } else {
        wiz.push_log("✗ add-peer failed — is this a founded lighthouse?".to_string());
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
    let role = mde_role::load()
        .map(|r| r.to_string())
        .unwrap_or_else(|_| "unpinned".to_string());
    wiz.push_log(format!("— status — role: {role} —"));
    for unit in WIZARD_SERVICES {
        let mut state = String::from("unknown");
        run_streaming(&is_active_argv(unit), |l| state = l);
        let glyph = if state == "active" { "✓" } else { "✗" };
        wiz.push_log(format!("{glyph} {unit:<22} {state}"));
    }
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
    // reads at a glance.
    let height = rows[1].height.saturating_sub(2) as usize;
    let start = wiz.log.len().saturating_sub(height.max(1));
    let log_lines: Vec<Line> = wiz.log[start..]
        .iter()
        .map(|l| Line::styled(l.as_str(), log_line_style(l)))
        .collect();
    f.render_widget(
        Paragraph::new(log_lines).block(Block::default().borders(Borders::ALL).title("Log")),
        rows[1],
    );
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
