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
//! (BIRTHRIGHT-1 LizardFS/QNM-Shared, the ONBOARD-9 service manager), so the
//! wizard is a narrated UX layer, not a reimplementation.

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
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};

use mde_enroll::setup::{Screen, Wizard};
use mde_enroll::setup_action::{
    found_argv, is_active_argv, join_argv, peers_argv, run_streaming, SetupRole, WIZARD_SERVICES,
};

/// The action screens that collect one input value before running a verb.
fn screen_prompt(screen: Screen) -> Option<&'static str> {
    match screen {
        Screen::Create => Some("Mesh id (e.g. home-mesh), then Enter to found:"),
        Screen::Join => Some("Paste join token (mesh:…@ip:port#bearer?fp=…), then Enter:"),
        _ => None,
    }
}

fn main() -> anyhow::Result<()> {
    let configured = mde_role::load().is_ok();
    let mut wiz = Wizard::new(configured);
    // Default role for found/join: lighthouse when founding, workstation when
    // joining (matches the `mackesd` verb defaults); operator cycles with Tab.
    let mut role = SetupRole::Lighthouse;
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
    loop {
        terminal.draw(|f| draw(f, wiz, *role, input))?;
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
            Screen::Status | Screen::Manage => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    wiz.back_to_menu();
                }
                KeyCode::Char('r') => {
                    if wiz.screen == Screen::Status {
                        run_status(wiz);
                    } else {
                        run_peers(wiz);
                    }
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
        SetupRole::Lighthouse => SetupRole::Server,
        SetupRole::Server => SetupRole::Workstation,
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
    wiz.push_log(if ok {
        "✓ mesh founded — services enabled + QNM-Shared up. Share the join line above.".to_string()
    } else {
        "✗ found failed — see the log above.".to_string()
    });
}

fn run_join(wiz: &mut Wizard, token: &str, role: SetupRole) {
    wiz.push_log(format!("joining as {}…", role.as_arg()));
    let argv = join_argv(token, role);
    let mut lines = Vec::new();
    let ok = run_streaming(&argv, |l| lines.push(l));
    for l in lines {
        wiz.push_log(l);
    }
    wiz.push_log(if ok {
        "✓ joined — overlay up, services enabled, QNM-Shared mounted.".to_string()
    } else {
        "✗ join failed — see the log above.".to_string()
    });
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

fn run_status(wiz: &mut Wizard) {
    let role = mde_role::load()
        .map(|r| r.to_string())
        .unwrap_or_else(|_| "unpinned".to_string());
    wiz.push_log(format!("— status — role: {role} —"));
    for unit in WIZARD_SERVICES {
        let mut state = String::from("unknown");
        run_streaming(&is_active_argv(unit), |l| state = l);
        wiz.push_log(format!("{unit:<22} {state}"));
    }
}

// ── render ──────────────────────────────────────────────────────────────────

fn draw(f: &mut Frame, wiz: &Wizard, role: SetupRole, input: &str) {
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
            "Magic Mesh — Setup",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("   [{configured}]")),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    match wiz.screen {
        Screen::Menu => draw_menu(f, wiz, chunks[1]),
        _ => draw_screen(f, wiz, role, input, chunks[1]),
    }

    let help = match wiz.screen {
        Screen::Menu => "↑/↓ move · Enter select · q quit",
        Screen::Create | Screen::Join => "type value · Tab cycle role · Enter run · Esc back",
        Screen::Status | Screen::Manage => "r refresh · Esc back",
    };
    f.render_widget(
        Paragraph::new(help).block(Block::default().borders(Borders::ALL)),
        chunks[2],
    );
}

fn draw_menu(f: &mut Frame, wiz: &Wizard, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = wiz
        .menu_items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let marker = if i == wiz.menu_index { "▶ " } else { "  " };
            let style = if i == wiz.menu_index {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            ListItem::new(format!("{marker}{}", item.label())).style(style)
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
    area: ratatui::layout::Rect,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(area);

    // Input prompt (Create/Join) or a screen heading.
    if let Some(prompt) = screen_prompt(wiz.screen) {
        let line = format!("{prompt}\n> {input}_   (role: {})", role.as_arg());
        f.render_widget(
            Paragraph::new(line).block(Block::default().borders(Borders::ALL).title("Input")),
            rows[0],
        );
    } else {
        let title = match wiz.screen {
            Screen::Status => "Status & services",
            Screen::Manage => "Peers & lighthouses",
            _ => "",
        };
        f.render_widget(
            Paragraph::new("press r to refresh")
                .block(Block::default().borders(Borders::ALL).title(title)),
            rows[0],
        );
    }

    // Live-log pane (newest lines, bounded to the visible height).
    let height = rows[1].height.saturating_sub(2) as usize;
    let start = wiz.log.len().saturating_sub(height.max(1));
    let log_lines: Vec<Line> = wiz.log[start..]
        .iter()
        .map(|l| Line::from(l.as_str()))
        .collect();
    f.render_widget(
        Paragraph::new(log_lines).block(Block::default().borders(Borders::ALL).title("Log")),
        rows[1],
    );
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
