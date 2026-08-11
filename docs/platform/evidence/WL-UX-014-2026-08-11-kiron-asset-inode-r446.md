# WL-UX-014 Kiron asset inode — 2026-08-11

- Scope: Kiron manifest and A–F scene verification read one bounded `O_NOFOLLOW` descriptor and revalidate identity and metadata after reading.
- Hostile boundary: a multiply-linked grade-F scene cannot retain an external mutation alias across restart.
- Focused gate: `python3 install-helpers/verify-kiron-assets.py --self-test`.
- Farm: fixed coordinator snapshot on `172.20.0.196`, slot 1.
- Result: **PASS**, hostile grade-F alias rejected.
- Remaining boundary: render and capture the verified grade-F fallback on direct-DRM hardware.
