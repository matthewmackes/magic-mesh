# Standalone Browser repository audit — 2026-08-01

## Verdict

`matthewmackes/magic-mesh-browser-stack` is **not ready for publication or
host-Browser removal**. A local directory exists at
`/root/magic-mesh-browser-stack`, but it is a clean browser-only snapshot in a
repository whose only configured remote points back to the local
`/root/magic-mesh` checkout. It is not evidence of a public, independently
buildable standalone repository.

This audit is an evidence record only. It does not authorize source deletion,
history rewriting, publication, or changes to the host workspace.

## Observed provenance

| Check | Observation |
| --- | --- |
| Source manifest | `docs/design/browser-stack-extraction/manifest.tsv`, schema v1, 199 rows, anchored at source commit `009804c8b3dca148eda3f147621608ec4f12fd17`. |
| Local candidate | `/root/magic-mesh-browser-stack`, clean worktree at `5b48542df28b8fb08ca6989ae9b3250a150ab40f`. |
| Local candidate remote | `origin` fetch/push both resolve to `/root/magic-mesh`; this is not a GitHub remote. |
| Requested remote | `git ls-remote https://github.com/matthewmackes/magic-mesh-browser-stack.git` returned `Repository not found` on 2026-08-01. |
| Source verifier | `install-helpers/verify-browser-extraction.sh --check` fails closed on the existing mixed/shared `Cargo.lock` worktree drift before extraction can be accepted. No extraction claim is made from a failed check. |

The local candidate's HEAD is a Construct release commit, not a documented
history-filtered extraction commit. Its tree contains browser files plus
`LICENSE`, `NOTICE`, `assets`, `crates`, `docs`, `install-helpers`, and
`packaging`, but no root build metadata.

## Standalone completeness blockers

The candidate is missing all of the root-level files required by the
WL-ARCH-008 outcome:

- `Cargo.toml` and `Cargo.lock`;
- `rust-toolchain.toml`;
- `.github/workflows/ci.yml`;
- `README.md` and contributor/build instructions.

The retained manifests are therefore not independently buildable. They still
inherit workspace metadata and contain path edges to material absent from the
candidate, including `mde-worker-core`, `mde-bus`, `mackes-mesh-types`,
`mde-seal`, and the shared `mde-egui` crate. The standalone candidate has no
clean-clone build proof and no published commit SHA to pin in downstream
provenance.

## Required next gates

1. Reconcile or commit the source manifest's mixed/shared dirty rows, then
   rerun `verify-browser-extraction.sh --check` from the intended source
   snapshot. Do not bypass the `Cargo.lock` drift failure.
2. In a disposable clone, perform the planned history-preserving extraction
   from that verified source snapshot. Preserve attribution, `LICENSE`,
   `NOTICE`, and the source-to-destination map.
3. Add standalone workspace/toolchain/lockfile/build instructions and either
   carry the required shared compatibility crates or replace every path edge
   with an explicit standalone contract. Add CI that builds from a clean clone
   with no sibling `magic-mesh` checkout.
4. Publish the resulting repository at the governed GitHub destination and
   record its immutable commit, remote, and clean-clone gate results in
   `UPSTREAM-SOURCE.md`.
5. Only after those gates, plus the separate live VDI guest-framebuffer proof,
   may host Browser removal proceed.

## Evidence commands

The observations above were obtained with read-only checks:

```text
git -C /root/magic-mesh-browser-stack status --porcelain=v1
git -C /root/magic-mesh-browser-stack rev-parse HEAD
git -C /root/magic-mesh-browser-stack remote -v
git ls-remote https://github.com/matthewmackes/magic-mesh-browser-stack.git
install-helpers/verify-browser-extraction.sh --check
```
