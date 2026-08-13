# WL-FUNC-022 peer timer authority — r504

Date: 2026-08-13

## Production gap closed

The Clock surface projected every daemon-delivered timer with local
pause/resume/restart/add-minute/remove controls. That included timers whose
typed schedule authority belonged to a peer node. The shell could therefore
sign an unauthorized schedule mutation and present it as pending until the
daemon refused it.

The Timers projection now marks peer-origin schedules with their provenance and
renders them read-only. The command builder independently rejects every peer
schedule mutation, so non-render callers cannot bypass the visual boundary.
A ringing peer timer may still be stopped locally through its exact typed
occurrence acknowledgement; that action acknowledges the alert delivered to
this node and does not mutate the peer-owned schedule.

Multiple local named timers and their existing pause, resume, restart,
add-minute, remove, overdue, and ringing behavior are unchanged. The renderer
continues to perform no Bus or persistence I/O.

## Farm verification

- BigBoy `.130`, slot `func022-peer-timer`:
  `cargo test -p mde-shell-egui peer_timer_refuses_local_schedule_mutations -- --nocapture`
  — passed 1/1; 1,581 filtered.
- `.50`, slot `func022-peer-timer-clippy`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings`
  — passed strictly.
- `git diff --check` — passed.

## Remaining acceptance

The first full release must include the Clock daemon and shell payloads. Under
the current deferred, non-blocking policy, post-release installed-seat proof
still covers multiple named timers, pause/resume/reset/delete/add-minute and
overdue/ringing transitions, alarms, stopwatch origin control, stale/lost
projection handling, audio/provider loss, restart, sleep/rejoin, and package
identity.
