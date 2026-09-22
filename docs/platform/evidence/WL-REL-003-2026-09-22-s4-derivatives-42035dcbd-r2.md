# WL-REL-003 S4 dest-cut derivatives — r2

Date: 2026-09-22  
Classification: source leftover S4 attempt; **not** six-role plan, publication,
live enroll, or dest invention  
`published: false`  
`production_admitted: false`

S6 is not done. Output path
`/home/mm/mcnf-private-s4/derivatives-42035dcbd` remains absent.

## Farm restore

`mcnf-build-52` `172.20.0.130` is running again after RAM handoff from
halted `mcnf-build-f44` `.131`. Dest-cut signed RPMs at
`/home/mm/mcnf-signed-rpms-42035dcbd` match S3 manifests:

| Role | `rpm_sha256` |
|---|---|
| Workstation | `9f78ec2bb6641788d3a68aa4ee91ab6295241bf9f97614ec6c036b3238ca1e9f` |
| Lighthouse | `c4057d9c7b2155bbfcd57797d55d8e7adfe2491e1a012d714496092119dd5edb` |
| Server | `298fde382ebbbdf0f0e7049289afd241271c237fadc0a93be475a74b29f2123b` |

Host PATH wrapper `/home/mm/bin/rpm` (`TransactionSet.initDB`) admits both
RPMs as `mm`. Hostile suite on the control host PASS.

## Helper invocation

Admitted BigBoy slot 1. Freeze SHA tree `42035dcbd` / epoch `1788153988`.
`sudo` `build-release-derivative-images.sh` with dest-cut inputs and Fedora
registry Browser base `sha256:3a5e74e6…`. Did not start `.131`.

Result: **FAIL**, exit `2`. Collection unpublished.

## Exact failed gate

Inside the App VM image `RUN`, after `dnf -y install gnupg2`:

```
FATAL: App VM RPM supply refused: cpio is required
release-derivative-images: REFUSED: App VM derivative build or verification failed
```

The dest-cut Fedora base digest does not ship `cpio`. `verify-rpm-supply.sh`
requires it before payload attestation. Follow-up source fix: install `cpio`
in `packaging/app-vm/Containerfile` before that verifier runs.
