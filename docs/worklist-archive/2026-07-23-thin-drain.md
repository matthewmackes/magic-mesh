# Thin-lighthouse drain dispositions — 2026-07-23

## WL-SEC-005 — Done

Cloud-worker identifiers are validated as bounded single path components at the
storage sinks; hostile desired/image/container/placement/lifecycle requests are
refused before filesystem or backend I/O. The current-tree farm cloud suite
passed 112/112, and the integrated format and diff checks passed.

## WL-BUILD-004 — Done

The canonical farm coverage command and CI status stage now run the governed
workspace with `--locked` and a hard 80% line floor. BigBoy coverage passed at
84.67% lines over 601,432 lines (3,719 `mackesd` tests plus the full workspace),
and the current-tree `mackesd` library gate passed 3,719/3,719. Bootstrap,
ShellCheck, policy self-tests, formatting, and diff checks also passed.

The same drain enforces the 2026-07-23 thin-lighthouse policy: role pinning,
install profiles, onboarding, directory/DNS discovery, worker gates, transfer
destinations, and media helper entrypoints reject or ignore the retired media and
file-sharing lighthouse class. Media workloads remain possible only on
explicitly provisioned non-lighthouse hosts.
