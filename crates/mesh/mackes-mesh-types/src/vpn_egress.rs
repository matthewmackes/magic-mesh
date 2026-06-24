//! VPN-GW-3 — selective egress: policy-routing + NAT + a leak-proof kill-switch
//! (design: `docs/design/vpn-gateway.md` §5/§8 + Risks "Policy-routing + Nebula
//! interaction").
//!
//! A gateway node steers *chosen* traffic out a VPN tunnel `mvpn-<id>` without
//! ever capturing its own mesh/Nebula traffic. The mechanism is the one the
//! survey locked (Q5/Q8):
//!
//!   - an **`fwmark`** marks the packets selected for this tunnel,
//!   - an **`ip rule`** sends marked packets to a per-tunnel routing **table**
//!     whose default route is the tunnel interface (policy routing),
//!   - an **nftables masquerade** rule NATs that marked traffic out `mvpn-<id>`,
//!   - the **Nebula overlay subnet is explicitly carved out** (`ip rule` with a
//!     lower priority that routes the overlay to the `main` table, plus an
//!     `nft` accept-before-mark) so mesh traffic *never* tunnels — the §-risk
//!     that breaks the overlay if you get it wrong,
//!   - a **kill-switch** `drop` rule blocks the marked traffic when the tunnel
//!     is down so there is no WAN leak on a flap.
//!
//! This module is **pure**: every function builds argv (`Vec<Vec<String>>`,
//! one inner vec per command) that the `mackesd` `vpn_gw` responder spawns. No
//! process is run here, so it is exhaustively unit-tested. The teardown is the
//! exact inverse of the setup so a tunnel-down (or a failed bring-up) leaves the
//! box clean — except the kill-switch, which the down/failure path installs so a
//! drop precedes the route teardown (leak-proof ordering).

use serde::{Deserialize, Serialize};

/// The canonical Nebula overlay subnet — the mesh's `10.42.0.0/16`.
///
/// Locked per `docs/design/`; the same value `mackesd`'s CA hands out overlay
/// certs from. Carried as the default so a caller that does not thread a custom
/// overlay CIDR still carves out the real mesh. A node on a non-default overlay
/// passes its own CIDR to [`EgressPlan::new`].
pub const DEFAULT_OVERLAY_CIDR: &str = "10.42.0.0/16";

/// The base for the per-tunnel `fwmark` / routing-table number.
///
/// A tunnel's mark and table are derived from this so two tunnels never collide
/// and the values stay clear of the kernel's reserved low tables
/// (`main`/`default`/`local`). `0x2a` == 42, the mesh's signature octet — picked
/// so the marks are easy to spot in `nft list ruleset` / `ip rule` output
/// during an incident.
pub const MARK_TABLE_BASE: u32 = 0x2a00;

/// One tunnel's egress policy.
///
/// Holds the interface it NATs out of, the `fwmark` that selects its traffic,
/// the routing `table` that points at it, and the overlay CIDR carved out so the
/// mesh is never tunneled. Pure value; the argv builders are methods so a caller
/// cannot mismatch the mark/table/iface across commands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPlan {
    /// The tunnel interface, e.g. `mvpn-mullvad1` (from `TunnelDef::ifname()`).
    pub ifname: String,
    /// The firewall mark that selects this tunnel's traffic (hex in argv).
    pub fwmark: u32,
    /// The policy-routing table number whose default route is `ifname`.
    pub table: u32,
    /// The Nebula overlay subnet carved out so mesh traffic stays direct.
    pub overlay_cidr: String,
}

/// `ip rule` priority for the overlay carve-out — **lower (higher priority)**
/// than the tunnel rule so the overlay is matched *first* and sent to `main`,
/// guaranteeing mesh traffic is evaluated before the fwmark→tunnel rule.
const OVERLAY_RULE_PRIO: u32 = 100;

/// `ip rule` priority for the fwmark→tunnel rule. Strictly greater than
/// [`OVERLAY_RULE_PRIO`] (evaluated *after* the carve-out).
const TUNNEL_RULE_PRIO: u32 = 200;

