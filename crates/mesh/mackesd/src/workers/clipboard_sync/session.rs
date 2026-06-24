//! CLIP-SYNC-2 — active graphical-session discovery for the system daemon.
//!
//! `mackesd` runs as a **root system service** (`mackesd.service`,
//! `WantedBy=multi-user.target`) — it has **no `$WAYLAND_DISPLAY`** in its
//! environment, because it is not part of any user's graphical session. The
//! clipboard-sync worker's capture command (`wl-paste --watch`) needs a live
//! Wayland session to attach to, so before this module the worker idled
//! forever on the system daemon (`$WAYLAND_DISPLAY unset; idling`) and the
//! mesh-global `history.json` was never written → the Notification-Center
//! Clipboard Viewer showed "Clipboard history is empty." on every Workstation.
//! (Found live on Eagle, 2026-06-24.)
//!
//! This module discovers the active graphical session the daemon should drive:
//!
//!   1. `loginctl list-sessions` → the graphical session (`Type=wayland`,
//!      `State=active`/`online`) owned by a **regular login uid** (a `gdm`
//!      greeter, uid 42, never out-scores the operator), preferring `seat0` +
//!      `active`. Its `RuntimePath` (`XDG_RUNTIME_DIR`) is read, not assumed.
//!   2. The `wayland-*` socket inside that runtime dir becomes `$WAYLAND_DISPLAY`.
//!   3. The owning uid's **primary gid + `$HOME`** are resolved from
//!      `/etc/passwd` so the capture child is a COMPLETE credential drop (uid
//!      alone would leave it in root's group) with a correct `$HOME`.
//!   4. The worker spawns `wl-paste` as that uid/gid with `HOME` +
//!      `XDG_RUNTIME_DIR` + `WAYLAND_DISPLAY`, so the capture attaches to the
//!      operator's real desktop session, not root's.
//!
//! On a genuinely headless node (a Lighthouse/Server, or a Workstation before
//! the desktop comes up) there is no graphical session — discovery returns
//! `None` and the worker idles quietly, no error spam (graceful degrade).
//!
//! **Testability.** The `loginctl`/passwd parses + the socket pick are pure
//! functions (`parse_graphical_session`, `parse_passwd_for_uid`,
//! `pick_wayland_socket`) unit-tested without any `loginctl`/filesystem; the
//! thin I/O wrapper `discover` shells `loginctl` + reads the runtime dir and
//! `/etc/passwd` only at runtime.

use std::path::{Path, PathBuf};

/// Conventional floor for a regular (human) login uid on Fedora/systemd —
/// `UID_MIN`. System service accounts (e.g. `gdm` = 42) sit below it, so a
/// graphical greeter session never out-scores the operator's own session.
const REGULAR_UID_MIN: u32 = 1000;

/// The active graphical session the daemon will spawn the capture child into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphicalSession {
    /// Owning user's numeric uid (the capture child is spawned as this uid).
    pub uid: u32,
    /// Owning user's primary gid (the capture child is spawned with this gid so
    /// the privilege drop is complete — uid alone leaves root's gid/groups).
    pub gid: u32,
    /// The user's `$HOME` (so the child's XDG/config lookups resolve under the
    /// operator, not root's `/root`).
    pub home: PathBuf,
    /// `XDG_RUNTIME_DIR` for the session (logind's `RuntimePath`, normally
    /// `/run/user/<uid>`).
    pub runtime_dir: PathBuf,
    /// The `wayland-*` socket name inside `runtime_dir` (the `$WAYLAND_DISPLAY`
    /// value, e.g. `wayland-0` / `wayland-1`).
    pub wayland_display: String,
}

/// One row of the `loginctl list-sessions` machine output we care about.
///
/// We invoke `loginctl list-sessions --no-legend --no-pager -o json`, but to
/// stay dependency-free we don't pull a JSON-path crate — we parse the small,
/// stable subset of fields we need from each object. See [`parse_sessions`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionRow {
    session_id: String,
    uid: u32,
    seat: String,
}

