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
    ConsoleAddr, ConsoleBrokerError, ConsoleRelay, LiveConsoleRelay,
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
    let Some(workload) = workload_name(body) else {
        return CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(format!(
                "`{verb_name}` requires a workload `name` (the running VM/domain) to attach a console to"
            )),
            ..Default::default()
        };
    };
    console_endpoint_reply(&LiveConsoleRelay::new(), verb_name, workload)
}

/// The workload/domain name a `console-attach` targets — its `name`, else the
/// lifecycle `instance` field. `None` when neither is a non-empty string.
fn workload_name(body: &CloudActionBody) -> Option<&str> {
    let raw = body.name.as_deref().or(body.instance.as_deref())?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Resolve `workload`'s live console through the injected `relay` and shape the
/// reply. Pure over the [`ConsoleRelay`] seam (tests inject a fake), so the whole
/// resolve → map → reply path runs without a live hypervisor.
fn console_endpoint_reply(relay: &dyn ConsoleRelay, verb_name: &str, workload: &str) -> CloudReply {
    match relay.resolve(workload) {
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

/// Map a resolved [`ConsoleAddr`] onto the neutral [`ConsoleEndpoint`] the shell
/// attaches over. Spice/VNC map to their proto; an RDP head (never emitted by
/// `virsh domdisplay` for a KVM guest) has no `ConsoleProto` form, so it is honestly
/// `None` rather than silently coerced.
fn endpoint_from_addr(addr: &ConsoleAddr) -> Option<ConsoleEndpoint> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::console_broker::RelayHandle;

    /// A scripted [`ConsoleRelay`] — returns a canned resolve result. `console-attach`
    /// only calls `resolve`, so `overlay_addr`/`start_relay` are inert stubs.
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
        let relay = FakeRelay(Ok(addr(DesktopProtocol::Spice, "127.0.0.1", 5900)));
        let reply = console_endpoint_reply(&relay, "console-attach", "win11");
        assert!(reply.ok, "gated: {:?} err: {:?}", reply.gated, reply.error);
        let console = reply.console.expect("console handle");
        assert_eq!(console.proto, ConsoleProto::Spice);
        assert_eq!(console.uri, "spice://127.0.0.1:5900");
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
    fn an_absent_virsh_toolchain_is_gated_not_errored() {
        let relay = FakeRelay(Err(ConsoleBrokerError::Gated("virsh not found".into())));
        let reply = console_endpoint_reply(&relay, "console-attach", "dev");
        assert!(!reply.ok);
        assert!(reply.console.is_none());
        assert!(reply.gated.unwrap().contains("not ready"));
    }

    #[test]
    fn an_rdp_head_has_no_attachable_console_endpoint_form() {
        let relay = FakeRelay(Ok(addr(DesktopProtocol::Rdp, "127.0.0.1", 3389)));
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
            Some("web"),
            "falls back to instance"
        );
        assert_eq!(
            workload_name(&body(Some("db"), Some("web"))),
            Some("db"),
            "name wins over instance"
        );
        assert_eq!(
            workload_name(&body(Some("  "), None)),
            None,
            "blank is none"
        );
    }
}