/// The width of the per-tunnel slot space. The `fwmark`/`table` is
/// `MARK_TABLE_BASE + (slot % SLOT_SPACE)`, keeping every mark in
/// `0x2a00..=0x69ff` — clear of the kernel's reserved low tables and well within
/// `u32`. 14 bits (16 384 slots) makes a hash collision between the handful of
/// tunnels a node runs astronomically unlikely.
const SLOT_SPACE: u32 = 0x4000;

impl EgressPlan {
    /// Build the egress plan for `ifname`, deriving the `fwmark`/`table` pair
    /// from `slot` and carving out `overlay_cidr`. The slot is taken modulo
    /// [`SLOT_SPACE`] so the mark/table always land in the dedicated range.
    ///
    /// Prefer [`EgressPlan::for_ifname`] (a *stable*, id-derived slot) over a
    /// caller-chosen index — a positional index silently remaps a live tunnel's
    /// mark when the config is reordered. Pass [`DEFAULT_OVERLAY_CIDR`] for the
    /// standard mesh.
    #[must_use]
    pub fn new(ifname: &str, slot: u32, overlay_cidr: &str) -> Self {
        let mark = MARK_TABLE_BASE + (slot % SLOT_SPACE);
        Self {
            ifname: ifname.to_string(),
            fwmark: mark,
            table: mark,
            overlay_cidr: overlay_cidr.to_string(),
        }
    }

    /// The plan for `ifname` with a **stable** slot derived from the interface
    /// name itself (an FNV-1a hash into [`SLOT_SPACE`]). Because the mark/table
    /// is a pure function of the interface — not its position in any config — it
    /// never shifts when another tunnel is added or removed, so the teardown
    /// argv always reclaim the rules a matching bring-up installed and a removed
    /// tunnel's mark is never silently reused by a surviving one.
    ///
    /// `mvpn-<id>` interface names are unique per node (`VpnConfig::validate`
    /// rejects ifname collisions), so distinct tunnels get distinct slots except
    /// for the vanishingly rare hash collision.
    #[must_use]
    pub fn for_ifname(ifname: &str, overlay_cidr: &str) -> Self {
        Self::new(ifname, fnv1a_slot(ifname), overlay_cidr)
    }

    /// [`EgressPlan::for_ifname`] on the default mesh overlay — the production
    /// entry point used by the `vpn_gw` responder.
    #[must_use]
    pub fn for_ifname_on_default_overlay(ifname: &str) -> Self {
        Self::for_ifname(ifname, DEFAULT_OVERLAY_CIDR)
    }

    /// Convenience: the plan for `ifname` on the default mesh overlay from an
    /// explicit `slot` (tests / callers that already hold a unique index).
    #[must_use]
    pub fn on_default_overlay(ifname: &str, slot: u32) -> Self {
        Self::new(ifname, slot, DEFAULT_OVERLAY_CIDR)
    }

    /// The nftables table name this plan owns — one per tunnel interface so a
    /// `nft delete table` on teardown reclaims exactly this tunnel's rules and
    /// can never touch another tunnel's (or the system's) ruleset.
    #[must_use]
    pub fn nft_table(&self) -> String {
        format!("inet mvpn_{}", nft_ident(&self.ifname))
    }