/// Parse `loginctl list-sessions -o json` output into the rows we probe.
///
/// `loginctl … -o json` emits a JSON array of objects, each with at least
/// `session`, `uid`, `user`, `seat`. We only need `session`/`uid`/`seat`; an
/// object missing a field is skipped rather than erroring (forward-compat with
/// systemd adding/removing keys). Pure — no I/O.
fn parse_sessions(json: &str) -> Vec<SessionRow> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(arr) = value.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|obj| {
            let session_id = obj.get("session")?.as_str()?.to_string();
            // `uid` may serialize as a JSON number; tolerate a string form too.
            // `try_from` (not `as`) so a bogus out-of-range uid is dropped, not
            // silently truncated.
            let uid: u32 = obj
                .get("uid")
                .and_then(|u| {
                    u.as_u64()
                        .or_else(|| u.as_str().and_then(|s| s.parse().ok()))
                })
                .and_then(|n| u32::try_from(n).ok())?;
            // `seat` is sometimes null/absent for a non-seated session.
            let seat = obj
                .get("seat")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            Some(SessionRow {
                session_id,
                uid,
                seat,
            })
        })
        .collect()
}

/// One session's relevant `loginctl show-session` properties, already parsed
/// from the `KEY=VALUE` lines `loginctl show-session <id>` prints.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct SessionProps {
    /// `Type=` — `wayland` is the one we can drive `wl-paste` against.
    session_type: String,
    /// `State=` — `active` (foreground) or `online` (backgrounded but live).
    state: String,
    /// `RuntimePath=` — the session's real `XDG_RUNTIME_DIR` (normally
    /// `/run/user/<uid>`, but read it rather than assume so a custom runtime
    /// dir still resolves). Empty when logind doesn't report it.
    runtime_path: String,
}

/// Parse the `KEY=VALUE` lines of `loginctl show-session <id>` for the
/// properties we gate on. Pure — unknown keys ignored. Whitespace-tolerant.
fn parse_session_props(show_output: &str) -> SessionProps {
    let mut props = SessionProps::default();
    for line in show_output.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("Type=") {
            props.session_type = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("State=") {
            props.state = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("RuntimePath=") {
            props.runtime_path = v.trim().to_string();
        }
    }
    props
}

/// Is this session one we can attach a Wayland clipboard watcher to?
/// `Type=wayland` and a live state (`active` or `online`). Pure.
fn is_drivable_graphical(props: &SessionProps) -> bool {
    props.session_type.eq_ignore_ascii_case("wayland")
        && (props.state.eq_ignore_ascii_case("active")
            || props.state.eq_ignore_ascii_case("online"))
}

/// The chosen graphical session's identity from the `loginctl` probe: the
/// owning uid and its `XDG_RUNTIME_DIR` (logind's `RuntimePath`, falling back
/// to `/run/user/<uid>` when logind doesn't report one).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChosenSession {
    uid: u32,
    runtime_dir: PathBuf,
}

/// Pure core of discovery: given the `list-sessions` JSON and a resolver that
/// yields each candidate session's `show-session` output, pick the active
/// graphical session.
///
/// Scoring (highest wins): a **regular login uid** (>= [`REGULAR_UID_MIN`])
/// outranks a system account so a `gdm` greeter never beats the operator; a
/// `seat0` session (the physical desktop) outranks a seatless one; an `active`
/// session outranks a merely `online` one. On an exact tie the first session
/// in `loginctl` order is kept (a single-user desktop has exactly one such
/// session, so ties don't arise in practice). Returns `None` when no session
/// is a drivable Wayland session (headless / pre-desktop).
fn parse_graphical_session<F>(list_json: &str, mut show_session: F) -> Option<ChosenSession>
where
    F: FnMut(&str) -> Option<String>,
{
    let rows = parse_sessions(list_json);
    let mut best: Option<(u8, ChosenSession)> = None;
    for row in rows {
        let Some(show) = show_session(&row.session_id) else {
            continue;
        };
        let props = parse_session_props(&show);
        if !is_drivable_graphical(&props) {
            continue;
        }
        // Score: regular-uid (weight 4) > seat0 (weight 2) > active (weight 1).
        // The regular-uid bit dominates so a system greeter (gdm=42) can never
        // out-score the operator's own session even if it is the active seat0 one.
        let uid_score = u8::from(row.uid >= REGULAR_UID_MIN);
        let seat_score = u8::from(row.seat == "seat0");
        let state_score = u8::from(props.state.eq_ignore_ascii_case("active"));
        let score = uid_score * 4 + seat_score * 2 + state_score;
        if best.as_ref().is_none_or(|(b, _)| score > *b) {
            let runtime_dir = if props.runtime_path.is_empty() {
                PathBuf::from(format!("/run/user/{}", row.uid))
            } else {
                PathBuf::from(&props.runtime_path)
            };
            best = Some((
                score,
                ChosenSession {
                    uid: row.uid,
                    runtime_dir,
                },
            ));
        }
    }
    best.map(|(_, chosen)| chosen)
}

