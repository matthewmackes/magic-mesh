# WL-REL-006 S1 leftover — current six-role open-source input inventory — r1

Date: 2026-08-22 UTC  
Classification: redacted six-role inventory and license manifest; **not**
production admission, preflight close, or Maps `production_admitted`  
Source revision: `e47399620b703a6968af4434fb3b953eb9490716`  
Source epoch: `1787439800`  
Worktree: `/tmp/mcnf-drain-worktrees/cursor-WL-REL-006-qu0019iv`  
Unit: `qu0019iv`  
Kind: `mcnf-open-source-input-inventory` schema 1

This record replaces the HISTORICAL/SUPERSEDED fixture inventory
`WL-REL-006-open-source-input-inventory-r1.md` (Cuttlefish + obsolete
revision). Source selection was not reopened. Android/Cuttlefish is not a
production family. No Flatpak catalog refs were invented. PBF, TIGER zip,
MBTiles, and private receipts were not copied into Git.

## Act

Helper `install-helpers/produce-open-source-input-inventory.py` emits the
already-selected six-role set only. `production_admitted` stays `false` on
Maps. App VM catalog refs stay leftover. Browser VM names the in-tree
producer/verifier family only; no current-revision-bound image digest was
invented.

| family | license / identity | leftover |
| --- | --- | --- |
| maps | ODbL-1.0; dest `/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` on BigBoy; dest sha256 `6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895` | `production_admitted: false`; live-seat dest is WL-TEST-002 |
| app-vm | Fedora Project terms; `quay.io/fedora/fedora:42` amd64 wayland-standard; receipt `aca7573bc` / sha256 `f939be3864024f0e7bbfe53a26272eb796e3f85d9a35231f2a9b7ca6f4fb7891`; resolved `sha256:e78cd1a688cd079c23864f289a89a49a3f4ad66d817864e325e1d058310ee95c` | real curated catalog refs; `org.example.App` is refused |
| bootc | Fedora Project terms; `quay.io/fedora/fedora-bootc:44` amd64 `all-roles`; receipt `479ec2b8c` / sha256 `2e1a183fc48de8124624881d7ec5f99770d954d81a61dcc4cf4d07919f2326ae`; digest `sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357` | S7 private preflight consume |
| browser-vm | Fedora Project terms; `packaging/browser-vm/produce-base-image-receipt.py` + `verify-image-manifest.py`; target `mcnf-browser-vm/browser-vm-chromium-v1` | no current-revision-bound image digest |
| rpm | operator-controlled key policy; staged fingerprint `06B1C27EA0E08A225155EB3314018AA1497DDC7C` | current-revision signer receipt waits on freeze (WL-REL-001) |
| ux-014 | CC0-1.0 Kiron; `packaging/kiron/verify-package.sh --source` | none for this inventory |

## Local test

```text
python3 install-helpers/test-produce-open-source-input-inventory.py
open-source input inventory hostile self-test: PASS
```

No network. Suite covers the exact six-role set, Android/Cuttlefish
refusal, Maps `production_admitted` false, and `org.example.App` fixture
catalog-ref refusal.

## Farm

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=0 ./install-helpers/xcp-build.sh sync
```

Admitted on `172.20.0.50` slot `0` (`~/magic-mesh-farm-0`) at `71395636`
KiB free (required `8388608` KiB). Same suite:

```text
python3 ~/magic-mesh-farm-0/install-helpers/test-produce-open-source-input-inventory.py
open-source input inventory hostile self-test: PASS
```

Python on the slot compiled the producer. This does not close
`release-input-preflight.sh` and does not flip Maps `production_admitted`.

## Leftover / blocker

Leftover remains Maps `production_admitted`, App catalog real refs, RPM
signer receipt after freeze (WL-REL-001), S7 `REPLACE_*`, and live-seat
dest (WL-TEST-002). App VM `aca7573bc` and bootc `479ec2b8c` stay the
current receipt leftovers versus later HEADs. This inventory does not
satisfy the release-input gate.
