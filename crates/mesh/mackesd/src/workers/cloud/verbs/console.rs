//! Workloads U8 — the `console-attach` verb handler.
//!
//! Mints a [`ConsoleEndpoint`] for a **running** workload by reusing the existing
//! [`console_broker`](crate::workers::console_broker) resolution seam
//! ([`ConsoleRelay::resolve`] + `virsh domdisplay` parsing): it resolves the live
//! SPICE/VNC console libvirt actually assigned to the workload's domain and hands
//! it back as the shell's SPICE/VNC → VDI attach handle.
//!
//! `console-attach` is placement-routed as a mutation by the drain, so by the time
//! this handler runs THIS node hosts the workload — it resolves its own local
//! console head. Cross-node overlay tunnelling is NOT this verb's job: that is the
//! `console_broker` worker's per-VDI-session relay (it retains a live `socat`
//! handle across a session, which a one-shot verb reply cannot). So this verb
//! returns the resolved console head for the placement node — never a fabricated or
//! immediately-dead relay (§7).
//!
//! Honest-not-connectable: a shut-off / graphics-less / absent workload (or an
//! absent `virsh`) yields an honest `error`/`gated` reply, never a fake endpoint.

use mackes_mesh_types::cloud::{CloudReply, ConsoleEndpoint, ConsoleProto};

use crate::workers::console_broker::{
    is_loopback, ConsoleAddr, ConsoleBrokerError, ConsoleRelay, LiveConsoleRelay,
};
use crate::workers::desktop_sources::DesktopProtocol;

use super::CloudActionBody;

/// Handle one `action/cloud/console-attach` request → a typed [`CloudReply`].
///
/// The workload to attach to is the request's `name` (its libvirt domain), falling
/// back to `instance`. Resolution goes through the production
/// [`LiveConsoleRelay`] (`virsh domdisplay`); the reply-shaping is the pure,
/// fake-testable [`console_endpoint_reply`].
pub(super) fn handle(verb_name: &str, body: &CloudActionBody) -> CloudReply {
    handle_with_relay(&LiveConsoleRelay::new(), verb_name, body)
}

/// Validate the target before dispatching to the live console backend. Keeping
/// the relay injected here makes the rejection-before-`virsh` ordering explicit
/// and regression-testable, while production still uses [`LiveConsoleRelay`].
fn handle_with_relay(
    relay: &dyn ConsoleRelay,
    verb_name: &str,
    body: &CloudActionBody,
) -> CloudReply {
    let workload = match workload_name(body) {
        Ok(Some(workload)) => workload,
        Ok(None) => {
            return CloudReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!(
                    "`{verb_name}` requires a workload `name` (the running VM/domain) to attach a console to"
                )),
                ..Default::default()
            }
        }
        Err(reason) => {
            return CloudReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!("`{verb_name}` rejects the workload target: {reason}")),
                ..Default::default()
            }
        }
    };
    console_endpoint_reply(relay, verb_name, workload)
}