/// Pick the Wayland display socket name from a runtime dir's entries.
///
/// `$WAYLAND_DISPLAY` is the basename of the compositor's socket in
/// `XDG_RUNTIME_DIR` — `wayland-0`, `wayland-1`, … We ignore the `.lock`
/// sidecar files Wayland writes beside each socket. Prefer the
/// lowest-numbered display (`wayland-0` is the primary on a single-GPU
/// desktop). Pure — takes the already-listed names.
fn pick_wayland_socket(entry_names: &[String]) -> Option<String> {
    // The display number for `wayland-<N>` (the socket), or `None` for anything
    // else — including the `.lock` sidecar (`wayland-0.lock` has a non-numeric
    // suffix, so it's filtered out) and unrelated entries (`bus`, `pipewire-0`).
    // Prefer the lowest-numbered display (`wayland-0` is the primary on a
    // single-GPU desktop); the min over the parsed number gives a stable pick.
    entry_names
        .iter()
        .filter_map(|n| {
            n.strip_prefix("wayland-")
                .and_then(|t| t.parse::<u32>().ok())
                .map(|num| (num, n))
        })
        .min_by_key(|(num, _)| *num)
        .map(|(_, n)| n.clone())
}

/// List the `wayland-*` socket names present in `runtime_dir` (I/O wrapper over
/// [`pick_wayland_socket`]). Returns an empty vec when the dir is unreadable.
fn list_runtime_dir(runtime_dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(runtime_dir) else {
        return Vec::new();
    };
    read.filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

/// Run `loginctl show-session <id>` and return its stdout (the `KEY=VALUE`
/// property dump). `None` on spawn/exec failure (loginctl absent / no logind).
fn loginctl_show_session(session_id: &str) -> Option<String> {
    let out = std::process::Command::new("loginctl")
        .args(["show-session", session_id])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

/// Run `loginctl list-sessions -o json` and return its stdout. `None` on
/// spawn/exec failure (no logind, e.g. a minimal container) — the caller then
/// degrades to "no graphical session", i.e. the worker idles.
fn loginctl_list_sessions() -> Option<String> {
    let out = std::process::Command::new("loginctl")
        .args(["list-sessions", "--no-legend", "--no-pager", "-o", "json"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

/// The `(gid, home)` of a passwd entry — what the capture child needs beyond
/// its uid for a complete credential drop + a correct `$HOME`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PasswdIdentity {
    gid: u32,
    home: PathBuf,
}

/// Parse `/etc/passwd` content for `uid`'s primary gid + home dir. Pure — the
/// `name:passwd:uid:gid:gecos:home:shell` colon format, skipping malformed
/// lines. `None` when the uid isn't present.
fn parse_passwd_for_uid(passwd: &str, uid: u32) -> Option<PasswdIdentity> {
    for line in passwd.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        // name:passwd:uid:gid:gecos:home:shell → 7 fields.
        if fields.len() < 7 {
            continue;
        }
        let Ok(row_uid) = fields[2].parse::<u32>() else {
            continue;
        };
        if row_uid != uid {
            continue;
        }
        let Ok(gid) = fields[3].parse::<u32>() else {
            continue;
        };
        return Some(PasswdIdentity {
            gid,
            home: PathBuf::from(fields[5]),
        });
    }
    None
}

/// Resolve `uid`'s primary gid + home from `/etc/passwd`. I/O wrapper over
/// [`parse_passwd_for_uid`]; `None` when `/etc/passwd` is unreadable or the
/// uid is absent (the caller then declines to spawn — a half-dropped child is
/// worse than idling).
fn passwd_identity(uid: u32) -> Option<PasswdIdentity> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    parse_passwd_for_uid(&passwd, uid)
}

/// Discover the active graphical session the daemon should spawn into.
///
/// Shells `loginctl`, reads the session's `XDG_RUNTIME_DIR`, and resolves the
/// owning uid's gid + home from `/etc/passwd`.
///
/// **Blocking** — forks `loginctl` and reads files; the async caller wraps it
/// in `spawn_blocking` so it never parks a tokio worker thread.
///
/// Returns `None` (worker idles quietly) when:
///   * `loginctl` is unavailable / there is no logind (minimal container), or
///   * no session is a drivable Wayland session (genuinely headless node), or
///   * the winning session has no `wayland-*` socket yet (desktop still coming
///     up — the worker retries on its respawn tick), or
///   * the owning uid has no `/etc/passwd` entry (we decline rather than spawn
///     a half-credentialed child).
#[must_use]
pub fn discover() -> Option<GraphicalSession> {
    let list = loginctl_list_sessions()?;
    let chosen = parse_graphical_session(&list, loginctl_show_session)?;
    let wayland_display = pick_wayland_socket(&list_runtime_dir(&chosen.runtime_dir))?;
    let id = passwd_identity(chosen.uid)?;
    Some(GraphicalSession {
        uid: chosen.uid,
        gid: id.gid,
        home: id.home,
        runtime_dir: chosen.runtime_dir,
        wayland_display,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sessions_extracts_id_uid_seat() {
        let json = r#"[
            {"session":"2","uid":1000,"user":"mm","seat":"seat0","leader":1234},
            {"session":"c1","uid":42,"user":"gdm","seat":"seat0"}
        ]"#;
        let rows = parse_sessions(json);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].session_id, "2");
        assert_eq!(rows[0].uid, 1000);
        assert_eq!(rows[0].seat, "seat0");
        assert_eq!(rows[1].uid, 42);
    }

    #[test]
    fn parse_sessions_tolerates_string_uid_and_missing_seat() {
        // Some logind builds emit uid as a string; a seatless session omits seat.
        let json = r#"[{"session":"x","uid":"1000","user":"mm"}]"#;
        let rows = parse_sessions(json);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].uid, 1000);
        assert_eq!(rows[0].seat, "");
    }

    #[test]
    fn parse_sessions_empty_on_garbage() {
        assert!(parse_sessions("not json").is_empty());
        assert!(parse_sessions("{}").is_empty()); // object, not an array
        assert!(parse_sessions("[]").is_empty());
    }

    #[test]
    fn parse_props_reads_type_state_and_runtime_path() {
        let show = "Id=2\nUser=1000\nName=mm\nType=wayland\nState=active\n\
                    RuntimePath=/run/user/1000\nSeat=seat0\n";
        let p = parse_session_props(show);
        assert_eq!(p.session_type, "wayland");
        assert_eq!(p.state, "active");
        assert_eq!(p.runtime_path, "/run/user/1000");
    }

    #[test]
    fn drivable_requires_wayland_and_live_state() {
        let props = |ty: &str, state: &str| SessionProps {
            session_type: ty.into(),
            state: state.into(),
            ..Default::default()
        };
        assert!(is_drivable_graphical(&props("wayland", "active")));
        assert!(
            is_drivable_graphical(&props("wayland", "online")),
            "online is still live"
        );
        assert!(
            !is_drivable_graphical(&props("x11", "active")),
            "x11 is not wl-paste-drivable"
        );
        assert!(!is_drivable_graphical(&props("wayland", "closing")));
    }

    #[test]
    fn pick_socket_prefers_lowest_numbered_and_ignores_locks() {
        let names = vec![
            "wayland-1".to_string(),
            "wayland-1.lock".to_string(),
            "wayland-0".to_string(),
            "wayland-0.lock".to_string(),
            "bus".to_string(),
            "pipewire-0".to_string(),
        ];
        assert_eq!(pick_wayland_socket(&names), Some("wayland-0".to_string()));
    }

    #[test]
    fn pick_socket_none_when_no_wayland_socket() {
        let names = vec!["bus".to_string(), "pulse".to_string()];
        assert_eq!(pick_wayland_socket(&names), None);
        // A bare lock file with no live socket → still None (display not up yet).
        let only_lock = vec!["wayland-0.lock".to_string()];
        assert_eq!(pick_wayland_socket(&only_lock), None);
    }

    #[test]
    fn graphical_session_picks_active_seat0_wayland_uid() {
        // Two sessions: gdm greeter (uid 42, online) + the operator (uid 1000,
        // active). Both seat0 + wayland → the operator (regular uid + active) wins.
        let list = r#"[
            {"session":"c1","uid":42,"seat":"seat0"},
            {"session":"2","uid":1000,"seat":"seat0"}
        ]"#;
        let show = |id: &str| {
            Some(match id {
                "c1" => "Type=wayland\nState=online\nRuntimePath=/run/user/42\n".to_string(),
                "2" => "Type=wayland\nState=active\nRuntimePath=/run/user/1000\n".to_string(),
                _ => String::new(),
            })
        };
        let chosen = parse_graphical_session(list, show).unwrap();
        assert_eq!(chosen.uid, 1000);
        assert_eq!(chosen.runtime_dir, PathBuf::from("/run/user/1000"));
    }

    #[test]
    fn graphical_session_regular_uid_beats_system_greeter_even_when_greeter_active() {
        // The Eagle-window bug: the gdm greeter (uid 42) is the ACTIVE seat0
        // wayland session while the operator's (uid 1000) is merely `online`.
        // Without the regular-uid preference the greeter would win and the
        // worker would watch the greeter's clipboard → operator clips lost.
        let list = r#"[
            {"session":"greeter","uid":42,"seat":"seat0"},
            {"session":"user","uid":1000,"seat":"seat0"}
        ]"#;
        let show = |id: &str| {
            Some(match id {
                "greeter" => "Type=wayland\nState=active\n".to_string(),
                "user" => "Type=wayland\nState=online\n".to_string(),
                _ => String::new(),
            })
        };
        assert_eq!(parse_graphical_session(list, show).unwrap().uid, 1000);
    }

    #[test]
    fn graphical_session_falls_back_to_run_user_when_no_runtime_path() {
        // logind didn't report RuntimePath → derive /run/user/<uid>.
        let list = r#"[{"session":"2","uid":1000,"seat":"seat0"}]"#;
        let show = |_: &str| Some("Type=wayland\nState=active\n".to_string());
        let chosen = parse_graphical_session(list, show).unwrap();
        assert_eq!(chosen.runtime_dir, PathBuf::from("/run/user/1000"));
    }

    #[test]
    fn graphical_session_prefers_seat0_over_seatless() {
        // A seatless active wayland session (e.g. a nested/headless compositor)
        // loses to the seat0 one — the physical desktop is what the operator sees.
        // (Both regular uids, so the seat0 bit is the tiebreak.)
        let list = r#"[
            {"session":"nested","uid":1001,"seat":""},
            {"session":"2","uid":1000,"seat":"seat0"}
        ]"#;
        let show = |_: &str| Some("Type=wayland\nState=active\n".to_string());
        assert_eq!(parse_graphical_session(list, show).unwrap().uid, 1000);
    }

    #[test]
    fn graphical_session_none_when_headless() {
        // tty / x11 only — nothing wl-paste can drive → None (worker idles).
        let list = r#"[
            {"session":"1","uid":1000,"seat":"seat0"},
            {"session":"3","uid":1000,"seat":""}
        ]"#;
        let show = |id: &str| {
            Some(match id {
                "1" => "Type=tty\nState=active\n".to_string(),
                "3" => "Type=x11\nState=active\n".to_string(),
                _ => String::new(),
            })
        };
        assert_eq!(parse_graphical_session(list, show), None);
    }

    #[test]
    fn graphical_session_none_on_empty_list() {
        assert_eq!(parse_graphical_session("[]", |_| Some(String::new())), None);
    }

    #[test]
    fn passwd_parse_reads_gid_and_home_for_uid() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                      gdm:x:42:42:GDM:/var/lib/gdm:/usr/sbin/nologin\n\
                      mm:x:1000:1001:Matthew:/home/mm:/bin/bash\n";
        let id = parse_passwd_for_uid(passwd, 1000).unwrap();
        assert_eq!(id.gid, 1001);
        assert_eq!(id.home, PathBuf::from("/home/mm"));
        // A different uid resolves independently.
        assert_eq!(parse_passwd_for_uid(passwd, 42).unwrap().gid, 42);
        // Absent uid → None.
        assert_eq!(parse_passwd_for_uid(passwd, 9999), None);
    }

    #[test]
    fn passwd_parse_skips_malformed_lines() {
        let passwd = "# a comment\n\
                      truncated:x:1000\n\
                      mm:x:1000:1001:Matthew:/home/mm:/bin/bash\n";
        let id = parse_passwd_for_uid(passwd, 1000).unwrap();
        assert_eq!(id.gid, 1001);
        assert_eq!(id.home, PathBuf::from("/home/mm"));
    }
}
