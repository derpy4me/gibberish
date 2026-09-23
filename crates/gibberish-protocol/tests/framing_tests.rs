use gibberish_protocol::frame::*;

#[test]
fn test_wire_framing_constants_and_sizes() {
    assert_eq!(PHY_MTU, 127);
    assert_eq!(MHR_LEN, 11);
    assert_eq!(MESH_HEADER_LEN, 18);
    assert_eq!(CIPHERTEXT_LEN, 96);
    assert_eq!(PLAINTEXT_CHUNK_LEN, 80);
    assert_eq!(AUTH_TAG_LEN, 16);
    assert_eq!(FCS_LEN, 2);
    assert_eq!(MHR_LEN + MESH_HEADER_LEN + CIPHERTEXT_LEN + FCS_LEN, 127);
}

#[test]
fn test_mesh_header_serialization_roundtrip() {
    let original = MeshHeader {
        network_tag: 0x0123456789ABCDEF,
        msg_id: 0xDEADBEEF,
        chunk_idx: 3,
        total_chunks: 10,
        ttl: 7,
        hop_count: 2,
        flags: FLAG_DIRECT | FLAG_CLIPBOARD,
    };

    let mut buf = [0u8; MESH_HEADER_LEN];
    original.serialize(&mut buf);

    let deserialized = MeshHeader::deserialize(&buf);
    assert_eq!(original, deserialized);
}

#[test]
fn test_mesh_packet_payload_roundtrip_and_ttl() {
    let header = MeshHeader {
        network_tag: 0xAABBCCDDEEFF0011,
        msg_id: 42,
        chunk_idx: 0,
        total_chunks: 1,
        ttl: 5,
        hop_count: 0,
        flags: FLAG_GROUP,
    };

    let mut payload = [0u8; CIPHERTEXT_LEN];
    for i in 0..CIPHERTEXT_LEN {
        payload[i] = (i & 0xFF) as u8;
    }

    let mut packet = MeshPacket { header, payload };
    let mut wire = [0u8; MeshPacket::WIRE_PAYLOAD_LEN];
    packet.serialize_payload(&mut wire);

    let parsed = MeshPacket::deserialize_payload(&wire);
    assert_eq!(packet, parsed);

    assert!(packet.matches_tag(0xAABBCCDDEEFF0011));
    assert!(!packet.matches_tag(0x1122334455667788));

    assert!(packet.decrement_ttl());
    assert_eq!(packet.header.ttl, 4);
    assert_eq!(packet.header.hop_count, 1);
}

#[test]
fn test_closed_telemetry_serialization() {
    let mut telem = ClosedTelemetry::new();
    telem.uptime_secs = 120;
    telem.rx_packet_count = 15;
    telem.tx_packet_count = 10;
    telem.storage_mode = StorageModeStatus::MicroSdActive;
    telem.last_event = DiagnosticEventCode::RadioTxOk;
    telem.last_rssi = -45;

    let mut buf = [0u8; 64];
    let encoded = postcard::to_slice(&telem, &mut buf).expect("telemetry serialization failed");
    let decoded: ClosedTelemetry = postcard::from_bytes(encoded).expect("telemetry deserialization failed");
    assert_eq!(telem, decoded);
}

#[test]
fn test_flag_telemetry_orthogonality() {
    // Verify FLAG_TELEMETRY does not collide with any existing flag
    let flags = [
        FLAG_DIRECT,
        FLAG_GROUP,
        FLAG_CLIPBOARD,
        FLAG_SNEAKERNET,
        FLAG_ACK_REQ,
        FLAG_TELEMETRY,
    ];

    for i in 0..flags.len() {
        for j in (i + 1)..flags.len() {
            assert_eq!(flags[i] & flags[j], 0, "Flag collision between 0x{:04X} and 0x{:04X}", flags[i], flags[j]);
        }
    }
    assert_eq!(FLAG_TELEMETRY, 0x0020);
}

#[test]
fn test_debug_telemetry_payload_roundtrip() {
    let mut dbg = DebugTelemetryPayload::new();
    dbg.uptime_secs = 3600;
    dbg.rx_count = 1024;
    dbg.tx_count = 512;
    dbg.drop_count = 3;
    dbg.sram_used = 42;
    dbg.storage_mode = StorageModeStatus::MicroSdActive;
    dbg.last_event = DiagnosticEventCode::RadioRxOk;
    dbg.build_tier = TelemetryTier::Debug;
    dbg.free_heap_kb = 28;
    dbg.last_rssi = -19;
    dbg.last_lqi = 255;
    dbg.node_mac_tail = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77];
    for i in 0..64 {
        dbg.diagnostic_reserve[i] = (i as u8).wrapping_mul(3);
    }

    let mut wire = [0u8; CIPHERTEXT_LEN];
    dbg.serialize(&mut wire);

    assert_eq!(wire.len(), 96);
    // Validate individual wire offsets according to plan
    assert_eq!(&wire[0..4], &3600u32.to_be_bytes());
    assert_eq!(&wire[4..8], &1024u32.to_be_bytes());
    assert_eq!(&wire[8..12], &512u32.to_be_bytes());
    assert_eq!(&wire[12..16], &3u32.to_be_bytes());
    assert_eq!(&wire[16..18], &42u16.to_be_bytes());
    assert_eq!(wire[18], StorageModeStatus::MicroSdActive as u8);
    assert_eq!(wire[19], DiagnosticEventCode::RadioRxOk as u8);
    assert_eq!(wire[20], TelemetryTier::Debug as u8);
    assert_eq!(wire[21], 28);
    assert_eq!(wire[22] as i8, -19);
    assert_eq!(wire[23], 255);
    assert_eq!(&wire[24..32], &[0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77]);

    let deserialized = DebugTelemetryPayload::deserialize(&wire);
    assert_eq!(dbg, deserialized);
    assert_eq!(dbg.node_id(), 0x44556677);
    assert_eq!(deserialized.node_id(), 0x44556677);
}

#[test]
fn test_telemetry_tier_and_status_from_u8() {
    assert_eq!(TelemetryTier::from_u8(0xDB), TelemetryTier::Debug);
    assert_eq!(TelemetryTier::from_u8(0x50), TelemetryTier::Prod);
    assert_eq!(TelemetryTier::from_u8(0x01), TelemetryTier::Prod);
    assert_eq!(TelemetryTier::from_u8(0x52), TelemetryTier::Prod);

    assert_eq!(StorageModeStatus::from_u8(0), StorageModeStatus::RamOnly);
    assert_eq!(StorageModeStatus::from_u8(1), StorageModeStatus::MicroSdActive);
    assert_eq!(StorageModeStatus::from_u8(99), StorageModeStatus::RamOnly);

    assert_eq!(DiagnosticEventCode::from_u8(1), DiagnosticEventCode::Boot);
    assert_eq!(DiagnosticEventCode::from_u8(4), DiagnosticEventCode::RadioTxOk);
    assert_eq!(DiagnosticEventCode::from_u8(255), DiagnosticEventCode::None);
}
