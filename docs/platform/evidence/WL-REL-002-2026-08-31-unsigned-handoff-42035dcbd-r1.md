# WL-REL-002 freeze-SHA unsigned three-RPM handoff — r1

Date: 2026-08-31  
Classification: unpublished freeze-SHA unsigned handoff; **not** native F44
close, six-role resume, publication, or live enroll  
Operator authority: 2026-08-31 generate-keys / freeze leftovers  
`published: false`  
`production_admitted: false`

## Source

`42035dcbd76b03b8323399892052b21a96e2e233` / epoch `1788153988` on
protected `master`. Prepare ran from a clean dest-cut checkout at
`/tmp/mcnf-rpm-receipt-repo` on `172.20.0.130` (`mcnf-build-52`).

Native F44 builder `172.20.0.131` had no route. Official prepare used the
same `container-rpm` Fedora 44 farm lane dest-cut historically used. That
is compatibility evidence, not the native F44 production leftover on
`WL-REL-002` S1.

## Preflight argv

Dest-cut `release-input-argv.py` `driver_argv` left the preflight script
path in `--emit-driver-arguments` output, so
`run-first-full-release.sh prepare --preflight-object` refused
(`unknown or incomplete argument: …/release-input-preflight.sh`).
Workaround: `--preflight-arguments` dest file
`/home/mm/preflight-driver-args-42035dcbd.json` (0400) with dest flags
only. Drain-branch `driver_argv` now drops the script path
(`del result[0:5]`); that fix is **not** in the freeze tree.

`release-input-preflight.sh` PASS for this revision before either RPM
lane.

## Cut

| Lane | Host | Slot workspace | Receipt |
|---|---|---|---|
| `container-rpm --full 44` | `172.20.0.130` | `freeze-cut-full` | `42035dcbd` / `1788153988` |
| `container-rpm --server 44` | `172.20.0.130` | `freeze-cut-server` | same |

Observed image: `registry.fedoraproject.org/fedora:44` config
`sha256:16daa734077fd52f9f3edc1acf3b5fc5b8d111d1ec568b983791d7f3fe2f5b59`
(tag-pin warning; not digest-pinned).

`first-full-release: PASS: unsigned operator handoff /home/mm/mcnf-unsigned-handoff-42035dcbd (promotion forbidden)`

| Role | File | NEVRA | Payload SHA-256 | size |
|---|---|---|---|---|
| workstation | `workstation-unsigned.rpm` | `magic-mesh-13.0.0-35.x86_64` | `a20ea60cb5603600c8cce5264dfc6623af5eb426651f6a03a820c2e59a019b27` | 93910340 |
| server | `server-unsigned.rpm` | `magic-mesh-server-13.0.0-35.x86_64` | `75752a5e1b94ce18f7da031c754f23f3503598e03d8b4a3a0a3b64b2c7d42b28` | 58080580 |
| lighthouse | `lighthouse-unsigned.rpm` | `magic-mesh-lighthouse-13.0.0-11.x86_64` | `feceac5c59ca9297c75e41b21c8c3bcfec4180b598790eac50a14517219d19f7` | 15663386 |

Handoff directory mode `500`; files mode `400`. Promotion forbidden.

## Leftover

Native F44 `.131` remains down. Container-F44 output is not the native
production RPM leftover. Do not publish or mark `production_admitted`.
