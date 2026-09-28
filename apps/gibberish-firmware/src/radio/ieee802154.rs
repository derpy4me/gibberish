//! IEEE 802.15.4 Frame Driver for ESP32-C5 (R1, R2, R14, R15, R22, KTD4).

use esp_hal::peripherals::IEEE802154;
use esp_radio::ieee802154::{Config as RadioConfig, Ieee802154};
use gibberish_protocol::{
    assemble_variable_phy_frame, calculate_lqi_relay_jitter, is_valid_network_tag,
    parse_variable_phy_frame, CompactDeltaPayload, MeshHeader, MeshPacket, StaticMetadataBeacon,
    CIPHERTEXT_LEN, FLAG_TELEMETRY, FLAG_TELEMETRY_STATIC, MESH_HEADER_LEN, MHR_LEN, PHY_MTU,
};

pub use gibberish_protocol::MIN_RELAY_LQI;

pub const CHANNEL_15_FREQ_MHZ: u16 = 2425;
pub const PAN_ID_BROADCAST: u16 = 0xFFFF;
pub const ADDR_BROADCAST: u16 = 0xFFFF;

pub trait RadioDriver {
    fn transmit_frame(&mut self, frame: &[u8]) -> Result<(), ()>;
    fn receive_frame<'a>(&mut self, buf: &'a mut [u8; PHY_MTU]) -> Option<usize>;
}

/// Discriminator for typed mesh airwave payloads
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Mesh(MeshPacket),
    CompactDelta(CompactDeltaPayload),
    StaticMetadata(StaticMetadataBeacon),
}

/// A received physical frame alongside decoded payload and hardware RF link metrics
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceivedMeshFrame {
    pub kind: FrameKind,
    pub header: MeshHeader,
    pub src_node_id: u32,
    pub rssi: i8,
    pub lqi: u8,
}

/// Legacy received packet alongside hardware-measured RF link metrics
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceivedMeshPacket {
    pub packet: MeshPacket,
    /// 32-bit hardware node ID extracted from IEEE 802.15.4 Source Short Address
    pub src_node_id: u32,
    /// Hardware Received Signal Strength Indicator in dBm (-90 to -30 dBm)
    pub rssi: i8,
    /// Link Quality Indicator (0 to 255)
    pub lqi: u8,
}

/// Converts RSSI in dBm to an 8-bit Link Quality Indicator (LQI: 0-255).
/// Matches the official esp-radio formula from `esp-radio/src/ieee802154/mod.rs:382-390`.
#[inline]
pub fn rssi_to_lqi(rssi: i8) -> u8 {
    if rssi < -80 {
        0
    } else if rssi > -30 {
        255
    } else {
        let lqi_convert = ((rssi as u32).wrapping_add(80)) * 255;
        (lqi_convert / 50) as u8
    }
}

/// Hardware IEEE 802.15.4 Radio Transceiver Manager for ESP32-C5 on Channel 15
pub struct RadioManager<'a> {
    radio: Ieee802154<'a>,
    local_mac: [u8; 8],
    pub local_node_id: u32,
    frame_seq: u8,
}

impl<'a> RadioManager<'a> {
    pub fn new(radio_peripheral: IEEE802154<'a>, local_mac: [u8; 8], local_node_id: u32) -> Self {
        let mut radio = Ieee802154::new(radio_peripheral);
        let mut cfg = RadioConfig::default();
        cfg.channel = 15; // 2.425 GHz
        cfg.promiscuous = true;
        cfg.rx_when_idle = true;
        cfg.auto_ack_rx = false;
        cfg.auto_ack_tx = false;
        cfg.txpower = 20; // 20 dBm (100 mW)
        radio.set_config(cfg);
        radio.start_receive();

        Self {
            radio,
            local_mac,
            local_node_id,
            frame_seq: 0,
        }
    }

