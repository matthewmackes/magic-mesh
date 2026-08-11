# WL-FUNC-021 PipeWire dump bound — 2026-08-11

- Scope: seat-wide Clock ducking audio authority.
- Change: `pw-dump` output is refused above 16 MiB before JSON parsing, preventing malformed provider output from consuming unbounded memory.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func021-pw-dump-bound-r223 install-helpers/xcp-build.sh cargo test -p mde-musicd --lib seat_audio::tests::pw_dump_refuses_oversized_provider_output_before_json_parse -- --exact --nocapture`.
- Result: PASS — 1 passed, 0 failed.
- `git diff --check`: PASS.
