//! Bounded SPICE guest-agent clipboard protocol support.
//!
//! The main SPICE channel carries a byte stream of packed `VDAgentMessage`
//! frames. Clipboard ownership is demand-driven: either side advertises UTF-8
//! text with `CLIPBOARD_GRAB`, the peer requests it, and only then are the bytes
//! transferred in `CLIPBOARD`.

use crate::error::{Result, SpiceError};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 1024 * 1024;
const AGENT_HEADER_BYTES: usize = 20;
const MAX_AGENT_BODY_BYTES: usize = MAX_CLIPBOARD_TEXT_BYTES + 8;
const COMMAND_DEPTH: usize = 8;
const EVENT_DEPTH: usize = 8;

const VD_AGENT_PROTOCOL: u32 = 1;
const VD_AGENT_CLIPBOARD: u32 = 4;
const VD_AGENT_ANNOUNCE_CAPABILITIES: u32 = 6;
const VD_AGENT_CLIPBOARD_GRAB: u32 = 7;
const VD_AGENT_CLIPBOARD_REQUEST: u32 = 8;
const VD_AGENT_CLIPBOARD_RELEASE: u32 = 9;
const VD_AGENT_CLIPBOARD_UTF8_TEXT: u32 = 1;
const VD_AGENT_CAP_CLIPBOARD: u32 = 3;
const VD_AGENT_CAP_CLIPBOARD_BY_DEMAND: u32 = 5;
const VD_AGENT_CAP_CLIPBOARD_NO_RELEASE_ON_REGRAB: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardStatus {
    AgentDisconnected,
    CapabilityPending,
    Unsupported,
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardEvent {
    GuestText(String),
    HostTextRequested,
    HostTextSent,
    CapabilityLost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardError {
    TooLarge { bytes: usize, max_bytes: usize },
    AgentDisconnected,
    CapabilityPending,
    CapabilityAbsent,
    QueueFull,
    TransportClosed,
}

impl core::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooLarge { bytes, max_bytes } => {
                write!(
                    f,
                    "SPICE clipboard text is {bytes} bytes; maximum is {max_bytes}"
                )
            }
            Self::AgentDisconnected => f.write_str("SPICE guest agent is disconnected"),
            Self::CapabilityPending => {
                f.write_str("SPICE guest-agent clipboard capability is not negotiated yet")
            }
            Self::CapabilityAbsent => {
                f.write_str("SPICE guest agent did not advertise clipboard-by-demand")
            }
            Self::QueueFull => f.write_str("SPICE clipboard command queue is full"),
            Self::TransportClosed => f.write_str("SPICE main channel is closed"),
        }
    }
}

impl std::error::Error for ClipboardError {}

#[derive(Debug)]
pub(crate) enum ClipboardCommand {
    OfferText(String),
    SendOfferedText,
    CancelOffer,
}

#[derive(Debug)]
struct SharedState {
    status: ClipboardStatus,
    events: VecDeque<ClipboardEvent>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            status: ClipboardStatus::AgentDisconnected,
            events: VecDeque::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct VdAgentClipboard {
    command_tx: mpsc::Sender<ClipboardCommand>,
    shared: Arc<Mutex<SharedState>>,
}

impl VdAgentClipboard {
    pub fn status(&self) -> ClipboardStatus {
        self.shared
            .lock()
            .map_or(ClipboardStatus::AgentDisconnected, |state| state.status)
    }

    pub fn offer_text(&self, text: String) -> core::result::Result<(), ClipboardError> {
        if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
            return Err(ClipboardError::TooLarge {
                bytes: text.len(),
                max_bytes: MAX_CLIPBOARD_TEXT_BYTES,
            });
        }
        match self.status() {
            ClipboardStatus::AgentDisconnected => return Err(ClipboardError::AgentDisconnected),
            ClipboardStatus::CapabilityPending => return Err(ClipboardError::CapabilityPending),
            ClipboardStatus::Unsupported => return Err(ClipboardError::CapabilityAbsent),
            ClipboardStatus::Ready => {}
        }
        self.try_command(ClipboardCommand::OfferText(text))
    }

