# WL-FUNC-021 — installed-seat Music action-credential runtime audit (2026-08-07)

Status: read-only audit. No seat, credential, secret, service, package, or
worklist file was mutated while collecting this evidence.

## Scope and verdict

This audit checked the installed Dell workstation (`mm@172.20.146.225`,
`DELL-LAPTOP`) and the release-5 RPM in
`/root/mcnf-release-artifacts/magic-mesh-12.1.6-5.x86_64.rpm`.

Release 5 does expose the Music action-credential provisioning path:

* `/usr/libexec/mackesd/provision-music-action-credential`
* `/usr/libexec/mackesd/music-action-credential.conf`
* `/usr/lib/systemd/system/mcnf-music-action-credential.service`
* `/opt/mcnf/automation/secrets/mcnf-secret.sh`, including its `rotate`
  subcommand

Dell has the release-5 package installed, but the path has not been
provisioned. The generated public key and encrypted private credential are
absent, the enabled oneshot is inactive, and the running user Music daemon
reports that mutation authorization is disabled. No live mutation was
authorized or attempted in this audit.

## Repository and package checks

The following commands were run locally:

```text
bash -n install-helpers/provision-music-action-credential.sh && \
  install-helpers/provision-music-action-credential.sh --self-test
  PASS — provision-music-action-credential: self-test passed

rpm -qp --qf 'name=%{NAME}\nversion=%{VERSION}\nrelease=%{RELEASE}\narch=%{ARCH}\n' \
  /root/mcnf-release-artifacts/magic-mesh-12.1.6-5.x86_64.rpm
  name=magic-mesh
  version=12.1.6
  release=5
  arch=x86_64

sha256sum /root/mcnf-release-artifacts/magic-mesh-12.1.6-5.x86_64.rpm
  44289cd2022aae466769061d058a7f68ed4168d6fb4e2b3bccd053cdb161c64e

./install-helpers/verify-rpm-payload.sh payload \
  /root/mcnf-release-artifacts/magic-mesh-12.1.6-5.x86_64.rpm
  PASS — all payload, requirements, and size checks passed
```

The RPM query listed all four paths above and declared `age`, `curl`,
`libcurl.so.4()(64bit)`, and `openssl`. Its `%post` enables
`mcnf-music-action-credential.service` and globally enables `mde-musicd.service`;
it does not run the Music provisioner immediately.

The RPM payload digests for the three package-owned provisioning files are:

```text
/usr/lib/systemd/system/mcnf-music-action-credential.service
  336b73147807ad01a5f44dc54e5161c7e8e3248310722504510a9ab7a200408b
/usr/libexec/mackesd/music-action-credential.conf
  a8d7cc4b94e2aed02d142a86ec9b9bf449570342724c958a89e9f97db44ac155
/usr/libexec/mackesd/provision-music-action-credential
  b1d332be353a1b07e9546fbbfbd4226d945b488e23491e4cca7508e41b7c9423
```

## Dell read-only results

Commands used the existing read-only SSH connection and did not invoke the
provisioner, `systemctl start/restart`, `rotate`, `put`, or `get`:

```text
ssh ... mm@172.20.146.225 hostname
  DELL-LAPTOP

ssh ... mm@172.20.146.225 rpm -q magic-mesh
  magic-mesh-12.1.6-5.x86_64

ssh ... mm@172.20.146.225 \
  "rpm -ql magic-mesh | grep -E 'provision-music-action-credential|music-action-credential|mde-musicd|mde-shell-egui' | sort"
  /usr/bin/mde-musicd
  /usr/bin/mde-shell-egui
  /usr/libexec/mackesd/music-action-credential.conf
  /usr/libexec/mackesd/provision-music-action-credential
  /usr/lib/systemd/system/mcnf-music-action-credential.service
  /usr/lib/systemd/system/mde-shell-egui.service
  /usr/lib/systemd/user/mde-musicd.service
```

