# WL-ARCH-009 job playbook bound — 2026-08-11

- Scope: process-isolated local job execution.
- Change: signed playbooks are capped at 1 MiB and validated as UTF-8 before digest verification or apply, preventing oversized input from consuming executor memory.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch009-job-playbook-bound-r226 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::job_exec::tests::oversized_playbook_is_rejected_before_digest_or_apply -- --exact --nocapture`.
- Result: PASS — 1 passed, 0 failed.
- `git diff --check`: PASS.
