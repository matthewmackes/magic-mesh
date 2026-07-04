//! CURTAIN-2 — the seat-user authenticator behind the curtain's [`Verifier`]
//! seam (`docs/design/lock-curtain.md`, locks 1/9/10).
//!
//! CURTAIN-1 shipped the lock curtain with an honest deny-all default; this
//! module fills the seam with a REAL check of the seat user's **system
//! password** — the same credential store as SSH/sudo (lock 1), no parallel
//! password.
//!
//! ## Why `unix_chkpwd`, not a `pam` crate
//!
//! The design's first choice was a libpam binding (`pam` / `pam-sys`). The
//! airgapped-for-devel build farm was probed EARLY and **cannot build one**:
//! there is no `libpam.so` link symlink, no `pam-devel` headers, and no
//! `pkg-config` `.pc` for pam, so every `-sys` crate's `cargo:rustc-link-lib=pam`
//! fails to link (`ld: cannot find -lpam`), even though the crates themselves
//! resolve on the index. The design names the fallback for exactly this case:
//! shell out to `unix_chkpwd`.
//!
//! That is not a lesser check. `pam_unix` — the module a `login`/`system-auth`
//! stack authenticates against — **cannot read `/etc/shadow` from a non-root
//! process**, so it `exec`s the setuid-root `unix_chkpwd` helper to do the
//! comparison. Our DRM shell runs as the (non-root) seat user, so a real
//! `pam_authenticate` for the seat user would run this very helper underneath.
//! Calling it directly is therefore PAM's own password path, not a parallel one.
//!
//! ## The shape (design: OFF the render thread, honest verdicts)
//!
//! [`PamVerifier`] resolves the seat user once (`$USER`/`$LOGNAME`, else the
//! uid mapped through `/etc/passwd`) and, on each [`Verifier::begin`], spawns a
//! worker thread that runs the blocking helper spawn+wait and returns the
//! [`Verdict`] through a channel — [`Verifier::poll`] never blocks the egui
//! loop (the pairing-dialog channel-bridge pattern, [`crate::bt_pairing`]). The
//! helper's exit code maps to honest verdicts: a wrong password
//! ([`Verdict::Denied`] "incorrect") is kept DISTINCT from a service error or an
//! absent helper, so the operator is never told "wrong password" when auth is
//! actually broken, and the lock **never pretends to succeed** (§7).
//!
//! The real PAM work sits behind an injectable backend seam so unit tests drive
//! Granted/Denied/unavailable/dead-worker paths WITHOUT ever spawning the real
//! helper (the curtain machine's own backoff is exercised in `curtain.rs` over
//! this verifier with a scripted backend). The live grant leg — the seat user's
//! real password lifting the curtain — is verified on real hardware.

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private binary-crate module are this crate's \
              idiom (curtain.rs, chrome.rs, …); main.rs + curtain.rs consume them"
)]

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use crate::curtain::{Verdict, Verifier};

// ───────────────────────────── honest deny reasons ─────────────────────────────

/// The entered password did not match (`unix_chkpwd` exit 7 = `PAM_AUTH_ERR`) —
/// the honest "wrong password" line, kept distinct from a service failure.
const REASON_INCORRECT: &str = "Incorrect password.";
/// The seat account is unknown to the system (exit 10 = `PAM_USER_UNKNOWN`) — a
/// configuration fault, not a wrong password.
const REASON_UNKNOWN_USER: &str =
    "Could not verify the seat user — the account is unknown to the system.";
/// The password could not be checked at all — the helper failed, was killed, or
/// returned a service / authinfo error (e.g. exit 9). Honestly NOT an "incorrect
/// password", so the operator sees that authentication is broken, not wrong.
const REASON_SERVICE: &str =
    "Could not verify the password — the system authentication service failed.";
/// No password helper is present (PAM is unavailable on this host) — the
/// §7-honest "can't authenticate here" state, and still never a pretend grant.
const REASON_UNAVAILABLE: &str =
    "Password verification is unavailable on this system (no PAM helper found).";
/// The seat user could not be resolved (`$USER`/`$LOGNAME` unset and the uid was
/// unmapped) — there is no account to authenticate against.
const REASON_NO_SEAT_USER: &str = "Could not determine the seat user to authenticate.";

/// The `unix_chkpwd` helper's canonical locations, most-likely first (Fedora —
/// the target family — ships `/usr/sbin`). The first present path is used; none
/// present → [`REASON_UNAVAILABLE`].
const CHKPWD_PATHS: [&str; 3] = [
    "/usr/sbin/unix_chkpwd",
    "/sbin/unix_chkpwd",
    "/usr/bin/unix_chkpwd",
];

// ─────────────────────────── seat-user resolution ───────────────────────────

