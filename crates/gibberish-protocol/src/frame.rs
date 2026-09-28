//! Frame layouts, wire serialization, and closed scalar telemetry for Project Gibberish.

use serde::{Deserialize, Serialize};

/// IEEE 802.15.4 MAC Header (MHR) length in bytes
pub const MHR_LEN: usize = 11;
/// Gibberish Mesh Network Header length in bytes
pub const MESH_HEADER_LEN: usize = 18;
/// Usable plaintext chunk budget per mesh frame
pub const PLAINTEXT_CHUNK_LEN: usize = 80;
/// Poly1305 authentication tag length
pub const AUTH_TAG_LEN: usize = 16;
/// Encrypted ciphertext payload length (Plaintext 80B + Auth Tag 16B)
pub const CIPHERTEXT_LEN: usize = PLAINTEXT_CHUNK_LEN + AUTH_TAG_LEN; // 96
/// Hardware FCS CRC-16 length in bytes
pub const FCS_LEN: usize = 2;
/// Total IEEE 802.15.4 Physical MTU (11 + 18 + 96 + 2 = 127)
pub const PHY_MTU: usize = MHR_LEN + MESH_HEADER_LEN + CIPHERTEXT_LEN + FCS_LEN;

/// Power-loss commit marker magic: "GIBB"
pub const RECORD_MAGIC: [u8; 4] = *b"GIBB";
/// Power-loss record commit word (written last to finalize flash write)
pub const RECORD_COMMIT_MARKER: u16 = 0xAA55;

pub const FLAG_DIRECT: u16 = 0x0001;
pub const FLAG_GROUP: u16 = 0x0002;
pub const FLAG_CLIPBOARD: u16 = 0x0004;
pub const FLAG_SNEAKERNET: u16 = 0x0008;
pub const FLAG_ACK_REQ: u16 = 0x0010;
pub const FLAG_TELEMETRY: u16 = 0x0020;
pub const FLAG_SACK: u16 = 0x0040;
pub const FLAG_TELEMETRY_STATIC: u16 = 0x0080;

/// Minimum Link Quality Indicator (0-255) required to relay a packet (Issue #2).
/// Prevents fringe nodes with poor SNR (RSSI < -74 dBm) from repeating corrupted packets.
pub const MIN_RELAY_LQI: u8 = 30;

/// Calculate LQI/RSSI-weighted contention backoff delay in milliseconds (Issue #2).
/// Stronger links (high LQI) relay first with minimal backoff (15-30ms),
/// while weaker links (low LQI) wait longer (45-65ms), allowing stronger relays
/// to take precedence and trigger overhearing cancellation of redundant transmissions.
#[inline]
pub fn calculate_lqi_relay_jitter(lqi: u8, random_val: u16) -> u16 {
    let inv_lqi = 255u16.saturating_sub(lqi as u16);
    let base_delay = 15 + (inv_lqi * 35) / 255;
    base_delay + (random_val % 15)
}

/// Default Network Tag: "GIBBERIS" in big-endian ASCII
pub const DEFAULT_NETWORK_TAG: u64 = 0x4749424245524953;
/// Swarm Network Tag for paired nodes
pub const SWARM_NETWORK_TAG: u64 = 0xD75118D87E4159D2;

#[inline]
pub fn is_valid_network_tag(tag: u64) -> bool {
    tag == DEFAULT_NETWORK_TAG || tag == SWARM_NETWORK_TAG
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshHeader {
    /// 64-bit Pre-shared Network Admission Tag (BLAKE3-MAC truncated)
    pub network_tag: u64,
    /// 32-bit unique message identifier
    pub msg_id: u32,
    /// 0-indexed chunk sequence within message
    pub chunk_idx: u8,
    /// Total number of chunks in this message
    pub total_chunks: u8,
    /// Time-to-live hops remaining
    pub ttl: u8,
    /// Number of mesh hops traversed
    pub hop_count: u8,
    /// Feature and routing flags
    pub flags: u16,
}

impl MeshHeader {
    pub const BYTE_LEN: usize = MESH_HEADER_LEN;

    pub fn serialize(&self, buf: &mut [u8; MESH_HEADER_LEN]) {
        buf[0..8].copy_from_slice(&self.network_tag.to_be_bytes());
        buf[8..12].copy_from_slice(&self.msg_id.to_be_bytes());
        buf[12] = self.chunk_idx;
        buf[13] = self.total_chunks;
        buf[14] = self.ttl;
        buf[15] = self.hop_count;
        buf[16..18].copy_from_slice(&self.flags.to_be_bytes());
    }

    pub fn deserialize(buf: &[u8; MESH_HEADER_LEN]) -> Self {
        let network_tag = u64::from_be_bytes(buf[0..8].try_into().unwrap());
        let msg_id = u32::from_be_bytes(buf[8..12].try_into().unwrap());
        let chunk_idx = buf[12];
        let total_chunks = buf[13];
        let ttl = buf[14];
        let hop_count = buf[15];
        let flags = u16::from_be_bytes(buf[16..18].try_into().unwrap());

        Self {
            network_tag,
            msg_id,
            chunk_idx,
            total_chunks,
            ttl,
            hop_count,
            flags,
        }
    }
}

/// Fully framed mesh packet containing 18-byte mesh header and 96-byte opaque ciphertext.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshPacket {
    pub header: MeshHeader,
    pub payload: [u8; CIPHERTEXT_LEN],
}

