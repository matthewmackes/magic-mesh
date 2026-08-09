# WL-CRIT-007 Eagle recovery preflight r6

Date: 2026-08-09  
Revision: `de66002c5f06f1ad34e1eff5a4df3d8686f4074b`

## Governed target and pre-state

Repository authority resolves Eagle to T470S `172.20.146.88`, overlay
`10.42.0.6`; the retired `.13` address was not used. Read-only access returned
hostname `T470S-EAGLE`, user `mm` active with sessions `737 1`, system state
`degraded`, and `mde-shell-egui.service` active with `MainPID=1785551` and
`NRestarts=0`. `loginctl list-inhibitors` returned no inhibitor rows.

Safe unprivileged inventory observed Eagle's configured overlay as
`10.42.0.6/17`, package `magic-mesh-12.1.6-12.x86_64`, and no
`/usr/libexec/mackesd/mesh-peer-recovery` path. `sudo -n true` refused because
Eagle requires a password, so the repository controller stopped before its
package-bound verifier. The alert was not published and no suspend, reboot,
network-return service start, deployment, or other seat mutation occurred.
This is a blocker record, not recovery acceptance evidence.

## Rejected diagnostic path

An exploratory password-backed invocation concatenated password and verifier
source on one stdin stream. Security review rejected that routing because
cached or `NOPASSWD` sudo could pass secret bytes to Python. It is excluded
from acceptance evidence, and all sudo-password changes were removed from
`run-corrected-forward-recovery-probe.sh`. The controller remains the original
fail-closed `sudo -n` implementation; no mixed secret/payload route is retained.

## Farm verification

BigBoy `172.20.0.130`, slot `crit007-eagle-r6`:

- corrected-forward verifier self-test: 13/13 passed;
- complete root S2 peer-recovery fixture: passed, covering offline no-mutation,
  role and lighthouse coordination refusal, ordered Nebula/etcd/Syncthing/XDG/
  grouped-worker recovery, substrate failure, delayed child readiness, healthy
  no-op, single-flight coalescing, and resume/online trigger filtering.

## Remaining requirement

Install a candidate package that owns the S2 recovery helper, units, sleep hook,
network dispatcher, warning helper, and boot verifier on Eagle through the
governed rollout path. Then repeat preflight and one bounded physical
suspend/resume or network-return drill, proving one identity, Nebula, grouped
workers, Syncthing/Bus, shell/session, and no duplicate processes after return.
