# WL-FUNC-019 Nodes wide-LAN RDP correction — 2026-08-08

## Live diagnosis

The active seat at `172.20.0.15` and Dell at `172.20.146.225` both carry the
physical LAN as `172.20.0.0/16`. Their probe logs correctly refuse an automatic
65,536-address sweep and report that ordinary passive RDP does not advertise a
discoverable service. Neither node has an operator `probe-targets.toml` entry.

The seat's resolved physical-neighbor set contained six addresses. A bounded
TCP 3389 check of exactly those observed addresses found no open RDP listener,
and neither current probe inventory contained an RDP service. This means the
specific Windows address was not observable from the available live evidence;
its IP is still required for an explicit-target live round trip if it remains
quiet.

## Nodes-path defect and correction

The Nodes producer did not consume the probe resolver's wide-LAN fallback. Its
active `unit_aggregator::lan_scan` always swept only the `/24` derived from the
local address and rejected ARP/mDNS candidates in every other `/24`, even when
the host's real interface prefix was `/16`. Thus an already-observed Windows
host could enter `probe-inventory.json` yet remain absent from Nodes.

The Nodes scan environment now imports only the shared resolver's validated,
bounded wide-LAN neighbors. Those candidates have already passed physical
interface, real-prefix, unicast, self, duplicate, and 128-entry limit checks;
Nodes fingerprints them against its existing bounded port set, including RDP
on TCP 3389. It does not expand or sweep the `/16`.

## Verification

- Farm 9 (`.50`), isolated slot `func019-win-nodes-fmt-r1`: targeted
  `rustfmt --check` passed for `unit_aggregator/lan_scan.rs`.
- Farm 193 (`.90`), warm isolated slot `arch010-small-profile-r2`: focused test
  `observed_wide_lan_neighbor_reaches_rdp_fingerprint` passed, one passed and
  zero failed. The fixture places the node at `172.20.146.225`, the observed
  Windows host at `172.20.145.77`, and proves RDP classification across the
  otherwise-discarded `/24` boundary.

## Remaining boundary

This source correction is not yet installed on a seat. A completely quiet
Windows host cannot be inferred without an address or advertisement; provide
its IP through the governed explicit-target route, then install a corrected
build and capture the Nodes-to-RDP live round trip before closing FUNC-019.