    /// Broadcasts a MeshPacket with hardware Clear Channel Assessment (CCA).
    pub fn transmit(&mut self, packet: &MeshPacket) -> bool {
        self.transmit_variable(&packet.header, &packet.payload)
    }

    /// Broadcasts a variable-length physical IEEE 802.15.4 frame with hardware CCA.
    /// Cuts on-air transmission time for compact telemetry down to 55B / 59B PHY duration.
    pub fn transmit_variable(&mut self, header: &MeshHeader, payload: &[u8]) -> bool {
        let mut phy_frame = [0u8; PHY_MTU];
        self.frame_seq = self.frame_seq.wrapping_add(1);
        let mut mhr = [0u8; MHR_LEN];
        mhr[0] = 0x41;
        mhr[1] = 0x08;
        mhr[2] = self.frame_seq;
        mhr[3] = 0xFF;
        mhr[4] = 0xFF;
        mhr[5] = 0xFF;
        mhr[6] = 0xFF;
        mhr[7] = self.local_mac[4];
        mhr[8] = self.local_mac[5];
        mhr[9] = self.local_mac[6];
        mhr[10] = self.local_mac[7];

        if let Ok(total_len) = assemble_variable_phy_frame(&mhr, header, payload, &mut phy_frame) {
            self.radio.transmit_raw(&phy_frame[..total_len], true).is_ok()
        } else {
            false
        }
    }

    /// Polls for incoming variable-length physical frames (55B to 127B).
    pub fn poll_rx_frame(&mut self) -> Option<ReceivedMeshFrame> {
        while let Some(raw) = self.radio.raw_received() {
            let len = raw.data[0] as usize;
            if len >= MHR_LEN + MESH_HEADER_LEN + 2 && len < raw.data.len() {
                if let Ok((header, payload)) = parse_variable_phy_frame(&raw.data[1..1 + len]) {
                    // Loopback check: ignore packets transmitted by this node
                    let src_matches_local = raw.data[8..12] == self.local_mac[4..8];
                    if src_matches_local {
                        continue;
                    }

                    // Extract hardware RSSI (appended at offset raw.data[0] - 1)
                    let rssi_idx = len.saturating_sub(1);
                    let rssi = raw.data[rssi_idx] as i8;
                    let lqi = rssi_to_lqi(rssi);

                    let src_node_id = u32::from_be_bytes([
                        raw.data[8],
                        raw.data[9],
                        raw.data[10],
                        raw.data[11],
                    ]);

                    let kind = if (header.flags & FLAG_TELEMETRY_STATIC) != 0 {
                        if let Ok(beacon) = StaticMetadataBeacon::deserialize(payload) {
                            FrameKind::StaticMetadata(beacon)
                        } else {
                            continue;
                        }
                    } else if (header.flags & FLAG_TELEMETRY) != 0 {
                        if let Ok(delta) = CompactDeltaPayload::deserialize(payload) {
                            FrameKind::CompactDelta(delta)
                        } else {
                            continue;
                        }
                    } else if payload.len() == CIPHERTEXT_LEN {
                        let mut p = [0u8; CIPHERTEXT_LEN];
                        p.copy_from_slice(payload);
                        FrameKind::Mesh(MeshPacket { header, payload: p })
                    } else {
                        continue;
                    };

                    return Some(ReceivedMeshFrame {
                        kind,
                        header,
                        src_node_id,
                        rssi,
                        lqi,
                    });
                }
            }
        }
        None
    }

    /// Legacy poll returning Option<ReceivedMeshPacket> for MeshPacket payloads.
    pub fn poll_rx(&mut self) -> Option<ReceivedMeshPacket> {
        while let Some(frame) = self.poll_rx_frame() {
            match frame.kind {
                FrameKind::Mesh(packet) => {
                    return Some(ReceivedMeshPacket {
                        packet,
                        src_node_id: frame.src_node_id,
                        rssi: frame.rssi,
                        lqi: frame.lqi,
                    });
                }
                _ => continue,
            }
        }
        None
    }
}

