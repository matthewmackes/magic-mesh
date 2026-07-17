//! EXPLORER-9 — offline enrichment helpers (design E5).
//!
//! Pure, dependency-free lookups the fold layers onto every unit's
//! [`super::unit::Extras`]:
//! - an embedded **MAC-OUI vendor** table ([`oui_vendor`]) — a small curated set
//!   of common prefixes; a prefix outside it is an honest `None` (§7), never a
//!   guessed vendor;
//! - the **service → openable-action** map ([`service_action`] / [`openable_actions`])
//!   turning a fingerprint's service labels into the Hero card's launch verbs (E5
//!   "openable-actions");
//! - the **fingerprint → device-type** hint ([`service_type_hint`]).
//!
//! No I/O — the impure discovery (EXPLORER-2's port fingerprint, the ARP MAC key,
//! the mesh mirror's own-reported MAC / remote-access listeners) happens upstream;
//! this module only maps what those already produced. Everything unmatched stays an
//! explicit unknown (§7).

/// Curated MAC-OUI prefixes (first 3 octets, 6 uppercase hex chars) → vendor.
///
/// Small + offline by design (§7): the common virtualization / single-board /
/// vendor prefixes a mesh/homelab fleet actually meets. A prefix outside this table
/// is an honest unknown ([`oui_vendor`] returns `None`), never a fabricated vendor.
const OUI_TABLE: &[(&str, &str)] = &[
    // Virtualization (the fleet's own guest NICs — QEMU/KVM, Xen doms, ...).
    ("525400", "QEMU/KVM"),
    ("00163E", "Xen"),
    ("005056", "VMware"),
    ("000C29", "VMware"),
    ("000569", "VMware"),
    ("001C14", "VMware"),
    ("080027", "VirtualBox"),
    ("00155D", "Microsoft Hyper-V"),
    ("001C42", "Parallels"),
    // Single-board computers.
    ("B827EB", "Raspberry Pi"),
    ("DCA632", "Raspberry Pi"),
    ("E45F01", "Raspberry Pi"),
    ("28CDC1", "Raspberry Pi"),
    // Common host / NIC / network-device vendors.
    ("00000C", "Cisco"),
    ("000393", "Apple"),
    ("001B63", "Apple"),
    ("3C0754", "Apple"),
    ("F01898", "Apple"),
    ("001B21", "Intel"),
    ("3CFDFE", "Intel"),
    ("A4BF01", "Intel"),
    ("001422", "Dell"),
    ("00219B", "Dell"),
    ("002564", "Dell"),
    ("001A11", "Google"),
    ("F4F5E8", "Google"),
    ("3C5AB4", "Google"),
    ("00E04C", "Realtek"),
    ("24A43C", "Ubiquiti"),
    ("788A20", "Ubiquiti"),
    ("00146C", "Netgear"),
    ("001D0F", "TP-Link"),
];

/// The vendor for a MAC's OUI (first 3 octets), looked up offline.
///
/// Accepts any common MAC spelling (`aa:bb:cc:..`, `aa-bb-cc-..`, `aabbcc..`) — the
/// separators are ignored and the value upper-cased. Returns `None` for a non-MAC
/// (e.g. an IP-keyed host), a malformed value, or a MAC whose prefix isn't in the
/// curated [`OUI_TABLE`] — the honest unknown the surface renders as "unknown
/// vendor" (§7), never a guessed vendor.
#[must_use]
pub fn oui_vendor(mac: &str) -> Option<&'static str> {
    let norm: String = mac
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    // A full MAC is exactly 12 hex nibbles; anything else (an IP key, a partial, a
    // hostname) is not a MAC we can vendor.
    if norm.len() != 12 {
        return None;
    }
    let prefix = &norm[..6];
    OUI_TABLE
        .iter()
        .find(|(p, _)| *p == prefix)
        .map(|(_, v)| *v)
}

/// The openable action a discovered service label implies (E5).
///
/// The remote-desktop / shell / web launch verb the Hero card offers. `None` for a
/// service with no launcher (so an unmapped label never fabricates an action, §7).
#[must_use]
pub fn service_action(service: &str) -> Option<&'static str> {
    match service {
        "ssh" => Some("open-ssh"),
        "rdp" => Some("open-rdp"),
        "vnc" => Some("open-vnc"),
        "spice" => Some("open-spice"),
        "http" => Some("open-http"),
        "https" => Some("open-https"),
        "winrm" => Some("open-winrm"),
        _ => None,
    }
}

