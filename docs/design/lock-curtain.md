# CURTAIN — the secure login/lock curtain (Construct shell)

> **Design-standard note (2026-07-22):** look-and-feel guidance in this doc is subordinate to the platform interface standard — see [platform-interfaces.md](platform-interfaces.md) (Apple-HIG-principled Construct + Car). Feature/behavior content remains authoritative.

Operator-locked 2026-07-04 (10-Q `/plan` survey). The platform boots the DRM shell
straight to the desktop with **no login prompt** (lightdm removed; no display manager
by design). CURTAIN adds the missing gate: a full-screen **secure login page that is
also the lock screen** — raising and lowering like a theater curtain, Carbon-styled,
acting as a full-screen clock with embedded media playback controls.

## Locked decisions

| # | Decision | Lock |
|---|----------|------|
| 1 | Auth | **PAM, system password** — unlocking authenticates THE seat user via PAM (the `login`-class service; the same credentials as SSH/sudo). No parallel password store; fails closed. |
| 2 | Triggers | **Boot + lock + idle + lid** — the curtain gates the desktop at system start (the login), on **Super+L**, on the POWER-5 idle timeout via a new **Lock** idle action, on lid-close when the lid action is Lock, and on logind's `Lock` session signal. One curtain for all of it. |
| 3 | Locked media | **Playback only** — play/pause/next/prev + volume + track info work without auth (the phone-lockscreen model); every other interaction requires unlock. |
| 4 | Locked content | **Status glanceables only** — battery, mesh/network health, date; **no message content** (chat previews stay private until unlock). |
| 5 | Motion | **Slide + settle bounce** — lock: the curtain drops from the top edge with a slight settle/overshoot; unlock: it lifts up and out. Motion tokens, ~300ms ease. |
| 6 | Clock face | **Giant digital HH:MM + date** — huge thin-weight Carbon type centered-high, date beneath (matches the tray's stacked clock). |
| 7 | Media wiring | **Unified now-playing** — one transport strip driving whichever player is active/most-recent (mde-music Subsonic or mde-media mpv) via their existing in-shell transport state. |
| 8 | Unlock UX | **Two-stage reveal** — the curtain rests as the clock face; any key/click slides in the password field; Esc drops back. |
| 9 | User model | **Single seat user** — PAM for the configured session user; no user chooser, no multi-user session switching. |
| 10 | Hardening | **Backoff + dim + no-bypass** — 5 failed attempts → a 30s cooldown with an honest countdown; the curtain dims to near-black after idle (clock stays faint); input fully grabbed while locked (the shell owns the seat — nothing routes past the curtain; a tty switch remains root's domain). Media transport exempt per #3. |

## Architecture

New `crates/desktop/mde-shell-egui/src/curtain.rs` (+ `pam_auth.rs`):
- **State machine:** `Unlocked → Dropping → Locked(face) → Revealing(password) →
  Verifying → (Lifting | Backoff)`; the curtain renders as a full-screen top layer that
  slides via the Motion tokens (settle-bounce on drop). While not `Unlocked`, the
  curtain consumes ALL input first (the shell is the compositor — the grab is simply
  "the curtain handles events exclusively"), except the media-transport hit areas (#3).
- **PAM (`pam_auth.rs`):** a real PAM conversation for the seat user (a `pam`-binding
  crate over libpam; `unix_chkpwd` handles shadow access for the non-root shell).
  Blocking auth runs OFF the render thread (channel bridge, like the pairing dialog);
  failures count toward the backoff state. Probe libpam builds on the farm EARLY.
- **Triggers:** boot-locked when the (persisted) `require_login_at_boot` config is on —
  the shell starts in `Locked` before any surface renders; `Super+L` in the shell
  hotkeys; POWER-5's honorer gains **Lock** as an idle action + routes lid-Lock to the
  curtain in-process; a logind `Lock`/`Unlock` signal listener raises/lifts it.
- **The face:** giant digital clock + date; a status row (battery/mesh health) reusing
  the tray's fold helpers; the unified now-playing strip (title/artist +
  play/pause/next/prev + volume) driving the existing music/media transport seams;
  idle-dim after ~30s (clock faint, wake on input).
- **Backoff:** per-boot failed-attempt counter → cooldown with a visible countdown;
  the field disables during cooldown.

## Acceptance (runtime-observable)
- Boot (with require-login on) lands on the curtain — the desktop is not visible or
  interactable until the seat user's real password passes PAM.
- Super+L / the idle Lock action / lid-close(Lock) / `loginctl lock-session` all drop
  the curtain with the settle-bounce; unlock lifts it.
- Any key reveals the password field (two-stage); a wrong password errors honestly;
  5 fails → a counted-down cooldown.
- While locked: the clock + date + status row render; play/pause/next/prev + volume
  control the live player; NOTHING else responds; chat content never shows.
- The curtain dims after idle and wakes on input; all styling via Carbon tokens (§4).

## Risks
- **PAM linkage on the airgapped farm** (libpam headers) — probe first; shell-out to
  `unix_chkpwd`-style helpers is the fallback discussion if bindings won't build.
- **mde-shell-egui contention** — curtain.rs is new but triggers touch main.rs /
  hotkeys.rs / power_honor.rs → serialize with other shell work in flight.
- **True no-bypass** depends on the shell owning the seat exclusively (it does — DRM
  master + evdev via the shell); tty switching is a root/console concern, documented.
- Boot-locked must not break the `.13`-style autostart (the service still starts the
  shell; the shell just starts locked).

## Out of scope
- Multi-user session switching / user chooser (#9).
- Fingerprint/hardware tokens; remote unlock; Wayland lockers (no compositor).
- Notification content on the curtain (#4 — counts live in a later NOTIFY pass if wanted).

## Tasks → `docs/WORKLIST.md` CURTAIN-1..4.
