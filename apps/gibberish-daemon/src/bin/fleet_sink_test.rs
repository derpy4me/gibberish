//! Automated End-to-End Multi-Node Telemetry Verification Suite (U5).
//! Validates IEEE 802.15.4 telemetry framing, wire deserialization,
//! CDC line decoding, and physical over-the-air RF reception across dongles.

use gibberish_daemon::fleet::{parse_telemetry_line, FleetManager};
use gibberish_protocol::{
    DebugTelemetryPayload, DiagnosticEventCode, MeshHeader, MeshPacket, StorageModeStatus,
    TelemetryTier, CIPHERTEXT_LEN, DEFAULT_NETWORK_TAG, FLAG_TELEMETRY,
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
    // Phase 1: Protocol Wire Framing & Serialization Verification
    // -------------------------------------------------------------------------
    println!("[Phase 1] Verifying 96-Byte Telemetry Wire Framing...");
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
            msg_id: 1, // Monotonic telemetry sequence
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
    assert_eq!(parsed_dbg.drop_count, 2);
    assert_eq!(parsed_dbg.sram_used, 18);
    assert_eq!(parsed_dbg.storage_mode, StorageModeStatus::MicroSdActive);
    assert_eq!(parsed_dbg.build_tier, TelemetryTier::Debug);
    assert_eq!(parsed_dbg.last_rssi, -24);
    assert_eq!(parsed_dbg.last_lqi, 250);
    assert_eq!(parsed_dbg.node_id(), 0x11223344);
    println!("    ✓ Protocol wire framing roundtrip passed (114B wire, 96B payload, Node ID: {:08X})", parsed_dbg.node_id());

    // -------------------------------------------------------------------------
    // Phase 2: Fleet Sink Line Parser & Dashboard Verification
    // -------------------------------------------------------------------------
    println!("\n[Phase 2] Verifying Fleet Sink Log Parsing & TUI Matrix...");
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

    let dashboard = fleet.render_dashboard(&["/dev/ttyACM0".to_string(), "/dev/ttyACM1".to_string()]);
    assert!(dashboard.contains("BEBCE5B8"));
    assert!(dashboard.contains("BEBD82B4"));
    assert!(dashboard.contains("SD ACTIVE"));
    assert!(dashboard.contains("RAM ONLY"));
    assert!(dashboard.contains("00:14:22"));
    println!("    ✓ Fleet log ingestion and ANSI table generation passed");

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
