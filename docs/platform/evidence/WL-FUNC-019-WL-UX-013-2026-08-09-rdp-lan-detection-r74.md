# WL-FUNC-019 / WL-UX-013 — quiet Windows LAN RDP detection r74

Date: 2026-08-09

## Reported symptom and diagnosis

Node 15 was reported not to detect an online Windows 10 laptop with Remote Desktop enabled; the Dell laptop is on the physical LAN but is not a mesh peer.

The production path was traced through `unit_aggregator::lan_scan::LiveScanEnv`, `build_records`, `probe_one`, and the `LanHostRecord` projection.

- Discovery is not restricted to mesh peers. This producer has no peer-set input; it scans physical-LAN candidates and intentionally excludes overlay interfaces and self.
- TCP 3389 was already an approved fixed fingerprint port and already mapped to the `rdp` label. The defect was that the fingerprint stage only ran after candidate admission.
- Same-subnet candidate admission required at least one of successful ICMP, an ARP/neighbour row, or mDNS. A quiet Windows host that rejects ping and has not populated local ARP/mDNS therefore never reached the TCP 3389 probe, even while RDP accepted connections.
- The active sweep remains deliberately IPv4 and `/24` bounded. Wider-prefix discovery still admits only interface/prefix/size-validated observed neighbours; this correction does not sweep a `/16` or arbitrary CIDR.
- Bus publication is downstream of `LanHostRecord` production. A stale Bus can delay visibility, but it did not explain why this code path never produced the quiet host.

## Correction and safety boundary

The existing local `/24` sweep now checks one approved TCP port, 3389, for each ping-silent address under the existing 64-thread and 400 ms connect bounds. A successful presence probe admits that address for the normal fingerprint pass. The normal pass independently probes the complete approved fingerprint set; only a successful 3389 observation there emits `rdp`, port 3389, and the `computer` type. A transient first success followed by a failed fingerprint can therefore produce an unclassified host, never a fabricated RDP service.

The change does not add arbitrary ports, mesh-only requirements, broad CIDR enumeration, UDP probes, authentication attempts, or an RDP handshake. It runs only while the existing surface-gated scan is active. The worst-case added scope is exactly one bounded TCP 3389 connect per ping-silent address in each already-enumerated local `/24`.

## Focused BigBoy verification

Host: BigBoy `172.20.0.130`

Slot: `rdp-lan-detection-r74`

The farm helper performed the explicit isolated sync:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=rdp-lan-detection-r74 \
install-helpers/xcp-build.sh sync
```

Unrelated concurrent changes in `transfers/mod.rs`, `kvm_health.rs`, and `workload_compute.rs` were preserved locally. Their committed `HEAD` forms were overlaid only in the disposable r74 farm workspace so they could not contaminate this lane's compile result.

Each focused test used:

```text
cargo test -p mackesd --features async-services \
  workers::unit_aggregator::lan_scan::tests::<name> \
  -- --exact --nocapture
```

Results:

- `ping_silent_same_subnet_rdp_host_is_probed_and_classified`: PASS — 1 passed, 0 failed, 4570 filtered out. No ping, ARP, mDNS, mesh identity, or wide-neighbour hint was present; successful TCP 3389 alone reached the independent fingerprint and produced RDP.
- `failed_rdp_probe_does_not_admit_or_classify_ping_silent_host`: PASS — 1 passed, 0 failed, 4570 filtered out. The empty `/24` produced no records and exactly 253 bounded TCP probes, one for each non-self host address.
- `port_fingerprint_maps_service_labels_and_type_guess`: PASS — 1 passed, 0 failed, 4570 filtered out. Existing RDP/VNC and SSH fingerprint classification remains intact.

Formatting and scoped integrity:

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/unit_aggregator/lan_scan.rs
Result: PASS

git diff --check -- \
  crates/mesh/mackesd/src/workers/unit_aggregator/lan_scan.rs
Result: PASS

git diff --no-index --check /dev/null \
  docs/platform/evidence/WL-FUNC-019-WL-UX-013-2026-08-09-rdp-lan-detection-r74.md
Result: no whitespace diagnostics; exit 1 is expected for a new untracked file.

sha256sum crates/mesh/mackesd/src/workers/unit_aggregator/lan_scan.rs
4de6e7718bcc70e7ec972127084caa876e2a3b99e8599bd04d715366e107f317
```

The local and r74 farm source hashes matched exactly.

## Remaining live-network evidence gap

No live command was run against node 15 or the Dell/Windows laptop in this slice, and no corrected artifact was installed there. These deterministic tests prove the formerly missing production code path and its bounds; they do not prove the laptop's current address, Windows firewall/profile state, TCP 3389 reachability from node 15, scan-surface activation, Bus publication, typed card projection, credentials, login, or rendered RDP session. A deployed node-15 scan observing a successful TCP 3389 connection and the resulting fresh card remains required live evidence.