impl MeshPacket {
    pub const WIRE_PAYLOAD_LEN: usize = MESH_HEADER_LEN + CIPHERTEXT_LEN; // 114

    pub fn serialize_payload(&self, out: &mut [u8; Self::WIRE_PAYLOAD_LEN]) {
        let mut hdr_buf = [0u8; MESH_HEADER_LEN];
        self.header.serialize(&mut hdr_buf);
        out[0..MESH_HEADER_LEN].copy_from_slice(&hdr_buf);
        out[MESH_HEADER_LEN..Self::WIRE_PAYLOAD_LEN].copy_from_slice(&self.payload);
    }

    pub fn deserialize_payload(buf: &[u8; Self::WIRE_PAYLOAD_LEN]) -> Self {
        let mut hdr_buf = [0u8; MESH_HEADER_LEN];
        hdr_buf.copy_from_slice(&buf[0..MESH_HEADER_LEN]);
        let header = MeshHeader::deserialize(&hdr_buf);
        let mut payload = [0u8; CIPHERTEXT_LEN];
        payload.copy_from_slice(&buf[MESH_HEADER_LEN..Self::WIRE_PAYLOAD_LEN]);
        Self { header, payload }
    }

    pub fn matches_tag(&self, expected_tag: u64) -> bool {
        self.header.network_tag == expected_tag
    }

    pub fn decrement_ttl(&mut self) -> bool {
        if self.header.ttl > 0 {
            self.header.ttl -= 1;
            self.header.hop_count = self.header.hop_count.saturating_add(1);
            true
        } else {
            false
        }
    }
}

/// Maximum number of chunks represented in a single SACK bitmask.
/// 64 bytes * 8 bits = 512 chunks (512 * 80 bytes = 40,960 bytes).
pub const SACK_BITMASK_BYTES: usize = 64;

/// Selective Acknowledgment (SACK) payload carried in a MeshPacket payload (CIPHERTEXT_LEN = 96B).
/// Used to selectively request missing chunks or confirm received chunks without retransmitting the whole payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SackPayload {
    /// 32-bit Node ID of the receiver generating the SACK
    pub receiver_node_id: u32,
    /// 32-bit Node ID of the original sender/target
    pub sender_node_id: u32,
    /// Base chunk offset for this bitmask window (typically 0)
    pub base_chunk: u16,
    /// Bitmask indicating missing chunks (1 = missing/requested retransmit, 0 = received or out of range)
    pub bitmask: [u8; SACK_BITMASK_BYTES],
}

impl SackPayload {
    pub const BYTE_LEN: usize = CIPHERTEXT_LEN; // 96 bytes

    pub const fn new(receiver_node_id: u32, sender_node_id: u32, base_chunk: u16) -> Self {
        Self {
            receiver_node_id,
            sender_node_id,
            base_chunk,
            bitmask: [0u8; SACK_BITMASK_BYTES],
        }
    }

    /// Mark a chunk index as missing (bit = 1)
    pub fn mark_missing(&mut self, chunk_idx: usize) {
        let base = self.base_chunk as usize;
        let max_chunks = SACK_BITMASK_BYTES * 8;
        if chunk_idx >= base && chunk_idx < base + max_chunks {
            let offset = chunk_idx - base;
            self.bitmask[offset / 8] |= 1 << (offset % 8);
        }
    }

