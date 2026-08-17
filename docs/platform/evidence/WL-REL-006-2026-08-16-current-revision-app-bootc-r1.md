# WL-REL-006 current-revision App VM and bootc receipts

These two non-secret registry receipts were regenerated from the clean,
pushed source `b64c5db5efea44ec41084ae778a51fc2bd258c36` at commit epoch
`1786924949`. They are input receipts only; they do not claim release or live
seat acceptance.

## App VM

- Farm: `172.20.0.90`, slot 1
- Reference: `quay.io/fedora/fedora:42`
- Architecture: `amd64`
- Resolved digest: `sha256:e78cd1a688cd079c23864f289a89a49a3f4ad66d817864e325e1d058310ee95c`
- Receipt SHA-256: `b46b58ea56ef78ae30b1d61a169b9ac62a24baec550beeb02f843308d1f7b297`
- Producer: `packaging/app-vm/produce-base-image-receipt.py`

## bootc

- Farm: `172.20.0.130` (BigBoy), slot 1
- Reference: `quay.io/fedora/fedora-bootc:44`
- Architecture: `amd64`; role: `base`
- Resolved digest: `sha256:295dd6ecda23780e9babf6a889914762ae118c621819d777c879992884d2b681`
- Receipt SHA-256: `1066724f390743e5a459a792a440760d329203cf718c99bcd25444e164a76d93`
- Producer: `install-helpers/produce-bootc-digest-receipt.py`

Both producers verified the source revision and epoch from an immutable Git
bundle on the farm and performed live registry inspection. The receipts remain
outside Git for the private preflight input set.
