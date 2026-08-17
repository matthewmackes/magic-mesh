# WL-REL-006 Kiron admission on clean source `b64c5db5`

The committed UX-014 Kiron asset tree was verified without rerunning the
generator, from a clean immutable Git bundle at source revision
`b64c5db5efea44ec41084ae778a51fc2bd258c36` (epoch `1786924949`).

- Farm: `172.20.0.130` (BigBoy)
- Command: `packaging/kiron/verify-package.sh --source --expected-source-revision b64c5db5efea44ec41084ae778a51fc2bd258c36`
- Result: PASS
- Manifest and all 18 SVG scenes plus 6 PCM cues passed the governed hash,
  schema, wiring, and source-tree admission checks.

Running `produce-kiron-original-assets.py` from a later checkout rewrites the
manifest provenance field to that checkout's derived provenance value and
therefore changes the package tree. That generated working-tree mutation was
discarded; the clean committed source package is the admitted release input.
