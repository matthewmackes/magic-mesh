//! Clipboard V2 permission and audit model.
//!
//! This module is deliberately render- and transport-agnostic. Callers admit
//! already-received Clipboard V2 metadata on their polling/event path, pass an
//! injected clock value, and render the returned snapshot. No clipboard bytes,
//! Files paths, Bus handles, clocks, or persistence handles enter this model.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
};

use mackes_mesh_types::vdi_clipboard::{
    ClipboardEnvelopeV2, VdiClipboardDisclosureV2, VdiClipboardLeaseV2, VdiClipboardMessageV2,
    VdiClipboardReceiptV2,
};
use mde_egui::{egui, Style};

/// Maximum completed/refused actions retained for the operator-facing audit.
pub(crate) const MAX_CLIPBOARD_AUDIT_ROWS: usize = 128;
/// Maximum replay high-water marks retained across distinct clipboard sources.
const MAX_REPLAY_MARKS: usize = 128;
const MAX_TARGET_LABEL_BYTES: usize = 128;
const CLIPBOARD_GATE_QUEUE_DEPTH: usize = 8;
const CLIPBOARD_GATE_PENDING: u8 = 0;
const CLIPBOARD_GATE_APPROVED: u8 = 1;
const CLIPBOARD_GATE_REFUSED: u8 = 2;
const CLIPBOARD_GATE_MATERIALIZING: u8 = 3;

/// Payload-free V2 metadata copied at the admitted transport boundary. Lease
/// identity is retained only long enough to revoke a pending one-use decision;
/// it is never rendered or written to audit.
#[derive(Debug, Clone)]
struct ClipboardGateMetadata {
    source_node: String,
    source_seat: String,
    source_session: String,
    sequence: u64,
    mime_offers: Vec<String>,
    has_files_reference: bool,
    byte_count: u64,
    expires_at_ms: u64,
    selected_mime: String,
    disclosure: VdiClipboardDisclosureV2,
    session_id: String,
    session_generation: u64,
    lease_id: String,
    lease_expires_at_ms: u64,
    cross_boundary: bool,
    target: ClipboardTarget,
}

impl ClipboardGateMetadata {
    fn admitted_vdi(
        message: &VdiClipboardMessageV2,
        lease: &VdiClipboardLeaseV2,
        previous_receipt: Option<&VdiClipboardReceiptV2>,
        target: ClipboardTarget,
        now_ms: u64,
    ) -> Result<Self, ClipboardPermissionError> {
        message
            .admit(lease, previous_receipt, now_ms)
            .map_err(map_vdi_refusal)?;
        Ok(Self {
            source_node: message.envelope.source_node.clone(),
            source_seat: message.envelope.source_seat.clone(),
            source_session: message.envelope.source_session.clone(),
            sequence: message.envelope.sequence,
            mime_offers: message.envelope.mime_offers.clone(),
            has_files_reference: message.envelope.files_reference.is_some(),
            byte_count: message.envelope.byte_count,
            // A permission cannot outlive either authority that admitted it.
            // Keeping only the envelope expiry would let an approval minted
            // immediately before lease expiry materialize after lease expiry.
            expires_at_ms: message.envelope.expires_at_ms.min(lease.expires_at_ms),
            selected_mime: message.selected_mime.clone(),
            disclosure: message.disclosure,
            session_id: message.session_id.clone(),
            session_generation: message.generation,
            lease_id: lease.lease_id.clone(),
            lease_expires_at_ms: lease.expires_at_ms,
            cross_boundary: true,
            target,
        })
    }
}

#[derive(Debug)]
struct ClipboardGateSubmission {
    metadata: ClipboardGateMetadata,
    state: Arc<AtomicU8>,
    update_rx: Receiver<ClipboardGateUpdate>,
}

#[derive(Debug, Clone, Copy)]
enum ClipboardGateUpdate {
    Progress(u64),
    Complete(u64),
    Failed(ClipboardFailure, u64),
}

/// Cloneable, bounded transport ingress. Submission copies V2 metadata only;
/// the transport keeps ownership of its existing pending payload.
#[derive(Debug, Clone)]
pub(crate) struct ClipboardPermissionIngress {
    request_tx: SyncSender<ClipboardGateSubmission>,
}

impl ClipboardPermissionIngress {
    pub(crate) fn submit_vdi(
        &self,
        message: &VdiClipboardMessageV2,
        lease: &VdiClipboardLeaseV2,
        previous_receipt: Option<&VdiClipboardReceiptV2>,
        target: ClipboardTarget,
        now_ms: u64,
    ) -> Result<ClipboardGateTicket, ClipboardPermissionError> {
        let metadata =
            ClipboardGateMetadata::admitted_vdi(message, lease, previous_receipt, target, now_ms)?;
        let state = Arc::new(AtomicU8::new(CLIPBOARD_GATE_PENDING));
        let (update_tx, update_rx) = mpsc::sync_channel(CLIPBOARD_GATE_QUEUE_DEPTH);
        let submission = ClipboardGateSubmission {
            metadata,
            state: state.clone(),
            update_rx,
        };
        self.request_tx
            .try_send(submission)
            .map_err(|error| match error {
                TrySendError::Full(_) => ClipboardPermissionError::Busy,
                TrySendError::Disconnected(_) => ClipboardPermissionError::NoActiveTransfer,
            })?;
        Ok(ClipboardGateTicket { state, update_tx })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardGateReadiness {
    Pending,
    Materialize,
    Refused,
}

/// Transport-side one-use ticket. `try_begin_materialization` is the
/// linearization point: focus/session/lease revocation can win until its single
/// approved-to-materializing compare/exchange succeeds.
#[derive(Debug)]
pub(crate) struct ClipboardGateTicket {
    state: Arc<AtomicU8>,
    update_tx: SyncSender<ClipboardGateUpdate>,
}

impl ClipboardGateTicket {
    /// Observe whether an offer may be advertised without consuming the
    /// one-use materialization transition. SPICE needs this split because GRAB
    /// advertises metadata first and the guest requests the bytes later.
    pub(crate) fn readiness_before_materialization(&self) -> ClipboardGateReadiness {
        match self.state.load(Ordering::Acquire) {
            CLIPBOARD_GATE_APPROVED => ClipboardGateReadiness::Materialize,
            CLIPBOARD_GATE_PENDING | CLIPBOARD_GATE_MATERIALIZING => {
                ClipboardGateReadiness::Pending
            }
            _ => ClipboardGateReadiness::Refused,
        }
    }

    pub(crate) fn try_begin_materialization(&self) -> ClipboardGateReadiness {
        match self.state.load(Ordering::Acquire) {
            CLIPBOARD_GATE_PENDING => ClipboardGateReadiness::Pending,
            CLIPBOARD_GATE_REFUSED => ClipboardGateReadiness::Refused,
            CLIPBOARD_GATE_APPROVED => {
                if self
                    .state
                    .compare_exchange(
                        CLIPBOARD_GATE_APPROVED,
                        CLIPBOARD_GATE_MATERIALIZING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    ClipboardGateReadiness::Materialize
                } else {
                    self.try_begin_materialization()
                }
            }
            CLIPBOARD_GATE_MATERIALIZING => ClipboardGateReadiness::Pending,
            _ => ClipboardGateReadiness::Refused,
        }
    }

    pub(crate) fn report_progress(&self, transferred: u64) {
        let _ = self
            .update_tx
            .try_send(ClipboardGateUpdate::Progress(transferred));
    }

    pub(crate) fn report_complete(&self, now_ms: u64) {
        let _ = self
            .update_tx
            .try_send(ClipboardGateUpdate::Complete(now_ms));
    }

    pub(crate) fn report_failure(&self, failure: ClipboardFailure, now_ms: u64) {
        let _ = self
            .update_tx
            .try_send(ClipboardGateUpdate::Failed(failure, now_ms));
    }
}

/// Where an admitted clipboard representation would materialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardTargetKind {
    LocalSeat,
    Peer,
    Guest,
}

/// Safe display identity for the target. It is not a transport address or
/// capability and is bounded before being retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipboardTarget {
    pub(crate) kind: ClipboardTargetKind,
    pub(crate) label: String,
}

impl ClipboardTarget {
    pub(crate) fn new(
        kind: ClipboardTargetKind,
        label: impl Into<String>,
    ) -> Result<Self, ClipboardPermissionError> {
        let label = label.into();
        if label.is_empty()
            || label.len() > MAX_TARGET_LABEL_BYTES
            || label.trim() != label
            || label.chars().any(char::is_control)
        {
            return Err(ClipboardPermissionError::InvalidMetadata);
        }
        Ok(Self { kind, label })
    }
}

