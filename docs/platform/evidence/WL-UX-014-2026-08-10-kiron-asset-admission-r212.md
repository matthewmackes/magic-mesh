# WL-UX-014 — Kiron asset admission (r212)

- Scope: the manifest validator enforces six A–F grades across three fallback
  modes, approved SPDX license, bounded regular files, exact sizes, safe paths,
  and lowercase non-zero SHA-256 hashes.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=ux014-kiron-asset-admission-r212 install-helpers/xcp-build.sh sync`; farm shell ran `python3 install-helpers/verify-kiron-assets.py --self-test`.
- Result: `Kiron asset manifest verification self-tests passed`, including digest tampering and incomplete static-fallback rejection.