/// The workload/domain name a `console-attach` targets — its `name`, else the
/// lifecycle `instance` field. When both identity fields are present they must
/// agree; silently choosing one would make the authorization and backend target
/// ambiguous. `None` when neither is a non-empty string. A target is also a
/// libvirt/path-safe component before it can reach `virsh` or become the
/// authorization target.
fn workload_name(body: &CloudActionBody) -> Result<Option<&str>, String> {
    let raw = match (body.name.as_deref(), body.instance.as_deref()) {
        (None, None) => return Ok(None),
        (Some(name), None) | (None, Some(name)) => name,
        (Some(name), Some(instance)) => {
            if name.trim() != instance.trim() {
                return Err(
                    "workload `name` and lifecycle `instance` must identify the same target"
                        .to_string(),
                );
            }
            name
        }
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else if trimmed.starts_with('-') {
        Err("workload target must not begin with `-`".to_string())
    } else {
        super::super::path_key::segment("workload", trimmed).map(Some)
    }
}

pub(super) fn authorization_target(body: &CloudActionBody) -> Option<&str> {
    workload_name(body).ok().flatten()
}

/// Resolve `workload`'s live console through the injected `relay` and shape the
/// reply. Pure over the [`ConsoleRelay`] seam (tests inject a fake), so the whole
/// resolve → map → reply path runs without a live hypervisor.
fn console_endpoint_reply(relay: &dyn ConsoleRelay, verb_name: &str, workload: &str) -> CloudReply {
    match relay.resolve(workload) {
        Ok(addr) if endpoint_requires_retained_relay(&addr.host) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!(
                "`{verb_name}`: workload `{workload}` exposes a loopback or wildcard console; use the retained VDI session broker for mesh reachability"
            )),
            ..Default::default()
        },
        Ok(addr) => match endpoint_from_addr(&addr) {
            Some(console) => CloudReply {
                ok: true,
                verb: verb_name.to_string(),
                console: Some(console),
                ..Default::default()
            },
            None => CloudReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!(
                    "`{verb_name}`: workload `{workload}` console uses `{}`, which has no attachable console-endpoint form",
                    addr.protocol.tag()
                )),
                ..Default::default()
            },
        },
        // `virsh` absent / toolchain not present ⇒ the backend isn't ready (retry).
        Err(ConsoleBrokerError::Gated(reason)) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!("console backend not ready: {reason}")),
            ..Default::default()
        },
        // VM off / no graphics / domain absent ⇒ an honest error (nothing to attach).
        Err(e) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(format!("no console for workload `{workload}`: {}", e.reason())),
            ..Default::default()
        },
    }
}

/// Map a directly reachable [`ConsoleAddr`] onto the neutral
/// [`ConsoleEndpoint`] the shell attaches over. Loopback addresses are rejected
/// by [`console_endpoint_reply`]: a one-shot reply cannot retain the relay handle
/// required to make them mesh-reachable. An RDP head (never emitted by
/// `virsh domdisplay` for a KVM guest) has no `ConsoleProto` form, so it is honestly
/// `None` rather than silently coerced.
fn endpoint_from_addr(addr: &ConsoleAddr) -> Option<ConsoleEndpoint> {
    if addr.port == 0 || !valid_console_host(&addr.host) {
        return None;
    }
    let proto = match addr.protocol {
        DesktopProtocol::Spice => ConsoleProto::Spice,
        DesktopProtocol::Vnc => ConsoleProto::Vnc,
        DesktopProtocol::Rdp => return None,
    };
    Some(ConsoleEndpoint {
        proto,
        uri: format!("{}://{}:{}", addr.protocol.tag(), addr.host, addr.port),
        ticket: None,
    })
}

/// Keep an endpoint URI an authority, not an attacker-controlled URI fragment.
/// `ConsoleAddr` normally comes from local `virsh`, but this boundary is also
/// exercised by the typed relay seam and must stay fail-closed there.
fn valid_console_host(host: &str) -> bool {
    let host = host.trim();
    !host.is_empty()
        && host == host.trim()
        && !host.chars().any(|c| {
            c.is_ascii_control() || c.is_ascii_whitespace() || matches!(c, '/' | '?' | '#' | '@')
        })
}