    /// Bring the selective egress **up** after a successful tunnel-up: install
    /// the carve-out rule, the fwmark policy rule, the per-tunnel route table,
    /// and the masquerade/carve-out nftables table. Returns one argv per command
    /// (run in order). Pure — the caller spawns each.
    ///
    /// Ordering matters: the overlay carve-out (`ip rule` + the nft `accept`)
    /// is installed *before* the fwmark rule so mesh traffic is matched first.
    #[must_use]
    pub fn up_argv(&self) -> Vec<Vec<String>> {
        let mark = hex_mark(self.fwmark);
        let table = self.table.to_string();
        vec![
            // 1. Carve out the overlay: marked-or-not, overlay-destined traffic
            //    is routed by `main` (direct), never the tunnel table. Lower
            //    priority ⇒ evaluated first.
            vec![
                "ip".into(),
                "rule".into(),
                "add".into(),
                "to".into(),
                self.overlay_cidr.clone(),
                "lookup".into(),
                "main".into(),
                "priority".into(),
                OVERLAY_RULE_PRIO.to_string(),
            ],
            // 2. Policy rule: fwmark-selected traffic uses the tunnel table.
            vec![
                "ip".into(),
                "rule".into(),
                "add".into(),
                "fwmark".into(),
                mark.clone(),
                "lookup".into(),
                table.clone(),
                "priority".into(),
                TUNNEL_RULE_PRIO.to_string(),
            ],
            // 3. The tunnel table's default route → out the tunnel interface.
            vec![
                "ip".into(),
                "route".into(),
                "add".into(),
                "default".into(),
                "dev".into(),
                self.ifname.clone(),
                "table".into(),
                table,
            ],
            // 4. The nftables table: masquerade marked traffic out the tunnel,
            //    with the overlay accepted (returned direct) *before* the
            //    masquerade so mesh return traffic is never NATed.
            vec![
                "nft".into(),
                "add".into(),
                "table".into(),
                "inet".into(),
                nft_ident(&self.ifname),
            ],
            vec![
                "nft".into(),
                "add".into(),
                "chain".into(),
                "inet".into(),
                nft_ident(&self.ifname),
                "postrouting".into(),
                "{ type nat hook postrouting priority 100 ; }".into(),
            ],
            // Carve-out: overlay-destined traffic returns before masquerade.
            vec![
                "nft".into(),
                "add".into(),
                "rule".into(),
                "inet".into(),
                nft_ident(&self.ifname),
                "postrouting".into(),
                "ip".into(),
                "daddr".into(),
                self.overlay_cidr.clone(),
                "return".into(),
            ],
            // Masquerade the marked traffic out the tunnel interface.
            vec![
                "nft".into(),
                "add".into(),
                "rule".into(),
                "inet".into(),
                nft_ident(&self.ifname),
                "postrouting".into(),
                "meta".into(),
                "mark".into(),
                mark,
                "oifname".into(),
                quote_nft(&self.ifname),
                "masquerade".into(),
            ],
        ]
    }

    /// Tear the selective egress **down** — the exact inverse of [`up_argv`], so
    /// a tunnel-down leaves no policy rule, route, or nft table behind. The
    /// `nft delete table` reclaims this tunnel's whole ruleset in one shot.
    ///
    /// [`up_argv`]: Self::up_argv
    #[must_use]
    pub fn down_argv(&self) -> Vec<Vec<String>> {
        let mark = hex_mark(self.fwmark);
        let table = self.table.to_string();
        vec![
            vec![
                "nft".into(),
                "delete".into(),
                "table".into(),
                "inet".into(),
                nft_ident(&self.ifname),
            ],
            vec![
                "ip".into(),
                "route".into(),
                "flush".into(),
                "table".into(),
                table.clone(),
            ],
            vec![
                "ip".into(),
                "rule".into(),
                "del".into(),
                "fwmark".into(),
                mark,
                "lookup".into(),
                table,
                "priority".into(),
                TUNNEL_RULE_PRIO.to_string(),
            ],
            vec![
                "ip".into(),
                "rule".into(),
                "del".into(),
                "to".into(),
                self.overlay_cidr.clone(),
                "lookup".into(),
                "main".into(),
                "priority".into(),
                OVERLAY_RULE_PRIO.to_string(),
            ],
        ]
    }

