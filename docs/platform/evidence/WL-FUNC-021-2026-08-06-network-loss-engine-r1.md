# WL-FUNC-021 — daemon network/provider-loss engine audit (2026-08-06)

## Scope

This note covers only `crates/services/mde-musicd/src/engine.rs`. The audit
followed the native decoder's HTTP stream, Symphonia packet-read handling, and
the ordered `PlaybackTrack` source-candidate policy.

## Concrete improvement

The live/unseekable HTTP path previously copied the `reqwest` response into a
pipe and discarded the producer's `io::copy` result. A provider disconnect
could therefore reach Symphonia only as an apparent EOF, causing the daemon
to mark the source complete and advance without distinguishing normal stream
completion from network loss.

The engine now keeps the `reqwest` response as the unseekable decoder source.
Packet reads treat only Symphonia `IoError` with
`std::io::ErrorKind::UnexpectedEof` as clean completion; other read errors are
returned to the candidate policy. The Opus path uses the same rule. Decoder
errors other than recoverable `DecodeError` are also surfaced.

Fallback remains bounded: a next admitted source is tried only when the
failed source emitted no decoded frames for the logical queue track. If a
provider fails after audio has begun, the daemon does not replay that track
from byte zero; it preserves one queue boundary and advances, avoiding
duplicate audio.

## Focused regression coverage

- `packet_read_only_treats_unexpected_eof_as_clean_completion` proves clean
  EOF, connection reset, and malformed packet errors are classified
  differently.
- `fallback_is_bounded_to_failures_before_audio` proves partial playback does
  not trigger a from-zero fallback replay.

## Farm verification

- `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=musicd-network-loss-r2 install-helpers/xcp-build.sh cargo test -p mde-musicd engine -- --nocapture`
  passed **18/18** engine-filtered tests; the remaining package tests were
  filtered by the command.
- `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=musicd-network-loss-fmt-r2 install-helpers/xcp-build.sh cargo fmt -p mde-musicd -- --check`
  passed.
- `git diff --check` passed.

## Remaining boundary

This is not a claim of mid-track resume. After a loss following emitted audio,
the daemon advances rather than guessing a byte/time resume point. Resuming
the same provider stream would require an explicit seek/range contract and
stable source identity; that live recovery behavior remains open for a future
worklist slice. Initial provider failure still uses the existing fully cached
track fallback when one is available.
