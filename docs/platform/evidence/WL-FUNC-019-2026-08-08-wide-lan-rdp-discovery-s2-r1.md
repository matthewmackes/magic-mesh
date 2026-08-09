# WL-FUNC-019 wide-LAN Windows RDP discovery — 2026-08-08

The node probe previously skipped active scanning for wide local CIDRs. A quiet
Windows 10 host with RDP enabled but no usable mDNS/SSDP advertisement therefore
never became a resource card unless its address was explicitly configured.

The probe now detects when wide local CIDRs were skipped and supplements the
safe scan set with at most 128 valid observed-neighbor addresses. Invalid,
duplicate, multicast, unspecified, loopback, and non-neighbor rows do not
consume the bound. If this still cannot find the host, one actionable warning
per daemon process names the deterministic configuration route:
`~/.config/mde/probe-targets.toml` with `targets = ["<windows-ip>"]`.

This intentionally does not sweep an entire `/16`: Windows may remain absent
until it has produced a neighbor-table observation or the operator supplies its
IP, avoiding an unbounded LAN port scan.

## Verification

- Farm `.90`, slot `func019-win-rdp-discovery-s2-r2`: focused `probe_nmap`
  suite passed 45/45 after the bounded-neighbor refinement.
- The current integrated tree repeated the same 45/45 result on `.50`, slot
  `integrated-win-rdp-s6-r1`.
- Fixtures cover skipped-wide-CIDR detection, valid-neighbor admission, hostile
  row filtering before the 128-address bound, deduplication, and warning
  suppression after the first actionable notice.
- Scoped formatting and whitespace checks passed.

## Remaining acceptance gap

A live Windows 10 discovery/connect round trip and complete universal-resource
UI proof remain, so FUNC-019 stays `Remaining`.