/// Caller-owned live context. Generations make approval invalidation explicit;
/// the model never polls focus, sessions, leases, or a clock itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipboardPermissionContext {
    pub(crate) focused: bool,
    pub(crate) focus_generation: u64,
    pub(crate) session_generation: u64,
    pub(crate) lease_generation: u64,
}

impl ClipboardPermissionContext {
    fn valid(&self) -> bool {
        self.focused
            && self.focus_generation > 0
            && self.session_generation > 0
            && self.lease_generation > 0
    }
}

/// The complete payload-free disclosure shown before approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipboardTransferSummary {
    pub(crate) source: String,
    pub(crate) target: ClipboardTarget,
    pub(crate) mime: String,
    pub(crate) byte_count: u64,
    pub(crate) expires_at_ms: u64,
}

/// One-use approval handle. A stale prompt cannot approve a newer transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClipboardApprovalToken(u64);

/// Typed terminal/runtime failures suitable for UI copy and audit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardFailure {
    Expired,
    FocusLost,
    SessionChanged,
    LeaseChanged,
    Transport,
    Policy,
}

/// Current state of the one operator-visible transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipboardTransferState {
    AwaitingApproval { token: ClipboardApprovalToken },
    Approved,
    InProgress { transferred: u64, total: u64 },
    Denied,
    Cancelled,
    Failed(ClipboardFailure),
    Completed,
}

/// Typed, stable refusal vocabulary for controller/UI integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardPermissionError {
    InvalidMetadata,
    UnsupportedMime,
    SecretBearing,
    Expired,
    StaleOrReplay,
    FocusRequired,
    Busy,
    NoActiveTransfer,
    ApprovalRequired,
    ApprovalReplay,
    InvalidProgress,
}

/// Credential- and payload-redacted audit outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardAuditOutcome {
    Approved,
    Denied,
    Cancelled,
    Completed,
    Refused(ClipboardPermissionError),
    Failed(ClipboardFailure),
}

/// Bounded audit row. There is intentionally no preview, content hash, payload
/// reference, source session, lease identity, credential, or payload field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClipboardAuditRow {
    pub(crate) source: String,
    pub(crate) target: String,
    pub(crate) mime: String,
    pub(crate) byte_count: u64,
    pub(crate) expires_at_ms: u64,
    pub(crate) recorded_at_ms: u64,
    pub(crate) outcome: ClipboardAuditOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReplayMark {
    source_node: String,
    source_seat: String,
    source_session: String,
    sequence: u64,
    /// The high-water mark is useful only while the authority that admitted
    /// its sequence could still be live. Keeping it longer can permanently
    /// strand a restarted source/session that legitimately resets sequencing.
    expires_at_ms: u64,
}

#[derive(Debug, Clone)]
struct ActiveTransfer {
    summary: ClipboardTransferSummary,
    state: ClipboardTransferState,
    binding: ClipboardPermissionContext,
    replay: ReplayMark,
}

/// Bounded operational permission model. It is not a clipboard store: only one
/// active metadata summary, payload-free replay marks, and redacted audit rows
/// are retained.
#[derive(Debug, Default)]
pub(crate) struct ClipboardPermissionModel {
    active: Option<ActiveTransfer>,
    replay_marks: VecDeque<ReplayMark>,
    audit: VecDeque<ClipboardAuditRow>,
    next_token: u64,
}

impl ClipboardPermissionModel {
    pub(crate) fn active_summary(&self) -> Option<&ClipboardTransferSummary> {
        self.active.as_ref().map(|active| &active.summary)
    }

    pub(crate) fn active_state(&self) -> Option<&ClipboardTransferState> {
        self.active.as_ref().map(|active| &active.state)
    }

    pub(crate) fn audit_rows(&self) -> impl ExactSizeIterator<Item = &ClipboardAuditRow> {
        self.audit.iter()
    }

    fn dismiss_terminal(&mut self) -> Result<(), ClipboardPermissionError> {
        if self
            .active
            .as_ref()
            .is_some_and(|active| terminal_state(&active.state))
        {
            self.active = None;
            Ok(())
        } else {
            Err(ClipboardPermissionError::NoActiveTransfer)
        }
    }

    /// Admit a VDI transfer through the existing lease/receipt contract, then
    /// create a permission decision from metadata only.
    pub(crate) fn request_vdi(
        &mut self,
        message: &VdiClipboardMessageV2,
        lease: &VdiClipboardLeaseV2,
        previous_receipt: Option<&VdiClipboardReceiptV2>,
        target: ClipboardTarget,
        context: ClipboardPermissionContext,
        now_ms: u64,
    ) -> Result<Option<ClipboardApprovalToken>, ClipboardPermissionError> {
        let metadata = match ClipboardGateMetadata::admitted_vdi(
            message,
            lease,
            previous_receipt,
            target.clone(),
            now_ms,
        ) {
            Ok(metadata) => metadata,
            Err(refusal) => {
                self.audit_refusal(
                    &message.envelope,
                    &target,
                    &message.selected_mime,
                    now_ms,
                    refusal,
                );
                return Err(refusal);
            }
        };
        self.request_metadata(metadata, context, now_ms)
    }

    /// Admit a direct-DRM or peer Clipboard V2 envelope. This copies only its
    /// safe display metadata and source ordering identity.
    pub(crate) fn request(
        &mut self,
        envelope: &ClipboardEnvelopeV2,
        selected_mime: &str,
        disclosure: VdiClipboardDisclosureV2,
        target: ClipboardTarget,
        context: ClipboardPermissionContext,
        now_ms: u64,
    ) -> Result<Option<ClipboardApprovalToken>, ClipboardPermissionError> {
        if envelope.validate_at(now_ms).is_err() {
            let refusal = if now_ms >= envelope.expires_at_ms {
                ClipboardPermissionError::Expired
            } else {
                ClipboardPermissionError::InvalidMetadata
            };
            self.audit_refusal(envelope, &target, selected_mime, now_ms, refusal);
            return Err(refusal);
        }
        let metadata = ClipboardGateMetadata {
            source_node: envelope.source_node.clone(),
            source_seat: envelope.source_seat.clone(),
            source_session: envelope.source_session.clone(),
            sequence: envelope.sequence,
            mime_offers: envelope.mime_offers.clone(),
            has_files_reference: envelope.files_reference.is_some(),
            byte_count: envelope.byte_count,
            expires_at_ms: envelope.expires_at_ms,
            selected_mime: selected_mime.to_owned(),
            disclosure,
            session_id: envelope.source_session.clone(),
            session_generation: context.session_generation,
            lease_id: String::new(),
            lease_expires_at_ms: envelope.expires_at_ms,
            cross_boundary: false,
            target,
        };
        self.request_metadata(metadata, context, now_ms)
    }

    fn request_metadata(
        &mut self,
        metadata: ClipboardGateMetadata,
        context: ClipboardPermissionContext,
        now_ms: u64,
    ) -> Result<Option<ClipboardApprovalToken>, ClipboardPermissionError> {
        if self.active.as_ref().is_some_and(|active| {
            !matches!(
                active.state,
                ClipboardTransferState::Denied
                    | ClipboardTransferState::Cancelled
                    | ClipboardTransferState::Failed(_)
                    | ClipboardTransferState::Completed
            )
        }) {
            return Err(ClipboardPermissionError::Busy);
        }
        if !context.valid() {
            self.audit_metadata_refusal(&metadata, now_ms, ClipboardPermissionError::FocusRequired);
            return Err(ClipboardPermissionError::FocusRequired);
        }
        let refusal = if metadata.source_node.is_empty()
            || metadata.source_seat.is_empty()
            || metadata.source_session.is_empty()
            || metadata.sequence == 0
            || metadata.mime_offers.is_empty()
            || metadata.expires_at_ms == 0
        {
            Some(ClipboardPermissionError::InvalidMetadata)
        } else if now_ms >= metadata.expires_at_ms {
            Some(ClipboardPermissionError::Expired)
        } else if metadata.disclosure == VdiClipboardDisclosureV2::Secret
            || secret_bearing_mime(&metadata.selected_mime)
        {
            Some(ClipboardPermissionError::SecretBearing)
        } else if !metadata
            .mime_offers
            .iter()
            .any(|offer| offer.eq_ignore_ascii_case(&metadata.selected_mime))
        {
            Some(ClipboardPermissionError::UnsupportedMime)
        } else if self.is_metadata_replay(&metadata, now_ms) {
            Some(ClipboardPermissionError::StaleOrReplay)
        } else {
            None
        };
        if let Some(refusal) = refusal {
            self.audit_metadata_refusal(&metadata, now_ms, refusal);
            return Err(refusal);
        }

        let summary = ClipboardTransferSummary {
            source: format!(
                "{} / {} / {}",
                metadata.source_node, metadata.source_seat, metadata.source_session
            ),
            target: metadata.target.clone(),
            mime: metadata.selected_mime.clone(),
            byte_count: metadata.byte_count,
            expires_at_ms: metadata.expires_at_ms,
        };
        let replay = ReplayMark {
            source_node: metadata.source_node.clone(),
            source_seat: metadata.source_seat.clone(),
            source_session: metadata.source_session.clone(),
            sequence: metadata.sequence,
            expires_at_ms: metadata.expires_at_ms,
        };
        let requires_approval = (metadata.cross_boundary
            || summary.target.kind != ClipboardTargetKind::LocalSeat)
            && rich_metadata_representation(&metadata);
        let token = requires_approval.then(|| self.issue_token());
        self.active = Some(ActiveTransfer {
            summary,
            state: token.map_or(ClipboardTransferState::Approved, |token| {
                ClipboardTransferState::AwaitingApproval { token }
            }),
            binding: context,
            replay,
        });
        if token.is_none() {
            self.record_active(now_ms, ClipboardAuditOutcome::Approved);
        }
        Ok(token)
    }