    /// Install the **kill-switch**: a dedicated nft table that `drop`s the
    /// marked egress traffic (with the overlay carved out so mesh stays up) so
    /// there is NO WAN leak when the tunnel is down. Installed on the
    /// down/failure path so the drop is in place before/while the tunnel route
    /// is gone. Idempotent-by-construction: it owns its own table, removed by
    /// [`kill_switch_clear_argv`].
    ///
    /// The drop chain hooks `output` at a negative priority so it runs *before*
    /// routing picks an interface — marked traffic is dropped even with no
    /// tunnel route present (leak-proof on a mid-transfer kill).
    ///
    /// [`kill_switch_clear_argv`]: Self::kill_switch_clear_argv
    #[must_use]
    pub fn kill_switch_argv(&self) -> Vec<Vec<String>> {
        let mark = hex_mark(self.fwmark);
        let ks = ks_ident(&self.ifname);
        vec![
            vec![
                "nft".into(),
                "add".into(),
                "table".into(),
                "inet".into(),
                ks.clone(),
            ],
            vec![
                "nft".into(),
                "add".into(),
                "chain".into(),
                "inet".into(),
                ks.clone(),
                "killswitch".into(),
                "{ type filter hook output priority -100 ; }".into(),
            ],
            // Carve-out: overlay-destined traffic is always accepted — the
            // kill-switch never blocks the mesh.
            vec![
                "nft".into(),
                "add".into(),
                "rule".into(),
                "inet".into(),
                ks.clone(),
                "killswitch".into(),
                "ip".into(),
                "daddr".into(),
                self.overlay_cidr.clone(),
                "accept".into(),
            ],
            // Drop the marked egress: no tunnel ⇒ no leak.
            vec![
                "nft".into(),
                "add".into(),
                "rule".into(),
                "inet".into(),
                ks,
                "killswitch".into(),
                "meta".into(),
                "mark".into(),
                mark,
                "drop".into(),
            ],
        ]
    }

    /// Remove the kill-switch table (a clean tunnel-up clears it after the
    /// egress route is back so traffic can flow again).
    #[must_use]
    pub fn kill_switch_clear_argv(&self) -> Vec<Vec<String>> {
        vec![vec![
            "nft".into(),
            "delete".into(),
            "table".into(),
            "inet".into(),
            ks_ident(&self.ifname),
        ]]
    }
}

/// A stable slot in `0..SLOT_SPACE` derived from `s` via FNV-1a — deterministic
/// and dependency-free (not `DefaultHasher`, whose output is unspecified across
/// toolchains) so a node's fwmark for a given interface is reproducible.
#[must_use]
fn fnv1a_slot(s: &str) -> u32 {
    const OFFSET: u32 = 0x811c_9dc5;
    const PRIME: u32 = 0x0100_0193;
    let mut h = OFFSET;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(PRIME);
    }
    h % SLOT_SPACE
}

/// Render an `fwmark` as the `0x…` hex string both `ip rule` and `nft` accept.
#[must_use]
fn hex_mark(mark: u32) -> String {
    format!("0x{mark:x}")
}