    /// Mark a chunk index as received (bit = 0)
    pub fn mark_received(&mut self, chunk_idx: usize) {
        let base = self.base_chunk as usize;
        let max_chunks = SACK_BITMASK_BYTES * 8;
        if chunk_idx >= base && chunk_idx < base + max_chunks {
            let offset = chunk_idx - base;
            self.bitmask[offset / 8] &= !(1 << (offset % 8));
        }
    }

    /// Query whether a specific chunk index is marked as missing
    pub fn is_missing(&self, chunk_idx: usize) -> bool {
        let base = self.base_chunk as usize;
        let max_chunks = SACK_BITMASK_BYTES * 8;
        if chunk_idx >= base && chunk_idx < base + max_chunks {
            let offset = chunk_idx - base;
            (self.bitmask[offset / 8] & (1 << (offset % 8))) != 0
        } else {
            false
        }
    }

    /// Returns the total number of missing chunks in this SACK bitmask
    pub fn missing_count(&self) -> usize {
        let mut count = 0;
        for byte in self.bitmask.iter() {
            count += byte.count_ones() as usize;
        }
        count
    }

    /// Returns true if all chunks are received (bitmask is all zeros).
    /// Used for end-to-end delivery confirmations (ACK).
    pub fn is_full_ack(&self) -> bool {
        self.bitmask.iter().all(|&b| b == 0)
    }

    /// Iterates over every chunk index marked as missing in this bitmask
    pub fn for_each_missing<F: FnMut(usize)>(&self, mut f: F) {
        let base = self.base_chunk as usize;
        for (byte_idx, &byte) in self.bitmask.iter().enumerate() {
            if byte == 0 {
                continue;
            }
            for bit in 0..8 {
                if (byte & (1 << bit)) != 0 {
                    f(base + byte_idx * 8 + bit);
                }
            }
        }
    }

    pub fn serialize(&self, out: &mut [u8; CIPHERTEXT_LEN]) {
        out.fill(0);
        out[0..4].copy_from_slice(&self.receiver_node_id.to_be_bytes());
        out[4..8].copy_from_slice(&self.sender_node_id.to_be_bytes());
        out[8..10].copy_from_slice(&self.base_chunk.to_be_bytes());
        out[10..10 + SACK_BITMASK_BYTES].copy_from_slice(&self.bitmask);
    }

    pub fn deserialize(buf: &[u8; CIPHERTEXT_LEN]) -> Self {
        let receiver_node_id = u32::from_be_bytes(buf[0..4].try_into().unwrap());
        let sender_node_id = u32::from_be_bytes(buf[4..8].try_into().unwrap());
        let base_chunk = u16::from_be_bytes(buf[8..10].try_into().unwrap());
        let mut bitmask = [0u8; SACK_BITMASK_BYTES];
        bitmask.copy_from_slice(&buf[10..10 + SACK_BITMASK_BYTES]);
        Self {
            receiver_node_id,
            sender_node_id,
            base_chunk,
            bitmask,
        }
    }
}

/// Closed scalar event codes for telemetry logging without string formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum DiagnosticEventCode {
    None = 0,
    Boot = 1,
    SdMounted = 2,
    SdFallbackRamOnly = 3,
    RadioTxOk = 4,
    RadioRxOk = 5,
    RadioDroppedTagMismatch = 6,
    RadioDroppedDedup = 7,
    BleConnected = 8,
    BleDisconnected = 9,
    BleAuthPrompt = 10,
    BleAuthGranted = 11,
    BleAuthTimeout = 12,
    StorageTornWriteDiscarded = 13,
    StorageFlushed = 14,
    StorageOverflow = 15,
}

impl DiagnosticEventCode {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0 => DiagnosticEventCode::None,
            1 => DiagnosticEventCode::Boot,
            2 => DiagnosticEventCode::SdMounted,
            3 => DiagnosticEventCode::SdFallbackRamOnly,
            4 => DiagnosticEventCode::RadioTxOk,
            5 => DiagnosticEventCode::RadioRxOk,
            6 => DiagnosticEventCode::RadioDroppedTagMismatch,
            7 => DiagnosticEventCode::RadioDroppedDedup,
            8 => DiagnosticEventCode::BleConnected,
            9 => DiagnosticEventCode::BleDisconnected,
            10 => DiagnosticEventCode::BleAuthPrompt,
            11 => DiagnosticEventCode::BleAuthGranted,
            12 => DiagnosticEventCode::BleAuthTimeout,
            13 => DiagnosticEventCode::StorageTornWriteDiscarded,
            14 => DiagnosticEventCode::StorageFlushed,
            15 => DiagnosticEventCode::StorageOverflow,
            _ => DiagnosticEventCode::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum StorageModeStatus {
    RamOnly = 0,
    MicroSdActive = 1,
}

impl StorageModeStatus {
    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => StorageModeStatus::MicroSdActive,
            _ => StorageModeStatus::RamOnly,
        }
    }
}