    pub(crate) fn approve(
        &mut self,
        token: ClipboardApprovalToken,
        context: &ClipboardPermissionContext,
        now_ms: u64,
    ) -> Result<(), ClipboardPermissionError> {
        self.revoke_if_context_changed(context, now_ms)?;
        let expired = self
            .active
            .as_ref()
            .ok_or(ClipboardPermissionError::NoActiveTransfer)?
            .summary
            .expires_at_ms
            <= now_ms;
        if expired {
            self.fail(ClipboardFailure::Expired, now_ms)?;
            return Err(ClipboardPermissionError::Expired);
        }
        let active = self
            .active
            .as_mut()
            .ok_or(ClipboardPermissionError::NoActiveTransfer)?;
        match active.state {
            ClipboardTransferState::AwaitingApproval { token: expected } if expected == token => {
                active.state = ClipboardTransferState::Approved;
                self.record_active(now_ms, ClipboardAuditOutcome::Approved);
                Ok(())
            }
            ClipboardTransferState::AwaitingApproval { .. } => {
                Err(ClipboardPermissionError::ApprovalReplay)
            }
            _ => Err(ClipboardPermissionError::ApprovalReplay),
        }
    }

    pub(crate) fn deny(&mut self, now_ms: u64) -> Result<(), ClipboardPermissionError> {
        let active = self
            .active
            .as_mut()
            .ok_or(ClipboardPermissionError::NoActiveTransfer)?;
        if !matches!(
            active.state,
            ClipboardTransferState::AwaitingApproval { .. }
        ) {
            return Err(ClipboardPermissionError::ApprovalRequired);
        }
        active.state = ClipboardTransferState::Denied;
        self.remember_active_replay();
        self.record_active(now_ms, ClipboardAuditOutcome::Denied);
        Ok(())
    }

    pub(crate) fn cancel(&mut self, now_ms: u64) -> Result<(), ClipboardPermissionError> {
        let active = self
            .active
            .as_mut()
            .ok_or(ClipboardPermissionError::NoActiveTransfer)?;
        if matches!(
            active.state,
            ClipboardTransferState::Completed | ClipboardTransferState::Failed(_)
        ) {
            return Err(ClipboardPermissionError::NoActiveTransfer);
        }
        active.state = ClipboardTransferState::Cancelled;
        self.remember_active_replay();
        self.record_active(now_ms, ClipboardAuditOutcome::Cancelled);
        Ok(())
    }

    pub(crate) fn progress(&mut self, transferred: u64) -> Result<(), ClipboardPermissionError> {
        let active = self
            .active
            .as_mut()
            .ok_or(ClipboardPermissionError::NoActiveTransfer)?;
        let previous = match active.state {
            ClipboardTransferState::Approved => 0,
            ClipboardTransferState::InProgress { transferred, .. } => transferred,
            ClipboardTransferState::AwaitingApproval { .. } => {
                return Err(ClipboardPermissionError::ApprovalRequired)
            }
            _ => return Err(ClipboardPermissionError::NoActiveTransfer),
        };
        if transferred < previous || transferred > active.summary.byte_count {
            return Err(ClipboardPermissionError::InvalidProgress);
        }
        active.state = ClipboardTransferState::InProgress {
            transferred,
            total: active.summary.byte_count,
        };
        Ok(())
    }

    pub(crate) fn complete(&mut self, now_ms: u64) -> Result<(), ClipboardPermissionError> {
        let active = self
            .active
            .as_mut()
            .ok_or(ClipboardPermissionError::NoActiveTransfer)?;
        if !matches!(
            active.state,
            ClipboardTransferState::InProgress { transferred, total } if transferred == total
        ) {
            return Err(ClipboardPermissionError::InvalidProgress);
        }
        active.state = ClipboardTransferState::Completed;
        self.remember_active_replay();
        self.record_active(now_ms, ClipboardAuditOutcome::Completed);
        Ok(())
    }

    pub(crate) fn fail(
        &mut self,
        failure: ClipboardFailure,
        now_ms: u64,
    ) -> Result<(), ClipboardPermissionError> {
        let active = self
            .active
            .as_mut()
            .ok_or(ClipboardPermissionError::NoActiveTransfer)?;
        if matches!(
            active.state,
            ClipboardTransferState::Denied
                | ClipboardTransferState::Cancelled
                | ClipboardTransferState::Failed(_)
                | ClipboardTransferState::Completed
        ) {
            return Err(ClipboardPermissionError::NoActiveTransfer);
        }
        active.state = ClipboardTransferState::Failed(failure);
        self.remember_active_replay();
        self.record_active(now_ms, ClipboardAuditOutcome::Failed(failure));
        Ok(())
    }

    /// Revoke an approval/transfer immediately when its security binding moves.
    pub(crate) fn update_context(
        &mut self,
        context: &ClipboardPermissionContext,
        now_ms: u64,
    ) -> Result<(), ClipboardPermissionError> {
        self.revoke_if_context_changed(context, now_ms)
    }

    fn revoke_if_context_changed(
        &mut self,
        context: &ClipboardPermissionContext,
        now_ms: u64,
    ) -> Result<(), ClipboardPermissionError> {
        let Some(active) = self.active.as_ref() else {
            return Ok(());
        };
        if matches!(
            active.state,
            ClipboardTransferState::Denied
                | ClipboardTransferState::Cancelled
                | ClipboardTransferState::Failed(_)
                | ClipboardTransferState::Completed
        ) {
            return Ok(());
        }
        let failure =
            if !context.focused || context.focus_generation != active.binding.focus_generation {
                Some(ClipboardFailure::FocusLost)
            } else if context.session_generation != active.binding.session_generation {
                Some(ClipboardFailure::SessionChanged)
            } else if context.lease_generation != active.binding.lease_generation {
                Some(ClipboardFailure::LeaseChanged)
            } else {
                None
            };
        if let Some(failure) = failure {
            self.fail(failure, now_ms)?;
        }
        Ok(())
    }

    fn issue_token(&mut self) -> ClipboardApprovalToken {
        self.next_token = self.next_token.wrapping_add(1).max(1);
        ClipboardApprovalToken(self.next_token)
    }

    fn is_metadata_replay(&mut self, metadata: &ClipboardGateMetadata, now_ms: u64) -> bool {
        // Count-bounding alone leaves recently active sources vulnerable to
        // arbitrary eviction while quiet sources can remain blocked forever.
        // Expire marks at their admitting envelope/lease boundary before
        // comparing sequence high-water marks.
        self.replay_marks
            .retain(|mark| now_ms < mark.expires_at_ms);
        self.replay_marks.iter().any(|mark| {
            mark.source_node == metadata.source_node
                && mark.source_seat == metadata.source_seat
                && mark.source_session == metadata.source_session
                && metadata.sequence <= mark.sequence
        })
    }

    fn remember_active_replay(&mut self) {
        let Some(mark) = self.active.as_ref().map(|active| active.replay.clone()) else {
            return;
        };
        if let Some(existing) = self.replay_marks.iter_mut().find(|existing| {
            existing.source_node == mark.source_node
                && existing.source_seat == mark.source_seat
                && existing.source_session == mark.source_session
        }) {
            existing.sequence = existing.sequence.max(mark.sequence);
            existing.expires_at_ms = existing.expires_at_ms.max(mark.expires_at_ms);
            return;
        }
        push_bounded(&mut self.replay_marks, mark, MAX_REPLAY_MARKS);
    }

