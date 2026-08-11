# WL-CRIT-006 evidence — release command-control boundary (r219)

- Scope: canonical release gate matrix authority.
- Change: bounded gate commands reject shell control syntax while retaining
  safe parameter expansion used by the canonical matrix.
- Farm host: `172.20.0.50`.
- Farm slot: `crit006-command-boundary-r219`.
- Gates:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=crit006-command-boundary-r219 install-helpers/xcp-build.sh sync`
  followed by the farm self-test invocation of
  `python3 install-helpers/verify-release-gate-matrix.py --self-test`.
- Result: `1 valid, 18 hostile fixtures rejected`.