    pub fn send_offered_text(&self) -> core::result::Result<(), ClipboardError> {
        self.try_command(ClipboardCommand::SendOfferedText)
    }

    pub fn cancel_offer(&self) -> core::result::Result<(), ClipboardError> {
        self.try_command(ClipboardCommand::CancelOffer)
    }

    fn try_command(&self, command: ClipboardCommand) -> core::result::Result<(), ClipboardError> {
        self.command_tx
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => ClipboardError::QueueFull,
                mpsc::error::TrySendError::Closed(_) => ClipboardError::TransportClosed,
            })
    }

    pub fn take_events(&self) -> Vec<ClipboardEvent> {
        let Ok(mut state) = self.shared.lock() else {
            return Vec::new();
        };
        state.events.drain(..).collect()
    }
}

pub(crate) struct VdAgentClipboardChannel {
    shared: Arc<Mutex<SharedState>>,
    command_rx: mpsc::Receiver<ClipboardCommand>,
    decoder: AgentStreamDecoder,
    offered_text: Option<String>,
    host_request_pending: bool,
    agent_connected: bool,
}

impl VdAgentClipboardChannel {
    pub(crate) fn new() -> (VdAgentClipboard, Self) {
        let shared = Arc::new(Mutex::new(SharedState::default()));
        let (command_tx, command_rx) = mpsc::channel(COMMAND_DEPTH);
        (
            VdAgentClipboard {
                command_tx,
                shared: Arc::clone(&shared),
            },
            Self {
                shared,
                command_rx,
                decoder: AgentStreamDecoder::default(),
                offered_text: None,
                host_request_pending: false,
                agent_connected: false,
            },
        )
    }

    pub(crate) fn command_rx(&mut self) -> &mut mpsc::Receiver<ClipboardCommand> {
        &mut self.command_rx
    }

    pub(crate) fn connected(&mut self) -> Vec<u8> {
        self.agent_connected = true;
        self.set_status(ClipboardStatus::CapabilityPending);
        capabilities_frame(true)
    }

    pub(crate) fn disconnected(&mut self) {
        self.agent_connected = false;
        self.offered_text = None;
        self.host_request_pending = false;
        self.decoder.clear();
        self.set_status(ClipboardStatus::AgentDisconnected);
        self.push_event(ClipboardEvent::CapabilityLost);
    }

