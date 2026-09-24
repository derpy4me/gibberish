//! IEEE 802.15.4 Frame Driver for ESP32-C5 (R1, R2, R14, R15, R22, KTD4).

use esp_hal::peripherals::IEEE802154;
use esp_radio::ieee802154::{Config as RadioConfig, Ieee802154};
use gibberish_protocol::{
    calculate_lqi_relay_jitter, is_valid_network_tag, MeshHeader, MeshPacket, CIPHERTEXT_LEN,
    MESH_HEADER_LEN, MHR_LEN, PHY_MTU,
};

pub use gibberish_protocol::MIN_RELAY_LQI;

pub const CHANNEL_15_FREQ_MHZ: u16 = 2425;
pub const PAN_ID_BROADCAST: u16 = 0xFFFF;
pub const ADDR_BROADCAST: u16 = 0xFFFF;

pub trait RadioDriver {
    fn transmit_frame(&mut self, frame: &[u8]) -> Result<(), ()>;
    fn receive_frame<'a>(&mut self, buf: &'a mut [u8; PHY_MTU]) -> Option<usize>;
}

/// A received packet alongside hardware-measured RF link metrics.
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
    /// Pads the 127-byte transmit buffer with 2 trailing dummy FCS bytes
    /// so the ESP32-C5 radio hardware PHY overwrites bytes 125-126 with CRC-16
    /// rather than payload bytes (IEEE 802.15.4 Hardware FCS Overwrite solution).
    pub fn transmit(&mut self, packet: &MeshPacket) -> bool {
        let mut phy_frame = [0u8; PHY_MTU];
        self.frame_seq = self.frame_seq.wrapping_add(1);
        assemble_phy_frame(self.local_mac, packet, self.frame_seq, &mut phy_frame);
        self.radio.transmit_raw(&phy_frame, true).is_ok()
    }

    /// Polls for valid incoming 802.15.4 frames, extracting payload and link metrics.
    /// Filters out loopbacks from this node and frames failing network tag validation.
    pub fn poll_rx(&mut self) -> Option<ReceivedMeshPacket> {
        while let Some(raw) = self.radio.raw_received() {
            let len = raw.data[0] as usize;
            // Defensive bounds checks:
            // Must contain at least MHR (11) + MeshHeader (18) + Ciphertext (96) = 125 bytes.
            if len >= MHR_LEN + MESH_HEADER_LEN + CIPHERTEXT_LEN && len < raw.data.len() {
                // PSDU starts at raw.data[1]
                if let Some(packet) = parse_phy_frame_swarm(&raw.data[1..1 + len]) {
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

                    return Some(ReceivedMeshPacket {
                        packet,
                        src_node_id,
                        rssi,
                        lqi,
                    });
                }
            }
        }
        None
    }
}

pub const TX_QUEUE_CAPACITY: usize = 8;

/// CSMA/CA Backoff controller with multi-packet queue and overhearing cancellation (R22)
pub struct BackoffController {
    queue: [Option<MeshPacket>; TX_QUEUE_CAPACITY],
    head: usize,
    tail: usize,
    count: usize,
    backoff_remaining_ms: u16,
}

impl BackoffController {
    pub const fn new() -> Self {
        Self {
            queue: [None, None, None, None, None, None, None, None],
            head: 0,
            tail: 0,
            count: 0,
            backoff_remaining_ms: 0,
        }
    }

    /// Calculate LQI/RSSI-weighted contention backoff delay in milliseconds (Issue #2).
    /// Stronger links (high LQI) relay first with minimal backoff (15-30ms),
    /// while weaker links (low LQI) wait longer (45-65ms), allowing stronger relays
    /// to take precedence and trigger overhearing cancellation of redundant transmissions.
    #[inline]
    pub fn calculate_lqi_relay_jitter(lqi: u8, random_val: u16) -> u16 {
        calculate_lqi_relay_jitter(lqi, random_val)
    }

    /// Schedule a packet for transmission with contention backoff (15–90ms)
    pub fn schedule_tx(&mut self, packet: MeshPacket, jitter_ms: u16) {
        let clamped_jitter = jitter_ms.clamp(15, 90);
        if self.count < TX_QUEUE_CAPACITY {
            self.queue[self.tail] = Some(packet);
            self.tail = (self.tail + 1) % TX_QUEUE_CAPACITY;
            self.count += 1;
            if self.count == 1 {
                self.backoff_remaining_ms = clamped_jitter;
            }
        } else {
            // Queue full: replace oldest
            self.queue[self.head] = None;
            self.head = (self.head + 1) % TX_QUEUE_CAPACITY;
            self.queue[self.tail] = Some(packet);
            self.tail = (self.tail + 1) % TX_QUEUE_CAPACITY;
        }
    }

    pub fn has_pending(&self) -> bool {
        self.count > 0
    }

    pub fn is_pending_telemetry(&self) -> bool {
        if self.count > 0 {
            if let Some(ref p) = self.queue[self.head] {
                return (p.header.flags & gibberish_protocol::FLAG_TELEMETRY) != 0;
            }
        }
        false
    }

    /// Schedule telemetry packet only if no high-priority user traffic is pending (R11).
    pub fn schedule_telemetry(&mut self, packet: MeshPacket, jitter_ms: u16) -> bool {
        for i in 0..self.count {
            let idx = (self.head + i) % TX_QUEUE_CAPACITY;
            if let Some(ref p) = self.queue[idx] {
                if (p.header.flags & gibberish_protocol::FLAG_TELEMETRY) == 0 {
                    return false; // User traffic pending
                }
            }
        }
        self.schedule_tx(packet, jitter_ms);
        true
    }

    /// Decrement backoff timer. Returns true when backoff expires and packet is ready to send.
    pub fn tick(&mut self, elapsed_ms: u16) -> bool {
        if self.count == 0 {
            return false;
        }

        if elapsed_ms >= self.backoff_remaining_ms {
            self.backoff_remaining_ms = 0;
            true
        } else {
            self.backoff_remaining_ms -= elapsed_ms;
            false
        }
    }

    /// If an overheard packet matches the pending packet's msg_id and chunk_idx, cancel our TX (R22).
    pub fn on_overhear(&mut self, msg_id: u32, chunk_idx: u8) -> bool {
        for i in 0..self.count {
            let idx = (self.head + i) % TX_QUEUE_CAPACITY;
            if let Some(ref p) = self.queue[idx] {
                if p.header.msg_id == msg_id && p.header.chunk_idx == chunk_idx {
                    self.queue[idx] = None;
                    return true;
                }
            }
        }
        false
    }

    pub fn take_pending(&mut self) -> Option<MeshPacket> {
        while self.count > 0 {
            let pkt = self.queue[self.head].take();
            self.head = (self.head + 1) % TX_QUEUE_CAPACITY;
            self.count -= 1;
            if self.count > 0 {
                self.backoff_remaining_ms = 20; // 20ms between queued packets
            }
            if pkt.is_some() {
                return pkt;
            }
        }
        None
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
