# WL-FUNC-020 Android input generation binding — r542

Date: 2026-08-13

## Production seam

The Workloads Android projection previously accepted a valid, fresh WebRTC
source without comparing its generation to the successful running lifecycle
receipt. That allowed a delayed source from an older Android operation to be
offered for Remote Sessions attachment and eventual guest-input focus after a
newer generation had become authoritative.

`iac/android_governed.rs` now exposes `Running` and the typed attach source only
when the source belongs to the exact workload and its generation equals the
successful running receipt generation. A source without that receipt, or from
an older generation, remains visibly unavailable and cannot become an attach or
guest-input target. Stop remains available so the authoritative running
generation can still be cleaned up.

The hostile regression presents a valid, current, catalog- and image-bound
generation-7 source beside an authoritative generation-8 running receipt. It
proves the source is withheld, then proves the otherwise identical generation-8
source is admitted.

## Gates

- BigBoy `.130`, slot 2: focused
  `governed_android_input_attachment_requires_exact_running_generation` passed
  1/1 (1608 filtered out).
- BigBoy `.130`, slot 3: `cargo build -p mde-shell-egui --all-targets
  --all-features` passed.
- `.90`, slot 2: strict `cargo clippy -p mde-shell-egui --all-targets
  --all-features -- -D warnings` reached the shell and remained red only at the
  pre-existing, out-of-scope `communications/mod.rs:608`
  `clippy::while_let_loop`; the Android/IAC changes emitted no diagnostic.
- Exact package format check ran once and identified one line-wrap difference
  in this slice; that exact formatter output was applied. Per the no-rerun
  directive, the check was not repeated.
- Scoped `git diff --check` passed before evidence finalization.

## Residual FUNC-020 work

- The shipped shell still needs a real Cuttlefish WebRTC seat-side decoder; the
  current generic VDI module truthfully refuses because none is linked.
- The first release must consume the signed Cuttlefish image and deterministic
  guest packages.
- Live one-node nested-KVM, frame, input, audio, reconnect, isolation, upgrade,
  and UX acceptance remains deferred until after the first release.
