# WL-TEST-002 native F44 freeze-SHA workstation install — r1

Date: 2026-08-31
Classification: unpublished native Fedora 44 seat install of freeze SHA
`42035dcbd`; **not** six-role qualification, publication, live enroll, or
`production_admitted`
Operator request: 2026-08-31 “Build a new build and install seats.”
`published: false`
`production_admitted: false`

## Alert

Packaged `/usr/libexec/mackesd/seat-update-warning` ran on each seat before
mutation (`AI-GENERATED-ALERT` + 5s). Broker persisted `--no-broker` on
Dell and Seat 15; Surface persisted a toast ULID after overlay broker
`10.42.0.7:8443` was unreachable. Control-host `mde-bus` is absent.

## Cut

Native F44 builder `mcnf-build-f44` (`172.20.0.131`, Fedora 44) after RAM
handoff from `mcnf-build-52` (`172.20.0.130`). Source receipt
`42035dcbd76b03b8323399892052b21a96e2e233` / epoch `1788153988`.
`MCNF_RPM_TARGET_FEDORA=44` matched the builder. Isolated Maps verifier
was built into the same `target/` before `cargo generate-rpm` (native
`xcp-build.sh rpm` does not do that step; the container lane does).

Workstation + lighthouse only. Server was not recut natively. Do not use
the earlier container-F44 RPMs on physical seats.

| Role | NEVRA | payload SHA-256 (algo 8) | file SHA-256 after sign |
|---|---|---|---|
| workstation | `magic-mesh-13.0.0-35.x86_64` | `0f03292a063f34a8e442b16c53c9c42e719dceea3de2d82a958d07f2f48560f3` | `dced93b6d6702a8dd4978da794d9a2672ce0bc98785d96f765d61935152b8909` |
| lighthouse | `magic-mesh-lighthouse-13.0.0-11.x86_64` | `2d1733e7e6f5bfbacce4160602d8b40848452eb5fe909ab173fd64174b684ce0` | `ecd76e438c7308c0bcf7b4e33a28ac5e7a955b4b83648a511d9d6b25176cba8e` |

F44 ELF Requires on the workstation RPM: `libavcodec.so.62`,
`libswresample.so.6`, `libswscale.so.9`. Payload verify PASS. Size 89.6 MiB.

## Sign

F44 `rpm` 6.0.2 `rpmsign` did not verify. Signed on restored BigBoy F42
`rpm-sign` 4.20.1 with governed fingerprint
`06B1C27EA0E08A225155EB3314018AA1497DDC7C` (key id `497ddc7c`). Payload
digests unchanged. `rpm --checksig -v`: Header V4 EdDSA/SHA512 OK.

## Targets and results

Same NVR as dest-cut `bc14a22d7`; install used `rpm -Uvh --replacepkgs --force`
after `rpm --checksig` OK. `%post` logged transient systemd socket resets
(same class as dest-cut). `systemctl is-failed mackesd` is inactive, not
failed. No reboot. No lighthouse/server RPM on these workstations.

| Seat | Address | Fedora | Before identity | After |
|---|---|---|---|---|
| Dell | `172.20.146.225` `mm` | 44 | `bc14a22d7` | `42035dcbd` · `libswresample.so.6` · inactive |
| Seat 15 | `172.20.0.15` `mm` | 44 | `bc14a22d7` | same |
| Surface | `172.20.146.79` `root` | 44 | `bc14a22d7` | same |

Independent reread 2026-08-31 confirmed the three NEVRAs, freeze SHA in
`mackesd --version`, and F44 sonames.

## Leftover

Live enroll / `mackesd` inactive remains `WL-TEST-003`. Eagle and T480 were
not mutated. Farm VM `mcnf-build-52` is running again; `mcnf-build-f44` is
halted. The F44 builder disk still holds `mm`’s imported governed RPM
secret from preflight; wipe on the next `.131` boot if that host must not
retain it. `production_admitted` stays false.
