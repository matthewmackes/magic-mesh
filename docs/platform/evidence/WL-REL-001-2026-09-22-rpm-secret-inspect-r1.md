# WL-REL-001 official RPM signing-identity inspect — 2026-09-22 r1

Classification: official secret-backed inspect of the already-bound freeze
SHA receipt. Not freeze. Not dest-cut. Not `production_admitted`. This
does **not** unpark tagging or publication (`WL-REL-005` / `repo_gpgcheck`
still wait on `WL-REL-003` S6). No dest invented. `.131` was not started.

Date: 2026-09-22T16:55:00Z  
Host: `mcnf-build-52` (`172.20.0.130`) as `mm`  
Tree: freeze SHA `42035dcbd76b03b8323399892052b21a96e2e233` epoch
`1788153988` (detached worktree over `/home/mm/magic-mesh`; original
checkout `fbe24f95b` / `agent/drain-worklist-20260725` left untouched)

## Secret presence (fingerprint only)

`gpg --batch --no-options --with-colons --fingerprint --list-secret-keys 06B1C27EA0E08A225155EB3314018AA1497DDC7C`
resolved exactly one primary `sec` record whose fingerprint is
`06B1C27EA0E08A225155EB3314018AA1497DDC7C`. No secret key material was
exported or printed. Control-host `gpg` still has no secret; inspect
must run on `.130`.

## Inspect

Receipt copied from control-host
`/root/mcnf-private/rpm-signing-identity-42035dcbd.json` into
`/home/mm/mcnf-rpm-secret-inspect-42035dcbd/` (dir mode `0700`, receipt
mode `0400`). SHA-256
`582af51a94a788ec09c0a76e5b5c87aaa9ae41a204a32818af27af76ac28eb26`.

Command (from the freeze-SHA tree):

```
python3 install-helpers/produce-rpm-signing-identity-receipt.py inspect \
  --receipt /home/mm/mcnf-rpm-secret-inspect-42035dcbd/rpm-signing-identity-42035dcbd.json \
  --expected-source-revision 42035dcbd76b03b8323399892052b21a96e2e233 \
  --expected-release-epoch 1788153988 \
  --signing-identity 06B1C27EA0E08A225155EB3314018AA1497DDC7C
```

Result: **PASS** (exit 0). Printed fingerprint:
`06B1C27EA0E08A225155EB3314018AA1497DDC7C`.

## Not solved here

- Tagging and publication remain parked.
- Helpers still refuse to self-mark `production_admitted`.
- Native F44 `.131` was not started.
- Surface `bootc_base` / dest-operator leftovers are unchanged.