pub const HIGH_QUEUE_CAPACITY: usize = 4;
pub const LOW_QUEUE_CAPACITY: usize = 2;

/// A bounded variable-length frame for autonomous telemetry transmission (24B or 28B payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VariableFrame {
    pub header: MeshHeader,
    pub payload_len: u8,
    pub payload: [u8; 32],
}

impl VariableFrame {
    pub fn new(header: MeshHeader, payload: &[u8]) -> Self {
        let mut buf = [0u8; 32];
        let len = payload.len().min(32);
        buf[..len].copy_from_slice(&payload[..len]);
        Self {
            header,
            payload_len: len as u8,
            payload: buf,
        }
    }

    pub fn payload_slice(&self) -> &[u8] {
        &self.payload[..self.payload_len as usize]
    }
}

/// Pop result from the two-tier priority queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingTx {
    High(MeshPacket),
    Low(VariableFrame),
}

/// Two-tier CSMA/CA Backoff controller with immediate chat preemption over telemetry (R13, R14).
pub struct BackoffController {
    high_queue: [Option<MeshPacket>; HIGH_QUEUE_CAPACITY],
    high_head: usize,
    high_tail: usize,
    high_count: usize,

    low_queue: [Option<VariableFrame>; LOW_QUEUE_CAPACITY],
    low_head: usize,
    low_tail: usize,
    low_count: usize,

    backoff_remaining_ms: u16,
    is_low_active: bool,
}

impl BackoffController {
    pub const fn new() -> Self {
        Self {
            high_queue: [None, None, None, None],
            high_head: 0,
            high_tail: 0,
            high_count: 0,

            low_queue: [None, None],
            low_head: 0,
            low_tail: 0,
            low_count: 0,

            backoff_remaining_ms: 0,
            is_low_active: false,
        }
    }

    #[inline]
    pub fn calculate_lqi_relay_jitter(lqi: u8, random_val: u16) -> u16 {
        calculate_lqi_relay_jitter(lqi, random_val)
    }

    /// Schedule a high-priority packet (chat, clipboard, SACK) with minimal contention backoff (5-15ms).
    /// If an active low-priority telemetry backoff is running, immediately aborts it and re-arms
    /// for high-priority transmission. Low-priority frame is preserved at head of low queue.
    /// Returns Err(()) if the high queue is full (applying backpressure to prevent silent drop).
    pub fn schedule_high(&mut self, packet: MeshPacket, jitter_ms: u16) -> Result<(), ()> {
        if self.high_count >= HIGH_QUEUE_CAPACITY {
            return Err(());
        }

        // Abort active low-priority backoff immediately; low frame remains at low_head
        if self.is_low_active {
            self.is_low_active = false;
        }

        self.high_queue[self.high_tail] = Some(packet);
        self.high_tail = (self.high_tail + 1) % HIGH_QUEUE_CAPACITY;
        self.high_count += 1;

        if self.high_count == 1 {
            self.backoff_remaining_ms = jitter_ms.clamp(5, 15);
        }
        Ok(())
    }

    /// Schedule a low-priority autonomous telemetry frame (StaticMetadataBeacon, CompactDeltaPayload).
    /// If the low queue is full, newest state overwrites oldest pending.
    /// Telemetry yields immediately if high-priority user traffic is pending.
    pub fn schedule_low(&mut self, header: MeshHeader, payload: &[u8], jitter_ms: u16) {
        let frame = VariableFrame::new(header, payload);

        if self.low_count < LOW_QUEUE_CAPACITY {
            self.low_queue[self.low_tail] = Some(frame);
            self.low_tail = (self.low_tail + 1) % LOW_QUEUE_CAPACITY;
            self.low_count += 1;
        } else {
            // Overwrite oldest pending state
            self.low_queue[self.low_head] = None;
            self.low_head = (self.low_head + 1) % LOW_QUEUE_CAPACITY;
            self.low_queue[self.low_tail] = Some(frame);
            self.low_tail = (self.low_tail + 1) % LOW_QUEUE_CAPACITY;
        }

        // If no high-priority traffic is pending and not already backing off, arm low backoff
        if self.high_count == 0 && !self.is_low_active {
            self.is_low_active = true;
            self.backoff_remaining_ms = jitter_ms.clamp(15, 60);
        }
    }

