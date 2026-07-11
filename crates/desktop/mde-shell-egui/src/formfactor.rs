//! `formfactor` — the shell side of SURFACE-9's 2-in-1 formfactor signal (design
//! `docs/design/surface-tablet-enablement.md`, lock 9).
//!
//! The DRM seat ([`mde_egui::run_drm`]) owns the `SW_TABLET_MODE` switch + the Type-
//! Cover attach/detach evdev stream; it debounces them into a [`Formfactor`] and hands
//! a confirmed flip across the runner→surface seam on
//! [`mde_egui::drain_formfactor`] (§6: the shared harness never touches the Bus). This
//! module is the shell's publisher: each pump it drains that side channel and, on a
//! transition, writes the typed message to the mesh Bus topic
//! [`FORMFACTOR_TOPIC`] so the tablet-mode UX (OSK auto-raise, touch density) — here
//! and mesh-wide — reacts.
//!
//! On the windowed fallback (no DRM seat) the side channel is always empty, so the
//! publish self-gates to the real seat — never a fabricated formfactor (§7).

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_egui::Formfactor;
use serde::Serialize;
use std::path::PathBuf;

/// The mesh Bus topic the shell publishes the node's formfactor on. A tablet-mode
/// consumer (OSK, touch density) and the fleet rollup read it.
const FORMFACTOR_TOPIC: &str = "event/hardware/formfactor";

/// The typed formfactor message: the stable `"laptop"` / `"tablet"` token. Kept a
/// struct (not a bare string) so the wire shape can grow (e.g. a timestamp) without a
/// breaking retag, matching the other shell lanes' JSON contracts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct FormfactorMsg {
    /// `"laptop"` or `"tablet"` — [`Formfactor::as_wire`].
    formfactor: &'static str,
}

/// Publishes the seat's formfactor transitions to the mesh Bus.
///
/// Holds only the Bus root; a publish opens the spool, writes the typed JSON, and
/// drops it (the cheap open-write the toast / clipboard / host-mirror lanes use). A
/// missing Bus root (headless CI, no spool) is honest silence — no faked publish.
pub(crate) struct FormfactorPublisher {
    bus_root: Option<PathBuf>,
}

impl Default for FormfactorPublisher {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
        }
    }
}

impl FormfactorPublisher {
    /// Drain the seat's latest formfactor flip (if any), publish it, and return it so
    /// the caller can drive the shell's tablet-mode UX (the OSK auto-raise, SURFACE-10)
    /// off the same transition. Called each frame; returns `None` — and publishes
    /// nothing — when the seat reported no change since the last pump, so the Bus sees
    /// exactly one message per real formfactor change.
    pub(crate) fn pump(&self) -> Option<Formfactor> {
        let formfactor = mde_egui::drain_formfactor()?;
        self.publish(formfactor);
        Some(formfactor)
    }

    /// Write one formfactor transition to [`FORMFACTOR_TOPIC`]. A serialize/spool
    /// failure is dropped (fail-soft, like the Chat / Storage lanes) — a dark Bus never
    /// wedges the shell's pump.
    fn publish(&self, formfactor: Formfactor) {
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let msg = FormfactorMsg {
            formfactor: formfactor.as_wire(),
        };
        let Ok(body) = serde_json::to_string(&msg) else {
            return;
        };
        // arch-11: best-effort writer — kept on Persist::open (the shared
        // BusReader seam is read-only).
        let _ = Persist::open(root)
            .and_then(|p| p.write(FORMFACTOR_TOPIC, Priority::Default, None, Some(&body)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_serializes_the_wire_token() {
        let laptop = serde_json::to_string(&FormfactorMsg {
            formfactor: Formfactor::Laptop.as_wire(),
        })
        .expect("serializes");
        assert_eq!(laptop, r#"{"formfactor":"laptop"}"#);
        let tablet = serde_json::to_string(&FormfactorMsg {
            formfactor: Formfactor::Tablet.as_wire(),
        })
        .expect("serializes");
        assert_eq!(tablet, r#"{"formfactor":"tablet"}"#);
    }

    #[test]
    fn publish_without_a_bus_root_is_a_silent_no_op() {
        let pubr = FormfactorPublisher { bus_root: None };
        pubr.publish(Formfactor::Tablet); // no spool, no panic — honest silence.
    }

    #[test]
    fn publish_writes_the_typed_message_to_the_topic() {
        let dir = std::env::temp_dir().join(format!(
            "formfactor_pub_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let pubr = FormfactorPublisher {
            bus_root: Some(dir.clone()),
        };
        pubr.publish(Formfactor::Tablet);

        let persist = Persist::open(dir).expect("open temp bus");
        let latest = persist
            .list_since(FORMFACTOR_TOPIC, None)
            .expect("list")
            .into_iter()
            .next_back()
            .and_then(|m| m.body)
            .expect("a published formfactor");
        assert!(latest.contains(r#""formfactor":"tablet""#), "{latest}");
    }
}
