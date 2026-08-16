# Private seven-role preview evidence — afc24782

- Date: 2026-08-16 UTC
- Source revision: `afc24782ca9dc8e2e87f5676e403428a82285da1`
- Commit epoch: `1786905664`
- Signing identity: `06B1C27EA0E08A225155EB3314018AA1497DDC7C`
- Promotion: **forbidden**; this is a private preview collection, not a
  published release.

## Input admission

The canonical first-release preflight passed on BigBoy (`172.20.0.130`) with
the exact source revision and epoch above. The command consumed the strict
mode-0400 private argv document outside Git and admitted Maps, App VM trust and
base, Cuttlefish declaration/guest packages/image, RPM signer identity, and
bootc receipt inputs. The host-side project release key was restored at the
governed path `/etc/pki/rpm-gpg/RPM-GPG-KEY-magic-mesh`; its SHA-256 was
`39c4f65d7c7a44a8ab64e234dfa9989d1fb3f335f7e5221f619679aeb59183c9`.

Result:

```text
release-input-preflight: PASS: all mandatory first-release inputs admitted for afc24782ca9dc8e2e87f5676e403428a82285da1
```

## Seven-role collection

`produce-release-output-plan.py` produced the exact seven-role plan, and
`collect-release-outputs.py` re-ran every owning verifier successfully. The
immutable output manifest is retained privately at
`/tmp/mcnf-preview-afc/release-bundle-v6` on the orchestrator; the following
table is the durable redacted inventory.

| Role | Artifact size | SHA-256 |
|---|---:|---|
| workstation-rpm | 93,080,069 | `a4be88d9da6d8ab740013cfd326e78c1192dbade1cfe052b049e5a05bfed7406` |
| server-rpm | 57,688,798 | `7a6ee4eaaa91b3f40a2e661be7af4d003092d58bba308ffa4b309626182198e6` |
| lighthouse-rpm | 15,361,901 | `07e7976a66d06ba4d812c2c1a824518030b81b8a32075da229c60b0800628a21` |
| browser-vm | 1,892,418,560 | `f9c9711785594f955bcd08064d42445379814fbd69d737c3080b6c2836bf0021` |
| app-vm | 2,055,952,896 | `4786b27f6771183cfb3f4d695a3a8145d9c7ce5d4d24bfeef85ad8ecc0b63de7` |
| cuttlefish-image | 152,832,000 | `e6e2f1194010c2e1810b462283180c9bdad960b4becc9a67bc3dd2a104d7eb37` |
| bootc-image receipt | 491 | `475f4aa6bae8a3622e9cc17c5d0a8e03e33f07a59ff94eac7748d37e951e5c05` |

The collection manifest reports `promotion: forbidden`, exactly seven roles,
one source revision, and one signer. Browser and App VM qcow2 manifests were
verified against their complete compressed disk bytes; the App VM is a 10 GiB
virtual disk occupying about 1.91 GiB compressed.

## Current-source farm verification

The current checkout was routed through the governed farm with explicit host
and slot assignments. All gates passed:

- BigBoy `.130` slot 1: `cargo test -p mackesd --lib --locked --
  --test-threads=1` — `5023 passed; 0 failed; 1 ignored`.
- `.90` slot 2: `cargo test -p mackesd --lib onboard --locked -- --nocapture` —
  `229 passed; 0 failed`.
- `.170` slot 1: `cargo test -p mackesd --lib lifecycle_authority --locked --
  --nocapture` — `17 passed; 0 failed`.
- `.90` slot 1: `cargo test -p mackes-mesh-types lifecycle --locked --
  --nocapture` — `20 passed; 0 failed`.
- `.50` slot 1: `cargo test -p mde-enroll --lib --locked --
  --test-threads=1` — `33 passed; 0 failed`.
- `.196` slot 1: `cargo check -p mde-enroll --bin magic-setup --locked` —
  completed successfully.

Local release controls also passed: the release plan/collector seven-verifier
integration, release-evidence self-test, sign-release self-test, worklist lint
self-test, and `git diff --check`.

## Remaining release work

This evidence does not close WL-REL-001, WL-REL-004, or WL-REL-005. The
feature-complete source freeze, signed provenance/SBOM/gate envelope, clean-room
publication readback, immutable tag/release publication, signed repository
metadata promotion, and installed acceptance remain outstanding. No public
remote or package channel was mutated by this preview run.
