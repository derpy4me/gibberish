#![no_std]

pub mod frame;

pub use frame::{
    assemble_variable_phy_frame, calculate_lqi_relay_jitter, compare_epoch, crc16_ccitt,
    decode_cdc_frame, encode_cdc_frame, is_valid_network_tag, parse_variable_phy_frame,
    ClosedTelemetry, CompactDeltaPayload, DebugTelemetryPayload, DiagnosticEventCode,
    EpochComparison, FramingError, MeshHeader, MeshPacket, PeerMetric, PeerTable, SackPayload,
    StaticMetadataBeacon, StorageModeStatus, TelemetryTier, AUTH_TAG_LEN, CDC_FRAME_MAGIC,
    CDC_HEADER_LEN, CIPHERTEXT_LEN, DEFAULT_NETWORK_TAG, FCS_LEN, FLAG_ACK_REQ, FLAG_CLIPBOARD,
    FLAG_DIRECT, FLAG_GROUP, FLAG_SACK, FLAG_SNEAKERNET, FLAG_TELEMETRY, FLAG_TELEMETRY_STATIC,
    MESH_HEADER_LEN, MHR_LEN, MIN_RELAY_LQI, PHY_MTU, PLAINTEXT_CHUNK_LEN, RECORD_COMMIT_MARKER,
    RECORD_MAGIC, SACK_BITMASK_BYTES, SWARM_NETWORK_TAG,
};