/// Active firmware build profile tier (R3, R4, R5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum TelemetryTier {
    Debug = 0xDB,
    Prod = 0x50,
}

impl TelemetryTier {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0x50 | 0x01 | 0x52 => TelemetryTier::Prod,
            _ => TelemetryTier::Debug,
        }
    }
}

/// Closed-schema scalar telemetry struct encoded via postcard (R31).
/// Guarantees zero sensitive strings or memory leakage over USB-CDC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedTelemetry {
    pub uptime_secs: u32,
    pub rx_packet_count: u32,
    pub tx_packet_count: u32,
    pub dropped_count: u32,
    pub sram_ring_used: u16,
    pub storage_mode: StorageModeStatus,
    pub last_event: DiagnosticEventCode,
    pub last_rssi: i8,
}

impl Default for ClosedTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl ClosedTelemetry {
    pub fn new() -> Self {
        Self {
            uptime_secs: 0,
            rx_packet_count: 0,
            tx_packet_count: 0,
            dropped_count: 0,
            sram_ring_used: 0,
            storage_mode: StorageModeStatus::RamOnly,
            last_event: DiagnosticEventCode::Boot,
            last_rssi: 0,
        }
    }
}

/// Unencrypted diagnostic telemetry payload (96 bytes) for development build profile (R4, R14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebugTelemetryPayload {
    /// Monotonic device uptime in seconds
    pub uptime_secs: u32,
    /// Lifetime radio frames received
    pub rx_count: u32,
    /// Lifetime radio frames transmitted
    pub tx_count: u32,
    /// Packets dropped due to buffer overflow or dedup
    pub drop_count: u32,
    /// SRAM ring buffer utilization (number of packets, 0..256)
    pub sram_used: u16,
    /// Storage subsystem operating status
    pub storage_mode: StorageModeStatus,
    /// Most recent diagnostic event code
    pub last_event: DiagnosticEventCode,
    /// Active firmware build profile tier
    pub build_tier: TelemetryTier,
    /// Available internal heap in kilobytes
    pub free_heap_kb: u8,
    /// Hardware Received Signal Strength Indicator in dBm of last peer packet
    pub last_rssi: i8,
    /// Link Quality Indicator (0..255) of last peer packet
    pub last_lqi: u8,
    /// Hardware Station MAC tail (8 bytes)
    pub node_mac_tail: [u8; 8],
    /// Reserved diagnostic padding
    pub diagnostic_reserve: [u8; 64],
}

impl Default for DebugTelemetryPayload {
    fn default() -> Self {
        Self::new()
    }
}

impl DebugTelemetryPayload {
    pub const BYTE_LEN: usize = CIPHERTEXT_LEN; // 96 bytes

    pub fn new() -> Self {
        Self {
            uptime_secs: 0,
            rx_count: 0,
            tx_count: 0,
            drop_count: 0,
            sram_used: 0,
            storage_mode: StorageModeStatus::RamOnly,
            last_event: DiagnosticEventCode::Boot,
            build_tier: TelemetryTier::Debug,
            free_heap_kb: 0,
            last_rssi: 0,
            last_lqi: 0,
            node_mac_tail: [0u8; 8],
            diagnostic_reserve: [0u8; 64],
        }
    }

    pub fn serialize(&self, buf: &mut [u8; CIPHERTEXT_LEN]) {
        buf[0..4].copy_from_slice(&self.uptime_secs.to_be_bytes());
        buf[4..8].copy_from_slice(&self.rx_count.to_be_bytes());
        buf[8..12].copy_from_slice(&self.tx_count.to_be_bytes());
        buf[12..16].copy_from_slice(&self.drop_count.to_be_bytes());
        buf[16..18].copy_from_slice(&self.sram_used.to_be_bytes());
        buf[18] = self.storage_mode as u8;
        buf[19] = self.last_event as u8;
        buf[20] = self.build_tier as u8;
        buf[21] = self.free_heap_kb;
        buf[22] = self.last_rssi as u8;
        buf[23] = self.last_lqi;
        buf[24..32].copy_from_slice(&self.node_mac_tail);
        buf[32..96].copy_from_slice(&self.diagnostic_reserve);
    }

