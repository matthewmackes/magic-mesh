# WL-CRIT-007 — Dell release 31 boot-status recovery (r115)

Date: 2026-08-10

Artifact source revision: `b2895c9f96e01e5ba1e51714d93d489ed4f46156`

## Warned live reboot

Dell (`172.20.146.225`) was running exact
`magic-mesh-12.1.6-31.x86_64`. Its persistent `browser-vm` was running and
autostart-enabled before mutation. The centered AI-generated warning was
published and its mandatory five-second delay completed before the one-seat
reboot.

The boot identity changed from `2ee46140-f9af-4eb2-ab23-2142af2c2587` to
`106f715e-4914-4040-b4e4-4c9892ab2a4f`. SSH observed the host leave and return
with the same release in 69 seconds. The new kernel command line retained
`quiet systemd.show_status=1 rd.systemd.show_status=1` and contained no `rhgb`.

## Timed recovery

The prior boot reported 1 minute 30.390 seconds total, including 1 minute
8.780 seconds of userspace, with multi-user at 51.147 seconds. The release-31
reboot reported:

- total boot: 56.811 seconds;
- userspace: 35.494 seconds;
- multi-user target: 26.570 seconds in userspace;
- Construct shell start job: 23.650 seconds after kernel start;
- shell service active: 27.431 seconds;
- DRM shell process start: 36.383 seconds;
- seat milestone: 38.446 seconds;
- splash handoff to the desktop: 45.338 seconds.

After return, the package remained exact release 31; Nebula, all six grouped
daemon services, `mackesd.target`, and the shell were active; the shell had zero
restarts; no failed system unit remained. Dell's Browser VM returned running,
autostart-enabled, four-vCPU/8-GiB, with the exact pre-reboot overlay disk and
control seed paths preserved.

This directly proves that the prior >60-second userspace wait no longer delays
Construct takeover. SSH and journal timing cannot prove the exact pixels shown
by firmware, bootloader, or the pre-DRM console, so physical camera/console
capture remains the explicit visual limitation rather than an inferred pass.