    fn record_active(&mut self, now_ms: u64, outcome: ClipboardAuditOutcome) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        let row = ClipboardAuditRow {
            source: format!(
                "{} / {}",
                active.replay.source_node, active.replay.source_seat
            ),
            target: active.summary.target.label.clone(),
            mime: active.summary.mime.clone(),
            byte_count: active.summary.byte_count,
            expires_at_ms: active.summary.expires_at_ms,
            recorded_at_ms: now_ms,
            outcome,
        };
        push_bounded(&mut self.audit, row, MAX_CLIPBOARD_AUDIT_ROWS);
    }

    fn audit_refusal(
        &mut self,
        envelope: &ClipboardEnvelopeV2,
        target: &ClipboardTarget,
        mime: &str,
        now_ms: u64,
        refusal: ClipboardPermissionError,
    ) {
        let row = ClipboardAuditRow {
            source: bounded_audit_source(&envelope.source_node, &envelope.source_seat),
            target: bounded_audit_text(&target.label),
            mime: bounded_audit_text(mime),
            byte_count: envelope.byte_count,
            expires_at_ms: envelope.expires_at_ms,
            recorded_at_ms: now_ms,
            outcome: ClipboardAuditOutcome::Refused(refusal),
        };
        push_bounded(&mut self.audit, row, MAX_CLIPBOARD_AUDIT_ROWS);
    }

    fn audit_metadata_refusal(
        &mut self,
        metadata: &ClipboardGateMetadata,
        now_ms: u64,
        refusal: ClipboardPermissionError,
    ) {
        let row = ClipboardAuditRow {
            source: bounded_audit_source(&metadata.source_node, &metadata.source_seat),
            target: bounded_audit_text(&metadata.target.label),
            mime: bounded_audit_text(&metadata.selected_mime),
            byte_count: metadata.byte_count,
            expires_at_ms: metadata.expires_at_ms,
            recorded_at_ms: now_ms,
            outcome: ClipboardAuditOutcome::Refused(refusal),
        };
        push_bounded(&mut self.audit, row, MAX_CLIPBOARD_AUDIT_ROWS);
    }
}

/// Shell-level command emitted by the pure permission card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardOperatorAction {
    Approve(ClipboardApprovalToken),
    Deny,
    Cancel,
    Dismiss,
}

#[derive(Debug)]
struct ActiveClipboardGate {
    state: Arc<AtomicU8>,
    update_rx: Receiver<ClipboardGateUpdate>,
}

/// Runtime-reachable shell controller. It owns permission metadata only and
/// deliberately has no clipboard provider, payload cache, Bus handle, clock,
/// or persistence handle.
#[derive(Debug)]
pub(crate) struct ClipboardPermissionController {
    model: ClipboardPermissionModel,
    context: ClipboardPermissionContext,
    ingress: ClipboardPermissionIngress,
    request_rx: Receiver<ClipboardGateSubmission>,
    active_gate: Option<ActiveClipboardGate>,
    session_binding: Option<(String, u64)>,
    lease_binding: Option<(String, u64, u64)>,
}

impl Default for ClipboardPermissionController {
    fn default() -> Self {
        let (request_tx, request_rx) = mpsc::sync_channel(CLIPBOARD_GATE_QUEUE_DEPTH);
        Self {
            model: ClipboardPermissionModel::default(),
            context: ClipboardPermissionContext {
                focused: true,
                focus_generation: 1,
                session_generation: 1,
                lease_generation: 1,
            },
            ingress: ClipboardPermissionIngress { request_tx },
            request_rx,
            active_gate: None,
            session_binding: None,
            lease_binding: None,
        }
    }
}

impl ClipboardPermissionController {
    pub(crate) fn ingress(&self) -> ClipboardPermissionIngress {
        self.ingress.clone()
    }

    /// Drain the bounded transport bridge from the shell's normal model-pump
    /// path. This performs no Bus, clock, persistence, or payload work.
    pub(crate) fn poll_ingress(&mut self, now_ms: u64) {
        let mut updates = Vec::new();
        let mut transport_disconnected = false;
        if let Some(gate) = &self.active_gate {
            loop {
                match gate.update_rx.try_recv() {
                    Ok(update) => updates.push(update),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        transport_disconnected = true;
                        break;
                    }
                }
            }
        }
        for update in updates {
            match update {
                ClipboardGateUpdate::Progress(transferred) => {
                    let _ = self.model.progress(transferred);
                }
                ClipboardGateUpdate::Complete(recorded_at_ms) => {
                    let _ = self.model.complete(recorded_at_ms);
                }
                ClipboardGateUpdate::Failed(failure, recorded_at_ms) => {
                    let _ = self.model.fail(failure, recorded_at_ms);
                }
            }
        }
        // The ticket owns the only update sender. If its transport worker dies
        // during approval/materialization, keeping the receiver as an active
        // gate strands every reconnect submission behind `Busy` until expiry.
        // Convert that orphan into an explicit terminal failure before
        // releasing the payload-free gate. The replay mark is retained, so a
        // reconnect cannot duplicate the abandoned sequence; a newer command
        // can be admitted immediately.
        if transport_disconnected
            && self
                .model
                .active_state()
                .is_some_and(|state| !terminal_state(state))
        {
            let _ = self.model.fail(ClipboardFailure::Transport, now_ms);
        }
        self.sync_gate_state();
        self.release_terminal_gate();