    pub(crate) fn apply_command(
        &mut self,
        command: ClipboardCommand,
    ) -> core::result::Result<Vec<u8>, ClipboardError> {
        match command {
            ClipboardCommand::OfferText(text) => {
                if self.status() != ClipboardStatus::Ready {
                    return Err(match self.status() {
                        ClipboardStatus::AgentDisconnected => ClipboardError::AgentDisconnected,
                        ClipboardStatus::CapabilityPending => ClipboardError::CapabilityPending,
                        ClipboardStatus::Unsupported => ClipboardError::CapabilityAbsent,
                        ClipboardStatus::Ready => unreachable!(),
                    });
                }
                if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
                    return Err(ClipboardError::TooLarge {
                        bytes: text.len(),
                        max_bytes: MAX_CLIPBOARD_TEXT_BYTES,
                    });
                }
                self.offered_text = Some(text);
                self.host_request_pending = false;
                Ok(agent_frame(
                    VD_AGENT_CLIPBOARD_GRAB,
                    &VD_AGENT_CLIPBOARD_UTF8_TEXT.to_le_bytes(),
                ))
            }
            ClipboardCommand::SendOfferedText => {
                if !self.host_request_pending {
                    return Err(ClipboardError::CapabilityAbsent);
                }
                let text = self
                    .offered_text
                    .take()
                    .ok_or(ClipboardError::CapabilityAbsent)?;
                self.host_request_pending = false;
                let mut body = Vec::with_capacity(4 + text.len());
                body.extend_from_slice(&VD_AGENT_CLIPBOARD_UTF8_TEXT.to_le_bytes());
                body.extend_from_slice(text.as_bytes());
                Ok(agent_frame(VD_AGENT_CLIPBOARD, &body))
            }
            ClipboardCommand::CancelOffer => {
                self.offered_text = None;
                self.host_request_pending = false;
                Ok(agent_frame(VD_AGENT_CLIPBOARD_RELEASE, &[]))
            }
        }
    }

    pub(crate) fn ingest(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>> {
        let frames = self.decoder.push(chunk)?;
        let mut replies = Vec::new();
        for frame in frames {
            self.handle_frame(frame, &mut replies)?;
        }
        Ok(replies)
    }

    fn handle_frame(&mut self, frame: AgentFrame, replies: &mut Vec<Vec<u8>>) -> Result<()> {
        if frame.protocol != VD_AGENT_PROTOCOL {
            return Err(SpiceError::Protocol(format!(
                "unsupported SPICE agent protocol {}",
                frame.protocol
            )));
        }
        match frame.kind {
            VD_AGENT_ANNOUNCE_CAPABILITIES => {
                if frame.body.len() < 4 || frame.body.len() % 4 != 0 {
                    return Err(SpiceError::Protocol(
                        "malformed SPICE agent capability announcement".into(),
                    ));
                }
                let request = read_u32(&frame.body, 0)?;
                let caps = &frame.body[4..];
                let by_demand = has_cap(caps, VD_AGENT_CAP_CLIPBOARD_BY_DEMAND);
                let clipboard = has_cap(caps, VD_AGENT_CAP_CLIPBOARD);
                self.set_status(if self.agent_connected && by_demand && clipboard {
                    ClipboardStatus::Ready
                } else {
                    ClipboardStatus::Unsupported
                });
                if request != 0 {
                    replies.push(capabilities_frame(false));
                }
            }
            VD_AGENT_CLIPBOARD_GRAB => {
                self.require_ready()?;
                if frame.body.len() % 4 != 0 {
                    return Err(SpiceError::Protocol(
                        "malformed SPICE clipboard type offer".into(),
                    ));
                }
                let offers_text = frame.body.chunks_exact(4).any(|value| {
                    u32::from_le_bytes([value[0], value[1], value[2], value[3]])
                        == VD_AGENT_CLIPBOARD_UTF8_TEXT
                });
                if offers_text {
                    replies.push(agent_frame(
                        VD_AGENT_CLIPBOARD_REQUEST,
                        &VD_AGENT_CLIPBOARD_UTF8_TEXT.to_le_bytes(),
                    ));
                }
            }
            VD_AGENT_CLIPBOARD_REQUEST => {
                self.require_ready()?;
                if frame.body.len() != 4
                    || read_u32(&frame.body, 0)? != VD_AGENT_CLIPBOARD_UTF8_TEXT
                {
                    return Err(SpiceError::Protocol(
                        "SPICE guest requested an unsupported clipboard type".into(),
                    ));
                }
                if self.offered_text.is_none() {
                    return Err(SpiceError::Protocol(
                        "SPICE guest requested clipboard text without an active grab".into(),
                    ));
                }
                if !self.host_request_pending {
                    self.host_request_pending = true;
                    self.push_event(ClipboardEvent::HostTextRequested);
                }
            }
            VD_AGENT_CLIPBOARD => {
                self.require_ready()?;
                if frame.body.len() < 4 || read_u32(&frame.body, 0)? != VD_AGENT_CLIPBOARD_UTF8_TEXT
                {
                    return Err(SpiceError::Protocol(
                        "SPICE guest sent an unsupported clipboard type".into(),
                    ));
                }
                let bytes = &frame.body[4..];
                if bytes.len() > MAX_CLIPBOARD_TEXT_BYTES {
                    return Err(SpiceError::Protocol(
                        "SPICE guest clipboard exceeds the text byte limit".into(),
                    ));
                }
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| SpiceError::Protocol("SPICE clipboard is not UTF-8".into()))?;
                self.push_event(ClipboardEvent::GuestText(text.to_owned()));
            }
            VD_AGENT_CLIPBOARD_RELEASE => {
                self.offered_text = None;
                self.host_request_pending = false;
            }
            _ => {}
        }
        Ok(())
    }

    fn require_ready(&self) -> Result<()> {
        if self.status() == ClipboardStatus::Ready {
            Ok(())
        } else {
            Err(SpiceError::Protocol(
                "SPICE clipboard message arrived without negotiated clipboard-by-demand".into(),
            ))
        }
    }

    fn status(&self) -> ClipboardStatus {
        self.shared
            .lock()
            .map_or(ClipboardStatus::AgentDisconnected, |state| state.status)
    }

    fn set_status(&self, status: ClipboardStatus) {
        if let Ok(mut state) = self.shared.lock() {
            if state.status == ClipboardStatus::Ready && status != ClipboardStatus::Ready {
                push_bounded(&mut state.events, ClipboardEvent::CapabilityLost);
            }
            state.status = status;
        }
    }

    fn push_event(&self, event: ClipboardEvent) {
        if let Ok(mut state) = self.shared.lock() {
            push_bounded(&mut state.events, event);
        }
    }

    pub(crate) fn host_text_sent(&self) {
        self.push_event(ClipboardEvent::HostTextSent);
    }
}

