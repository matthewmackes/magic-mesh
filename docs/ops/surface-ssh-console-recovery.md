# Surface SSH recovery from the local console

This procedure is intentionally **preflight-only and blocked**. Repository
audit found no tracked, approved operator SSH public key and no pinned SHA-256
fingerprint for one. Host-local `/root/.ssh/mackes_mesh_ed25519.pub` files and
the dynamically sealed mesh SSH key are not authorization artifacts and must
not be copied into this workflow. Do not paste, generate, download, or select a
substitute key to get around this blocker.

The helper is console-only, read-only, and emits one bounded JSON record. On
the canonical Surface Pro 6, switch to a local Linux virtual console and run:

```console
sudo install-helpers/preflight-surface-ssh-console-recovery.py --preview
```

Surface Pro 5 is never inferred from a generic model name. Its operator must
explicitly request generation 5; the helper then requires Microsoft DMI product
`Surface Pro` and SKU `Surface_Pro_1796` (Wi-Fi) or `Surface_Pro_1807` (LTE):

```console
sudo install-helpers/preflight-surface-ssh-console-recovery.py \
  --preview --generation 5
```

Exit `3` and `status=blocked` are mandatory today. The preview validates a
physical `/dev/ttyN` stdin, root execution, exact Microsoft generation, Fedora
44, the Pro 6 hostname `Surface`, root ownership and restrictive modes for
root's SSH files, and the effective `sshd -T` root public-key policy. It checks
only the reserved package paths
`/usr/share/magic-mesh/recovery/surface-root-ed25519.pub` and
`surface-root-ed25519.sha256`; it accepts no path, key, fingerprint, password,
private key, address, or environment override. Command input is `/dev/null`,
runtime and accepted command output are bounded, stderr/key contents are excluded from the
JSON, and the helper performs no network, service, firewall, reboot, or file
mutation.

`--commit` is present to make the preview/explicit-commit boundary unambiguous,
but it exits `3` without inspecting or writing any credential. Enabling commit
requires a separately reviewed repository change that supplies the approved
public key plus exact OpenSSH SHA-256 fingerprint, defines their provenance and
rotation owner, and atomically installs only that pair into the already-admitted
root `AuthorizedKeysFile`. That future change must retain every preflight gate,
recheck immediately before rename, preserve the prior file on failure, and emit
no public-key body or operational secret.

Fixture regression:

```console
install-helpers/preflight-surface-ssh-console-recovery.py --self-test
```