        loop {
            let submission = match self.request_rx.try_recv() {
                Ok(submission) => submission,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            };
            if self.active_gate.is_some() {
                submission
                    .state
                    .store(CLIPBOARD_GATE_REFUSED, Ordering::Release);
                continue;
            }
            if self
                .bind_runtime_metadata(&submission.metadata, now_ms)
                .is_err()
            {
                submission
                    .state
                    .store(CLIPBOARD_GATE_REFUSED, Ordering::Release);
                continue;
            }
            match self
                .model
                .request_metadata(submission.metadata, self.context.clone(), now_ms)
            {
                Ok(token) => {
                    submission.state.store(
                        if token.is_some() {
                            CLIPBOARD_GATE_PENDING
                        } else {
                            CLIPBOARD_GATE_APPROVED
                        },
                        Ordering::Release,
                    );
                    self.active_gate = Some(ActiveClipboardGate {
                        state: submission.state,
                        update_rx: submission.update_rx,
                    });
                }
                Err(_) => submission
                    .state
                    .store(CLIPBOARD_GATE_REFUSED, Ordering::Release),
            }
        }
    }

    /// Metadata-only ingress for a transport controller. The existing V2
    /// envelope/lease/receipt admission remains the source of truth.
    pub(crate) fn request_vdi(
        &mut self,
        message: &VdiClipboardMessageV2,
        lease: &VdiClipboardLeaseV2,
        previous_receipt: Option<&VdiClipboardReceiptV2>,
        target: ClipboardTarget,
        now_ms: u64,
    ) -> Result<Option<ClipboardApprovalToken>, ClipboardPermissionError> {
        self.bind_generations(message.generation, lease.generation, now_ms)?;
        self.model.request_vdi(
            message,
            lease,
            previous_receipt,
            target,
            self.context.clone(),
            now_ms,
        )
    }

    /// Metadata-only ingress for direct peer/DRM controllers.
    pub(crate) fn request(
        &mut self,
        envelope: &ClipboardEnvelopeV2,
        selected_mime: &str,
        disclosure: VdiClipboardDisclosureV2,
        target: ClipboardTarget,
        now_ms: u64,
    ) -> Result<Option<ClipboardApprovalToken>, ClipboardPermissionError> {
        self.model.request(
            envelope,
            selected_mime,
            disclosure,
            target,
            self.context.clone(),
            now_ms,
        )
    }

    /// Bind a transport's current session/lease generations. Any change revokes
    /// an approval or in-flight operation before a later payload can materialize.
    pub(crate) fn bind_generations(
        &mut self,
        session_generation: u64,
        lease_generation: u64,
        now_ms: u64,
    ) -> Result<(), ClipboardPermissionError> {
        if session_generation == 0 || lease_generation == 0 {
            return Err(ClipboardPermissionError::InvalidMetadata);
        }
        self.context.session_generation = session_generation;
        self.context.lease_generation = lease_generation;
        let result = self.model.update_context(&self.context, now_ms);
        self.sync_gate_state();
        self.release_terminal_gate();
        result
    }

    /// Fold the shell's already-known focus state. The caller supplies time;
    /// this method performs no compositor query or clock read.
    pub(crate) fn set_focused(
        &mut self,
        focused: bool,
        now_ms: u64,
    ) -> Result<(), ClipboardPermissionError> {
        if self.context.focused != focused {
            self.context.focused = focused;
            self.context.focus_generation = self.context.focus_generation.saturating_add(1);
        }
        let result = self.model.update_context(&self.context, now_ms);
        self.sync_gate_state();
        self.release_terminal_gate();
        result
    }

    pub(crate) fn report_progress(
        &mut self,
        transferred: u64,
    ) -> Result<(), ClipboardPermissionError> {
        self.model.progress(transferred)
    }

    pub(crate) fn report_complete(&mut self, now_ms: u64) -> Result<(), ClipboardPermissionError> {
        self.model.complete(now_ms)
    }

    pub(crate) fn report_failure(
        &mut self,
        failure: ClipboardFailure,
        now_ms: u64,
    ) -> Result<(), ClipboardPermissionError> {
        self.model.fail(failure, now_ms)
    }

    /// Advance expiry and paint the modal. This is pure over cached model state
    /// and the injected clock value; rendering never opens the Bus or filesystem.
    pub(crate) fn mount(&mut self, ctx: &egui::Context, now_ms: u64) {
        self.expire(now_ms);
        let Some((summary, state)) = self.view() else {
            return;
        };
        let mut action = None;
        egui::Window::new("Clipboard transfer")
            .id(egui::Id::new("clipboard-permission-dialog"))
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                action = render_transfer_card(ui, &summary, &state, now_ms);
            });
        if let Some(action) = action {
            let _ = self.apply_operator_action(action, now_ms);
        }
    }

    fn view(&self) -> Option<(ClipboardTransferSummary, ClipboardTransferState)> {
        Some((
            self.model.active_summary()?.clone(),
            self.model.active_state()?.clone(),
        ))
    }

    fn expire(&mut self, now_ms: u64) {
        let expired = self
            .model
            .active_summary()
            .zip(self.model.active_state())
            .is_some_and(|(summary, state)| {
                now_ms >= summary.expires_at_ms && !terminal_state(state)
            });
        if expired {
            let _ = self.model.fail(ClipboardFailure::Expired, now_ms);
            self.sync_gate_state();
            self.release_terminal_gate();
        }
    }

    fn apply_operator_action(
        &mut self,
        action: ClipboardOperatorAction,
        now_ms: u64,
    ) -> Result<(), ClipboardPermissionError> {
        let result = match action {
            ClipboardOperatorAction::Approve(token) => {
                self.model.approve(token, &self.context, now_ms)
            }
            ClipboardOperatorAction::Deny => self.model.deny(now_ms),
            ClipboardOperatorAction::Cancel => self.model.cancel(now_ms),
            ClipboardOperatorAction::Dismiss => {
                let result = self.model.dismiss_terminal();
                if result.is_ok() {
                    self.active_gate = None;
                }
                result
            }
        };
        self.sync_gate_state();
        self.release_terminal_gate();
        result
    }

    fn bind_runtime_metadata(
        &mut self,
        metadata: &ClipboardGateMetadata,
        now_ms: u64,
    ) -> Result<(), ClipboardPermissionError> {
        let session = (metadata.session_id.clone(), metadata.session_generation);
        if self
            .session_binding
            .as_ref()
            .is_some_and(|bound| bound != &session)
        {
            self.context.session_generation = self.context.session_generation.saturating_add(1);
        }
        self.session_binding = Some(session);

        let lease = (
            metadata.lease_id.clone(),
            metadata.session_generation,
            metadata.lease_expires_at_ms,
        );
        if self
            .lease_binding
            .as_ref()
            .is_some_and(|bound| bound != &lease)
        {
            self.context.lease_generation = self.context.lease_generation.saturating_add(1);
        }
        self.lease_binding = Some(lease);
        self.model.update_context(&self.context, now_ms)
    }

    fn sync_gate_state(&self) {
        let Some(gate) = &self.active_gate else {
            return;
        };
        let desired = match self.model.active_state() {
            Some(ClipboardTransferState::Approved) => Some(CLIPBOARD_GATE_APPROVED),
            Some(
                ClipboardTransferState::Denied
                | ClipboardTransferState::Cancelled
                | ClipboardTransferState::Failed(_),
            ) => Some(CLIPBOARD_GATE_REFUSED),
            _ => None,
        };
        if let Some(desired) = desired {
            let current = gate.state.load(Ordering::Acquire);
            // Approval is one-use and must never rewind a transport that has
            // already begun materialization. Revocation is different: cancel,
            // expiry, focus loss, and session/lease replacement must remain
            // observable by the transport even after materialization starts so
            // multi-step protocol writes can stop before publishing more bytes.
            if desired == CLIPBOARD_GATE_REFUSED || current != CLIPBOARD_GATE_MATERIALIZING {
                gate.state.store(desired, Ordering::Release);
            }
        }
    }

    fn release_terminal_gate(&mut self) {
        if self.model.active_state().is_some_and(terminal_state) {
            self.active_gate = None;
        }
    }
}

fn render_transfer_card(
    ui: &mut egui::Ui,
    summary: &ClipboardTransferSummary,
    state: &ClipboardTransferState,
    now_ms: u64,
) -> Option<ClipboardOperatorAction> {
    ui.label(
        egui::RichText::new("Clipboard permission")
            .size(Style::TITLE)
            .color(Style::TEXT_STRONG),
    );
    ui.label(
        egui::RichText::new(
            "Only metadata is shown. Clipboard content stays with its existing provider.",
        )
        .size(Style::SMALL)
        .color(Style::TEXT_DIM),
    );
    ui.add_space(Style::SP_S);
    metadata_row(ui, "Source", &summary.source);
    metadata_row(ui, "Target", &summary.target.label);
    metadata_row(ui, "MIME", &summary.mime);
    metadata_row(ui, "Size", &format_bytes(summary.byte_count));
    metadata_row(ui, "Expiry", &format_expiry(summary.expires_at_ms, now_ms));
    ui.add_space(Style::SP_S);

    match state {
        ClipboardTransferState::AwaitingApproval { token } => {
            ui.colored_label(Style::WARN, "Explicit approval required");
            let mut action = None;
            ui.horizontal(|ui| {
                if ui.button("Approve transfer").clicked() {
                    action = Some(ClipboardOperatorAction::Approve(*token));
                }
                if ui.button("Deny").clicked() {
                    action = Some(ClipboardOperatorAction::Deny);
                }
            });
            action
        }
        ClipboardTransferState::Approved => {
            ui.colored_label(Style::OK, "Approved — waiting for transport");
            ui.button("Cancel transfer")
                .clicked()
                .then_some(ClipboardOperatorAction::Cancel)
        }
        ClipboardTransferState::InProgress { transferred, total } => {
            let fraction = if *total == 0 {
                1.0
            } else {
                (*transferred as f64 / *total as f64).clamp(0.0, 1.0) as f32
            };
            ui.add(
                egui::ProgressBar::new(fraction)
                    .show_percentage()
                    .text(format!(
                        "{} / {}",
                        format_bytes(*transferred),
                        format_bytes(*total)
                    )),
            );
            ui.button("Cancel transfer")
                .clicked()
                .then_some(ClipboardOperatorAction::Cancel)
        }
        ClipboardTransferState::Denied => terminal_row(ui, "Transfer denied", Style::DANGER),
        ClipboardTransferState::Cancelled => {
            terminal_row(ui, "Transfer cancelled", Style::TEXT_DIM)
        }
        ClipboardTransferState::Failed(failure) => {
            terminal_row(ui, failure_label(*failure), Style::DANGER)
        }
        ClipboardTransferState::Completed => terminal_row(ui, "Transfer complete", Style::OK),
    }
}

fn metadata_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(format!("{label}:"))
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.label(
            egui::RichText::new(value)
                .size(Style::SMALL)
                .color(Style::TEXT_STRONG),
        );
    });
}

fn terminal_row(
    ui: &mut egui::Ui,
    label: &str,
    color: egui::Color32,
) -> Option<ClipboardOperatorAction> {
    ui.colored_label(color, label);
    ui.button("Dismiss")
        .clicked()
        .then_some(ClipboardOperatorAction::Dismiss)
}

fn failure_label(failure: ClipboardFailure) -> &'static str {
    match failure {
        ClipboardFailure::Expired => "Transfer failed — offer expired",
        ClipboardFailure::FocusLost => "Transfer revoked — focus changed",
        ClipboardFailure::SessionChanged => "Transfer revoked — session changed",
        ClipboardFailure::LeaseChanged => "Transfer revoked — lease changed",
        ClipboardFailure::Transport => "Transfer failed — transport error",
        ClipboardFailure::Policy => "Transfer failed — policy refusal",
    }
}

