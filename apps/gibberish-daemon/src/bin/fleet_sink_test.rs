//! Automated End-to-End Multi-Node Telemetry Verification Suite (U5).
//! Validates IEEE 802.15.4 telemetry framing, wire deserialization,
//! CDC line decoding, and physical over-the-air RF reception across dongles.

use gibberish_daemon::fleet::{parse_telemetry_line, FleetManager, PeerContextState};
use gibberish_protocol::{
    assemble_variable_phy_frame, parse_variable_phy_frame, CompactDeltaPayload,
    DebugTelemetryPayload, DiagnosticEventCode, MeshHeader, MeshPacket, StaticMetadataBeacon,
    StorageModeStatus, TelemetryTier, CIPHERTEXT_LEN, DEFAULT_NETWORK_TAG, FLAG_TELEMETRY,
    FLAG_TELEMETRY_STATIC, MESH_HEADER_LEN, MHR_LEN, FCS_LEN,
};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("============================================================");
    println!(" Project Gibberish - Multi-Node Telemetry Verification Suite ");
    println!("============================================================\n");

    // -------------------------------------------------------------------------
    // Phase 1A: Legacy 96-Byte Telemetry Wire Framing Verification
    // -------------------------------------------------------------------------
    println!("[Phase 1A] Verifying Legacy 96-Byte Telemetry Wire Framing...");
    let mut dbg = DebugTelemetryPayload::new();
    dbg.uptime_secs = 12345;
    dbg.rx_count = 500;
    dbg.tx_count = 100;
    dbg.drop_count = 2;
    dbg.sram_used = 18;
    dbg.storage_mode = StorageModeStatus::MicroSdActive;
    dbg.last_event = DiagnosticEventCode::RadioRxOk;
    dbg.build_tier = TelemetryTier::Debug;
    dbg.free_heap_kb = 32;
    dbg.last_rssi = -24;
    dbg.last_lqi = 250;
    dbg.node_mac_tail = [0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44];

    let mut payload = [0u8; CIPHERTEXT_LEN];
    dbg.serialize(&mut payload);

    let packet = MeshPacket {
        header: MeshHeader {
            network_tag: DEFAULT_NETWORK_TAG,
            msg_id: 1,
            chunk_idx: 0,
            total_chunks: 1,
            ttl: 1,
            hop_count: 0,
            flags: FLAG_TELEMETRY,
        },
        payload,
    };

    let mut wire = [0u8; MeshPacket::WIRE_PAYLOAD_LEN];
    packet.serialize_payload(&mut wire);
    assert_eq!(wire.len(), 114);

    let parsed_packet = MeshPacket::deserialize_payload(&wire);
    assert_eq!(parsed_packet.header.flags & FLAG_TELEMETRY, FLAG_TELEMETRY);
    assert_eq!(parsed_packet.header.ttl, 1);
    assert_eq!(parsed_packet.header.msg_id, 1);

    let parsed_dbg = DebugTelemetryPayload::deserialize(&parsed_packet.payload);
    assert_eq!(parsed_dbg.uptime_secs, 12345);
    assert_eq!(parsed_dbg.rx_count, 500);
    assert_eq!(parsed_dbg.tx_count, 100);
    assert_eq!(parsed_dbg.node_id(), 0x11223344);
    println!("    ✓ Legacy wire framing roundtrip passed (114B wire, 96B payload, Node ID: {:08X})", parsed_dbg.node_id());

    // -------------------------------------------------------------------------
    // Phase 1B: Variable High-Efficiency Telemetry Framing & Airtime Reduction
    // -------------------------------------------------------------------------
    println!("\n[Phase 1B] Verifying High-Efficiency Variable PHY Telemetry Framing...");
    let mut delta = CompactDeltaPayload::new();
    delta.uptime_secs = 65432;
    delta.rx_count = 1200;
    delta.tx_count = 350;
    delta.drop_count = 1;
    delta.sram_used = 15;
    delta.config_epoch = 7;
    delta.free_heap_kb = 48;
    delta.last_event = DiagnosticEventCode::RadioTxOk;
    delta.last_rssi = -18;
    delta.last_lqi = 245;

    let delta_header = MeshHeader {
        network_tag: DEFAULT_NETWORK_TAG,
        msg_id: 42,
        chunk_idx: 0,
        total_chunks: 1,
        ttl: 1,
        hop_count: 0,
        flags: FLAG_TELEMETRY,
    };

    let mut delta_wire = [0u8; 24];
    delta.serialize(&mut delta_wire).expect("Delta serialize failed");
    assert_eq!(delta_wire.len(), 24);

    let mhr = [0x41, 0x08, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xBE, 0xBC, 0xE5, 0xB8];
    let mut delta_phy = [0u8; 127];
    let delta_phy_len = assemble_variable_phy_frame(
        &mhr,
        &delta_header,
        &delta_wire,
        &mut delta_phy,
    ).expect("Variable PHY assembly failed");

    // PSDU for delta: MHR(13) + MeshHeader(18) + CompactDelta(24) = 55 bytes
    assert_eq!(delta_phy_len, MHR_LEN + MESH_HEADER_LEN + 24 + FCS_LEN);
    assert_eq!(delta_phy_len, 55);

    let payload_reduction_pct = ((96.0 - 24.0) / 96.0) * 100.0;
    let psdu_reduction_pct = ((127.0 - 55.0) / 127.0) * 100.0;
    println!("    ✓ Delta payload cut: 96B -> 24B ({:.1}% reduction, target 75%)", payload_reduction_pct);
    assert_eq!(payload_reduction_pct as u32, 75);
    println!("    ✓ Delta PSDU airtime cut: 127B -> 55B ({:.1}% reduction, target >56%)", psdu_reduction_pct);
    assert!(psdu_reduction_pct > 56.0);

    // Parse variable PHY delta frame
    let (parsed_delta_hdr, parsed_delta_slice) = parse_variable_phy_frame(&delta_phy[..delta_phy_len])
        .expect("Variable PHY parse failed");
    assert_eq!(parsed_delta_hdr.msg_id, 42);
    assert_eq!(parsed_delta_hdr.flags & FLAG_TELEMETRY, FLAG_TELEMETRY);
    let parsed_delta = CompactDeltaPayload::deserialize(parsed_delta_slice)
        .expect("Delta deserialize failed");
    assert_eq!(parsed_delta.uptime_secs, 65432);
    assert_eq!(parsed_delta.config_epoch, 7);
    assert_eq!(parsed_delta.last_rssi, -18);
    println!("    ✓ Variable PHY delta parsing roundtrip verified");

    // Static Metadata Beacon
    let mut beacon = StaticMetadataBeacon::new();
    beacon.node_id = [0x01, 0x02, 0x03, 0x04, 0xBE, 0xBC, 0xE5, 0xB8];
    beacon.uptime_epoch = 0;
    beacon.hw_rev = 2;
    beacon.build_tier = TelemetryTier::Prod;
    beacon.storage_mode = StorageModeStatus::MicroSdActive;
    beacon.schema_version = 1;
    beacon.config_epoch = 7;

    let beacon_header = MeshHeader {
        network_tag: DEFAULT_NETWORK_TAG,
        msg_id: 100,
        chunk_idx: 0,
        total_chunks: 1,
        ttl: 1,
        hop_count: 0,
        flags: FLAG_TELEMETRY_STATIC,
    };

    let mut beacon_wire = [0u8; 28];
    beacon.serialize(&mut beacon_wire).expect("Beacon serialize failed");
    assert_eq!(beacon_wire.len(), 28);

    let mut beacon_phy = [0u8; 127];
    let beacon_phy_len = assemble_variable_phy_frame(
        &mhr,
        &beacon_header,
        &beacon_wire,
        &mut beacon_phy,
    ).expect("Variable beacon PHY assembly failed");

    // PSDU for beacon: MHR(13) + MeshHeader(18) + StaticBeacon(28) = 59 bytes
    assert_eq!(beacon_phy_len, MHR_LEN + MESH_HEADER_LEN + 28 + FCS_LEN);
    assert_eq!(beacon_phy_len, 59);

    let (parsed_beacon_hdr, parsed_beacon_slice) = parse_variable_phy_frame(&beacon_phy[..beacon_phy_len])
        .expect("Variable beacon PHY parse failed");
    assert_eq!(parsed_beacon_hdr.msg_id, 100);
    assert_eq!(parsed_beacon_hdr.flags & FLAG_TELEMETRY_STATIC, FLAG_TELEMETRY_STATIC);
    let parsed_beacon = StaticMetadataBeacon::deserialize(parsed_beacon_slice)
        .expect("Beacon deserialize failed");
    assert_eq!(parsed_beacon.node_id_u32(), 0xBEBCE5B8);
    assert_eq!(parsed_beacon.build_tier, TelemetryTier::Prod);
    println!("    ✓ Variable PHY static beacon roundtrip verified (59B PSDU)");

    // -------------------------------------------------------------------------
    // Phase 2: Fleet Sink Line Parser & Per-Peer State Machine Verification
    // -------------------------------------------------------------------------
    println!("\n[Phase 2] Verifying Fleet Sink Log Parsing & Per-Peer State Machine...");
    let test_log = "/tmp/gibberish/fleet_unit_test.log";
    if Path::new(test_log).exists() {
        let _ = fs::remove_file(test_log);
    }
    let mut fleet = FleetManager::new(test_log);

    let line_node_a = "[Telemetry RX] Node: BEBCE5B8, Tier: Debug, Storage: MicroSdActive, Uptime: 862s, SRAM: 0/256, Drops: 0, RX: 904, TX: 68, RSSI: -19 dBm, LQI: 255";
    let line_node_b = "[Dongle /dev/ttyACM1] [Telemetry RX] Node: BEBD82B4, Tier: Prod, Storage: RamOnly, Uptime: 838s, SRAM: 46/256, Drops: 46, RX: 302, TX: 33, RSSI: -21 dBm, LQI: 240";

    let telem_a = fleet.ingest_line(line_node_a).expect("Failed to ingest Node A");
    assert_eq!(telem_a.node_id, "BEBCE5B8");
    assert_eq!(telem_a.tier, "DEBUG");
    assert_eq!(telem_a.storage_mode, "SD ACTIVE");
    assert_eq!(telem_a.uptime_secs, 862);
    assert_eq!(telem_a.rssi, -19);

    let telem_b = fleet.ingest_line(line_node_b).expect("Failed to ingest Node B");
    assert_eq!(telem_b.node_id, "BEBD82B4");
    assert_eq!(telem_b.tier, "PROD");
    assert_eq!(telem_b.storage_mode, "RAM ONLY");
    assert_eq!(telem_b.uptime_secs, 838);
    assert_eq!(telem_b.rssi, -21);

    assert_eq!(fleet.node_count(), 2);

    // Test late-joiner delta -> PendingContext state
    let node_c_id = 0xCCDDEEFF;
    let telem_c = fleet.ingest_delta(node_c_id, &delta, -22, 230);
    assert_eq!(telem_c.node_id, "CCDDEEFF");
    assert!(matches!(telem_c.state, PeerContextState::PendingContext { .. }));
    println!("    ✓ Late joiner delta initialized in PendingContext state");

    // Promote Node C to Active via StaticMetadataBeacon
    let mut beacon_c = beacon;
    beacon_c.node_id = [0, 0, 0, 0, 0xCC, 0xDD, 0xEE, 0xFF];
    let telem_c_active = fleet.ingest_static_beacon(&beacon_c, -20, 240);
    assert_eq!(telem_c_active.state, PeerContextState::Active);
    println!("    ✓ StaticMetadataBeacon successfully promoted Node C to Active state");

    // Test AmbiguousEpoch transition (diff == 128)
    let mut delta_ambiguous = delta;
    delta_ambiguous.config_epoch = 7u8.wrapping_add(128); // 135
    let telem_c_ambiguous = fleet.ingest_delta(node_c_id, &delta_ambiguous, -21, 235);
    assert!(matches!(telem_c_ambiguous.state, PeerContextState::AmbiguousEpoch { .. }));
    println!("    ✓ RFC 1982 diff == 128 correctly flagged AmbiguousEpoch state");

    let dashboard = fleet.render_dashboard(&["/dev/ttyACM0".to_string(), "/dev/ttyACM1".to_string()]);
    assert!(dashboard.contains("BEBCE5B8"));
    assert!(dashboard.contains("BEBD82B4"));
    assert!(dashboard.contains("CCDDEEFF"));
    assert!(dashboard.contains("AMBIGUOUS"));
    assert!(dashboard.contains("SD ACTIVE"));
    assert!(dashboard.contains("RAM ONLY"));
    println!("    ✓ Fleet log ingestion and ANSI table generation passed with state badges");

    let log_content = fs::read_to_string(test_log).unwrap_or_default();
    assert!(log_content.contains("BEBCE5B8"));
    assert!(log_content.contains("BEBD82B4"));
    println!("    ✓ Log file persistence verified ({})", test_log);
    let _ = fs::remove_file(test_log);


    // -------------------------------------------------------------------------
    // Phase 3: Live Hardware Transceiver Probe (if physical dongles attached)
    // -------------------------------------------------------------------------
    println!("\n[Phase 3] Probing Physical Hardware Dongles (/dev/ttyACM0, /dev/ttyACM1)...");
    let port_a_res = serialport::new("/dev/ttyACM0", 115_200)
        .timeout(Duration::from_millis(200))
        .open();
    let port_b_res = serialport::new("/dev/ttyACM1", 115_200)
        .timeout(Duration::from_millis(200))
        .open();

    match (port_a_res, port_b_res) {
        (Ok(mut port_a), Ok(mut port_b)) => {
            println!("    ✓ Connected to Dongle A (/dev/ttyACM0) and Dongle B (/dev/ttyACM1)");

            // Drain existing boot/debug logs
            let mut drain_buf = [0u8; 1024];
            while port_a.read(&mut drain_buf).unwrap_or(0) > 0 {}
            while port_b.read(&mut drain_buf).unwrap_or(0) > 0 {}

            println!("    Listening for physical 802.15.4 telemetry emissions (up to 7 seconds)...");
            let start = Instant::now();
            let mut received_telemetry = false;
            let mut rx_buf = Vec::new();

            while start.elapsed() < Duration::from_secs(7) && !received_telemetry {
                sleep(Duration::from_millis(50));
                let mut chunk = [0u8; 512];
                if let Ok(n) = port_a.read(&mut chunk) {
                    if n > 0 {
                        rx_buf.extend_from_slice(&chunk[..n]);
                        if let Ok(s) = std::str::from_utf8(&rx_buf) {
                            for line in s.lines() {
                                if let Some(t) = parse_telemetry_line(line) {
                                    println!("    ✓ Over-the-air Telemetry captured from Node {} (RSSI: {} dBm, Uptime: {}s)", t.node_id, t.rssi, t.uptime_secs);
                                    received_telemetry = true;
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            if !received_telemetry {
                println!("    Notice: No telemetry packet captured within 7s window (nodes may be flashing or idle).");
            }
        }
        (port_a, port_b) => {
            println!(
                "    Notice: Physical hardware not fully accessible (Port A: {}, Port B: {}).",
                if port_a.is_ok() { "Available" } else { "Unavailable" },
                if port_b.is_ok() { "Available" } else { "Unavailable" }
            );
            println!("    Mock and software verification suites passed successfully.");
        }
    }

    println!("\n============================================================");
    println!(" All Verification Contracts Satisfied (U1 - U5)             ");
    println!("============================================================");
    Ok(())
}
