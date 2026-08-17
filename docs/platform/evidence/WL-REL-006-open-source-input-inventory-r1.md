# HISTORICAL / SUPERSEDED — WL-REL-006 open-source input inventory — r1

Superseded on 2026-08-17 by the six-role release contract. This fixture-only
inventory is bound to an obsolete source revision and includes the deferred
Android/Cuttlefish family; it cannot be used for 13.0.0 release admission. A
replacement must be generated only after WL-REL-001 establishes the clean
candidate revision and WL-REL-006 materializes candidate-bound production
bytes.

Date: 2026-08-16 UTC  
Source revision: `daf3c695928e96553fe839450bd86aa6f371e3aa`  
Source epoch: `1786817528`  
Classification: fixture evidence; not a production release approval

This inventory records reproducible, non-secret inputs and their license
identity. A fixture row is admissible only with the signed substitution record
required by `AI_GOVERNANCE.md`; no row claims live-provider or installed-seat
behavior.

| Family | Source / method | License / attribution | Immutable identity | State |
| --- | --- | --- | --- | --- |
| Maps | bounded OpenStreetMap-derived tile fixture; canonical producer + Rust verifier + materializer | ODbL-1.0; © OpenStreetMap contributors | manifest `4a69db176c09a126bc56d69513cc45d32d51be0534b7e318888ccfae514b12c9`; cache catalog `94436590b0c1ecbea2da54ad4c6dfc5439d4e0308a708d39e2a2f99137a9d801` | Fixture admitted; live provider proof pending |
| App VM | official Fedora registry reference `quay.io/fedora/fedora:42`, amd64 | Fedora Project terms; registry attribution retained | resolved index `sha256:e78cd1a688cd079c23864f289a89a49a3f4ad66d817864e325e1d058310ee95c`; platform `sha256:63773f454664cd77e239f8e0b13ae7f18effe9e3d6612a325b5646eb3bda11f1` | Fixture receipt admitted; Wayland/App VM acceptance pending |
| bootc | official Fedora bootc reference `quay.io/fedora/fedora-bootc:44`, amd64 | Fedora Project terms; registry attribution retained | manifest `sha256:35f5a8e7e7417a3b15a4d62d1a950ab8a873af0a0a8c20105d079224c01ac64c` | Fixture receipt admitted; role/image acceptance pending |
| Cuttlefish host tooling | AOSP Cuttlefish Git source commit `a1162ca7a4e6297f1699b65052a8c2dd466fd518` | Apache-2.0 upstream notices | exact Git commit; guest DEB manifest `f3f40332a1a32d9ffa6c16507db258c1c00736d35047d3759f55d5994780f37f` | Guest packages built; Android image/host package/declaration pending |
| UX-014 | 18 authored SVG scene tiers + 6 synthesized CC0 PCM cues from `produce-kiron-original-assets.py` | CC0-1.0; MDE contributors | manifest `feb10a215415dc8a8a392a0b35481cd7f98497d86fee77efbbe1c9c4ab417c86` | Current-revision manifest verified; package/RPM admission pending |
| RPM signer | authorized operator signing identity; staged in private Spaces | operator-controlled key policy | fingerprint `06B1C27EA0E08A225155EB3314018AA1497DDC7C`; staging evidence `WL-REL-006-rpm-signing-key-staged-r1.md` | Staged; current-revision receipt pending source freeze |

## Reproducibility and restrictions

- All rows bind to the source revision and epoch above; stale or changed bytes
  must be rejected by the owning verifier.
- No private credentials, signing keys, or provider tokens are recorded here.
- The Cuttlefish source row does not substitute for the missing Android image,
  matching `cvd-host_package.tar.gz`, signed guest declaration, or package
  manifest over those external bytes.
- The three admitted fixture rows remain clearly labeled substitutions until
  production/provider/role validation supersedes them.
