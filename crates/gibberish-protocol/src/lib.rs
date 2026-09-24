#![no_std]

pub mod frame;

pub use frame::{
    is_valid_network_tag, ClosedTelemetry, DebugTelemetryPayload, DiagnosticEventCode, MeshHeader,
    MeshPacket, PeerMetric, PeerTable, StorageModeStatus, TelemetryTier, AUTH_TAG_LEN,
    CIPHERTEXT_LEN, DEFAULT_NETWORK_TAG, FCS_LEN, FLAG_ACK_REQ, FLAG_CLIPBOARD, FLAG_DIRECT,
    FLAG_GROUP, FLAG_SNEAKERNET, FLAG_TELEMETRY, MESH_HEADER_LEN, MHR_LEN, PHY_MTU,
    PLAINTEXT_CHUNK_LEN, RECORD_COMMIT_MARKER, RECORD_MAGIC, SWARM_NETWORK_TAG,
};
