# MDE Workbench — navigation grouping redesign (NAV-1)

**Status:** locked 2026-06-14 via a 15-question operator survey. Supersedes the
ad-hoc 15-group nav in `crates/workbench/mde-workbench/src/model.rs`.

## Problem

The left nav grew to **15 top-level groups / ~70 panels** with no consistent
taxonomy:

- "Inventory" meant three different things (this node's hardware, the peer list,
  the fleet roster).
- Mesh-wide concerns were scattered across **This Node** (Mesh Services),
  **Controller** (Mesh Control), and **Network** (Mesh DNS/Storage/Bus/
  Federation/Pending/History/Join).
- Local **desktop settings** (Apps, Devices, Look & Feel, Sound, Displays,
  System) sat alongside mesh operations.
- Config was split three ways (This Node Config, Controller Config, Settings,
  Policy); observability scattered across four groups.

## Locks (survey answers)

| # | Decision | Answer |
|---|----------|--------|
| 1 | Primary organizing axis | **Hybrid: scope → function** |
| 2 | Desktop settings | **Defer to Cosmic Settings** (Workbench goes mesh-focused) |
| 3 | "Inventory" triplicate | **Keep Peers + Fleet; move hardware into This Node** |
| 4 | Top-level group count | **5–7 target** (dedicated Monitoring + Config push to ~8) |
| 5 | Scattered Mesh* items | **One Mesh section** with function sub-groups |
| 6 | "Controller" group | **Merge into Fleet as Orchestration** |
| 7 | Local networking (Interfaces/Wi-Fi/VPN/Firewall) | **Keep under This Node** |
| 8 | Mesh Storage | **Under the Mesh section** |
| 9 | Peers (front door) | **Top of the Mesh section** |
| 10 | Onboarding/join | **Keep Provisioning; fold join (Registration/Join/Pending) into Mesh** |
| 11 | Observability | **One Monitoring section** |
| 12 | Config | **Unified Config area** (Local / Fleet / Policy) |
| 13 | Naming | **Full plain-language rename pass** |
| 14 | Default landing | **Overview / Home** |
| 15 | Top order | **Overview → This Node → Mesh → Fleet → Provisioning → Monitoring → Config → Maintain → Help** |

## Resulting taxonomy

Order is locked (Q15). `→` marks a renamed label (Q13).

1. **Overview** — Home *(default landing, Q14)*
2. **This Node** — Status · Hardware *(was This-Node "Inventory")* · Mesh Services
   · **Network** sub-group: Interfaces · Wi-Fi · VPN · Firewall · Remote Access
3. **Mesh** — **Peers** *(first, Q9)* · Mesh Control · Mesh Storage · Mesh DNS ·
   Routing · Mesh Federation · Message Bus *(← Mackes Bus)* · Published Services
   *(← Service Publishing)* · Discovered Hosts *(← Network Hosts)* ·
   **Join** sub-group: Registration · Mesh Join · Mesh Pending
4. **Fleet** — Fleet Roster *(← Fleet Inventory)* · Fleet Rollup · Tags
   *(← Capability Tags)* · **Orchestration** sub-group: Jobs · Playbooks ·
   Remediation
5. **Provisioning** — Node Roles · Install Profiles · Images · Mirrors ·
   Instances *(Compute folded in)*
6. **Monitoring** — Health · Logs & Metrics · Fleet Logs · Run History · Audit ·
   Mesh History · Resources · System Logs
7. **System** *(Config + Maintain + Help combined — operator follow-up
   2026-06-14)* — **Config** sub-group: Local · Fleet *(revision push)* · Policy
   *(unified, Q12)* · **Maintenance** sub-group: Hub · Snapshots · Debloat ·
   Repair · **Help** sub-group: Help Topics · About

**Deferred to Cosmic Settings (removed from Workbench, Q2):** Apps (Install/
Installed/Remove/Sources/Default Apps/Panel Apps), pure Devices (Displays/
Keyboard/Mouse/Power/Session/Sound/Printers/Removable), Look & Feel (Themes/
Fonts/Wallpaper/Panel Sync), System desktop bits (Date & Time/Notifications/
System Update). **Kept (mesh-relevant):** Connected Devices + phone pairing and
Music move under **Mesh** as mesh-peer services rather than being dropped.

### Count reconciliation (Q4)

Deferring desktop settings removes ~4 big groups (Apps, Devices, Look & Feel,
most of System), taking 15 → 9. Combining Config + Maintain + Help into one
**System** section (operator follow-up) lands at **7 top-level sections** — back
inside the 5–7 target (Q4). Monitoring stays dedicated (Q11); Config becomes a
sub-group of System rather than its own top section, keeping the unified Local/
Fleet/Policy tabs (Q12) intact.

## Acceptance (runtime-observable)

- The left nav renders exactly the 9 sections above, in the locked order, with
  Overview/Home selected on launch.
- No nav label is ambiguous or duplicated (no three "Inventory"s; renames applied).
- Every retained panel routes to its existing working view (no dead nav entry,
  no "coming soon"); deferred desktop panels are gone from the nav.
- `Group::all()` order matches Q15; `Group::from_slug` round-trips every section.
- `cargo test -p mde-workbench` green; renders through `mde-theme` Carbon tokens.

## Out of scope

Re-skinning panels, new panels, and the Cosmic-Settings hand-off plumbing for the
deferred desktop items (they simply leave the Workbench nav; Cosmic already owns
them).