/// The process's real uid, parsed from a `/proc/self/status` dump (the `Uid:`
/// line's first field — the real uid). Pure; `None` if absent or unparsable.
fn real_uid_from_proc(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|first| first.parse().ok())
}

/// The username owning `uid` in an `/etc/passwd` dump (the `name:passwd:uid:…`
/// record). Pure; `None` if no record matches.
fn username_for_uid(passwd: &str, uid: u32) -> Option<String> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _passwd = fields.next()?;
        let entry_uid: u32 = fields.next()?.parse().ok()?;
        (entry_uid == uid && !name.is_empty()).then(|| name.to_owned())
    })
}

/// Resolve the seat user's name from an environment lookup, a uid probe, and an
/// `/etc/passwd` dump: `$USER` → `$LOGNAME` → the real uid mapped through the
/// passwd table. Pure over its injected inputs, so the precedence and the uid
/// fallback are unit-tested without touching the process environment.
fn seat_user_from(
    getenv: impl Fn(&str) -> Option<String>,
    uid: impl Fn() -> Option<u32>,
    passwd: impl Fn() -> Option<String>,
) -> Option<String> {
    for key in ["USER", "LOGNAME"] {
        if let Some(value) = getenv(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    username_for_uid(&passwd()?, uid()?)
}

/// The real seat user: `$USER`/`$LOGNAME`, else the process's uid mapped through
/// `/etc/passwd`. `None` only when the shell has neither a login environment nor
/// a mappable uid — then every attempt denies with [`REASON_NO_SEAT_USER`].
fn resolve_seat_user() -> Option<String> {
    seat_user_from(
        |key| std::env::var(key).ok(),
        || {
            let status = std::fs::read_to_string("/proc/self/status").ok()?;
            real_uid_from_proc(&status)
        },
        || std::fs::read_to_string("/etc/passwd").ok(),
    )
}

// ───────────────────────────── the unix_chkpwd backend ─────────────────────────────

/// The first present `unix_chkpwd` helper path, or `None` when none is installed
/// (PAM password checking is then unavailable on this host).
fn chkpwd_path() -> Option<&'static str> {
    CHKPWD_PATHS
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
}

/// Map `unix_chkpwd`'s exit code to a verdict. The helper exits with the PAM
/// return code: `0` = success, `7` = `PAM_AUTH_ERR` (wrong password), `10` =
/// `PAM_USER_UNKNOWN`; every other code (or a killed helper — `None`) is a
/// service error the operator must see as such, never a silent "wrong password".
fn classify_exit(code: Option<i32>) -> Verdict {
    match code {
        Some(0) => Verdict::Granted,
        Some(7) => Verdict::Denied(REASON_INCORRECT.to_owned()),
        Some(10) => Verdict::Denied(REASON_UNKNOWN_USER.to_owned()),
        _ => Verdict::Denied(REASON_SERVICE.to_owned()),
    }
}

/// Verify `password` for `user` against the system shadow by running the setuid
/// `unix_chkpwd` helper — the exact mechanism `pam_unix` execs for a non-root
/// caller, so this is PAM's own password path (design lock 1), not a parallel
/// store. Blocking (spawn + wait): only ever called on the worker thread
/// [`PamVerifier::begin`] spawns.
fn unix_chkpwd_verify(user: &str, password: &str) -> Verdict {
    let Some(helper) = chkpwd_path() else {
        return Verdict::Denied(REASON_UNAVAILABLE.to_owned());
    };

    // `nonull` — never accept an empty password (no null-password bypass, lock
    // 10); the curtain already refuses an empty submit, this is defence in depth.
    let mut child = match Command::new(helper)
        .arg(user)
        .arg("nonull")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Verdict::Denied(REASON_UNAVAILABLE.to_owned());
        }
        Err(_) => return Verdict::Denied(REASON_SERVICE.to_owned()),
    };

    // The Linux-PAM 1.7 helper reads NUL-separated secrets from its stdin pipe
    // (`pam_unix/support.c`): the password bytes, a terminating NUL, then EOF.
    // A best-effort write — the exit code is the authority on success/failure.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(password.as_bytes());
        let _ = stdin.write_all(&[0]);
        // `stdin` drops here → the write end closes → the helper reads EOF.
    }

    child.wait().map_or_else(
        |_| Verdict::Denied(REASON_SERVICE.to_owned()),
        |status| classify_exit(status.code()),
    )
}

// ─────────────────────────────── the verifier ───────────────────────────────

/// The off-thread authentication backend: maps `(user, password)` to a
/// [`Verdict`]. The real one shells [`unix_chkpwd_verify`]; tests inject a
/// scripted closure so a unit test NEVER runs the real helper. `Send + Sync`
/// so it can be cloned into each attempt's worker thread.
pub(crate) type Backend = Arc<dyn Fn(&str, &str) -> Verdict + Send + Sync>;

