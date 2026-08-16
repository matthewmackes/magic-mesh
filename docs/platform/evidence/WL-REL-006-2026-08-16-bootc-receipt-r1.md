# WL-REL-006 bootc receipt — current revision

This evidence records the farm-produced bootc input for the current checkout.
It is an input receipt, not a release or seat-admission claim.

## Source identity

- Source revision: `52fd079333d0005f7810744c3956efba3da492c9`
- Commit epoch: `1786921532`
- Farm host: `172.20.0.170`
- Farm slot: `rel006-bootc`
- Receipt producer: `install-helpers/produce-bootc-digest-receipt.py`
- Receipt SHA-256: `199fec16a07357019e61206c44c062fb079dbbd35039a070bd08b8d7dd70a5e1`

## Resolved input

- Reference: `quay.io/fedora/fedora-bootc:44`
- Architecture: `amd64`
- Release role: `base`
- Manifest media type: `application/vnd.oci.image.index.v1+json`
- Resolved digest: `sha256:295dd6ecda23780e9babf6a889914762ae118c621819d777c879992884d2b681`

## Commands and result

The farm job supplied an immutable Git bundle so the producer could verify the
source revision without weakening the no-`.git` farm workspace policy:

```text
python3 install-helpers/produce-bootc-digest-receipt.py --repo /tmp/rel006-bootc-repo-2 produce \
  --image-reference quay.io/fedora/fedora-bootc:44 --architecture amd64 \
  --source-revision 52fd079333d0005f7810744c3956efba3da492c9 \
  --commit-epoch 1786921532 --release-role base
```

The command completed successfully after live registry inspection. The
canonical receipt was copied out of the farm and hashed above.

## Scope and remaining admission

This closes the bootc receipt-generation portion of WL-REL-006 S5 for the
current revision. It does not satisfy the full release-input preflight or
claim that a production release is ready; the other mandatory input families
and the private mode-0400 preflight argv remain outstanding.