    pub fn deserialize(buf: &[u8; CIPHERTEXT_LEN]) -> Self {
        let uptime_secs = u32::from_be_bytes(buf[0..4].try_into().unwrap());
        let rx_count = u32::from_be_bytes(buf[4..8].try_into().unwrap());
        let tx_count = u32::from_be_bytes(buf[8..12].try_into().unwrap());
        let drop_count = u32::from_be_bytes(buf[12..16].try_into().unwrap());
        let sram_used = u16::from_be_bytes(buf[16..18].try_into().unwrap());
        let storage_mode = StorageModeStatus::from_u8(buf[18]);
        let last_event = DiagnosticEventCode::from_u8(buf[19]);
        let build_tier = TelemetryTier::from_u8(buf[20]);
        let free_heap_kb = buf[21];
        let last_rssi = buf[22] as i8;
        let last_lqi = buf[23];
        let mut node_mac_tail = [0u8; 8];
        node_mac_tail.copy_from_slice(&buf[24..32]);
        let mut diagnostic_reserve = [0u8; 64];
        diagnostic_reserve.copy_from_slice(&buf[32..96]);

        Self {
            uptime_secs,
            rx_count,
            tx_count,
            drop_count,
            sram_used,
            storage_mode,
            last_event,
            build_tier,
            free_heap_kb,
            last_rssi,
            last_lqi,
            node_mac_tail,
            diagnostic_reserve,
        }
    }

    /// Extracts the 32-bit Node ID from the hardware MAC tail bytes [4..8].
    pub fn node_id(&self) -> u32 {
        u32::from_be_bytes([
            self.node_mac_tail[4],
            self.node_mac_tail[5],
            self.node_mac_tail[6],
            self.node_mac_tail[7],
        ])
    }
}

/// Framing and validation errors across wire serialization and deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramingError {
    BufferTooShort,
    BufferTooLarge,
    InvalidNetworkTag,
    InvalidField,
    CrcMismatch,
    MagicMismatch,
}

/// Compact static hardware and configuration metadata beacon (28 bytes packed).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticMetadataBeacon {
    /// 8-byte anonymized node ID (truncated BLAKE3 hash of station identity)
    pub node_id: [u8; 8],
    /// Monotonic uptime epoch counter (number of full u32 rollover cycles)
    pub uptime_epoch: u16,
    /// Hardware revision number
    pub hw_rev: u8,
    /// Build tier (Debug, Production, Airgap)
    pub build_tier: TelemetryTier,
    /// Storage subsystem operating status
    pub storage_mode: StorageModeStatus,
    /// Schema version for telemetry frames
    pub schema_version: u8,
    /// Configuration epoch counter (monotonic)
    pub config_epoch: u8,
    /// Reserved zeroed padding
    pub reserved: [u8; 13],
}

const _: () = assert!(core::mem::size_of::<StaticMetadataBeacon>() == 28);

impl Default for StaticMetadataBeacon {
    fn default() -> Self {
        Self::new()
    }
}

impl StaticMetadataBeacon {
    pub const BYTE_LEN: usize = 28;

    pub fn new() -> Self {
        Self {
            node_id: [0u8; 8],
            uptime_epoch: 0,
            hw_rev: 1,
            build_tier: TelemetryTier::Debug,
            storage_mode: StorageModeStatus::RamOnly,
            schema_version: 1,
            config_epoch: 1,
            reserved: [0u8; 13],
        }
    }

    pub fn serialize(&self, buf: &mut [u8]) -> Result<(), FramingError> {
        if buf.len() < Self::BYTE_LEN {
            return Err(FramingError::BufferTooShort);
        }
        buf[0..8].copy_from_slice(&self.node_id);
        buf[8..10].copy_from_slice(&self.uptime_epoch.to_be_bytes());
        buf[10] = self.hw_rev;
        buf[11] = self.build_tier as u8;
        buf[12] = self.storage_mode as u8;
        buf[13] = self.schema_version;
        buf[14] = self.config_epoch;
        buf[15..28].copy_from_slice(&self.reserved);
        Ok(())
    }