/// The **CURTAIN-2 PAM verifier**: verifies each curtain unlock against the seat
/// user's system password, off the render thread. Constructed by
/// [`crate::curtain::Curtain::pam`] at the shell's real mount; the curtain polls
/// it through the [`Verifier`] seam.
pub(crate) struct PamVerifier {
    /// The seat user each attempt authenticates. `None` when resolution failed —
    /// every attempt then denies with [`REASON_NO_SEAT_USER`] (no pretend grant).
    user: Option<String>,
    /// The `(user, password)` → [`Verdict`] backend, cloned into each worker.
    backend: Backend,
    /// The in-flight attempt's result channel; `None` when nothing is running.
    inflight: Option<Receiver<Verdict>>,
}

impl PamVerifier {
    /// The real seat-user authenticator: resolve the seat user and verify each
    /// attempt against the system password via `unix_chkpwd`, off the render
    /// thread. The constructor the shell mounts (via `Curtain::pam`).
    pub(crate) fn new() -> Self {
        let backend: Backend =
            Arc::new(|user: &str, password: &str| unix_chkpwd_verify(user, password));
        Self {
            user: resolve_seat_user(),
            backend,
            inflight: None,
        }
    }

    /// Build over an explicit seat user + backend — the injection seam. Tests
    /// pass a scripted backend (never real PAM); it is also the seam a future
    /// real libpam binding would slot into without touching the curtain.
    #[cfg(test)]
    pub(crate) fn with_backend(user: Option<String>, backend: Backend) -> Self {
        Self {
            user,
            backend,
            inflight: None,
        }
    }
}

impl Verifier for PamVerifier {
    fn begin(&mut self, password: &str) {
        let (tx, rx) = mpsc::channel();
        self.inflight = Some(rx);

        let Some(user) = self.user.clone() else {
            // No seat user to check against — answer immediately, still through
            // the channel so `poll` delivers it next tick (`begin` stays
            // non-blocking and the machine leaves `Verifying` honestly).
            let _ = tx.send(Verdict::Denied(REASON_NO_SEAT_USER.to_owned()));
            return;
        };

        let backend = Arc::clone(&self.backend);
        let password = password.to_owned();
        // OFF the render thread: the blocking helper spawn+wait runs here so the
        // egui loop never stalls on `pam_authenticate`; the verdict returns
        // through the channel. The handle is dropped — the worker is detached and
        // always sends (or disconnects, handled in `poll`).
        let _ = std::thread::spawn(move || {
            let verdict = backend(&user, &password);
            let _ = tx.send(verdict);
        });
    }

    fn poll(&mut self) -> Option<Verdict> {
        let rx = self.inflight.as_ref()?;
        match rx.try_recv() {
            Ok(verdict) => {
                self.inflight = None;
                Some(verdict)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                // The worker vanished without sending (a panic in the backend) —
                // surface an honest service error rather than wedge the curtain
                // on `Verifying` forever.
                self.inflight = None;
                Some(Verdict::Denied(REASON_SERVICE.to_owned()))
            }
        }
    }
}

