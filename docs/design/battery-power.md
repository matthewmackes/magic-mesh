# POWER — battery indicator + full power settings (Quazar shell)

Operator-locked 2026-07-03 (live bug "battery does not show" + "include power settings",
after a 4-reader recon + 3-fork survey). Battery/power is **not** dead: `mde-seat`'s
UPower + logind clients are real and the System surface's "Power & Battery" section
(E12-18) consumes them. The gaps: (a) **no at-a-glance battery in the chrome status bar**
(the "doesn't show"), and (b) power **settings** (profiles, idle/lid, thresholds) are
absent or unreachable from the shell.

## Locked decisions

| # | Decision | Lock |
|---|----------|------|
| 1 | Battery visibility | **At-a-glance battery slot in the chrome status bar** (`battery_view`), over the already-polled `SeatSnapshot.batteries`. It stays only in System today — that's the "doesn't show". |
| 2 | Power-profile seam | **Native `mde-seat` zbus client** for `net.hadess.PowerProfiles` (read active + available, set) — all local, no mesh round-trip; matches "mde-seat is THE hardware library" and how the shell already gets batteries/verbs. (NOT the mackesd Bus relay.) Degrade to honest "unavailable" when power-profiles-daemon is absent. |
| 3 | Idle / lid | **Build the DRM-native honorer now** — a shell-owned idle timer that calls `seat.power(Suspend)` after a configurable timeout, plus an `mde-seat` evdev **`SW_LID`** lid-switch watch driving a lid-close action. No inert controls (§7): the settings only ship with a real honorer behind them. |
| 4 | Battery depth | **Include now** — charge threshold control (sysfs `charge_control_end_threshold`) + rich telemetry (time-to-empty/full, energy-rate draw in W) folded from UPower. |
| 5 | Hibernate + AC | Add `PowerVerb::Hibernate` (+ `PowerCaps.hibernate`, `CanHibernate`) and surface **on-AC / on-battery** (fold the UPower LinePower `Online` prop the battery fold currently drops). |
| 6 | Verbs already shipped | Lock/Suspend/Reboot/Power off stay as-is (mde-seat logind, confirm-gated) — extended with Hibernate. |

## Architecture

- **Chrome indicator (#1):** a `battery_view(seat) -> SlotView` in `chrome.rs` mirroring
  `bluetooth_view`/`volume_view`: system-pack %, charging bolt, severity dot (OK/WARN/
  DANGER), honest "No battery"/"unavailable". A clean standalone `SlotView` so NAVBAR-4
  folds it into the bottom-bar tray later.
- **mde-seat power backend v2 (#2/#4/#5):** all real D-Bus / sysfs / evdev, honest `None`
  fallbacks, folded into `SeatSnapshot` (new fields: `power_profile: Probe<ProfileState>`,
  `on_ac`/power-source, lid state, thresholds; extended `Battery` telemetry). A native
  `net.hadess.PowerProfiles` client (get/set/list), `CanHibernate`, the LinePower `Online`
  read, sysfs threshold read/write, and an evdev `SW_LID` watch. **These all touch
  `snapshot.rs`/`upower.rs` — one serialized backend worker owns them.**
- **Power Settings panel (#2/#4/#5):** extend `system.rs` "Power & Battery" + a new
  `power_settings.rs` — a profile segmented control (Performance/Balanced/Power-saver from
  the native client, honest "unavailable" when absent), a Hibernate verb row (caps-gated),
  an on-AC/on-battery source line, a charge-threshold slider, the telemetry readout, and
  the idle-timeout + lid-action settings. §4 tokens, §7 real (each control drives the real
  backend / the honorer, never inert).
- **DRM-native idle honorer (#3):** a shell-owned idle timer (last-input → timeout →
  `seat.power(Suspend)`) + a lid-close handler reading the `SW_LID` watch, both driven by
  the persisted settings. This is what makes the idle/lid settings real on the compositor-
  less DRM shell (swayidle/Wayland is out of scope).

## Acceptance (runtime-observable)
- The status bar shows battery % + charging state at a glance; low battery reads WARN/
  DANGER; a desktop with no battery reads honestly.
- The System Power settings show + change the power profile (verified: `powerprofilesctl
  get` reflects it), a working Hibernate (when `CanHibernate`), the AC/battery source, a
  charge-threshold cap that writes sysfs, and time-to-empty/draw telemetry.
- Setting an idle timeout actually suspends the machine after that idle; closing the lid
  performs the configured action. Both work on the DRM shell (no Wayland).
- power-profiles-daemon absent → the profile control reads "unavailable", never a fake
  active state (§7).

## Risks
- `chrome.rs` + `system.rs` are shared with the just-landed BT work and the pending
  NAVBAR-4 tray fold-in — keep the indicator a standalone `SlotView`; serialize the
  system.rs Power settings against other system.rs units.
- The mde-seat backend adds all touch `snapshot.rs`/`upower.rs` (shared structs) → they
  **serialize into one backend worker**, landing + pushing before the panel worker
  (workers branch from pushed master).
- evdev `SW_LID` + sysfs thresholds need real hardware to fully live-verify (headless farm
  has none) — code paths real + unit-tested; live-verify on Eagle/a Surface.
- Suspend/Hibernate on a mesh node interacts with the daemon/overlay — the honorer targets
  the local seat only; confirm no watchdog false-restart on resume.

## Tasks → see `docs/WORKLIST.md` POWER-1..5.
