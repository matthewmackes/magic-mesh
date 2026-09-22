# WL-REL-003 S4 freeze-SHA Browser/App VM derivatives — r1

Date: 2026-09-22  
Classification: source leftover S4 attempt; **not** six-role plan, publication,
live enroll, or dest invention  
`published: false`  
`production_admitted: false`

S6 is not done. This record is a failed admission gate, not a derivative
collection.

## Inputs (control host `rocky9-kvm2`)

Every named freeze-SHA input is a regular non-symlink file, mode `0400`, not
group/other writable. Parent `/root/mcnf-private` is mode `0700`. Suggested
output `/root/mcnf-private/derivatives-42035dcbd` was absent before the run
and remains absent. No complete `derivative-images.json` /
`app-vm-wayland-standard.qcow2` / `browser-vm-chromium.qcow2` pair exists
under `/root/mcnf-private`.

| Input | Path | Mode | SHA-256 |
|---|---|---|---|
| source revision | `42035dcbd76b03b8323399892052b21a96e2e233` (epoch `1788153988`) | n/a | n/a |
| signed Workstation RPM | `/root/mcnf-private/native-f44-signed-42035dcbd/magic-mesh-13.0.0-35.x86_64.rpm` | `0400` | `dced93b6d6702a8dd4978da794d9a2672ce0bc98785d96f765d61935152b8909` |
| signed Lighthouse RPM | `/root/mcnf-private/native-f44-signed-42035dcbd/magic-mesh-lighthouse-13.0.0-11.x86_64.rpm` | `0400` | `ecd76e438c7308c0bcf7b4e33a28ac5e7a955b4b83648a511d9d6b25176cba8e` |
| App candidate | `/root/mcnf-private/s3-42035dcbd/workstation-candidate.json` | `0400` | `b04f49b29d3eb0c8a6bfcf88002b4d199893f4adc57855e1176032eeccae0e2b` |
| Browser/Lighthouse candidate | `/root/mcnf-private/s3-42035dcbd/lighthouse-candidate.json` | `0400` | `3d411d007157183d67ffeb49f39dac1946303e56a75ccd49688b0bc05ed2fc76` |
| App base receipt | `/root/mcnf-private/app-vm-base-digest-42035dcbd.json` | `0400` | `a5ad8d4bfb4e66f7809bee0b0add61ce53b98ec3158772ac06b77f362605ec44` |
| Browser base receipt | `/root/mcnf-private/browser-vm-base-digest-42035dcbd.json` | `0400` | `00d73a3e06af50e52c74ca034e776c5bb21cf7e910337f58d5040086108029de` |

App base image in the receipt:
`quay.io/fedora/fedora@sha256:e78cd1a688cd079c23864f289a89a49a3f4ad66d817864e325e1d058310ee95c`.

Browser base image in the receipt (Fedora registry recovery; did not follow
moved quay `:44`):
`registry.fedoraproject.org/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`.

## Hostile suite

Host: `rocky9-kvm2` (control).  
Command: `install-helpers/test-build-release-derivative-images.sh`  
Result: `test-build-release-derivative-images: hostile orchestration suite passed`  
Exit: `0`

Helpers were not rewritten. `packaging/app-vm` and `packaging/browser-vm` were
not edited (identical to freeze SHA `42035dcbd`).

## Helper invocation (exactly one farm run)

`.130` / XEN-BIGBOY `172.20.145.165` unreachable. Local Rocky `rpm --initdb
--dbpath` also failed (`temporary RPM database initialization failed`), so the
capable invocation used admitted farm host `172.20.0.90`
(`mcnf-build-kvm-xcp1`), slot 1 held then released. Did not start `.131`.

Command (sudo on `172.20.0.90`, `2026-09-22T13:54:18Z` … `13:54:19Z`):

```
sudo /home/mm/mcnf-s4-42035dcbd/install-helpers/build-release-derivative-images.sh \
  --source-revision 42035dcbd76b03b8323399892052b21a96e2e233 \
  --signed-workstation-rpm /home/mm/mcnf-private-s4/inputs/workstation.rpm \
  --app-rpm-candidate-manifest /home/mm/mcnf-private-s4/inputs/workstation-candidate.json \
  --app-base-receipt /home/mm/mcnf-private-s4/inputs/app-vm-base-digest-42035dcbd.json \
  --app-base-image quay.io/fedora/fedora@sha256:e78cd1a688cd079c23864f289a89a49a3f4ad66d817864e325e1d058310ee95c \
  --signed-lighthouse-rpm /home/mm/mcnf-private-s4/inputs/lighthouse.rpm \
  --browser-rpm-candidate-manifest /home/mm/mcnf-private-s4/inputs/lighthouse-candidate.json \
  --browser-base-receipt /home/mm/mcnf-private-s4/inputs/browser-vm-base-digest-42035dcbd.json \
  --browser-base-image registry.fedoraproject.org/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357 \
  --output /home/mm/mcnf-private-s4/derivatives-42035dcbd
```

Result: **FAIL**, exit `2`. Output never published. Farm work copy and private
staging were removed after the refuse. Control-host
`/root/mcnf-private/derivatives-42035dcbd` was not created.

## Exact failed gate

```
error: can't create transaction lock on /tmp/tmp.GR0gky1SCD/.rpm.lock (Permission denied)
FATAL: App VM RPM supply refused: temporary RPM database initialization failed
release-derivative-images: REFUSED: signed Workstation RPM admission failed
```

This is the same Fedora 42 `rpm --initdb --dbpath` lock refusal recorded in
`WL-REL-003-2026-08-31-s3-candidates-42035dcbd-r1.md`. S3 produce used an
external PATH wrapper and did not edit dest-cut helpers. This S4 run did not
rewrite `verify-rpm-supply.sh` or invent that wrapper.

Builders were not invoked. No qcow2, manifest, or profile was emitted.

## Independent identity mismatch (not used as a pass)

Even if RPM-database init is repaired, the named native-F44 signed RPMs do
not match the named S3 dest-cut candidate manifests. The helper would then
refuse `RPM whole-file SHA-256 does not match the candidate manifest` and
`RPM payload digest does not match the source-revision candidate manifest`
(App) / `signed Lighthouse RPM bytes or governed identity do not match the
manifest` (Browser).

| Role | Native-F44 whole-file | S3 candidate `rpm_sha256` | Native-F44 payload (sidecar) | S3 candidate `payload_sha256` |
|---|---|---|---|---|
| Workstation | `dced93b6…8909` | `9f78ec2b…1e9f` | `0f03292a…60f3` | `a20ea60c…9b27` |
| Lighthouse | `ecd76e43…ba8e` | `c4057d9c…5edb` | `2d1733e7…4ce0` | `feceac5c…19f7` |

No local RPM under `/root/mcnf-private` hashes to the S3 dest-cut
`rpm_sha256` values. Last recorded matching dest-cut signed directory is
`/home/mm/mcnf-signed-rpms-42035dcbd` on unreachable `172.20.0.130`. Did not
invent a dest, did not follow moved quay `:44`, did not generate a second RPM
GPG key, and did not substitute an older unpublished cut.

## Leftover

S4 derivatives remain. S6 six-role plan input waits on a verified immutable
Browser VM + App VM collection from one admitted helper publication.