    pub fn deserialize(buf: &[u8]) -> Result<Self, FramingError> {
        if buf.len() < Self::BYTE_LEN {
            return Err(FramingError::BufferTooShort);
        }
        let mut node_id = [0u8; 8];
        node_id.copy_from_slice(&buf[0..8]);
        let uptime_epoch = u16::from_be_bytes(buf[8..10].try_into().unwrap());
        let hw_rev = buf[10];
        let build_tier = TelemetryTier::from_u8(buf[11]);
        let storage_mode = StorageModeStatus::from_u8(buf[12]);
        let schema_version = buf[13];
        let config_epoch = buf[14];
        let mut reserved = [0u8; 13];
        reserved.copy_from_slice(&buf[15..28]);

        Ok(Self {
            node_id,
            uptime_epoch,
            hw_rev,
            build_tier,
            storage_mode,
            schema_version,
            config_epoch,
            reserved,
        })
    }

    pub fn node_id_u32(&self) -> u32 {
        u32::from_be_bytes([
            self.node_id[4],
            self.node_id[5],
            self.node_id[6],
            self.node_id[7],
        ])
    }
}

/// Compact dynamic delta telemetry payload (24 bytes packed).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactDeltaPayload {
    /// Monotonic device uptime in seconds within current uptime_epoch
    pub uptime_secs: u32,
    /// Lifetime radio frames received
    pub rx_count: u32,
    /// Lifetime radio frames transmitted
    pub tx_count: u32,
    /// Packets dropped due to buffer overflow or dedup
    pub drop_count: u32,
    /// SRAM ring buffer utilization (number of packets, 0..256)
    pub sram_used: u16,
    /// Configuration epoch counter from corresponding StaticMetadataBeacon
    pub config_epoch: u8,
    /// Available internal heap in kilobytes
    pub free_heap_kb: u8,
    /// Most recent diagnostic event code
    pub last_event: DiagnosticEventCode,
    /// Hardware Received Signal Strength Indicator in dBm of last peer packet
    pub last_rssi: i8,
    /// Link Quality Indicator (0..255) of last peer packet
    pub last_lqi: u8,
    /// Reserved padding byte for 4-byte struct alignment
    pub reserved: u8,
}

const _: () = assert!(core::mem::size_of::<CompactDeltaPayload>() == 24);

impl Default for CompactDeltaPayload {
    fn default() -> Self {
        Self::new()
    }
}

impl CompactDeltaPayload {
    pub const BYTE_LEN: usize = 24;

    pub fn new() -> Self {
        Self {
            uptime_secs: 0,
            rx_count: 0,
            tx_count: 0,
            drop_count: 0,
            sram_used: 0,
            config_epoch: 1,
            free_heap_kb: 0,
            last_event: DiagnosticEventCode::Boot,
            last_rssi: 0,
            last_lqi: 0,
            reserved: 0,
        }
    }

    pub fn serialize(&self, buf: &mut [u8]) -> Result<(), FramingError> {
        if buf.len() < Self::BYTE_LEN {
            return Err(FramingError::BufferTooShort);
        }
        buf[0..4].copy_from_slice(&self.uptime_secs.to_be_bytes());
        buf[4..8].copy_from_slice(&self.rx_count.to_be_bytes());
        buf[8..12].copy_from_slice(&self.tx_count.to_be_bytes());
        buf[12..16].copy_from_slice(&self.drop_count.to_be_bytes());
        buf[16..18].copy_from_slice(&self.sram_used.to_be_bytes());
        buf[18] = self.config_epoch;
        buf[19] = self.free_heap_kb;
        buf[20] = self.last_event as u8;
        buf[21] = self.last_rssi as u8;
        buf[22] = self.last_lqi;
        buf[23] = self.reserved;
        Ok(())
    }

    pub fn deserialize(buf: &[u8]) -> Result<Self, FramingError> {
        if buf.len() < Self::BYTE_LEN {
            return Err(FramingError::BufferTooShort);
        }
        let uptime_secs = u32::from_be_bytes(buf[0..4].try_into().unwrap());
        let rx_count = u32::from_be_bytes(buf[4..8].try_into().unwrap());
        let tx_count = u32::from_be_bytes(buf[8..12].try_into().unwrap());
        let drop_count = u32::from_be_bytes(buf[12..16].try_into().unwrap());
        let sram_used = u16::from_be_bytes(buf[16..18].try_into().unwrap());
        let config_epoch = buf[18];
        let free_heap_kb = buf[19];
        let last_event = DiagnosticEventCode::from_u8(buf[20]);
        let last_rssi = buf[21] as i8;
        let last_lqi = buf[22];
        let reserved = buf[23];

        Ok(Self {
            uptime_secs,
            rx_count,
            tx_count,
            drop_count,
            sram_used,
            config_epoch,
            free_heap_kb,
            last_event,
            last_rssi,
            last_lqi,
            reserved,
        })
    }
}