fn push_bounded(queue: &mut VecDeque<ClipboardEvent>, event: ClipboardEvent) {
    if queue.len() == EVENT_DEPTH {
        queue.pop_front();
    }
    queue.push_back(event);
}

#[derive(Debug)]
struct AgentFrame {
    protocol: u32,
    kind: u32,
    body: Vec<u8>,
}

#[derive(Default)]
struct AgentStreamDecoder {
    bytes: Vec<u8>,
}

impl AgentStreamDecoder {
    fn clear(&mut self) {
        self.bytes.clear();
    }

    fn push(&mut self, chunk: &[u8]) -> Result<Vec<AgentFrame>> {
        if self.bytes.len().saturating_add(chunk.len()) > AGENT_HEADER_BYTES + MAX_AGENT_BODY_BYTES
        {
            self.clear();
            return Err(SpiceError::Protocol(
                "SPICE agent frame exceeds the bounded reassembly limit".into(),
            ));
        }
        self.bytes.extend_from_slice(chunk);
        let mut frames = Vec::new();
        loop {
            if self.bytes.len() < AGENT_HEADER_BYTES {
                break;
            }
            let size = usize::try_from(read_u32(&self.bytes, 16)?).map_err(|_| {
                SpiceError::Protocol("SPICE agent frame size cannot fit this platform".into())
            })?;
            if size > MAX_AGENT_BODY_BYTES {
                self.clear();
                return Err(SpiceError::Protocol(
                    "SPICE agent body exceeds the bounded clipboard limit".into(),
                ));
            }
            let frame_len = AGENT_HEADER_BYTES + size;
            if self.bytes.len() < frame_len {
                break;
            }
            let frame = self.bytes.drain(..frame_len).collect::<Vec<_>>();
            frames.push(AgentFrame {
                protocol: read_u32(&frame, 0)?,
                kind: read_u32(&frame, 4)?,
                body: frame[AGENT_HEADER_BYTES..].to_vec(),
            });
        }
        Ok(frames)
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| SpiceError::Protocol("truncated SPICE guest-agent integer".into()))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn has_cap(caps: &[u8], index: u32) -> bool {
    let word_offset = usize::try_from(index / 32).unwrap_or(usize::MAX) * 4;
    read_u32(caps, word_offset)
        .map(|word| word & (1 << (index % 32)) != 0)
        .unwrap_or(false)
}

fn capabilities_frame(request: bool) -> Vec<u8> {
    let mut word = 0_u32;
    for capability in [
        VD_AGENT_CAP_CLIPBOARD,
        VD_AGENT_CAP_CLIPBOARD_BY_DEMAND,
        VD_AGENT_CAP_CLIPBOARD_NO_RELEASE_ON_REGRAB,
    ] {
        word |= 1 << capability;
    }
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&u32::from(request).to_le_bytes());
    body.extend_from_slice(&word.to_le_bytes());
    agent_frame(VD_AGENT_ANNOUNCE_CAPABILITIES, &body)
}