    /// Legacy backward compatibility wrapper for chat packet scheduling
    pub fn schedule_tx(&mut self, packet: MeshPacket, jitter_ms: u16) {
        let _ = self.schedule_high(packet, jitter_ms);
    }

    pub fn has_pending(&self) -> bool {
        self.high_count > 0 || self.low_count > 0
    }

    pub fn has_high_pending(&self) -> bool {
        self.high_count > 0
    }

    pub fn is_pending_telemetry(&self) -> bool {
        self.high_count == 0 && self.low_count > 0
    }

    /// Decrement backoff timer. Returns true when backoff expires and packet is ready to send.
    pub fn tick(&mut self, elapsed_ms: u16) -> bool {
        if self.high_count == 0 && self.low_count == 0 {
            self.backoff_remaining_ms = 0;
            self.is_low_active = false;
            return false;
        }

        // If high queue emptied and low frames are waiting, arm low backoff
        if self.high_count == 0 && self.low_count > 0 && !self.is_low_active {
            self.is_low_active = true;
            self.backoff_remaining_ms = 15;
        }

        if self.backoff_remaining_ms == 0 {
            true
        } else if elapsed_ms >= self.backoff_remaining_ms {
            self.backoff_remaining_ms = 0;
            true
        } else {
            self.backoff_remaining_ms -= elapsed_ms;
            false
        }
    }

    /// If an overheard packet matches the pending packet's msg_id and chunk_idx, cancel our TX (R22).
    pub fn on_overhear(&mut self, msg_id: u32, chunk_idx: u8) -> bool {
        for i in 0..self.high_count {
            let idx = (self.high_head + i) % HIGH_QUEUE_CAPACITY;
            if let Some(ref p) = self.high_queue[idx] {
                if p.header.msg_id == msg_id && p.header.chunk_idx == chunk_idx {
                    self.high_queue[idx] = None;
                    return true;
                }
            }
        }
        for i in 0..self.low_count {
            let idx = (self.low_head + i) % LOW_QUEUE_CAPACITY;
            if let Some(ref p) = self.low_queue[idx] {
                if p.header.msg_id == msg_id && p.header.chunk_idx == chunk_idx {
                    self.low_queue[idx] = None;
                    return true;
                }
            }
        }
        false
    }

    /// Pops the next ready transmission. High priority frames always take precedence.
    pub fn take_pending(&mut self) -> Option<PendingTx> {
        if self.high_count > 0 {
            self.is_low_active = false;
            let pkt = self.high_queue[self.high_head].take();
            self.high_head = (self.high_head + 1) % HIGH_QUEUE_CAPACITY;
            self.high_count -= 1;
            if self.high_count > 0 {
                self.backoff_remaining_ms = 10;
            } else if self.low_count > 0 {
                self.is_low_active = true;
                self.backoff_remaining_ms = 20;
            }
            return pkt.map(PendingTx::High);
        }

        if self.low_count > 0 {
            self.is_low_active = false;
            let frame = self.low_queue[self.low_head].take();
            self.low_head = (self.low_head + 1) % LOW_QUEUE_CAPACITY;
            self.low_count -= 1;
            if self.low_count > 0 {
                self.is_low_active = true;
                self.backoff_remaining_ms = 25;
            }
            return frame.map(PendingTx::Low);
        }

        None
    }

