# WL-RUN-003 disposition — 2026-07-23

**Done.** The controlled DigitalOcean add-retire-add drill passed on the
smallest `s-1vcpu-512mb-10gb` shape: a retired lighthouse was drained and
removed without losing quorum, then a replacement joined through the typed
enrollment path. The final three-node mesh reports all etcd endpoints healthy,
all members started, pairwise Nebula overlay pings passing, and every peer as
`class=lighthouse`, `health=healthy`, `media=false`. Each node runs only
`magic-mesh-lighthouse` plus the Nebula hard dependency; the published RPM is
10.3 MiB and its payload excludes media, file-sharing, browser, and
virtualization assets. Evidence: live checks on 10.42.0.1/.2/.3 and
`magic-mesh-lighthouse-12.1.0-1.x86_64.rpm` payload verification.
