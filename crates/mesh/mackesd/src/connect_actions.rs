//! MESH-A-7 (v5.0.0) — well-known port → connect-action mappings.
//!
//! The 12 well-known service ports locked in
//! `docs/design/v6.0-mde-portal.md` §7.3 (R8-Q50) each map to a
//! "connect action": the concrete launch command an operator — or a
//! future host-card right-click (R8-Q11) / Portal-22 Network layer —
//! runs to reach that service on a host. URI services launch via the
//! desktop handler (`xdg-open <scheme>://<ip>[:<port>]`); database /
//! shell services launch a terminal client (`ssh <ip>`,
//! `psql -h <ip>`, …).
//!
//! Pure + side-effect-free: [`connect_argv`] only BUILDS the argv; the
//! caller decides whether to print it (the `mackesd connect` CLI) or
//! spawn it (a UI gesture). Runtime-reachable today via
//! `mackesd connect <ip> <port>`; the host-card wiring lands with the
//! Portal Network layer (Portal-22).
//!
//! The design doc says "12 well-known ports" but lists 13 service
//! names; HTTP-alt (8080) is the alternate HTTP port rather than a
//! distinct service. All 13 listed ports map here.

/// Resolve the connect-action for `ip` on `port`. Returns the
/// human-readable service name + the launch argv, or `None` when the
/// port is not one of the well-known services (R8-Q50).
#[must_use]
pub fn connect_argv(ip: &str, port: u16) -> Option<(&'static str, Vec<String>)> {
    // `xdg-open <scheme>://<ip>[:<port>]` — the desktop URI handler
    // routes http/https to the browser, smb/ftp to the file manager,
    // vnc/rdp to the remote-desktop client. `with_port` keeps a
    // non-default port (CUPS 631, HTTP-alt 8080) in the URI.
    let uri = |scheme: &str, with_port: bool| -> Vec<String> {
        let target = if with_port {
            format!("{scheme}://{ip}:{port}")
        } else {
            format!("{scheme}://{ip}")
        };
        vec!["xdg-open".to_string(), target]
    };
    // `<client> <flag> <ip>` terminal launch for the DB / cache shells.
    let host_flag = |client: &str, flag: &str| -> Vec<String> {
        vec![client.to_string(), flag.to_string(), ip.to_string()]
    };

    let (service, argv): (&'static str, Vec<String>) = match port {
        22 => ("SSH", vec!["ssh".to_string(), ip.to_string()]),
        80 => ("HTTP", uri("http", false)),
        443 => ("HTTPS", uri("https", false)),
        5900 => ("VNC", uri("vnc", false)),
        3389 => ("RDP", uri("rdp", false)),
        445 => ("SMB", uri("smb", false)),
        21 => ("FTP", uri("ftp", false)),
        631 => ("CUPS", uri("http", true)),
        5432 => ("PostgreSQL", host_flag("psql", "-h")),
        3306 => ("MySQL", host_flag("mysql", "-h")),
        6379 => ("Redis", host_flag("redis-cli", "-h")),
        8080 => ("HTTP-alt", uri("http", true)),
        27017 => ("MongoDB", host_flag("mongosh", "--host")),
        _ => return None,
    };
    Some((service, argv))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_maps_to_bare_ssh_argv() {
        let (service, argv) = connect_argv("10.42.0.5", 22).unwrap();
        assert_eq!(service, "SSH");
        assert_eq!(argv.join(" "), "ssh 10.42.0.5");
    }

    #[test]
    fn http_maps_to_xdg_open_uri() {
        let (service, argv) = connect_argv("10.42.0.5", 80).unwrap();
        assert_eq!(service, "HTTP");
        assert_eq!(argv.join(" "), "xdg-open http://10.42.0.5");
    }

    #[test]
    fn non_default_port_uris_keep_the_port() {
        // CUPS (631) + HTTP-alt (8080) carry the port into the URI.
        assert_eq!(
            connect_argv("10.42.0.5", 631).unwrap().1.join(" "),
            "xdg-open http://10.42.0.5:631"
        );
        assert_eq!(
            connect_argv("10.42.0.5", 8080).unwrap().1.join(" "),
            "xdg-open http://10.42.0.5:8080"
        );
    }

    #[test]
    fn database_services_use_host_flagged_clients() {
        assert_eq!(connect_argv("h", 5432).unwrap().1.join(" "), "psql -h h");
        assert_eq!(connect_argv("h", 3306).unwrap().1.join(" "), "mysql -h h");
        assert_eq!(
            connect_argv("h", 6379).unwrap().1.join(" "),
            "redis-cli -h h"
        );
        assert_eq!(
            connect_argv("h", 27017).unwrap().1.join(" "),
            "mongosh --host h"
        );
    }

    #[test]
    fn all_well_known_ports_resolve_with_a_service_name() {
        for port in [
            22u16, 80, 443, 5900, 3389, 445, 21, 631, 5432, 3306, 6379, 8080, 27017,
        ] {
            let resolved = connect_argv("10.0.0.1", port);
            assert!(resolved.is_some(), "port {port} should map");
            assert!(!resolved.unwrap().0.is_empty(), "service name non-empty");
        }
    }

    #[test]
    fn unknown_port_yields_none() {
        assert!(connect_argv("10.0.0.1", 12345).is_none());
        assert!(connect_argv("10.0.0.1", 0).is_none());
    }
}