/// Sanitize an interface name into an nftables identifier (alnum + `_`). The
/// `mvpn-<id>` interface always yields a non-empty, unique ident because the id
/// body is validated non-empty upstream (`TunnelDef::validate`).
#[must_use]
fn nft_ident(ifname: &str) -> String {
    ifname
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// The kill-switch table's ident — distinct from the egress table so the two
/// coexist (kill-switch up while the egress table is torn down).
#[must_use]
fn ks_ident(ifname: &str) -> String {
    format!("{}_ks", nft_ident(ifname))
}

/// Quote an interface name for an nft `oifname` match (nft wants a quoted
/// string there). Pure string wrap.
#[must_use]
fn quote_nft(ifname: &str) -> String {
    format!("\"{ifname}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> EgressPlan {
        EgressPlan::on_default_overlay("mvpn-mullvad1", 1)
    }

    #[test]
    fn default_overlay_cidr_is_the_mesh() {
        assert_eq!(DEFAULT_OVERLAY_CIDR, "10.42.0.0/16");
    }

    #[test]
    fn mark_and_table_are_derived_and_collision_free() {
        let a = EgressPlan::on_default_overlay("mvpn-a", 0);
        let b = EgressPlan::on_default_overlay("mvpn-b", 1);
        assert_eq!(a.fwmark, MARK_TABLE_BASE);
        assert_eq!(a.table, MARK_TABLE_BASE);
        assert_eq!(b.fwmark, MARK_TABLE_BASE + 1);
        // Distinct slots ⇒ distinct marks/tables (no cross-tunnel capture).
        assert_ne!(a.fwmark, b.fwmark);
        assert_ne!(a.table, b.table);
        // Clear of the kernel's reserved low tables (local=255/main=254/default=253).
        assert!(a.table > 255);
    }

    #[test]
    fn up_argv_carves_overlay_before_the_fwmark_rule() {
        let p = plan();
        let up = p.up_argv();
        // First command is the overlay carve-out ip rule at the lower priority.
        let first = &up[0];
        assert_eq!(first[0], "ip");
        assert_eq!(first[1], "rule");
        assert!(first.contains(&p.overlay_cidr));
        assert!(first.contains(&"main".to_string()));
        assert!(first.contains(&OVERLAY_RULE_PRIO.to_string()));
        // The fwmark rule comes after and at the higher (later) priority.
        let mark_rule = &up[1];
        assert!(mark_rule.contains(&"fwmark".to_string()));
        assert!(mark_rule.contains(&TUNNEL_RULE_PRIO.to_string()));
        // Overlay rule priority < tunnel rule priority ⇒ overlay matched first.
        assert!(OVERLAY_RULE_PRIO < TUNNEL_RULE_PRIO);
    }

    #[test]
    fn up_argv_masquerades_marked_traffic_out_the_tunnel() {
        let p = plan();
        let up = p.up_argv();
        let masq = up
            .iter()
            .find(|c| c.contains(&"masquerade".to_string()))
            .expect("a masquerade rule");
        assert!(masq.contains(&"0x2a01".to_string())); // slot 1 ⇒ 0x2a00+1
        assert!(masq.contains(&"oifname".to_string()));
        assert!(masq.contains(&"\"mvpn-mullvad1\"".to_string()));
    }

    #[test]
    fn up_argv_returns_overlay_before_masquerade_in_nft() {
        let p = plan();
        let up = p.up_argv();
        // The nft overlay `return` rule must come before the masquerade rule so
        // mesh return traffic is never NATed.
        let ret_idx = up
            .iter()
            .position(|c| c.contains(&"return".to_string()) && c.contains(&p.overlay_cidr))
            .expect("an overlay return rule");
        let masq_idx = up
            .iter()
            .position(|c| c.contains(&"masquerade".to_string()))
            .expect("a masquerade rule");
        assert!(ret_idx < masq_idx, "overlay return must precede masquerade");
    }

    #[test]
    fn down_argv_is_the_inverse_and_deletes_the_nft_table() {
        let p = plan();
        let down = p.down_argv();
        // Tears down the nft table, flushes the route table, and deletes both rules.
        assert!(down
            .iter()
            .any(|c| c[0] == "nft" && c.contains(&"delete".to_string())));
        assert!(down
            .iter()
            .any(|c| c.contains(&"flush".to_string()) && c.contains(&p.table.to_string())));
        // Both ip rules are removed (fwmark + overlay carve-out).
        let dels: Vec<&Vec<String>> = down
            .iter()
            .filter(|c| c[0] == "ip" && c.contains(&"del".to_string()))
            .collect();
        assert_eq!(dels.len(), 2);
        assert!(dels.iter().any(|c| c.contains(&"fwmark".to_string())));
        assert!(dels.iter().any(|c| c.contains(&p.overlay_cidr)));
    }

    #[test]
    fn up_and_down_reference_the_same_mark_table_and_iface() {
        let p = plan();
        let up = p.up_argv();
        let down = p.down_argv();
        let mark = format!("0x{:x}", p.fwmark);
        // Mark appears in both; teardown can't drift from setup.
        assert!(up.iter().any(|c| c.contains(&mark)));
        assert!(down.iter().any(|c| c.contains(&mark)));
        assert!(up.iter().any(|c| c.contains(&p.table.to_string())));
        assert!(down.iter().any(|c| c.contains(&p.table.to_string())));
    }

    #[test]
    fn kill_switch_drops_marked_traffic_and_accepts_overlay() {
        let p = plan();
        let ks = p.kill_switch_argv();
        // The overlay is accepted (mesh stays up under kill-switch)…
        let accept_idx = ks
            .iter()
            .position(|c| c.contains(&"accept".to_string()) && c.contains(&p.overlay_cidr))
            .expect("an overlay accept rule");
        // …and the marked egress is dropped (no WAN leak)…
        let drop_idx = ks
            .iter()
            .position(|c| c.contains(&"drop".to_string()))
            .expect("a drop rule");
        // …with the accept evaluated first (so the overlay is never dropped).
        assert!(accept_idx < drop_idx);
        let drop_rule = &ks[drop_idx];
        assert!(drop_rule.contains(&format!("0x{:x}", p.fwmark)));
    }

    #[test]
    fn kill_switch_hooks_output_so_it_blocks_without_a_tunnel_route() {
        let p = plan();
        let ks = p.kill_switch_argv();
        // The chain hooks `output` at a negative priority → it runs before
        // routing, so marked traffic is dropped even with no tunnel route
        // present (leak-proof on a mid-transfer tunnel kill).
        let chain = ks
            .iter()
            .find(|c| c.contains(&"chain".to_string()))
            .expect("a chain decl");
        assert!(chain.iter().any(|a| a.contains("hook output")));
        assert!(chain.iter().any(|a| a.contains("priority -100")));
    }

    #[test]
    fn kill_switch_table_is_distinct_from_the_egress_table() {
        let p = plan();
        // The kill-switch table and the egress masquerade table must differ so
        // they can coexist (kill-switch up while egress is torn down).
        assert_ne!(ks_ident(&p.ifname), nft_ident(&p.ifname));
        let clear = p.kill_switch_clear_argv();
        assert_eq!(clear.len(), 1);
        assert!(clear[0].contains(&"delete".to_string()));
        assert!(clear[0].contains(&ks_ident(&p.ifname)));
    }

    #[test]
    fn nft_ident_sanitizes_the_dash_in_mvpn_names() {
        // `mvpn-mullvad1` → `mvpn_mullvad1` (nft idents can't hold a dash).
        assert_eq!(nft_ident("mvpn-mullvad1"), "mvpn_mullvad1");
        assert_eq!(ks_ident("mvpn-mullvad1"), "mvpn_mullvad1_ks");
        // The nft table name string embeds the sanitized ident.
        assert_eq!(plan().nft_table(), "inet mvpn_mvpn_mullvad1");
    }

    #[test]
    fn for_ifname_slot_is_stable_and_in_range() {
        // The id-derived mark is a pure function of the interface name: it does
        // not move when other tunnels come and go (no positional remap), and it
        // stays in the dedicated mark range.
        let a = EgressPlan::for_ifname_on_default_overlay("mvpn-mullvad1");
        let again = EgressPlan::for_ifname_on_default_overlay("mvpn-mullvad1");
        assert_eq!(a.fwmark, again.fwmark, "same iface ⇒ same mark, always");
        assert_eq!(a.table, a.fwmark);
        assert!(a.fwmark >= MARK_TABLE_BASE);
        assert!(a.fwmark < MARK_TABLE_BASE + SLOT_SPACE);
        // Distinct interfaces get distinct marks (no collision for these names).
        let b = EgressPlan::for_ifname_on_default_overlay("mvpn-proton2");
        assert_ne!(a.fwmark, b.fwmark);
    }

    #[test]
    fn slot_is_taken_modulo_the_space_so_marks_never_escape_the_range() {
        // An out-of-range explicit slot wraps into the dedicated range instead
        // of colliding with a reserved kernel table or overflowing.
        let p = EgressPlan::on_default_overlay("mvpn-x", SLOT_SPACE + 7);
        assert_eq!(p.fwmark, MARK_TABLE_BASE + 7);
        assert!(p.fwmark < MARK_TABLE_BASE + SLOT_SPACE);
    }

    #[test]
    fn custom_overlay_cidr_is_carved_out_not_the_default() {
        let p = EgressPlan::new("mvpn-x", 3, "10.99.0.0/16");
        assert_eq!(p.overlay_cidr, "10.99.0.0/16");
        assert!(p.up_argv().iter().any(|c| c.contains(&"10.99.0.0/16".to_string())));
        assert!(p
            .kill_switch_argv()
            .iter()
            .any(|c| c.contains(&"10.99.0.0/16".to_string())));
        // The default mesh CIDR is NOT present when a custom overlay is used.
        assert!(!p
            .up_argv()
            .iter()
            .any(|c| c.contains(&DEFAULT_OVERLAY_CIDR.to_string())));
    }
}
