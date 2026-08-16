# WL-REL-006 App VM base receipt — current revision

This evidence records the farm-produced App VM base-image receipt. It is a
registry input receipt, not a built-image or live-seat acceptance claim.

- Source revision: `0e0cd1b34314614e6380c03326d835b13c885039`
- Commit epoch: `1786921850`
- Farm host: `172.20.0.90`
- Farm slot: `rel006-appvm`
- Producer: `packaging/app-vm/produce-base-image-receipt.py`
- Receipt SHA-256: `7fcfd11ebc079ac8e30f25266ce536b8964df01906bfc08b9b8a3474d6807135`
- Image reference: `quay.io/fedora/fedora:42`
- Architecture: `amd64`
- App VM target/profile: `mcnf-app-vm/wayland-standard-v1` / `wayland-standard`
- Manifest digest: `sha256:e78cd1a688cd079c23864f289a89a49a3f4ad66d817864e325e1d058310ee95c`
- Platform digest: `sha256:63773f454664cd77e239f8e0b13ae7f18effe9e3d6612a325b5646eb3bda11f1`
- Media type: `application/vnd.oci.image.index.v1+json`

The producer validated the source revision and epoch from an immutable Git
bundle on the farm, performed live registry inspection with `skopeo`, and
published the canonical mode-0400 receipt. No image-context mutation occurred.

This advances WL-REL-006 S3. Building the derivative image, RPM admission, and
live-seat acceptance remain downstream work.
