# Construct Clock

> **Authoritative Clock design.** This document records the hard-cut interface,
> authority, migration, and package contract for WL-FUNC-022. It refines the
> Construct locks in `docs/design/platform-interfaces.md`; historical Timers &
> Alarms designs do not describe the shipped platform.

## Interface contract

Construct has two distinct persistent targets:

| Target | Result |
|---|---|
| Visible status/taskbar clock | Open `Surface::Clock` directly |
| Dedicated bell | Open Notification Center |

Clock contains World Clock, Alarms, Timers, and Stopwatch. “Timers” is a section
inside Clock, not a surface or launcher. Alarm and timer banners can be retained
in Notification Center, but notification history does not own Clock schedules
and does not change either target's route.

## Authority contract

The egui shell is presentation and command intent only. It may render the
daemon projection, validate bounded UI input, sign and publish typed Clock
commands, and show daemon-derived banners. It does not persist schedules,
evaluate deadlines, ring alarms, advance timers, or reconstruct state after a
restart.

`mackesd` is the sole runtime authority for persisted schedules, deadline and
recurrence evaluation, ringing, snooze/stop transitions, restart recovery, and
replicated stopwatch state. Music remains the audio provider; it does not own
scheduling.

## Hard migration

The retired Timers surface and shell-owned alarm scheduler/store have no
compatibility route. The legacy `timers-alarms.json` file is deliberately never
read or imported. It is left untouched for manual rollback, while Clock starts
with no imported alarms or timers.

Only display-zone preferences migrate. The five recognized legacy values map
atomically to IANA identifiers; unknown values fail closed without rewriting the
settings file. This migration cannot inspect or mutate alarm data.

## Package contract

There is no separate Timers or Alarms executable, desktop launcher, service, or
installed data payload. Clock ships inside `/usr/bin/mde-shell-egui`; scheduling
ships inside `/usr/bin/mackesd`. The base workstation RPM carries both binaries,
while headless variants do not acquire shell UI merely to provide scheduling.

`install-helpers/lint-clock-cutover.sh` enforces the source, documentation, and
package hard cut. `install-helpers/verify-rpm-payload.sh payload [RPM]` invokes
the same gate and, when given an RPM, rejects retired installed payload names.
