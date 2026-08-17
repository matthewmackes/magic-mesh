# WL-REL-006 Cuttlefish guest DEBs — source `b64c5db5`

BigBoy built and verified the deterministic Cuttlefish guest runtime stage and
two guest DEBs from source revision
`b64c5db5efea44ec41084ae778a51fc2bd258c36` at epoch `1786924949`.

- Farm: `172.20.0.130` (BigBoy)
- Stage command: `packaging/android/stage-guest-runtime-artifacts.sh`
- DEB command: `packaging/android/build-guest-debs.sh`
- Stage and package verification: PASS
- Guest DEB manifest SHA-256: `0c011c017f32fb8f8f6b8c46fce9ea3e672e7e0a62f2058f67f500c73aeba6e9`
- `mcnf-cuttlefish-readiness-relay.deb`: 275468 bytes,
  SHA-256 `89635d7ad0f00af64246b84a296b77577563a350b525f7203b06dd79e40b1892`
- `mcnf-cuttlefish-vdi-agent.deb`: 263076 bytes,
  SHA-256 `f7347c5e54f468b98974e49704d6b3fc83fd3185ce914261b959789abfcd6212`

The guest packages are source-bound and verified. This does not close the
Cuttlefish release input: the Android/Cuttlefish image artifact, immutable
image receipt, and declaration remain required before preflight admission.