fn terminal_state(state: &ClipboardTransferState) -> bool {
    matches!(
        state,
        ClipboardTransferState::Denied
            | ClipboardTransferState::Cancelled
            | ClipboardTransferState::Failed(_)
            | ClipboardTransferState::Completed
    )
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn format_expiry(expires_at_ms: u64, now_ms: u64) -> String {
    let remaining = expires_at_ms.saturating_sub(now_ms);
    format!(
        "{expires_at_ms} ({:.1}s remaining)",
        remaining as f64 / 1_000.0
    )
}

fn rich_metadata_representation(metadata: &ClipboardGateMetadata) -> bool {
    metadata.has_files_reference
        || metadata
            .mime_offers
            .iter()
            .any(|mime| !plain_text_mime(mime))
        || !plain_text_mime(&metadata.selected_mime)
}

fn plain_text_mime(mime: &str) -> bool {
    mime.split(';')
        .next()
        .is_some_and(|base| base.eq_ignore_ascii_case("text/plain"))
}

fn map_vdi_refusal(
    error: mackes_mesh_types::vdi_clipboard::VdiClipboardTransportV2Error,
) -> ClipboardPermissionError {
    use mackes_mesh_types::vdi_clipboard::VdiClipboardTransportV2Error;
    match error {
        VdiClipboardTransportV2Error::SecretBearing => ClipboardPermissionError::SecretBearing,
        VdiClipboardTransportV2Error::Replay => ClipboardPermissionError::StaleOrReplay,
        VdiClipboardTransportV2Error::ExpiredLease => ClipboardPermissionError::Expired,
        VdiClipboardTransportV2Error::UnsupportedMime
        | VdiClipboardTransportV2Error::UnsupportedPayload => {
            ClipboardPermissionError::UnsupportedMime
        }
        _ => ClipboardPermissionError::InvalidMetadata,
    }
}

fn secret_bearing_mime(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    mime.contains("password") || mime.contains("secret") || mime.contains("credential")
}

fn bounded_audit_source(node: &str, seat: &str) -> String {
    bounded_audit_text(&format!(
        "{} / {}",
        bounded_audit_text(node),
        bounded_audit_text(seat)
    ))
}

fn bounded_audit_text(value: &str) -> String {
    let mut bounded = String::new();
    for character in value.chars().filter(|character| !character.is_control()) {
        if bounded.len() + character.len_utf8() > MAX_TARGET_LABEL_BYTES {
            break;
        }
        bounded.push(character);
    }
    bounded
}

fn push_bounded<T>(queue: &mut VecDeque<T>, value: T, maximum: usize) {
    if queue.len() == maximum {
        queue.pop_front();
    }
    queue.push_back(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::vdi_clipboard::{
        VdiClipboardText, CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION,
        VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
    };

    const NOW: u64 = 1_800_000_000_000;

    fn context() -> ClipboardPermissionContext {
        ClipboardPermissionContext {
            focused: true,
            focus_generation: 1,
            session_generation: 7,
            lease_generation: 11,
        }
    }

    fn target(kind: ClipboardTargetKind) -> ClipboardTarget {
        ClipboardTarget::new(kind, "workstation-b / guest-a").expect("target")
    }

    fn envelope(sequence: u64, mime: &str) -> ClipboardEnvelopeV2 {
        ClipboardEnvelopeV2::new_inline_text(
            "node-a",
            "seat-a",
            "source-session-secret",
            sequence,
            NOW - 100,
            vec![mime.to_string(), "text/plain;charset=utf-8".to_string()],
            "PAYLOAD-PREVIEW-MUST-NOT-BE-AUDITED",
            VdiClipboardText::new("payload bytes must not enter the model").expect("text"),
            NOW + 60_000,
        )
        .expect("envelope")
    }

    #[test]
    fn rich_cross_guest_transfer_requires_one_use_approval_and_tracks_progress() {
        let mut model = ClipboardPermissionModel::default();
        let token = model
            .request(
                &envelope(1, "text/html"),
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Guest),
                context(),
                NOW,
            )
            .expect("request")
            .expect("rich guest approval");
        let summary = model.active_summary().expect("summary");
        assert_eq!(summary.source, "node-a / seat-a / source-session-secret");
        assert_eq!(summary.target.label, "workstation-b / guest-a");
        assert_eq!(summary.mime, "text/html");
        assert_eq!(summary.byte_count, 38);
        assert_eq!(summary.expires_at_ms, NOW + 60_000);
        assert_eq!(
            model.progress(1),
            Err(ClipboardPermissionError::ApprovalRequired)
        );

        model.approve(token, &context(), NOW + 1).expect("approve");
        assert_eq!(
            model.approve(token, &context(), NOW + 2),
            Err(ClipboardPermissionError::ApprovalReplay)
        );
        model.progress(20).expect("partial progress");
        assert_eq!(
            model.progress(19),
            Err(ClipboardPermissionError::InvalidProgress)
        );
        model.progress(38).expect("full progress");
        model.complete(NOW + 3).expect("complete");
        assert_eq!(
            model.active_state(),
            Some(&ClipboardTransferState::Completed)
        );
    }

    fn vdi_message(
        sequence: u64,
        offered_mime: &str,
        selected_mime: &str,
    ) -> (VdiClipboardLeaseV2, VdiClipboardMessageV2) {
        let envelope = envelope(sequence, offered_mime);
        let lease = VdiClipboardLeaseV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: "guest-a".into(),
            generation: 11,
            lease_id: "lease-private".into(),
            issued_at_ms: NOW - 1_000,
            expires_at_ms: NOW + 60_000,
            permitted_mime_offers: envelope.mime_offers.clone(),
        };
        let message = VdiClipboardMessageV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: lease.session_id.clone(),
            generation: lease.generation,
            lease_id: lease.lease_id.clone(),
            lease_expires_at_ms: lease.expires_at_ms,
            message_sequence: sequence,
            selected_mime: selected_mime.into(),
            disclosure: VdiClipboardDisclosureV2::Shareable,
            envelope,
        };
        (lease, message)
    }

    #[test]
    fn host_to_guest_rich_ticket_cannot_materialize_before_one_use_approval() {
        let mut controller = ClipboardPermissionController::default();
        let ingress = controller.ingress();
        let (lease, message) = vdi_message(21, "text/html", "text/plain;charset=utf-8");
        let ticket = ingress
            .submit_vdi(
                &message,
                &lease,
                None,
                target(ClipboardTargetKind::Guest),
                NOW,
            )
            .expect("bounded host-to-guest ingress");
        assert_eq!(
            ticket.try_begin_materialization(),
            ClipboardGateReadiness::Pending
        );
        controller.poll_ingress(NOW);
        let token = match controller.model.active_state() {
            Some(ClipboardTransferState::AwaitingApproval { token }) => *token,
            state => panic!("rich host-to-guest transfer was not gated: {state:?}"),
        };
        controller
            .apply_operator_action(ClipboardOperatorAction::Approve(token), NOW + 1)
            .expect("operator approval");
        assert_eq!(
            ticket.try_begin_materialization(),
            ClipboardGateReadiness::Materialize
        );
        assert_eq!(
            ticket.try_begin_materialization(),
            ClipboardGateReadiness::Pending,
            "one-use ticket must not begin materialization twice"
        );
    }

    #[test]
    fn operator_cancel_after_materialization_revokes_transport_ticket() {
        let mut controller = ClipboardPermissionController::default();
        let ingress = controller.ingress();
        let (lease, message) = vdi_message(24, "text/html", "text/plain;charset=utf-8");
        let ticket = ingress
            .submit_vdi(
                &message,
                &lease,
                None,
                target(ClipboardTargetKind::Guest),
                NOW,
            )
            .expect("bounded host-to-guest ingress");

        controller.poll_ingress(NOW);
        let token = match controller.model.active_state() {
            Some(ClipboardTransferState::AwaitingApproval { token }) => *token,
            state => panic!("rich host-to-guest transfer was not gated: {state:?}"),
        };
        controller
            .apply_operator_action(ClipboardOperatorAction::Approve(token), NOW + 1)
            .expect("operator approval");
        assert_eq!(
            ticket.try_begin_materialization(),
            ClipboardGateReadiness::Materialize
        );

        controller
            .apply_operator_action(ClipboardOperatorAction::Cancel, NOW + 2)
            .expect("operator cancellation");
        assert_eq!(
            ticket.readiness_before_materialization(),
            ClipboardGateReadiness::Refused,
            "transport must observe cancellation after materialization begins"
        );
        assert_eq!(
            ticket.try_begin_materialization(),
            ClipboardGateReadiness::Refused
        );
    }

    #[test]
    fn rich_vdi_transport_drop_releases_gate_without_replaying_on_reconnect() {
        let mut controller = ClipboardPermissionController::default();
        let ingress = controller.ingress();
        let (lease, first_message) = vdi_message(31, "text/html", "text/plain;charset=utf-8");
        let first_ticket = ingress
            .submit_vdi(
                &first_message,
                &lease,
                None,
                target(ClipboardTargetKind::Guest),
                NOW,
            )
            .expect("first rich transport ingress");

        controller.poll_ingress(NOW);
        let token = match controller.model.active_state() {
            Some(ClipboardTransferState::AwaitingApproval { token }) => *token,
            state => panic!("rich transfer was not approval-gated: {state:?}"),
        };
        controller
            .apply_operator_action(ClipboardOperatorAction::Approve(token), NOW + 1)
            .expect("approve first rich transfer");
        assert_eq!(
            first_ticket.try_begin_materialization(),
            ClipboardGateReadiness::Materialize
        );

        // Hostile reconnect boundary: the worker disappears after consuming
        // its one-use approval but before reporting payload completion.
        drop(first_ticket);
        controller.poll_ingress(NOW + 2);
        assert_eq!(
            controller.model.active_state(),
            Some(&ClipboardTransferState::Failed(ClipboardFailure::Transport)),
            "an orphaned materialization must become a visible terminal failure"
        );
        assert!(
            controller.active_gate.is_none(),
            "the dead worker must not retain the bounded transport gate"
        );

        let stale_ticket = ingress
            .submit_vdi(
                &first_message,
                &lease,
                None,
                target(ClipboardTargetKind::Guest),
                NOW + 3,
            )
            .expect("stale reconnect submission reaches controller");
        controller.poll_ingress(NOW + 3);
        assert_eq!(
            stale_ticket.try_begin_materialization(),
            ClipboardGateReadiness::Refused,
            "reconnect must not duplicate the abandoned sequence"
        );

        let (_, newer_message) = vdi_message(32, "text/html", "text/plain;charset=utf-8");
        let newer_ticket = ingress
            .submit_vdi(
                &newer_message,
                &lease,
                None,
                target(ClipboardTargetKind::Guest),
                NOW + 4,
            )
            .expect("newer reconnect submission");
        controller.poll_ingress(NOW + 4);
        assert_eq!(
            newer_ticket.readiness_before_materialization(),
            ClipboardGateReadiness::Pending,
            "a newer rich payload must reach approval instead of remaining Busy"
        );
        assert!(matches!(
            controller.model.active_state(),
            Some(ClipboardTransferState::AwaitingApproval { .. })
        ));
    }

    #[test]
    fn vdi_permission_expires_with_lease_before_offer_and_cleans_up_ticket() {
        let mut controller = ClipboardPermissionController::default();
        let ingress = controller.ingress();
        let (mut lease, mut message) = vdi_message(23, "text/html", "text/plain;charset=utf-8");
        lease.expires_at_ms = NOW + 5;
        message.lease_expires_at_ms = lease.expires_at_ms;
        let ticket = ingress
            .submit_vdi(
                &message,
                &lease,
                None,
                target(ClipboardTargetKind::Guest),
                NOW,
            )
            .expect("offer is valid before its lease expires");

        controller.poll_ingress(NOW);
        let token = match controller.model.active_state() {
            Some(ClipboardTransferState::AwaitingApproval { token }) => *token,
            state => panic!("rich VDI transfer was not gated: {state:?}"),
        };
        assert_eq!(
            controller
                .model
                .active_summary()
                .map(|summary| summary.expires_at_ms),
            Some(lease.expires_at_ms),
            "permission lifetime must be bounded by the shorter lease"
        );

        assert_eq!(
            controller.apply_operator_action(ClipboardOperatorAction::Approve(token), NOW + 5),
            Err(ClipboardPermissionError::Expired)
        );
        assert_eq!(
            ticket.try_begin_materialization(),
            ClipboardGateReadiness::Refused
        );
        assert_eq!(
            controller.model.active_state(),
            Some(&ClipboardTransferState::Failed(ClipboardFailure::Expired))
        );

        let mut renewed_lease = lease.clone();
        renewed_lease.lease_id = "renewed-lease".into();
        renewed_lease.expires_at_ms = NOW + 60_000;
        let mut replay = message.clone();
        replay.lease_id = renewed_lease.lease_id.clone();
        replay.lease_expires_at_ms = renewed_lease.expires_at_ms;
        assert_eq!(
            controller.model.request_vdi(
                &replay,
                &renewed_lease,
                None,
                target(ClipboardTargetKind::Guest),
                context(),
                NOW + 6,
            ),
            Err(ClipboardPermissionError::StaleOrReplay),
            "renewing lease authority must not replay an expired source sequence"
        );
    }

    #[test]
    fn guest_to_host_plain_ticket_still_crosses_controller_before_publication() {
        let mut controller = ClipboardPermissionController::default();
        let ingress = controller.ingress();
        let (lease, message) = vdi_message(22, "text/plain", "text/plain");
        let ticket = ingress
            .submit_vdi(
                &message,
                &lease,
                None,
                target(ClipboardTargetKind::LocalSeat),
                NOW,
            )
            .expect("bounded guest-to-host ingress");
        assert_eq!(
            ticket.try_begin_materialization(),
            ClipboardGateReadiness::Pending,
            "transport must not publish the legacy lane before controller admission"
        );
        controller.poll_ingress(NOW);
        assert_eq!(
            ticket.try_begin_materialization(),
            ClipboardGateReadiness::Materialize
        );
    }

    #[test]
    fn stale_source_sequence_and_old_approval_are_refused() {
        let mut model = ClipboardPermissionModel::default();
        let first = model
            .request(
                &envelope(4, "text/html"),
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Peer),
                context(),
                NOW,
            )
            .expect("first")
            .expect("token");
        model.deny(NOW + 1).expect("deny");
        assert_eq!(
            model.request(
                &envelope(4, "text/html"),
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Peer),
                context(),
                NOW + 2,
            ),
            Err(ClipboardPermissionError::StaleOrReplay)
        );
        let second = model
            .request(
                &envelope(5, "text/html"),
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Peer),
                context(),
                NOW + 3,
            )
            .expect("newer")
            .expect("new token");
        assert_ne!(first, second);
        assert_eq!(
            model.approve(first, &context(), NOW + 4),
            Err(ClipboardPermissionError::ApprovalReplay)
        );
    }

    #[test]
    fn replay_mark_expires_at_its_authority_boundary() {
        let mut model = ClipboardPermissionModel::default();
        let mut first = envelope(40, "text/html");
        first.expires_at_ms = NOW + 10;
        model
            .request(
                &first,
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Peer),
                context(),
                NOW,
            )
            .expect("first authority window")
            .expect("approval token");
        model.deny(NOW + 1).expect("terminal decision records replay");

        let renewed = envelope(40, "text/html");
        assert_eq!(
            model.request(
                &renewed,
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Peer),
                context(),
                NOW + 9,
            ),
            Err(ClipboardPermissionError::StaleOrReplay),
            "same source/session sequence remains hostile before authority expiry"
        );

        assert!(
            model
                .request(
                    &renewed,
                    "text/html",
                    VdiClipboardDisclosureV2::Shareable,
                    target(ClipboardTargetKind::Peer),
                    context(),
                    NOW + 10,
                )
                .expect("expired replay mark must not strand renewed authority")
                .is_some(),
            "renewed rich transfer still requires approval"
        );
        assert_eq!(model.replay_marks.len(), 0);
    }

    #[test]
    fn replay_mark_newer_terminal_extends_retention_without_sequence_rewind() {
        let mut model = ClipboardPermissionModel::default();
        let mut first = envelope(50, "text/html");
        first.expires_at_ms = NOW + 20;
        model
            .request(
                &first,
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Peer),
                context(),
                NOW,
            )
            .expect("first request");
        model
            .fail(ClipboardFailure::Transport, NOW + 1)
            .expect("failed transfer records replay");

        let mut newer = envelope(51, "text/html");
        newer.expires_at_ms = NOW + 40;
        model
            .request(
                &newer,
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Peer),
                context(),
                NOW + 2,
            )
            .expect("newer sequence")
            .expect("approval token");
        model.deny(NOW + 3).expect("newer terminal decision");

        let renewed_old_sequence = envelope(50, "text/html");
        assert_eq!(
            model.request(
                &renewed_old_sequence,
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Peer),
                context(),
                NOW + 20,
            ),
            Err(ClipboardPermissionError::StaleOrReplay),
            "older sequence must remain blocked through the newer authority window"
        );
        assert_eq!(model.replay_marks.len(), 1);
        assert_eq!(model.replay_marks[0].sequence, 51);
        assert_eq!(model.replay_marks[0].expires_at_ms, NOW + 40);
    }

    #[test]
    fn vdi_secret_and_secret_mime_fail_before_approval() {
        let lease = VdiClipboardLeaseV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: "guest-a".into(),
            generation: 11,
            lease_id: "lease-private".into(),
            issued_at_ms: NOW - 1_000,
            expires_at_ms: NOW + 60_000,
            permitted_mime_offers: vec!["text/html".into()],
        };
        let message = VdiClipboardMessageV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: lease.session_id.clone(),
            generation: lease.generation,
            lease_id: lease.lease_id.clone(),
            lease_expires_at_ms: lease.expires_at_ms,
            message_sequence: 1,
            selected_mime: "text/html".into(),
            disclosure: VdiClipboardDisclosureV2::Secret,
            envelope: envelope(1, "text/html"),
        };
        let mut model = ClipboardPermissionModel::default();
        assert_eq!(
            model.request_vdi(
                &message,
                &lease,
                None,
                target(ClipboardTargetKind::Guest),
                context(),
                NOW
            ),
            Err(ClipboardPermissionError::SecretBearing)
        );

        let mut secret_mime = envelope(2, "application/x-secret");
        secret_mime.schema_version = CLIPBOARD_ENVELOPE_V2_SCHEMA_VERSION;
        assert_eq!(
            model.request(
                &secret_mime,
                "application/x-secret",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Peer),
                context(),
                NOW
            ),
            Err(ClipboardPermissionError::SecretBearing)
        );
    }

    #[test]
    fn focus_session_and_lease_generation_changes_revoke_in_flight_work() {
        for (changed, expected) in [
            (
                {
                    let mut value = context();
                    value.focused = false;
                    value
                },
                ClipboardFailure::FocusLost,
            ),
            (
                {
                    let mut value = context();
                    value.session_generation += 1;
                    value
                },
                ClipboardFailure::SessionChanged,
            ),
            (
                {
                    let mut value = context();
                    value.lease_generation += 1;
                    value
                },
                ClipboardFailure::LeaseChanged,
            ),
        ] {
            let mut model = ClipboardPermissionModel::default();
            model
                .request(
                    &envelope(1, "text/plain"),
                    "text/plain",
                    VdiClipboardDisclosureV2::Shareable,
                    target(ClipboardTargetKind::LocalSeat),
                    context(),
                    NOW,
                )
                .expect("local request");
            model.progress(1).expect("started");
            model.update_context(&changed, NOW + 1).expect("revoke");
            assert_eq!(
                model.active_state(),
                Some(&ClipboardTransferState::Failed(expected))
            );
        }
    }

    #[test]
    fn audit_is_fifo_bounded_under_more_than_capacity_terminal_events() {
        let mut model = ClipboardPermissionModel::default();
        for sequence in 1..=(MAX_CLIPBOARD_AUDIT_ROWS as u64 + 37) {
            model
                .request(
                    &envelope(sequence, "text/plain"),
                    "text/plain",
                    VdiClipboardDisclosureV2::Shareable,
                    target(ClipboardTargetKind::LocalSeat),
                    context(),
                    NOW + sequence,
                )
                .expect("request");
            model.cancel(NOW + sequence).expect("cancel");
        }
        let rows = model.audit_rows().collect::<Vec<_>>();
        assert_eq!(rows.len(), MAX_CLIPBOARD_AUDIT_ROWS);
        assert_eq!(
            rows.first().expect("oldest retained").recorded_at_ms,
            NOW + 102
        );
        assert_eq!(
            rows.last().expect("newest retained").recorded_at_ms,
            NOW + MAX_CLIPBOARD_AUDIT_ROWS as u64 + 37
        );
    }

    #[test]
    fn audit_rows_are_structurally_payload_and_credential_redacted() {
        let clip = envelope(1, "text/html");
        let hash = clip.content_hash.clone();
        let mut model = ClipboardPermissionModel::default();
        model
            .request(
                &clip,
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Guest),
                context(),
                NOW,
            )
            .expect("request");
        model.deny(NOW + 1).expect("deny");
        let rendered = format!("{:?}", model.audit_rows().last().expect("audit"));
        for forbidden in [
            "PAYLOAD-PREVIEW-MUST-NOT-BE-AUDITED",
            "payload bytes must not enter the model",
            "source-session-secret",
            "lease-private",
            hash.as_str(),
        ] {
            assert!(!rendered.contains(forbidden), "audit leaked {forbidden}");
        }
        assert!(rendered.contains("node-a / seat-a"));
        assert!(rendered.contains("text/html"));
    }

    fn collect_text(shape: &egui::Shape, output: &mut String) {
        match shape {
            egui::Shape::Text(text) => {
                output.push_str(text.galley.text());
                output.push('|');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, output);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn mounted_controller_renders_payload_free_metadata_and_operator_actions() {
        let clip = envelope(9, "text/html");
        let hash = clip.content_hash.clone();
        let mut controller = ClipboardPermissionController::default();
        controller
            .request(
                &clip,
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Guest),
                NOW,
            )
            .expect("request");

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            ..Default::default()
        };
        let _warm = ctx.run(input(), |ctx| controller.mount(ctx, NOW));
        let output = ctx.run(input(), |ctx| controller.mount(ctx, NOW));
        let mut text = String::new();
        for clipped in output.shapes {
            collect_text(&clipped.shape, &mut text);
        }
        for expected in [
            "Clipboard permission",
            "Source:",
            "node-a / seat-a / source-session-secret",
            "Target:",
            "workstation-b / guest-a",
            "MIME:",
            "text/html",
            "Size:",
            "38 B",
            "Expiry:",
            "Approve transfer",
            "Deny",
        ] {
            assert!(
                text.contains(expected),
                "missing rendered text {expected:?}: {text}"
            );
        }
        for forbidden in [
            "PAYLOAD-PREVIEW-MUST-NOT-BE-AUDITED",
            "payload bytes must not enter the model",
            hash.as_str(),
        ] {
            assert!(!text.contains(forbidden), "render leaked {forbidden}");
        }
    }

    #[test]
    fn controller_actions_expose_approval_progress_cancel_and_typed_failure() {
        let mut controller = ClipboardPermissionController::default();
        let token = controller
            .request(
                &envelope(1, "text/html"),
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Peer),
                NOW,
            )
            .expect("request")
            .expect("approval token");
        controller
            .apply_operator_action(ClipboardOperatorAction::Approve(token), NOW + 1)
            .expect("approve");
        controller.report_progress(20).expect("progress");
        assert!(matches!(
            controller.model.active_state(),
            Some(ClipboardTransferState::InProgress {
                transferred: 20,
                total: 38
            })
        ));
        controller
            .apply_operator_action(ClipboardOperatorAction::Cancel, NOW + 2)
            .expect("cancel");
        assert_eq!(
            controller.model.active_state(),
            Some(&ClipboardTransferState::Cancelled)
        );

        controller
            .request(
                &envelope(2, "text/html"),
                "text/html",
                VdiClipboardDisclosureV2::Shareable,
                target(ClipboardTargetKind::Peer),
                NOW + 3,
            )
            .expect("next request");
        controller
            .report_failure(ClipboardFailure::Transport, NOW + 4)
            .expect("typed failure");
        assert_eq!(
            controller.model.active_state(),
            Some(&ClipboardTransferState::Failed(ClipboardFailure::Transport))
        );
    }

    #[test]
    fn controller_focus_and_generation_updates_revoke_before_render() {
        for revoke in ["focus", "session", "lease"] {
            let mut controller = ClipboardPermissionController::default();
            controller
                .request(
                    &envelope(1, "text/html"),
                    "text/html",
                    VdiClipboardDisclosureV2::Shareable,
                    target(ClipboardTargetKind::Guest),
                    NOW,
                )
                .expect("request");
            match revoke {
                "focus" => controller
                    .set_focused(false, NOW + 1)
                    .expect("focus revoke"),
                "session" => controller
                    .bind_generations(2, 1, NOW + 1)
                    .expect("session revoke"),
                "lease" => controller
                    .bind_generations(1, 2, NOW + 1)
                    .expect("lease revoke"),
                _ => unreachable!(),
            }
            let expected = match revoke {
                "focus" => ClipboardFailure::FocusLost,
                "session" => ClipboardFailure::SessionChanged,
                "lease" => ClipboardFailure::LeaseChanged,
                _ => unreachable!(),
            };
            assert_eq!(
                controller.model.active_state(),
                Some(&ClipboardTransferState::Failed(expected))
            );
        }
    }
}
