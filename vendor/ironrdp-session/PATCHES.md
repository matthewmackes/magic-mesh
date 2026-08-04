# Vendored IronRDP Session Patch

This directory contains `ironrdp-session` 0.10.0 from crates.io under its
upstream MIT OR Apache-2.0 license. The source archive checksum is
`sha256:006a6a4c1da1f67b14da7e89bd9a05c9ffe8485e54320a229623d077da51f5be`.
The source is vendored so Magic Mesh can retain the dependency-compatible
IronRDP 0.10 family while carrying a narrow bitmap-decoder correctness fix.

The 0.10.0 decoder chunks bitmap rows using the clipped destination width. An
RDP update may declare a wider encoded source row than its destination
rectangle. At the lower or right framebuffer edge, interpreting that source as
additional destination rows can index beyond the framebuffer and panic.

The local patch backports the behavior present in the current upstream source:

- keep the encoded source width as the source-row stride;
- limit writes to the validated destination width and height;
- reject invalid rectangles and inconsistent source-buffer lengths; and
- exercise the clipped lower-right-edge case with unit tests.

Upstream reference:
<https://github.com/Devolutions/IronRDP/blob/master/crates/ironrdp-session/src/image.rs>

The adjacent `LICENSE-MIT` and `LICENSE-APACHE` files are the upstream license
texts. Re-evaluate and remove this vendored copy when the platform upgrades the
whole IronRDP dependency family to a release containing equivalent behavior.