    /// Legacy compatibility pop returning Option<MeshPacket>
    pub fn take_pending_mesh(&mut self) -> Option<MeshPacket> {
        match self.take_pending() {
            Some(PendingTx::High(pkt)) => Some(pkt),
            Some(PendingTx::Low(frame)) => {
                let mut payload = [0u8; CIPHERTEXT_LEN];
                let len = (frame.payload_len as usize).min(CIPHERTEXT_LEN);
                payload[..len].copy_from_slice(&frame.payload[..len]);
                Some(MeshPacket {
                    header: frame.header,
                    payload,
                })
            }
            None => None,
        }
    }
}

/// Assembles a complete 127-byte IEEE 802.15.4 frame ready for PHY transmission
pub fn assemble_phy_frame(
    local_mac: [u8; 8],
    packet: &MeshPacket,
    frame_seq: u8,
    out: &mut [u8; PHY_MTU],
) {
    // 1. MAC Header (11 bytes):
    // Frame Control (2B): Data Frame (0x01), Intra-PAN (0x0800) -> 0x0841
    out[0] = 0x41;
    out[1] = 0x08;
    out[2] = frame_seq;
    // Destination PAN ID (2B): 0xFFFF (Broadcast)
    out[3] = 0xFF;
    out[4] = 0xFF;
    // Destination Short Address (2B): 0xFFFF (Broadcast)
    out[5] = 0xFF;
    out[6] = 0xFF;
    // Source Short Address (4B truncated from local_mac or 2B)
    out[7] = local_mac[4];
    out[8] = local_mac[5];
    out[9] = local_mac[6];
    out[10] = local_mac[7];

    // 2. Mesh Header (18 bytes)
    let mut mesh_hdr_bytes = [0u8; MESH_HEADER_LEN];
    packet.header.serialize(&mut mesh_hdr_bytes);
    out[MHR_LEN..MHR_LEN + MESH_HEADER_LEN].copy_from_slice(&mesh_hdr_bytes);

    // 3. Ciphertext Payload (96 bytes)
    out[MHR_LEN + MESH_HEADER_LEN..MHR_LEN + MESH_HEADER_LEN + CIPHERTEXT_LEN]
        .copy_from_slice(&packet.payload);

    // 4. FCS CRC-16 (2 bytes) - zeroed dummy bytes; calculated/appended by radio hardware
    out[125] = 0;
    out[126] = 0;
}

/// Dissect raw incoming 802.15.4 frame, verifying admission tag (R15)
pub fn parse_phy_frame(frame: &[u8], expected_tag: u64) -> Option<MeshPacket> {
    parse_phy_frame_with_validator(frame, |tag| tag == expected_tag)
}

/// Dissect raw incoming 802.15.4 frame, verifying admission tag against known swarm tags
pub fn parse_phy_frame_swarm(frame: &[u8]) -> Option<MeshPacket> {
    parse_phy_frame_with_validator(frame, is_valid_network_tag)
}

/// Dissect raw incoming 802.15.4 frame with custom tag validator
pub fn parse_phy_frame_with_validator<F>(frame: &[u8], tag_validator: F) -> Option<MeshPacket>
where
    F: Fn(u64) -> bool,
{
    if frame.len() < MHR_LEN + MESH_HEADER_LEN + CIPHERTEXT_LEN {
        return None;
    }

    let mut mesh_hdr_buf = [0u8; MESH_HEADER_LEN];
    mesh_hdr_buf.copy_from_slice(&frame[MHR_LEN..MHR_LEN + MESH_HEADER_LEN]);
    let header = MeshHeader::deserialize(&mesh_hdr_buf);

    // Admission Tag check: drop if tag does not match expected swarm network tag
    if !tag_validator(header.network_tag) {
        return None;
    }

    let mut payload = [0u8; CIPHERTEXT_LEN];
    payload.copy_from_slice(&frame[MHR_LEN + MESH_HEADER_LEN..MHR_LEN + MESH_HEADER_LEN + CIPHERTEXT_LEN]);

    Some(MeshPacket { header, payload })
}
