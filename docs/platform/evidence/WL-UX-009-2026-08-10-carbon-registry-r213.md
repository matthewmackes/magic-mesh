# WL-UX-009 — Carbon icon registry drift gate (r213)

- Scope: the icon gate enforces exact registry/asset parity, safe names,
  symbolic `currentColor` SVGs, supported elements, and Apache-2.0 provenance.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=ux009-carbon-registry-r213 install-helpers/xcp-build.sh sync`; farm shell ran `python3 install-helpers/lint-carbon-icon-registry.py --self-test && python3 install-helpers/lint-carbon-icon-registry.py`.
- Result: self-tests passed; `[OK] Carbon icon registry: 44 assets, exact parity, symbolic SVGs, Apache-2.0 provenance`.