fn agent_frame(kind: u32, body: &[u8]) -> Vec<u8> {
    let size = u32::try_from(body.len()).unwrap_or(u32::MAX);
    let mut frame = Vec::with_capacity(AGENT_HEADER_BYTES + body.len());
    frame.extend_from_slice(&VD_AGENT_PROTOCOL.to_le_bytes());
    frame.extend_from_slice(&kind.to_le_bytes());
    frame.extend_from_slice(&0_u64.to_le_bytes());
    frame.extend_from_slice(&size.to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer_capabilities() -> Vec<u8> {
        let mut word = 0_u32;
        word |= 1 << VD_AGENT_CAP_CLIPBOARD;
        word |= 1 << VD_AGENT_CAP_CLIPBOARD_BY_DEMAND;
        let mut body = Vec::new();
        body.extend_from_slice(&0_u32.to_le_bytes());
        body.extend_from_slice(&word.to_le_bytes());
        agent_frame(VD_AGENT_ANNOUNCE_CAPABILITIES, &body)
    }

    fn ready_channel() -> (VdAgentClipboard, VdAgentClipboardChannel) {
        let (handle, mut channel) = VdAgentClipboardChannel::new();
        channel.connected();
        channel.ingest(&peer_capabilities()).expect("capabilities");
        (handle, channel)
    }

    #[test]
    fn fragmented_guest_grab_requests_and_decodes_bounded_utf8() {
        let (handle, mut channel) = ready_channel();
        let grab = agent_frame(
            VD_AGENT_CLIPBOARD_GRAB,
            &VD_AGENT_CLIPBOARD_UTF8_TEXT.to_le_bytes(),
        );
        assert!(channel.ingest(&grab[..7]).expect("fragment one").is_empty());
        let replies = channel.ingest(&grab[7..]).expect("fragment two");
        assert_eq!(replies.len(), 1);

        let mut body = VD_AGENT_CLIPBOARD_UTF8_TEXT.to_le_bytes().to_vec();
        body.extend_from_slice("guest → host".as_bytes());
        channel
            .ingest(&agent_frame(VD_AGENT_CLIPBOARD, &body))
            .expect("clipboard data");
        assert_eq!(
            handle.take_events(),
            vec![ClipboardEvent::GuestText("guest → host".into())]
        );
    }

    #[test]
    fn host_offer_waits_for_request_and_reconnect_clears_capability() {
        let (handle, mut channel) = ready_channel();
        handle
            .offer_text("host → guest".into())
            .expect("queue offer");
        let command = channel.command_rx.try_recv().expect("queued command");
        let grab = channel.apply_command(command).expect("apply offer");
        assert_eq!(read_u32(&grab, 4).expect("kind"), VD_AGENT_CLIPBOARD_GRAB);
        let replies = channel
            .ingest(&agent_frame(
                VD_AGENT_CLIPBOARD_REQUEST,
                &VD_AGENT_CLIPBOARD_UTF8_TEXT.to_le_bytes(),
            ))
            .expect("guest request");
        assert!(replies.is_empty());
        assert_eq!(
            handle.take_events(),
            vec![ClipboardEvent::HostTextRequested]
        );
        let frame = channel
            .apply_command(ClipboardCommand::SendOfferedText)
            .expect("approved data");
        assert_eq!(read_u32(&frame, 4).expect("kind"), VD_AGENT_CLIPBOARD);
        channel.host_text_sent();
        assert_eq!(handle.take_events(), vec![ClipboardEvent::HostTextSent]);

        channel.disconnected();
        assert_eq!(handle.status(), ClipboardStatus::AgentDisconnected);
        assert_eq!(
            handle.offer_text("must fail".into()),
            Err(ClipboardError::AgentDisconnected)
        );
    }
}
