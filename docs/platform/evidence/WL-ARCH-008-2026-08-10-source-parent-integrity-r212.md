# WL-ARCH-008 — Browser source-parent integrity (r212)

- Scope: Browser migration rejects symlinked or non-directory source ancestors,
  preventing redirected source provenance and bundle publication.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch008-source-parent-integrity-r212 install-helpers/xcp-build.sh sync`; farm shell ran `python3 install-helpers/verify-browser-portable-boundary.py --self-test`.
- Result: `PASS`; migration and portable-boundary self-tests passed.
