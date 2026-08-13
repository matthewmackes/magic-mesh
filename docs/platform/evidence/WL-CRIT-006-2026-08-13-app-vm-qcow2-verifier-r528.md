# WL-CRIT-006 — App VM qcow2 release verifier (r528)

Date: 2026-08-13

The first-release derivative pipeline now re-admits its App VM qcow2 through a
role-appropriate verifier before atomic collection publication. The verifier
requires the exact `mcnf-app-vm/wayland-standard-v1` profile, requested source
revision, canonical manifest shape, matching filename/size/SHA-256, qcow2 magic,
private immutable inputs, and stable single-link image identity throughout the
read. This gives the seven-role release collector a repository-owned verifier
argv accepting `{artifact}`, `{source_revision}`, and a companion manifest.

## Farm evidence

- `.50`, slot `crit006-app-qcow-r528`:
  `python3 packaging/app-vm/test-verify-qcow2-manifest.py` passed the positive
  fixture and hostile revision, profile, kind, size, digest, raw-image, symlink,
  requested-revision, and duplicate-key cases.
- `.196`, slot `crit006-app-qcow-static-r528`:
  Python compilation and tabnanny passed for the verifier and hostile suite.
- Local Python compilation and `git diff --check` passed.

The derivative orchestration suite and strict ShellCheck are rerun after this
verifier wiring; publication remains forbidden unless all owning verifiers pass.
