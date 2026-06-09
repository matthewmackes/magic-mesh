//! D-Bus single-instance handshake.
//!
//! CB-1.1 lock: "Single-instance via the already-shipped
//! `dev.mackes.MDE.Shell` D-Bus surface (existing service
//! registers the workbench when one is already running, else
//! launches)." The Shell service in `crates/mackesd/src/ipc/shell.rs`
//! ships the host name + healthz; the workbench *itself* claims
//! [`BUS_NAME`] (`dev.mackes.MDE.Workbench`) as its own
//! well-known name. If the request returns `Exists` /
//! `AlreadyOwner`, a sibling process is already running and the
//! caller should hand its `--focus <slug>` argument off via
//! CB-1.13's `Focus` interface instead of opening a second
//! window.
//!
//! The decision logic is split off from the zbus I/O so it can
//! be tested without spinning up a real session bus.

use zbus::fdo::RequestNameReply;

/// Well-known D-Bus name the workbench acquires at startup.
/// Sibling-process detection uses [`Connection::request_name_with_flags`]
/// + the [`DoNotQueue`](zbus::fdo::RequestNameFlags::DO_NOT_QUEUE)
/// flag so a second process gets a clean `Exists` immediately,
/// not a queued reply.
pub const BUS_NAME: &str = "dev.mackes.MDE.Workbench";

/// Result of asking the bus for [`BUS_NAME`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryStatus {
    /// This process now owns [`BUS_NAME`]. Open the workbench
    /// window normally.
    Primary,
    /// A sibling already owns the name. Hand the `--focus`
    /// argument off via CB-1.13's `Focus` and exit.
    Existing,
}

/// Pure decision function — maps zbus's request-name outcome
/// onto the workbench's single-instance contract. Tested in
/// isolation against every [`RequestNameReply`] variant.
#[must_use]
pub const fn decide_primary_status(reply: RequestNameReply) -> PrimaryStatus {
    match reply {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => PrimaryStatus::Primary,
        RequestNameReply::Exists | RequestNameReply::InQueue => PrimaryStatus::Existing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_name_carries_mde_namespace() {
        assert_eq!(BUS_NAME, "dev.mackes.MDE.Workbench");
        assert!(BUS_NAME.starts_with("dev.mackes.MDE."));
    }

    #[test]
    fn primary_owner_means_we_are_primary() {
        assert_eq!(
            decide_primary_status(RequestNameReply::PrimaryOwner),
            PrimaryStatus::Primary
        );
    }

    #[test]
    fn already_owner_treated_as_primary() {
        // Re-requesting our own name on the same connection
        // returns AlreadyOwner — still us, so still Primary.
        assert_eq!(
            decide_primary_status(RequestNameReply::AlreadyOwner),
            PrimaryStatus::Primary
        );
    }

    #[test]
    fn exists_means_hand_off() {
        assert_eq!(
            decide_primary_status(RequestNameReply::Exists),
            PrimaryStatus::Existing
        );
    }

    #[test]
    fn in_queue_still_means_hand_off() {
        // DO_NOT_QUEUE should keep us out of this branch in
        // practice; the mapping still has to be defined so
        // an accidental queue request doesn't silently launch
        // a duplicate window.
        assert_eq!(
            decide_primary_status(RequestNameReply::InQueue),
            PrimaryStatus::Existing
        );
    }
}