// ──────────────────────────────────── tests ────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, Sender};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Poll `verifier` until its off-thread verdict lands (bounded — a stuck
    /// worker fails the test rather than hanging the suite).
    fn wait_verdict(verifier: &mut PamVerifier) -> Verdict {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(verdict) = verifier.poll() {
                return verdict;
            }
            assert!(Instant::now() < deadline, "the verdict never arrived off-thread");
            std::thread::yield_now();
        }
    }

    // ── seat-user resolution ──

    #[test]
    fn seat_user_prefers_env_then_falls_back_to_uid() {
        let passwd = "root:x:0:0::/root:/bin/sh\nbob:x:1000:1000::/home/bob:/bin/bash\n";

        // $USER wins outright.
        assert_eq!(
            seat_user_from(
                |k| (k == "USER").then(|| "alice".to_owned()),
                || Some(1000),
                || Some(passwd.to_owned()),
            )
            .as_deref(),
            Some("alice")
        );
        // $LOGNAME is the second choice.
        assert_eq!(
            seat_user_from(|k| (k == "LOGNAME").then(|| "carol".to_owned()), || None, || None)
                .as_deref(),
            Some("carol")
        );
        // A blank env value is ignored — resolution falls through to the uid.
        assert_eq!(
            seat_user_from(
                |k| (k == "USER").then(|| "   ".to_owned()),
                || Some(0),
                || Some(passwd.to_owned()),
            )
            .as_deref(),
            Some("root")
        );
        // No env at all → the uid mapped through /etc/passwd.
        assert_eq!(
            seat_user_from(|_| None, || Some(1000), || Some(passwd.to_owned())).as_deref(),
            Some("bob")
        );
        // Nothing resolvable → None (every attempt then denies honestly).
        assert_eq!(seat_user_from(|_| None, || None, || None), None);
    }

    #[test]
    fn uid_parses_from_proc_status_and_maps_through_passwd() {
        let status = "Name:\tmde-shell\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\n";
        assert_eq!(real_uid_from_proc(status), Some(1000));
        assert_eq!(real_uid_from_proc("no uid line here"), None);

        let passwd = "root:x:0:0::/root:/bin/sh\nmm:x:1000:1000::/home/mm:/bin/bash\n";
        assert_eq!(username_for_uid(passwd, 1000).as_deref(), Some("mm"));
        assert_eq!(username_for_uid(passwd, 0).as_deref(), Some("root"));
        assert_eq!(username_for_uid(passwd, 4242), None);
    }

    // ── exit-code mapping ──

    #[test]
    fn exit_codes_map_to_honest_verdicts() {
        assert_eq!(classify_exit(Some(0)), Verdict::Granted);
        assert!(matches!(classify_exit(Some(7)), Verdict::Denied(r) if r == REASON_INCORRECT));
        assert!(matches!(classify_exit(Some(10)), Verdict::Denied(r) if r == REASON_UNKNOWN_USER));
        // A service/authinfo error (9), an odd code, and a killed helper are all
        // honest service errors — never a silent "incorrect password".
        for code in [Some(9), Some(3), Some(255), None] {
            assert!(
                matches!(classify_exit(code), Verdict::Denied(r) if r == REASON_SERVICE),
                "exit {code:?} must be a service error, not a wrong-password deny"
            );
        }
    }

    // ── the off-thread verifier contract ──

    #[test]
    fn poll_is_none_until_the_off_thread_verdict_lands() {
        // A backend that blocks until the test releases it — proves `begin` is
        // non-blocking and `poll` answers `None` while the worker runs.
        let (release_tx, release_rx): (Sender<()>, Receiver<()>) = mpsc::channel();
        let gate = Arc::new(Mutex::new(release_rx));
        let backend: Backend = Arc::new(move |_user, _password| {
            let guard = gate.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = guard.recv(); // park until the test releases the worker
            Verdict::Granted
        });

        let mut verifier = PamVerifier::with_backend(Some("seat".to_owned()), backend);
        verifier.begin("hunter2");
        assert!(verifier.poll().is_none(), "poll must be None while the worker runs");
        assert!(verifier.poll().is_none(), "and stays None until the verdict lands");

        release_tx.send(()).expect("release the parked worker");
        assert_eq!(wait_verdict(&mut verifier), Verdict::Granted);
        assert!(verifier.poll().is_none(), "idle again — nothing in flight");
    }

    #[test]
    fn a_deny_backend_reports_the_denial_honestly() {
        let backend: Backend = Arc::new(|_u, _p| Verdict::Denied(REASON_INCORRECT.to_owned()));
        let mut verifier = PamVerifier::with_backend(Some("seat".to_owned()), backend);
        verifier.begin("nope");
        assert!(matches!(wait_verdict(&mut verifier), Verdict::Denied(r) if r == REASON_INCORRECT));
    }

    #[test]
    fn an_unavailable_backend_denies_without_ever_pretending() {
        // The §7 honest-unavailable state: no PAM helper → a denial that can
        // never become a grant.
        let backend: Backend = Arc::new(|_u, _p| Verdict::Denied(REASON_UNAVAILABLE.to_owned()));
        let mut verifier = PamVerifier::with_backend(Some("seat".to_owned()), backend);
        verifier.begin("x");
        assert!(
            matches!(wait_verdict(&mut verifier), Verdict::Denied(r) if r == REASON_UNAVAILABLE)
        );
    }

    #[test]
    fn an_unresolved_seat_user_denies_immediately_and_honestly() {
        let backend: Backend = Arc::new(|_u, _p| Verdict::Granted); // never consulted
        let mut verifier = PamVerifier::with_backend(None, backend);
        verifier.begin("whatever");
        assert!(
            matches!(wait_verdict(&mut verifier), Verdict::Denied(r) if r == REASON_NO_SEAT_USER)
        );
    }

    #[test]
    #[expect(
        clippy::panic,
        reason = "the fake worker deliberately panics to exercise poll()'s \
                  dead-worker (channel-disconnect) recovery branch"
    )]
    fn a_dead_worker_surfaces_a_service_error_not_a_hang() {
        // Silence the deliberate worker-panic's default stderr backtrace.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let backend: Backend = Arc::new(|_u, _p| panic!("worker died"));
        let mut verifier = PamVerifier::with_backend(Some("seat".to_owned()), backend);
        verifier.begin("x");
        let verdict = wait_verdict(&mut verifier);
        std::panic::set_hook(previous);
        assert!(
            matches!(verdict, Verdict::Denied(r) if r == REASON_SERVICE),
            "a panicked worker must surface a service error, not wedge the curtain"
        );
    }
}