/// Comparison result for RFC 1982 serial-number arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpochComparison {
    Newer,
    Older,
    Equal,
    Ambiguous, // difference == 128
}

/// Evaluates monotonic serial epoch progression using RFC 1982 arithmetic.
#[inline]
pub fn compare_epoch(new_epoch: u8, old_epoch: u8) -> EpochComparison {
    if new_epoch == old_epoch {
        return EpochComparison::Equal;
    }
    let diff = new_epoch.wrapping_sub(old_epoch);
    if diff == 128 {
        EpochComparison::Ambiguous
    } else if (diff as i8) > 0 {
        EpochComparison::Newer
    } else {
        EpochComparison::Older
    }
}

/// Assembles a variable-length physical IEEE 802.15.4 frame without padding.
pub fn assemble_variable_phy_frame(
    mhr: &[u8; MHR_LEN],
    header: &MeshHeader,
    payload: &[u8],
    out: &mut [u8; PHY_MTU],
) -> Result<usize, FramingError> {
    let total_len = MHR_LEN + MESH_HEADER_LEN + payload.len() + FCS_LEN;
    if total_len > PHY_MTU {
        return Err(FramingError::BufferTooLarge);
    }
    out[0..MHR_LEN].copy_from_slice(mhr);
    header.serialize((&mut out[MHR_LEN..MHR_LEN + MESH_HEADER_LEN]).try_into().unwrap());
    out[MHR_LEN + MESH_HEADER_LEN..total_len - FCS_LEN].copy_from_slice(payload);
    out[total_len - FCS_LEN..total_len].copy_from_slice(&[0x00, 0x00]); // Dummy FCS for PHY driver
    Ok(total_len)
}

/// Parses a variable-length physical IEEE 802.15.4 frame using defensive computed offsets.
pub fn parse_variable_phy_frame<'a>(
    raw: &'a [u8],
) -> Result<(MeshHeader, &'a [u8]), FramingError> {
    if raw.len() < MHR_LEN + MESH_HEADER_LEN + FCS_LEN {
        return Err(FramingError::BufferTooShort);
    }
    if raw.len() > PHY_MTU {
        return Err(FramingError::BufferTooLarge);
    }
    let hdr_slice: &[u8; MESH_HEADER_LEN] = raw[MHR_LEN..MHR_LEN + MESH_HEADER_LEN].try_into().unwrap();
    let header = MeshHeader::deserialize(hdr_slice);
    if !is_valid_network_tag(header.network_tag) {
        return Err(FramingError::InvalidNetworkTag);
    }
    let payload = &raw[MHR_LEN + MESH_HEADER_LEN..raw.len() - FCS_LEN];
    Ok((header, payload))
}

/// USB-CDC binary frame sync delimiter
pub const CDC_FRAME_MAGIC: [u8; 2] = [0xAA, 0x55];
/// USB-CDC header length: magic (2B) + length (2B) + crc16 (2B)
pub const CDC_HEADER_LEN: usize = 6;

