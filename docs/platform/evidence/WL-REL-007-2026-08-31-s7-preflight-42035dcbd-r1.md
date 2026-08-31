# WL-REL-007 / WL-REL-001 S7 preflight at current HEAD `42035dcbd` — 2026-08-31

Classification: dest rebind + first-release preflight pass at the then
clean HEAD. Not freeze. Not `production_admitted`. Surface `bootc_base`
stays null.

Tree: `42035dcbd` epoch `1788153988`. Same dest bytes as the
`73828796f` pass. Farm cargo was already fresh.

## Result

BigBoy `mm` + `MAGIC_MESH_SIGN_KEY=06B1C27EA0E08A225155EB3314018AA1497DDC7C`:

```
release-input-preflight: PASS: all mandatory first-release inputs admitted for 42035dcbd76b03b8323399892052b21a96e2e233
```

Private argv: `/root/mcnf-private/release-preflight-42035dcbd.json`
(mode 0400). Candidate record:
`/root/mcnf-private/source-input-candidate-42035dcbd.json`
(`final_freeze: false`). Canonical Maps dest was not replaced.

Recording this evidence is a source change; final freeze still requires
reconfirming the same revision after REL-006 admission. Do not grind
`cargo test --workspace`.
