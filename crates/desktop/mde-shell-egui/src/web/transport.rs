use mackes_mesh_types::vdi_session::BrowserVmTransport;

const RDP_READY_DETAIL: &str =
    "The in-shell RDP client is available; endpoint health is verified by the live attachment.";
const SUNSHINE_UNAVAILABLE_DETAIL: &str =
    "No seat-side Moonlight adapter is installed, so Sunshine cannot render this workload yet.";
const MAX_FAILURE_DETAIL_CHARS: usize = 512;

/// Health known by the Browser controller for one display path.
///
/// `ReadyToTry` deliberately does not claim an endpoint is healthy before the
/// broker and transport complete a live attachment. `Unavailable` is reserved
/// for a known missing capability, while `AttemptFailed` retains the exact last
/// connection failure for the bounded retry path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TransportHealth {
    ReadyToTry,
    AttemptFailed(String),
    Unavailable(&'static str),
}

impl TransportHealth {
    pub(super) const fn label(&self) -> &'static str {
        match self {
            Self::ReadyToTry => "Ready to try",
            Self::AttemptFailed(_) => "Last attempt failed",
            Self::Unavailable(_) => "Unavailable",
        }
    }

    pub(super) fn detail(&self) -> &str {
        match self {
            Self::ReadyToTry => RDP_READY_DETAIL,
            Self::AttemptFailed(detail) => detail,
            Self::Unavailable(detail) => detail,
        }
    }

    pub(super) const fn can_attempt(&self) -> bool {
        !matches!(self, Self::Unavailable(_))
    }
}

/// Per-transport capability/attempt state. Generic workload reachability is
/// intentionally kept separate: it is not proof that either protocol works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BrowserVmTransportHealth {
    rdp: TransportHealth,
    sunshine: TransportHealth,
}

impl Default for BrowserVmTransportHealth {
    fn default() -> Self {
        Self {
            rdp: TransportHealth::ReadyToTry,
            sunshine: TransportHealth::Unavailable(SUNSHINE_UNAVAILABLE_DETAIL),
        }
    }
}

impl BrowserVmTransportHealth {
    pub(super) const fn get(&self, transport: BrowserVmTransport) -> &TransportHealth {
        match transport {
            BrowserVmTransport::Rdp => &self.rdp,
            BrowserVmTransport::Sunshine => &self.sunshine,
        }
    }

    pub(super) fn note_attempt_failed(
        &mut self,
        transport: BrowserVmTransport,
        detail: impl Into<String>,
    ) {
        let slot = match transport {
            BrowserVmTransport::Rdp => &mut self.rdp,
            BrowserVmTransport::Sunshine => &mut self.sunshine,
        };
        if matches!(slot, TransportHealth::Unavailable(_)) {
            return;
        }
        let detail = detail.into();
        let detail = detail
            .trim()
            .chars()
            .take(MAX_FAILURE_DETAIL_CHARS)
            .collect();
        *slot = TransportHealth::AttemptFailed(detail);
    }

    /// An explicit operator selection permits a fresh bounded attempt after a
    /// prior connection failure. It never upgrades a statically unavailable
    /// transport into a usable one.
    pub(super) fn prepare_explicit_attempt(&mut self, transport: BrowserVmTransport) {
        let slot = match transport {
            BrowserVmTransport::Rdp => &mut self.rdp,
            BrowserVmTransport::Sunshine => &mut self.sunshine,
        };
        if matches!(slot, TransportHealth::AttemptFailed(_)) {
            *slot = TransportHealth::ReadyToTry;
        }
    }
}