/// Computes CRC16-CCITT (polynomial 0x1021, init 0xFFFF).
#[inline]
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// Encodes payload into a length-prefixed, CRC16-protected USB-CDC binary frame.
pub fn encode_cdc_frame(payload: &[u8], out: &mut [u8]) -> Result<usize, FramingError> {
    let total_len = CDC_HEADER_LEN + payload.len();
    if out.len() < total_len {
        return Err(FramingError::BufferTooShort);
    }
    if payload.len() > u16::MAX as usize {
        return Err(FramingError::BufferTooLarge);
    }
    out[0..2].copy_from_slice(&CDC_FRAME_MAGIC);
    let len_u16 = payload.len() as u16;
    let len_bytes = len_u16.to_be_bytes();
    out[2..4].copy_from_slice(&len_bytes);

    let mut crc: u16 = 0xFFFF;
    for &b in &len_bytes {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    for &b in payload {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    out[4..6].copy_from_slice(&crc.to_be_bytes());
    out[CDC_HEADER_LEN..total_len].copy_from_slice(payload);
    Ok(total_len)
}

/// Decodes and validates a length-prefixed, CRC16-protected USB-CDC binary frame.
/// Returns Ok(Some((consumed_len, payload))) when a complete frame is decoded,
/// Ok(None) when more bytes are needed, or Err if header magic or CRC fails.
pub fn decode_cdc_frame<'a>(buf: &'a [u8]) -> Result<Option<(usize, &'a [u8])>, FramingError> {
    if buf.len() < CDC_HEADER_LEN {
        return Ok(None);
    }
    if buf[0..2] != CDC_FRAME_MAGIC {
        return Err(FramingError::MagicMismatch);
    }
    let payload_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let total_expected = CDC_HEADER_LEN + payload_len;
    if buf.len() < total_expected {
        return Ok(None);
    }
    let expected_crc = u16::from_be_bytes([buf[4], buf[5]]);
    let mut crc: u16 = 0xFFFF;
    for &b in &buf[2..4] {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    for &b in &buf[CDC_HEADER_LEN..total_expected] {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    if crc != expected_crc {
        return Err(FramingError::CrcMismatch);
    }
    Ok(Some((total_expected, &buf[CDC_HEADER_LEN..total_expected])))
}

/// Captured peer link metrics from received airwave frames
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerMetric {
    pub node_id: u32,
    pub last_rssi: i8,
    pub last_lqi: u8,
    pub packet_count: u32,
}

impl PeerMetric {
    pub const fn new(node_id: u32, last_rssi: i8, last_lqi: u8) -> Self {
        Self {
            node_id,
            last_rssi,
            last_lqi,
            packet_count: 1,
        }
    }
}

pub const PEER_TABLE_CAPACITY: usize = 4;

/// Bounded LRU/MRU table of active mesh peers for UI and telemetry.
/// Entries are maintained in order of recency of contact (index 0 is least-recently used,
/// last non-None index is most-recently active).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerTable {
    pub peers: [Option<PeerMetric>; PEER_TABLE_CAPACITY],
}

impl Default for PeerTable {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerTable {
    pub const fn new() -> Self {
        Self {
            peers: [None; PEER_TABLE_CAPACITY],
        }
    }

    /// Number of active tracked peers in the table
    pub fn len(&self) -> usize {
        self.peers.iter().flatten().count()
    }

    /// Whether the table has no tracked peers
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Looks up a peer metric by 32-bit Node ID
    pub fn get_peer(&self, node_id: u32) -> Option<PeerMetric> {
        if node_id == 0 {
            return None;
        }
        for peer in self.peers.iter().flatten() {
            if peer.node_id == node_id {
                return Some(*peer);
            }
        }
        None
    }

    /// Records or updates an active peer node in the table.
    /// Uses true LRU/MRU ordering: active peers are promoted to the MRU position
    /// (end of active entries), ensuring active communicating peers are never prematurely evicted.
    pub fn record_peer(&mut self, node_id: u32, rssi: i8, lqi: u8) {
        // Node ID 0 is invalid/reserved broadcast address in the Gibberish mesh protocol
        if node_id == 0 {
            return;
        }

        // 1. If peer already exists, update and promote to MRU
        for i in 0..PEER_TABLE_CAPACITY {
            if let Some(mut peer) = self.peers[i] {
                if peer.node_id == node_id {
                    peer.last_rssi = rssi;
                    peer.last_lqi = lqi;
                    peer.packet_count = peer.packet_count.saturating_add(1);

                    // Find last active slot index
                    let mut last_idx = i;
                    for j in (i + 1)..PEER_TABLE_CAPACITY {
                        if self.peers[j].is_some() {
                            last_idx = j;
                        }
                    }

                    // Shift elements down between i and last_idx
                    for j in i..last_idx {
                        self.peers[j] = self.peers[j + 1];
                    }
                    self.peers[last_idx] = Some(peer);
                    return;
                }
            }
        }

        // 2. If peer does not exist, find first empty slot
        for i in 0..PEER_TABLE_CAPACITY {
            if self.peers[i].is_none() {
                self.peers[i] = Some(PeerMetric::new(node_id, rssi, lqi));
                return;
            }
        }

        // 3. Table is full: evict LRU entry (slot 0), shift left, and insert at MRU (end)
        for i in 0..(PEER_TABLE_CAPACITY - 1) {
            self.peers[i] = self.peers[i + 1];
        }
        self.peers[PEER_TABLE_CAPACITY - 1] = Some(PeerMetric::new(node_id, rssi, lqi));
    }

    pub fn primary_peer(&self) -> Option<PeerMetric> {
        // Most recent peer is the last non-None entry
        self.peers.iter().rev().flatten().next().copied()
    }

    pub fn secondary_peer(&self) -> Option<PeerMetric> {
        self.peers.iter().rev().flatten().nth(1).copied()
    }
}

