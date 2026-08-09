use crate::channels::{Channel, ChannelConnection};
use crate::error::{Result, SpiceError};
use crate::protocol::*;
use crate::utils::sleep;
use crate::vdagent::{ClipboardCommand, VdAgentClipboard, VdAgentClipboardChannel};
use binrw::BinRead;
use instant::{Duration, Instant};
use tracing::{debug, error, info, warn};

const VD_AGENT_MAX_DATA_SIZE: usize = 2048;

pub struct MainChannel {
    connection: ChannelConnection,
    session_id: Option<u32>,
    agent_connected: bool,
    clipboard_handle: VdAgentClipboard,
    clipboard: VdAgentClipboardChannel,
}

impl MainChannel {
    pub async fn new(host: &str, port: u16) -> Result<Self> {
        let mut connection = ChannelConnection::new(host, port, ChannelType::Main, 0).await?;
        connection.handshake().await?;

        let (clipboard_handle, clipboard) = VdAgentClipboardChannel::new();
        Ok(Self {
            connection,
            session_id: None,
            agent_connected: false,
            clipboard_handle,
            clipboard,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn new_websocket(websocket_url: &str) -> Result<Self> {
        Self::new_websocket_with_auth(websocket_url, None).await
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn new_websocket_with_auth(
        websocket_url: &str,
        auth_token: Option<String>,
    ) -> Result<Self> {
        let mut connection = ChannelConnection::new_websocket_with_auth(
            websocket_url,
            ChannelType::Main,
            0,
            auth_token,
        )
        .await?;
        connection.handshake().await?;

        let (clipboard_handle, clipboard) = VdAgentClipboardChannel::new();
        Ok(Self {
            connection,
            session_id: None,
            agent_connected: false,
            clipboard_handle,
            clipboard,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn new_websocket_with_password(
        websocket_url: &str,
        auth_token: Option<String>,
        password: Option<String>,
    ) -> Result<Self> {
        let mut connection = ChannelConnection::new_websocket_with_auth(
            websocket_url,
            ChannelType::Main,
            0,
            auth_token,
        )
        .await?;
        if let Some(password) = password {
            connection.set_password(password);
        }
        connection.handshake().await?;

        let (clipboard_handle, clipboard) = VdAgentClipboardChannel::new();
        Ok(Self {
            connection,
            session_id: None,
            agent_connected: false,
            clipboard_handle,
            clipboard,
        })
    }

    pub fn get_session_id(&self) -> Option<u32> {
        self.session_id
    }

    pub fn clipboard(&self) -> VdAgentClipboard {
        self.clipboard_handle.clone()
    }

    pub async fn send_attach_channels(&mut self) -> Result<()> {
        // ATTACH_CHANNELS message has no data - it just tells the server
        // to start sending data on all connected channels
        self.connection
            .send_message(crate::protocol::SPICE_MSGC_MAIN_ATTACH_CHANNELS, &[])
            .await?;
        info!("Sent ATTACH_CHANNELS message");
        Ok(())
    }

    /// Start the SPICE guest-agent lane and grant it an effectively unlimited
    /// token window, matching spice-gtk's main-channel handshake.
    async fn start_agent(&mut self) -> Result<()> {
        self.connection
            .send_message(SPICE_MSGC_MAIN_AGENT_START, &u32::MAX.to_le_bytes())
            .await?;
        self.agent_connected = true;
        let capabilities = self.clipboard.connected();
        self.send_agent_payload(&capabilities).await?;
        info!("Started SPICE guest-agent channel");
        Ok(())
    }

    /// Ask the guest to expose one 32-bit monitor at the requested size.
    ///
    /// The payload is the packed `VDAgentMessage` envelope followed by one
    /// packed `VDAgentMonitorsConfig`/`VDAgentMonConfig`, as specified by
    /// `spice-protocol/spice/vd_agent.h`.
    pub async fn send_monitor_config(&mut self, width: u32, height: u32) -> Result<()> {
        if !self.agent_connected {
            return Err(SpiceError::Protocol(
                "SPICE guest agent is not connected; cannot set monitor size".to_string(),
            ));
        }
        if width == 0 || height == 0 {
            return Err(SpiceError::Protocol(
                "SPICE monitor dimensions must be non-zero".to_string(),
            ));
        }
        let payload = monitor_config_message(width, height);
        self.connection
            .send_message(SPICE_MSGC_MAIN_AGENT_DATA, &payload)
            .await?;
        info!("Requested SPICE guest monitor size {}x{}", width, height);
        Ok(())
    }

    pub async fn initialize(&mut self) -> Result<()> {
        // Some SPICE servers send Init first, others expect client to request it
        info!("Trying to receive server init message or proceeding with client-initiated flow");

        // First, try to wait for a server-initiated message (WASM-compatible)
        #[cfg(target_arch = "wasm32")]
        {
            use gloo_timers::future::TimeoutFuture;

            let read_future = self.connection.read_message();
            let timeout_future = TimeoutFuture::new(2000); // Shorter timeout for fallback

            match tokio::select! {
                result = read_future => Some(result),
                _ = timeout_future => {
                    info!("No server init message received, trying client-initiated flow");
                    None
                }
            } {
                Some(Ok((header, data))) => {
                    info!(
                        "Received server message: type={}, size={}",
                        header.msg_type, header.msg_size
                    );
                    if header.msg_type == MainChannelMessage::Init as u16 {
                        info!("Received SpiceMsgMainInit from server");
                        self.handle_message(&header, &data).await?;
                    } else {
                        info!(
                            "Unexpected first message type: {}, handling anyway",
                            header.msg_type
                        );
                        self.handle_message(&header, &data).await?;
                    }
                }
                Some(Err(e)) => {
                    info!(
                        "Error waiting for server init: {}, trying client-initiated flow",
                        e
                    );
                    // Try client-initiated flow
                    self.try_client_initiated_flow().await?;
                }
                None => {
                    // Timeout - try client-initiated flow
                    self.try_client_initiated_flow().await?;
                }
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            match tokio::time::timeout(
                std::time::Duration::from_millis(2000),
                self.connection.read_message(),
            )
            .await
            {
                Ok(Ok((header, data))) => {
                    info!(
                        "Received server message: type={}, size={}",
                        header.msg_type, header.msg_size
                    );
                    if header.msg_type == MainChannelMessage::Init as u16 {
                        info!("Received SpiceMsgMainInit from server");
                        self.handle_message(&header, &data).await?;
                    } else {
                        info!(
                            "Unexpected first message type: {}, handling anyway",
                            header.msg_type
                        );
                        self.handle_message(&header, &data).await?;
                    }
                }
                Ok(Err(e)) => {
                    info!(
                        "Error waiting for server init: {}, trying client-initiated flow",
                        e
                    );
                    self.try_client_initiated_flow().await?;
                }
                Err(_) => {
                    info!("No server init message received, trying client-initiated flow");
                    self.try_client_initiated_flow().await?;
                }
            }
        }

        info!("SPICE main channel ready");
        Ok(())
    }

    async fn try_client_initiated_flow(&mut self) -> Result<()> {
        // According to SPICE protocol, server should send SPICE_MSG_MAIN_INIT
        // after successful link handshake. Let's actively wait for it.
        info!("Waiting for server to send SPICE_MSG_MAIN_INIT");

        // Wait for SPICE_MSG_MAIN_INIT with a longer timeout
        let start_time = Instant::now();
        let timeout_duration = Duration::from_secs(5);

        while start_time.elapsed() < timeout_duration {
            match self.connection.read_message().await {
                Ok((header, data)) => {
                    info!(
                        "Received message while waiting for init: type={}, size={}",
                        header.msg_type, header.msg_size
                    );

                    // Handle the message
                    self.handle_message(&header, &data).await?;

                    // If it was the init message, we're done
                    if header.msg_type == MainChannelMessage::Init as u16 {
                        info!("Successfully received SPICE_MSG_MAIN_INIT");
                        return Ok(());
                    }
                }
                Err(e) => {
                    // Only log if it's not a timeout
                    if !e.to_string().contains("timeout") {
                        debug!("Error while waiting for init: {}", e);
                    }
                    // Small delay before retry
                    sleep(Duration::from_millis(50)).await;
                }
            }
        }

        warn!("Timeout waiting for SPICE_MSG_MAIN_INIT, proceeding anyway");
        Ok(())
    }

    pub async fn get_channels_list(&mut self) -> Result<Vec<(ChannelType, u8)>> {
        info!("Waiting for server to send SPICE_MSG_MAIN_CHANNELS_LIST");

        // Wait for the channels list message from the server
        let start_time = Instant::now();
        let timeout = Duration::from_secs(2);

        while start_time.elapsed() < timeout {
            match self.connection.read_message().await {
                Ok((header, data)) => {
                    if header.msg_type == MainChannelMessage::ChannelsList as u16 {
                        info!("Received SPICE_MSG_MAIN_CHANNELS_LIST");

                        // Parse the channels list
                        let mut channels = Vec::new();
                        if data.len() >= 4 {
                            let num_channels =
                                u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
                            info!("Server reports {} channels", num_channels);

                            // Each channel entry is 2 bytes (type + id)
                            let mut offset = 4;
                            for i in 0..num_channels {
                                if offset + 1 < data.len() {
                                    let channel_type = data[offset];
                                    let channel_id = data[offset + 1];

                                    let ch_type = ChannelType::from(channel_type);
                                    channels.push((ch_type, channel_id));
                                    info!("Channel {}: type={:?}, id={}", i, ch_type, channel_id);
                                    offset += 2;
                                }
                            }
                        }
                        return Ok(channels);
                    } else {
                        // Handle other messages while waiting
                        self.handle_message(&header, &data).await?;
                    }
                }
                Err(e) => {
                    if !e.to_string().contains("timeout") {
                        warn!("Error while waiting for channels list: {}", e);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }

        // Fallback to default channels if server doesn't send list
        warn!("Timeout waiting for SPICE_MSG_MAIN_CHANNELS_LIST, using defaults");
        let channels = vec![
            (ChannelType::Display, 0),
            (ChannelType::Inputs, 0),
            (ChannelType::Cursor, 0),
        ];
        Ok(channels)
    }

    pub async fn run(&mut self) -> Result<()> {
        loop {
            tokio::select! {
                message = self.connection.read_message() => {
                    let (header, data) = message?;
                    self.handle_message(&header, &data).await?;
                }
                command = self.clipboard.command_rx().recv() => {
                    let Some(command) = command else {
                        return Err(SpiceError::ConnectionClosed);
                    };
                    self.send_clipboard_command(command).await?;
                }
            }
        }
    }

    async fn send_clipboard_command(&mut self, command: ClipboardCommand) -> Result<()> {
        let sends_host_text = matches!(command, ClipboardCommand::SendOfferedText);
        let frame = self.clipboard.apply_command(command).map_err(|error| {
            SpiceError::Protocol(format!("SPICE clipboard command refused: {error}"))
        })?;
        self.send_agent_payload(&frame).await?;
        if sends_host_text {
            self.clipboard.host_text_sent();
        }
        Ok(())
    }

    async fn send_agent_payload(&mut self, payload: &[u8]) -> Result<()> {
        for chunk in agent_chunks(payload) {
            self.connection
                .send_message(SPICE_MSGC_MAIN_AGENT_DATA, chunk)
                .await?;
        }
        Ok(())
    }
}

fn agent_chunks(payload: &[u8]) -> std::slice::Chunks<'_, u8> {
    payload.chunks(VD_AGENT_MAX_DATA_SIZE)
}

impl Channel for MainChannel {
    async fn handle_message(&mut self, header: &SpiceDataHeader, data: &[u8]) -> Result<()> {
        debug!(
            "Main channel message: serial={}, type={}, size={}, sub_list={}",
            header.serial, header.msg_type, header.msg_size, header.sub_list
        );
        // Handle common messages first
        if header.msg_type < 100 {
            match header.msg_type {
                x if x == crate::protocol::SPICE_MSG_PING => {
                    debug!(
                        "Received SPICE_MSG_PING with {} bytes, sending PONG",
                        data.len()
                    );
                    // PING messages contain an id (4 bytes) and optionally a timestamp (8 bytes)
                    // The SPICE protocol expects only these fields to be echoed back,
                    // not any additional data that may follow for debugging purposes
                    if data.len() >= 4 {
                        let id = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                        let time = if data.len() >= 12 {
                            u64::from_le_bytes([
                                data[4], data[5], data[6], data[7], data[8], data[9], data[10],
                                data[11],
                            ])
                        } else {
                            0
                        };
                        debug!("PING id: {}, time: {}", id, time);
                        // Send PONG response with only id and timestamp
                        let mut pong_data = Vec::with_capacity(12);
                        pong_data.extend_from_slice(&id.to_le_bytes());
                        pong_data.extend_from_slice(&time.to_le_bytes());
                        self.connection
                            .send_message(crate::protocol::SPICE_MSGC_PONG, &pong_data)
                            .await?;
                        debug!("Sent PONG response (12 bytes)");
                    } else {
                        warn!("PING message too small ({} bytes), ignoring", data.len());
                    }
                    return Ok(());
                }
                x if x == crate::protocol::SPICE_MSG_SET_ACK => {
                    debug!("Received SPICE_MSG_SET_ACK");
                    // TODO: Implement acknowledgment flow control
                    return Ok(());
                }
                x if x == crate::protocol::SPICE_MSG_NOTIFY => {
                    debug!("Received SPICE_MSG_NOTIFY");
                    // TODO: Handle notification
                    return Ok(());
                }
                x if x == crate::protocol::SPICE_MSG_DISCONNECTING => {
                    info!("Server sent SPICE_MSG_DISCONNECTING");
                    return Err(SpiceError::ConnectionClosed);
                }
                _ => {
                    warn!("Unknown common message type: {}", header.msg_type);
                    return Ok(());
                }
            }
        }

        // Handle main channel specific messages
        match header.msg_type {
            x if x == MainChannelMessage::Init as u16 => {
                // First log the raw data
                info!(
                    "Raw SPICE_MSG_MAIN_INIT data ({} bytes): {:?}",
                    data.len(),
                    data
                );

                // Log each u32 field manually
                if data.len() >= 32 {
                    let field1 = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                    let field2 = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
                    let field3 = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
                    let field4 = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
                    info!(
                        "Manual parse - Field 1 (session_id?): {} (0x{:08x})",
                        field1, field1
                    );
                    info!("Manual parse - Field 2: {} (0x{:08x})", field2, field2);
                    info!("Manual parse - Field 3: {} (0x{:08x})", field3, field3);
                    info!("Manual parse - Field 4: {} (0x{:08x})", field4, field4);
                }

                let mut cursor = std::io::Cursor::new(data);
                let init_msg = crate::protocol::SpiceMsgMainInit::read(&mut cursor)
                    .map_err(|e| SpiceError::Protocol(format!("Failed to parse Init: {e}")))?;

                // Debug: Show raw bytes of session_id
                let session_id_bytes = init_msg.session_id.to_le_bytes();
                info!(
                    "Parsed SPICE_MSG_MAIN_INIT: session_id={} (0x{:08x}, bytes={:?})",
                    init_msg.session_id, init_msg.session_id, session_id_bytes
                );
                info!(
                    "  display_hint={}, mouse_modes={:x}",
                    init_msg.display_channels_hint, init_msg.supported_mouse_modes
                );
                info!(
                    "  Server mouse mode: {}, agent_connected: {}",
                    init_msg.current_mouse_mode, init_msg.agent_connected
                );

                // Store the session_id for use by other channels
                self.session_id = Some(init_msg.session_id);

                if init_msg.agent_connected != 0 {
                    self.start_agent().await?;
                }

                // NOTE: The debug server rejects SPICE_MSGC_MAIN_CLIENT_INFO (type 101)
                // with "invalid message type". This might be because:
                // 1. The message type value is wrong
                // 2. The server doesn't expect this message
                // 3. The message format is incorrect
                //
                // Send ATTACH_CHANNELS message to activate the channels
                debug!("Sending ATTACH_CHANNELS message");
                self.send_attach_channels().await?;

                // TODO: Investigate why server rejects these messages
                // Possibly the message type numbers are channel-specific offsets?
            }
            x if x == MainChannelMessage::ChannelsList as u16 => {
                debug!("Received channels list");
                // Already handled in handle_channels_list_message
            }
            x if x == MainChannelMessage::MouseMode as u16 => {
                let mut cursor = std::io::Cursor::new(data);
                let mouse_mode = SpiceMsgMainMouseMode::read(&mut cursor)
                    .map_err(|e| SpiceError::Protocol(format!("Failed to parse MouseMode: {e}")))?;
                info!("Mouse mode changed to: {}", mouse_mode.mode);
                // TODO: Store mouse mode and notify input handling
            }
            x if x == MainChannelMessage::MultiMediaTime as u16 => {
                let mut cursor = std::io::Cursor::new(data);
                let mm_time = SpiceMsgMainMultiMediaTime::read(&mut cursor).map_err(|e| {
                    SpiceError::Protocol(format!("Failed to parse MultiMediaTime: {e}"))
                })?;
                debug!("Multimedia time: {}", mm_time.time);
                // TODO: Synchronize with multimedia time
            }
            x if x == MainChannelMessage::AgentConnected as u16 => {
                let mut cursor = std::io::Cursor::new(data);
                let agent_connected =
                    SpiceMsgMainAgentConnected::read(&mut cursor).map_err(|e| {
                        SpiceError::Protocol(format!("Failed to parse AgentConnected: {e}"))
                    })?;
                info!(
                    "Agent connected with error code: {}",
                    agent_connected.error_code
                );
                if agent_connected.error_code == 0 {
                    self.start_agent().await?;
                }
            }
            x if x == MainChannelMessage::AgentDisconnected as u16 => {
                info!("Agent disconnected");
                self.agent_connected = false;
                self.clipboard.disconnected();
            }
            x if x == MainChannelMessage::AgentData as u16 => {
                for reply in self.clipboard.ingest(data)? {
                    self.send_agent_payload(&reply).await?;
                }
            }
            x if x == MainChannelMessage::AgentToken as u16 => {
                let mut cursor = std::io::Cursor::new(data);
                let agent_tokens = SpiceMsgMainAgentTokens::read(&mut cursor).map_err(|e| {
                    SpiceError::Protocol(format!("Failed to parse AgentTokens: {e}"))
                })?;
                debug!("Agent tokens: {}", agent_tokens.num_tokens);
                // TODO: Update agent token count for flow control
            }
            x if x == SPICE_MSG_NOTIFY => {
                let mut cursor = std::io::Cursor::new(data);
                let notify = SpiceMsgMainNotify::read(&mut cursor)
                    .map_err(|e| SpiceError::Protocol(format!("Failed to parse Notify: {e}")))?;
                let message = String::from_utf8_lossy(&notify.message);
                match notify.severity {
                    0 => info!("Server info: {}", message),
                    1 => warn!("Server warning: {}", message),
                    2 => error!("Server error: {}", message),
                    _ => debug!(
                        "Server notification (severity {}): {}",
                        notify.severity, message
                    ),
                }
            }
            x if x == SPICE_MSG_DISCONNECTING => {
                info!("Server is disconnecting");
                return Err(SpiceError::ConnectionClosed);
            }
            x if x == MainChannelMessage::Name as u16 => {
                debug!("Received VM name");
                if data.len() >= 4 {
                    let name_len =
                        u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
                    if data.len() >= 4 + name_len {
                        let name = String::from_utf8_lossy(&data[4..4 + name_len]);
                        info!("VM name: {}", name);
                    }
                }
            }
            x if x == MainChannelMessage::Uuid as u16 => {
                debug!("Received VM UUID");
                if data.len() >= 16 {
                    let uuid_bytes: [u8; 16] = data[0..16].try_into().unwrap_or_default();
                    info!("VM UUID: {:02x?}", uuid_bytes);
                }
            }
            x if x == MainChannelMessage::MigrateBegin as u16 => {
                warn!("Migration begin not implemented");
            }
            x if x == MainChannelMessage::MigrateCancel as u16 => {
                warn!("Migration cancel not implemented");
            }
            x if x == MainChannelMessage::MigrateSwitchHost as u16 => {
                warn!("Migration switch host not implemented");
            }
            x if x == MainChannelMessage::MigrateEnd as u16 => {
                warn!("Migration end not implemented");
            }
            x if x == MainChannelMessage::AgentConnectedTokens as u16 => {
                warn!("Agent connected tokens not implemented");
            }
            x if x == MainChannelMessage::MigrateBeginSeamless as u16 => {
                warn!("Seamless migration begin not implemented");
            }
            x if x == MainChannelMessage::MigrateDstSeamlessAck as u16 => {
                warn!("Seamless migration dst ack not implemented");
            }
            x if x == MainChannelMessage::MigrateDstSeamlessNack as u16 => {
                warn!("Seamless migration dst nack not implemented");
            }
            x if x == SPICE_MSG_MIGRATE => {
                warn!("Migration message not implemented");
            }
            x if x == SPICE_MSG_MIGRATE_DATA => {
                warn!("Migration data message not implemented");
            }
            x if x == SPICE_MSG_WAIT_FOR_CHANNELS => {
                info!("Received SPICE_MSG_WAIT_FOR_CHANNELS");
                let mut cursor = std::io::Cursor::new(data);
                let wait_msg = crate::protocol::SpiceMsgWaitForChannels::read(&mut cursor)
                    .map_err(|e| {
                        SpiceError::Protocol(format!("Failed to parse WaitForChannels: {e}"))
                    })?;

                info!(
                    "Server requesting synchronization for {} channels",
                    wait_msg.wait_count
                );

                for (idx, channel_id) in wait_msg.wait_list.iter().enumerate() {
                    info!(
                        "  Channel {}: type={}, id={}",
                        idx, channel_id.type_, channel_id.id
                    );
                }

                // TODO: Implement full synchronization mechanism
                // For now, we acknowledge that we received the message
                // In a complete implementation, we would:
                // 1. Track readiness of each channel in the wait list
                // 2. Block operations until all channels are ready
                // 3. Send an acknowledgment when complete
                warn!("Full wait-for-channels synchronization not yet implemented");
                warn!("Channels will continue without explicit synchronization");
            }
            _ => {
                warn!("Unknown message type: {}", header.msg_type);
            }
        }

        Ok(())
    }

    fn channel_type(&self) -> ChannelType {
        ChannelType::Main
    }
}

/// Build one complete `VDAgentMessage(VD_AGENT_MONITORS_CONFIG)` frame.
fn monitor_config_message(width: u32, height: u32) -> Vec<u8> {
    // VDAgentMonitorsConfig (8) + one VDAgentMonConfig (20).
    const BODY_SIZE: u32 = 28;
    let mut data = Vec::with_capacity(20 + BODY_SIZE as usize);
    data.extend_from_slice(&VD_AGENT_PROTOCOL.to_le_bytes());
    data.extend_from_slice(&VD_AGENT_MONITORS_CONFIG.to_le_bytes());
    data.extend_from_slice(&0_u64.to_le_bytes()); // opaque
    data.extend_from_slice(&BODY_SIZE.to_le_bytes());
    data.extend_from_slice(&1_u32.to_le_bytes()); // num_of_monitors
    data.extend_from_slice(&VD_AGENT_CONFIG_MONITORS_FLAG_USE_POS.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    data.extend_from_slice(&width.to_le_bytes());
    data.extend_from_slice(&32_u32.to_le_bytes()); // depth
    data.extend_from_slice(&0_i32.to_le_bytes()); // x
    data.extend_from_slice(&0_i32.to_le_bytes()); // y
    data
}

#[cfg(test)]
mod monitor_config_tests {
    use super::{agent_chunks, monitor_config_message, VD_AGENT_MAX_DATA_SIZE};

    #[test]
    fn monitor_config_is_packed_spice_agent_frame() {
        let data = monitor_config_message(1920, 1080);
        assert_eq!(data.len(), 48);
        assert_eq!(u32::from_le_bytes(data[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(data[4..8].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(data[16..20].try_into().unwrap()), 28);
        assert_eq!(u32::from_le_bytes(data[20..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(data[28..32].try_into().unwrap()), 1080);
        assert_eq!(u32::from_le_bytes(data[32..36].try_into().unwrap()), 1920);
        assert_eq!(u32::from_le_bytes(data[36..40].try_into().unwrap()), 32);
    }

    #[test]
    fn agent_payloads_are_split_at_the_protocol_data_limit() {
        let payload = vec![0_u8; VD_AGENT_MAX_DATA_SIZE * 2 + 1];
        let chunks = agent_chunks(&payload).map(<[u8]>::len).collect::<Vec<_>>();
        assert_eq!(
            chunks,
            vec![VD_AGENT_MAX_DATA_SIZE, VD_AGENT_MAX_DATA_SIZE, 1]
        );
    }
}
