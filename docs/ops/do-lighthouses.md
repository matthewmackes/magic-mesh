# On-demand DigitalOcean lighthouses (Option A: doctl + cloud-init)

Stand up a MCNF founding lighthouse on a fresh DO Fedora droplet with one
command. Each droplet founds its **own** mesh — `AI_GOVERNANCE.md` §8 (one
founding lighthouse per mesh, ≤8 flat-trust peers). "Replicate lighthouses on
demand" = spin up the Nth independent mesh; it is **not** multiple lighthouses
for one mesh (that needs the out-of-scope multi-lighthouse roster work).

## Pieces

- **`install-helpers/do-lighthouse-cloudinit.sh`** — the cloud-init user-data DO
  runs as root on first boot: read the droplet's public IP from the DO metadata
  service → install `magic-mesh` (+ `nebula`) → `mackesd found <mesh>
  --external-addr <public-ip>` → open `4242/udp` + `<enroll-port>/tcp` +
  `443/tcp` → `systemctl enable --now mackesd` (activates the `/enroll`
  listener) → write the v3 join token to `/root/mesh-join-token.txt`.
- **`install-helpers/do-lighthouse-up.sh`** — the operator-side `doctl` wrapper:
  render the cloud-init, ensure a DO Cloud Firewall for the ports, create the
  droplet with the user-data, wait for bootstrap, and print the IP + join token.

## Use

```sh
doctl auth init                       # once
./install-helpers/do-lighthouse-up.sh acme-mesh \
    --region nyc3 --size s-1vcpu-512mb-10gb --image fedora-43-x64
```

Output ends with a ready-to-paste line:

```
mackesd join 'mesh:acme-mesh@<ip>:4243#<bearer>?fp=<sha256>'
```

Run that on any joining box (with the new build), or `mde-enroll` and paste it.

Options: `--region --size --image --ssh-key --repo-baseurl --rpm-url
--enroll-port --tag --keep-on-fail` (see `--help`). The wrapper rejects every
role other than the thin `lighthouse` control-plane role.

The default is the small control-plane profile documented in
[`docs/design/digitalocean-lighthouse-small.md`](../design/digitalocean-lighthouse-small.md).
Provisioning applies its cgroup, swap, journal, weak-dependency, and
optional-service guardrails automatically. Media and file-sharing lighthouses
are retired; place those duties on a non-lighthouse node instead.

## The glibc / image prerequisite (important)

The mesh binaries are portable — the F44-built `mackesd` runs on F43 (glibc
2.42) unchanged. What blocks a plain `dnf install` on an **older** DO Fedora
image is the RPM's auto-generated `Requires` pinning a newer glibc symbol
version, not a real runtime incompatibility. So:

- If the dnf channel has a `fedora-<releasever>-x86_64/` dir matching the DO
  image, `dnf install magic-mesh` just works.
- For an older DO image without a matching channel dir, pass a **portable RPM**
  (ONBOARD-7) via `--rpm-url <url>`; the cloud-init installs it directly.

Pick a `--image` whose Fedora version you publish a channel for, or finish
ONBOARD-7 and point `--rpm-url` at the portable build.

## Firewall

The droplet must accept inbound `4242/udp` (Nebula), `<enroll-port>/tcp` (the
`/enroll` endpoint, default 4243), and `443/tcp` (covert tunnel). The up-script
creates a DO Cloud Firewall `magic-mesh-<tag>` bound to the droplet's tag; the
cloud-init also opens firewalld host-side as a backstop.

## Token retrieval & teardown

The join token lands at `/root/mesh-join-token.txt`; the up-script fetches it
over SSH (it needs your DO ssh-key's private half locally). Re-fetch any time:
`ssh root@<ip> cat /root/mesh-join-token.txt`. Mint more single-use tokens with
`mackesd enroll-token` on the lighthouse. Tear down with `doctl compute droplet
delete lh-<mesh>-<n>` (and the firewall if no droplets remain on the tag).

## Next (Option D)

When this becomes a recurring platform need, fold it into a product verb —
`magic-fleet lighthouse up <mesh-id>` wrapping the DO API + this cloud-init +
firewall + token capture — so provisioning is a feature, not ops glue.
