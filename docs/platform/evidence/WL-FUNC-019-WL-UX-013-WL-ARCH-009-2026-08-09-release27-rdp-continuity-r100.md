# Release 27 Dell upgrade and seat 15 RDP continuity — 2026-08-09 r100

## Corrected-forward artifact

Source revision `ba446258e` was built natively on the dedicated BigBoy Fedora
44 builder (`172.20.0.131`, slot `release27-r99`) with Rust 1.94, 10 CPUs, and
23 GiB RAM. The normal Fedora 42 BigBoy VM was halted only for the cut and was
restored afterward; the five-machine farm returned ready.

The resulting `magic-mesh-12.1.6-27.x86_64.rpm` is 90,422,193 bytes (86.2 MiB)
with SHA-256:

```text
258ca80696031d756d4bbe27bcb912912d6b7599532ed98557313994f883a608
```

The complete payload and runtime-requirement gates passed. The native F44
header requires `libavcodec.so.62`, `libavformat.so.62`, `libavutil.so.60`,
`libswresample.so.6`, `libswscale.so.9`, and `qemu-ui-dbus`.

## Dell and seat 15 transactions

Each seat independently matched the staged hash. A package dry-run and the
real install each followed a fresh `/usr/libexec/mackesd/seat-update-warning`,
including its visible `AI-GENERATED-ALERT` and five-second intervention window.

Dell upgraded from release 26 to 27. All six grouped daemon services, the
aggregate target, and Construct were active; `rpm -V magic-mesh` was clean and
Construct reported `Result=success`, `NRestarts=0`. The retained `browser-vm`
remained running, persistent, and autostart-enabled with 4 vCPUs and 8 GiB.

Seat 15's first dry-run correctly refused the absent hard dependency. After a
fresh warning, Fedora installed `qemu-ui-dbus-10.2.2-1.fc44.x86_64` without
removals; a new warning preceded the passing dry-run. A final fresh warning
preceded the release 24 to 27 upgrade. RPM verification, all six grouped
services, the aggregate target, and Construct passed with zero shell restarts.

Both RPM transactions briefly printed transient Bus job submission failures
while the package replaced the local Bus. Post-transaction convergence was
checked independently and found no failed Magic Mesh unit.

## Installed RDP continuity proof

From seat 15, bounded TCP reachability to `172.20.146.54:3389` passed. The live
universal catalog exposed `probe-rdp/172.20.146.54` as an available Desktop/RDP
resource with the production client adapter and approval-gated connect action.
Every observed card used the exact five-minute lease
`expires_at_ms - last_seen_at_ms = 300000`.

Six fail-closed samples were taken 30 seconds apart. Absence of the typed card
would have failed the command. The critical old-cutoff sequence was:

```text
sample 1 wall=1786316420555 generated=1786316400376 last_seen=1786316361000 expires=1786316661000 available
sample 2 wall=1786316450626 generated=1786316430376 last_seen=1786316361000 expires=1786316661000 available
sample 3 wall=1786316480702 generated=1786316430376 last_seen=1786316361000 expires=1786316661000 available
sample 4 wall=1786316510777 generated=1786316430376 last_seen=1786316361000 expires=1786316661000 available
sample 5 wall=1786316540852 generated=1786316513226 last_seen=1786316451000 expires=1786316751000 available
sample 6 wall=1786316570923 generated=1786316557683 last_seen=1786316542000 expires=1786316842000 available
```

At sample 4, the unchanged underlying observation was 149,777 ms old, beyond
the former 120,000 ms card cutoff, yet the typed RDP card remained available.
Samples 5 and 6 then crossed two completed scan/catalog renewals without a
missing generation. This closes the installed seat 15 detection/TTL defect.

Authenticated Windows login, rendered pixels, and input were not attempted;
they are separate acceptance scope and are not claimed here.
