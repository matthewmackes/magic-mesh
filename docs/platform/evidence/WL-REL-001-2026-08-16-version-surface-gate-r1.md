# WL-REL-001 version-surface gate — current checkout

Farm host `172.20.0.50`, slot `rel001-version-surface`, ran:

```text
cargo metadata --no-deps --format-version 1
```

The current checkout contains 43 workspace members: 39 resolve to version
`13.0.0`; the four documented non-shipped packaging/test boundaries remain
`mackes-transport`, `magic-fleet`, `mde-kdc-host`, and `mde-kdc-proto` at
`0.0.0`. No shipped workspace member resolved to another release version.

This validates the version-surface portion of WL-REL-001 S2. It does not freeze
the source or admit release inputs; WL-FUNC-023 and WL-REL-006 remain required
before the final source-freeze receipt.
