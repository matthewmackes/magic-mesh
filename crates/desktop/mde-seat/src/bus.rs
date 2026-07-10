//! The one system-D-Bus call path the three zbus clients share — a cached
//! blocking connection plus typed-error classification.
//!
//! This module is the only place the D-Bus I/O happens; the per-backend folds
//! stay pure and unit-tested. The connection
//! is dialled lazily, cached, and dropped on any call error so the next probe
//! re-dials (self-healing across a bus restart) — a dead bus folds into a typed
//! [`SeatError::Unavailable`], never a panic.

use std::sync::Mutex;

use zbus::blocking::Connection;

use crate::error::{classify_call, Backend, SeatError};

/// A lazily-dialled, cached system-bus connection scoped to one backend (so its
/// failures classify under that backend's [`Backend`] tag).
pub struct SysBus {
    backend: Backend,
    conn: Mutex<Option<Connection>>,
}

impl SysBus {
    /// A bus handle for `backend`. No I/O happens until the first call.
    pub(crate) const fn new(backend: Backend) -> Self {
        Self {
            backend,
            conn: Mutex::new(None),
        }
    }

    /// The cache lock, poison-proof: a panic elsewhere never wedges the probe
    /// (the inner `Option<Connection>` is valid in either case).
    fn slot(&self) -> std::sync::MutexGuard<'_, Option<Connection>> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The cached connection, dialling the system bus on first use. A host
    /// without a system bus (headless CI) folds to `Unavailable`.
    // The guard is deliberately held across `Connection::system()` so two threads
    // cannot both dial the bus and race to cache — tightening the drop would open
    // that double-dial window. (nursery lint; the wide lock is the intent.)
    #[allow(
        clippy::significant_drop_tightening,
        reason = "the guard serializes first-use bus dialing on purpose"
    )]
    fn connection(&self) -> Result<Connection, SeatError> {
        let mut slot = self.slot();
        if let Some(c) = slot.as_ref() {
            return Ok(c.clone());
        }
        let c = Connection::system().map_err(|e| SeatError::Unavailable {
            backend: self.backend,
            reason: format!("system D-Bus unreachable: {e}"),
        })?;
        *slot = Some(c.clone());
        Ok(c)
    }

    /// One method call, reply discarded (the fire-and-observe verbs).
    ///
    /// # Errors
    /// Typed: `Unavailable` for an absent service/bus, `Backend` otherwise.
    pub(crate) fn call_unit<B>(
        &self,
        destination: &str,
        path: &str,
        interface: &str,
        method: &str,
        body: &B,
    ) -> Result<(), SeatError>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.request(destination, path, interface, method, body)
            .map(|_| ())
    }

    /// One method call with a typed reply.
    ///
    /// # Errors
    /// Typed: `Unavailable` / `Backend` for the call, `Protocol` when the reply
    /// body does not deserialize as `R`.
    pub(crate) fn call<B, R>(
        &self,
        destination: &str,
        path: &str,
        interface: &str,
        method: &str,
        body: &B,
    ) -> Result<R, SeatError>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        let msg = self.request(destination, path, interface, method, body)?;
        let body = msg.body();
        body.deserialize::<R>().map_err(|e| SeatError::Protocol {
            backend: self.backend,
            reason: format!("{interface}.{method} reply: {e}"),
        })
    }

    /// The single I/O choke point: issue the call, and on failure drop the
    /// cached connection (next probe re-dials) + classify the error.
    fn request<B>(
        &self,
        destination: &str,
        path: &str,
        interface: &str,
        method: &str,
        body: &B,
    ) -> Result<zbus::Message, SeatError>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        let conn = self.connection()?;
        conn.call_method(Some(destination), path, Some(interface), method, body)
            .map_err(|e| {
                *self.slot() = None;
                classify_call(self.backend, &format!("{interface}.{method}"), &e)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_on_a_host_without_the_service_is_typed_not_a_panic() {
        // Whatever this host looks like (no system bus at all, or a bus without
        // a "mde.test.NoSuchService" name), the call must come back as a typed
        // SeatError tagged with the right backend — the honest probe path.
        let bus = SysBus::new(Backend::UPower);
        let r: Result<String, SeatError> = bus.call(
            "mde.test.NoSuchService",
            "/mde/test",
            "mde.test.Iface",
            "Nope",
            &(),
        );
        let e = r.expect_err("a missing service must not answer");
        assert_eq!(e.backend(), Backend::UPower);
        assert!(
            matches!(e, SeatError::Unavailable { .. } | SeatError::Backend { .. }),
            "{e}"
        );
    }
}