/// The openable actions for a set of service labels — each mapped via
/// [`service_action`], deduped, in first-seen order. Empty when no label maps
/// (honest, §7).
#[must_use]
pub fn openable_actions(services: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for svc in services {
        if let Some(action) = service_action(svc) {
            let action = action.to_string();
            if !out.contains(&action) {
                out.push(action);
            }
        }
    }
    out
}

/// A coarse device-type hint from a fingerprint's service set (E5 fingerprint→type).
///
/// A remote-desktop / management listener (RDP/VNC/Spice/WinRM) implies a desktop
/// `computer` (a Quazar broker target); SSH-only implies a headless `server`. A
/// weaker set (HTTP-only, empty) is too ambiguous to type ⇒ honest `None` (§7),
/// never guessed.
#[must_use]
pub fn service_type_hint(services: &[String]) -> Option<&'static str> {
    if services
        .iter()
        .any(|s| matches!(s.as_str(), "rdp" | "vnc" | "spice" | "winrm"))
    {
        Some("computer")
    } else if services.iter().any(|s| s == "ssh") {
        Some("server")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oui_maps_a_known_prefix_and_ignores_separators() {
        // A QEMU/KVM guest NIC (the fleet's own guests) — matched across spellings.
        assert_eq!(oui_vendor("52:54:00:ab:cd:ef"), Some("QEMU/KVM"));
        assert_eq!(oui_vendor("52-54-00-AB-CD-EF"), Some("QEMU/KVM"));
        assert_eq!(oui_vendor("525400abcdef"), Some("QEMU/KVM"));
        // A Xen dom NIC (the farm's hypervisor) + a Raspberry Pi.
        assert_eq!(oui_vendor("00:16:3e:11:22:33"), Some("Xen"));
        assert_eq!(oui_vendor("b8:27:eb:00:00:01"), Some("Raspberry Pi"));
    }

    #[test]
    fn oui_honest_unknowns_an_unknown_prefix_or_non_mac() {
        // A valid MAC whose prefix isn't in the table → honest unknown (§7), not a
        // fabricated vendor.
        assert_eq!(oui_vendor("aa:bb:cc:dd:ee:ff"), None);
        // An IP-keyed host (the scan's fallback key) is not a MAC → None.
        assert_eq!(oui_vendor("192.168.1.41"), None);
        assert_eq!(oui_vendor("10.0.0.1"), None);
        // Malformed / partial values → None (never index-panics).
        assert_eq!(oui_vendor(""), None);
        assert_eq!(oui_vendor("52:54:00"), None);
        assert_eq!(oui_vendor("not-a-mac"), None);
    }

    #[test]
    fn service_actions_map_the_openable_verbs() {
        assert_eq!(service_action("ssh"), Some("open-ssh"));
        assert_eq!(service_action("rdp"), Some("open-rdp"));
        assert_eq!(service_action("vnc"), Some("open-vnc"));
        assert_eq!(service_action("spice"), Some("open-spice"));
        assert_eq!(service_action("http"), Some("open-http"));
        // An unmapped label yields no action (§7).
        assert_eq!(service_action("mystery"), None);
    }

    #[test]
    fn openable_actions_dedup_in_order_and_drop_unmapped() {
        let svcs = vec![
            "rdp".to_string(),
            "vnc".to_string(),
            "rdp".to_string(),    // duplicate collapses
            "gopher".to_string(), // unmapped drops
        ];
        assert_eq!(
            openable_actions(&svcs),
            vec!["open-rdp".to_string(), "open-vnc".to_string()]
        );
        assert!(openable_actions(&[]).is_empty());
    }

    #[test]
    fn type_hint_maps_the_fingerprint_to_a_coarse_type() {
        // A remote-desktop listener → a desktop computer (broker target).
        assert_eq!(
            service_type_hint(&["rdp".to_string(), "vnc".to_string()]),
            Some("computer")
        );
        assert_eq!(service_type_hint(&["spice".to_string()]), Some("computer"));
        // SSH-only → a headless server.
        assert_eq!(service_type_hint(&["ssh".to_string()]), Some("server"));
        // HTTP-only / empty is too weak to type → honest unknown (§7).
        assert_eq!(service_type_hint(&["http".to_string()]), None);
        assert_eq!(service_type_hint(&[]), None);
    }
}