/// A loopback or wildcard bind is not a directly dialable mesh endpoint. The
/// long-lived broker can safely relay it; this one-shot verb cannot.
fn endpoint_requires_retained_relay(host: &str) -> bool {
    is_loopback(host) || matches!(host.trim(), "0.0.0.0" | "::" | "[::]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::console_broker::RelayHandle;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A scripted [`ConsoleRelay`] — returns a canned resolve result. The
    /// one-shot `console-attach` path intentionally does not call
    /// `overlay_addr`/`start_relay`, because it cannot retain their handle; the
    /// retained session broker owns that path.
    struct FakeRelay(Result<ConsoleAddr, ConsoleBrokerError>);

    impl ConsoleRelay for FakeRelay {
        fn resolve(&self, _vm_id: &str) -> Result<ConsoleAddr, ConsoleBrokerError> {
            self.0.clone()
        }
        fn overlay_addr(&self) -> String {
            String::new()
        }
        fn start_relay(
            &self,
            _overlay_addr: &str,
            _overlay_port: u16,
            _target: &ConsoleAddr,
        ) -> Result<RelayHandle, ConsoleBrokerError> {
            Ok(RelayHandle::detached())
        }
    }

    /// A backend probe used to prove malformed requests are rejected before the
    /// live console-resolution seam is dispatched.
    struct DispatchProbe(AtomicUsize);

    impl ConsoleRelay for DispatchProbe {
        fn resolve(&self, _vm_id: &str) -> Result<ConsoleAddr, ConsoleBrokerError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err(ConsoleBrokerError::Resolve("backend must not be called".into()))
        }
        fn overlay_addr(&self) -> String {
            String::new()
        }
        fn start_relay(
            &self,
            _overlay_addr: &str,
            _overlay_port: u16,
            _target: &ConsoleAddr,
        ) -> Result<RelayHandle, ConsoleBrokerError> {
            Ok(RelayHandle::detached())
        }
    }

    fn addr(protocol: DesktopProtocol, host: &str, port: u16) -> ConsoleAddr {
        ConsoleAddr {
            protocol,
            host: host.to_string(),
            port,
        }
    }

    fn body(name: Option<&str>, instance: Option<&str>) -> CloudActionBody {
        CloudActionBody {
            name: name.map(str::to_string),
            instance: instance.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn a_running_spice_workload_mints_a_spice_endpoint() {
        let relay = FakeRelay(Ok(addr(DesktopProtocol::Spice, "10.42.0.7", 5900)));
        let reply = console_endpoint_reply(&relay, "console-attach", "win11");
        assert!(reply.ok, "gated: {:?} err: {:?}", reply.gated, reply.error);
        let console = reply.console.expect("console handle");
        assert_eq!(console.proto, ConsoleProto::Spice);
        assert_eq!(console.uri, "spice://10.42.0.7:5900");
        assert!(console.ticket.is_none());
    }

    #[test]
    fn a_vnc_workload_mints_a_vnc_endpoint() {
        let relay = FakeRelay(Ok(addr(DesktopProtocol::Vnc, "10.42.0.7", 5901)));
        let reply = console_endpoint_reply(&relay, "console-attach", "droid");
        assert!(reply.ok);
        let console = reply.console.expect("console handle");
        assert_eq!(console.proto, ConsoleProto::Vnc);
        assert_eq!(console.uri, "vnc://10.42.0.7:5901");
    }

    #[test]
    fn a_shut_off_or_absent_workload_is_an_honest_error_not_a_fake_endpoint() {
        let relay = FakeRelay(Err(ConsoleBrokerError::Resolve("VM off".into())));
        let reply = console_endpoint_reply(&relay, "console-attach", "dev");
        assert!(!reply.ok);
        assert!(reply.console.is_none(), "no fabricated endpoint");
        assert!(reply.error.unwrap().contains("dev"));
    }

    #[test]
    fn a_loopback_console_is_gated_until_a_retained_relay_exists() {
        for host in ["127.0.0.1", "0.0.0.0", "::", "[::]"] {
            let relay = FakeRelay(Ok(addr(DesktopProtocol::Spice, host, 5900)));
            let reply = console_endpoint_reply(&relay, "console-attach", "win11");
            assert!(!reply.ok, "accepted non-dialable host {host:?}");
            assert!(reply.console.is_none());
            assert!(reply
                .gated
                .unwrap()
                .contains("retained VDI session broker"));
        }
    }

    #[test]
    fn an_absent_virsh_toolchain_is_gated_not_errored() {
        let relay = FakeRelay(Err(ConsoleBrokerError::Gated("virsh not found".into())));
        let reply = console_endpoint_reply(&relay, "console-attach", "dev");
        assert!(!reply.ok);
        assert!(reply.console.is_none());
        assert!(reply.gated.unwrap().contains("not ready"));
    }

    #[test]
    fn an_rdp_head_has_no_attachable_console_endpoint_form() {
        let relay = FakeRelay(Ok(addr(DesktopProtocol::Rdp, "10.42.0.7", 3389)));
        let reply = console_endpoint_reply(&relay, "console-attach", "winvm");
        assert!(!reply.ok);
        assert!(reply.console.is_none());
        assert!(reply.error.unwrap().contains("rdp"));
    }

    #[test]
    fn a_request_without_a_workload_name_is_honestly_rejected() {
        // The public handler rejects before touching any relay (so no live virsh).
        let reply = handle("console-attach", &body(None, None));
        assert!(!reply.ok);
        assert!(reply.console.is_none());
        assert!(reply.error.unwrap().contains("requires a workload"));
    }

    #[test]
    fn the_instance_field_is_accepted_as_the_workload_fallback() {
        assert_eq!(
            workload_name(&body(None, Some("web"))),
            Ok(Some("web")),
            "falls back to instance"
        );
        assert_eq!(
            workload_name(&body(Some("db"), Some("web"))),
            Err("workload `name` and lifecycle `instance` must identify the same target".into()),
            "conflicting identity fields fail closed"
        );
        assert_eq!(
            workload_name(&body(Some(" web "), Some("web"))),
            Ok(Some("web")),
            "equivalent trimmed identity fields are one target"
        );
        assert_eq!(
            workload_name(&body(Some("  "), None)),
            Ok(None),
            "blank is none"
        );
    }

    #[test]
    fn malformed_or_overlong_workload_targets_are_rejected_before_resolution() {
        for target in ["../vm", "vm/name", "vm name", "--help"] {
            let body = body(Some(target), None);
            assert!(workload_name(&body).is_err(), "accepted unsafe target {target:?}");
            assert!(authorization_target(&body).is_none());
        }
        let overlong = "x".repeat(256);
        let body = body(Some(&overlong), None);
        assert!(workload_name(&body).is_err());
        assert!(authorization_target(&body).is_none());

        let reply = handle("console-attach", &body);
        assert!(!reply.ok);
        assert!(reply
            .error
            .unwrap()
            .contains("rejects the workload target"));
    }

    #[test]
    fn conflicting_workload_identities_are_rejected_before_backend_dispatch() {
        let relay = DispatchProbe(AtomicUsize::new(0));
        let request = body(Some("db"), Some("web"));

        let reply = handle_with_relay(&relay, "console-attach", &request);

        assert!(!reply.ok);
        assert!(reply
            .error
            .as_deref()
            .is_some_and(|error| error.contains("must identify the same target")));
        assert_eq!(
            relay.0.load(Ordering::Relaxed),
            0,
            "invalid target is rejected before console backend resolution"
        );
        assert!(authorization_target(&request).is_none());
    }

    #[test]
    fn malformed_console_addresses_never_become_endpoint_uris() {
        for host in ["", "bad host", "10.42.0.7/path", "10.42.0.7?x=1"] {
            let relay = FakeRelay(Ok(addr(DesktopProtocol::Vnc, host, 5901)));
            let reply = console_endpoint_reply(&relay, "console-attach", "vm");
            assert!(!reply.ok, "accepted malformed host {host:?}");
            assert!(reply.console.is_none());
        }
        let relay = FakeRelay(Ok(addr(DesktopProtocol::Vnc, "10.42.0.7", 0)));
        let reply = console_endpoint_reply(&relay, "console-attach", "vm");
        assert!(!reply.ok);
        assert!(reply.console.is_none());
    }
}