The installed helper, template, and unit hashes exactly match the release-5
RPM payload hashes above. The installed helper, secret helper, and template
were executable/readable (`rc=0` for each `test`), and the packaged secret
helper is `root:root 0755`; `rpm -qf` identifies it as part of the same
release-5 RPM.

The service and generated-file state was:

```text
systemctl is-enabled mcnf-music-action-credential.service
  enabled
systemctl is-active mcnf-music-action-credential.service
  inactive
systemctl show mcnf-music-action-credential.service \
  -p ExecStart -p Result -p ActiveState -p SubState
  ExecStart={ path=/usr/bin/timeout ; argv[]=/usr/bin/timeout --signal=TERM --kill-after=5s 30s /usr/libexec/mackesd/provision-music-action-credential --refresh ; ... }
  Result=success
  ActiveState=inactive
  SubState=dead

sudo -n stat -c '%U:%G %a %s %n' \
  /etc/mde/music-action-public-key \
  /etc/credstore.encrypted/music-action-private-key
  /etc/mde/music-action-public-key — absent
  /etc/credstore.encrypted/music-action-private-key — absent
```

`/etc/credstore.encrypted` itself exists as `root:root 0700`. The shell unit's
installed drop-ins contain only the cloud and Browser-VM credentials; its
`DropInPaths` has no Music credential drop-in. The `mde-musicd` user unit is
enabled and running with `ExecStart=/usr/bin/mde-musicd serve` and
`MDE_BUS_ROOT=/run/mde-bus`. Its journal contains:

```text
mde_musicd::action_auth: music mutation authorization unavailable; mutations are disabled
```

This is consistent with the missing generated public key and is the expected
fail-closed state.

## Exact live command and authorization boundary

For an operator-authorized provisioning run when the sealed seed already
exists, the exact direct command is:

```bash
sudo -n /usr/libexec/mackesd/provision-music-action-credential --refresh
```

The package-defined systemd equivalent is:

```bash
sudo -n systemctl start mcnf-music-action-credential.service
```

Both commands materialize the encrypted root-shell credential, derive
`/etc/mde/music-action-public-key`, install the root-shell drop-in, and run a
daemon reload. `--refresh` does not restart the seat. `--init` is a separate,
high-impact bootstrap operation that creates the sealed seed only when it is
absent; it is not the normal rotation command. `--restart` additionally
`try-restart`s `mde-shell-egui.service` and can interrupt the active seat.

The Dell privilege check was read-only:

```text
sudo -n -l
  User mm may run the following commands on DELL-LAPTOP:
    (ALL) ALL
    (ALL) NOPASSWD: ALL
```

Thus the observed authorization boundary is the broad `mm` → root sudo policy,
not a narrow allowlist for this helper. This audit did not use that authority
to execute a mutating command.

## Rotation sequence and remaining package gap

The secret-store script defines rotation as `rotate <name>` with the new value
on stdin. The exact operator sequence, with the seed supplied out-of-band and
never placed in shell history or evidence, is:

```bash
printf '%s\n' '<new-64-hex-seed-from-approved-secret-input>' |
  sudo -n /opt/mcnf/automation/secrets/mcnf-secret.sh rotate music/action-ed25519-seed
sudo -n /usr/libexec/mackesd/provision-music-action-credential --restart
sudo -n -u mm env XDG_RUNTIME_DIR=/run/user/1000 \
  systemctl --user restart mde-musicd.service
```

The first command mutates the mesh secret; the second writes the new
host-bound credential/public key and restarts only the root shell; the third is
needed because `mde-musicd` loads the public key at daemon startup. The
release-5 package exposes all primitives but does not expose one atomic
Music-specific rotation/apply command, nor does the provisioner’s `--restart`
path restart the user `mde-musicd` service. A future live rotation therefore
requires explicit approval for all three mutations and a post-rotation
authorized-mutation/refusal proof.

## Boundary conclusion

Release-5 package exposure is proven. Installed-seat provisioning and rotation
are not proven: Dell currently has no generated Music credential material, and
no mutating command was run under this audit’s read-only instruction.
