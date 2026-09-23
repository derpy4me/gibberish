//! Bi-directional IEEE 802.15.4 Over-The-Air (OTA) RF Mesh Test.
//! Verifies Milestone 6: Asymmetric Framing, HKDF Subkeys, and Cross-Node Sync over 2.4 GHz RF Airwaves.

use gibberish_crypto::ratchet::derive_network_tag;
use gibberish_crypto::secrecy::Secret;
use gibberish_daemon::chunk::ChunkEngine;
use gibberish_daemon::nonce::NonceManager;
use gibberish_daemon::transport::SerialTransport;
use std::thread::sleep;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("============================================================");
    println!(" Project Gibberish - Physical IEEE 802.15.4 OTA Sync Test   ");
    println!(" Verifying Bi-directional 2.4 GHz RF Over-the-Air Transmit  ");
    println!(" With Asymmetric Framing, HKDF Subkeys, & Chunk Reassembly  ");
    println!("============================================================\n");

    let port_a_path = "/dev/ttyACM1"; // Dongle A: Node BEBCE5B8 (SD Node)
    let port_b_path = "/dev/ttyACM0"; // Dongle B: Node BEBD82B4 (RAM Node)

    let node_a_id = 0xBEBCE5B8u32;
    let node_b_id = 0xBEBD82B4u32;

    let swarm_master_key = Secret::new([0x55u8; 32]);
    let swarm_tag = derive_network_tag(swarm_master_key.expose_secret());

    println!("[1] Connecting to physical hardware dongles...");
    let mut transport_a = SerialTransport::open(port_a_path)
        .map_err(|e| format!("Failed to open Dongle A on {}: {}", port_a_path, e))?;
    println!("    ✓ Connected to Dongle A: {}", port_a_path);

    let mut transport_b = SerialTransport::open(port_b_path)
        .map_err(|e| format!("Failed to open Dongle B on {}: {}", port_b_path, e))?;
    println!("    ✓ Connected to Dongle B: {}", port_b_path);

    let mut nonce_mgr_a = NonceManager::new(Some(
        std::env::temp_dir().join("gibberish_ota_test_nonce_a.json"),
    ))?;
    let mut nonce_mgr_b = NonceManager::new(Some(
        std::env::temp_dir().join("gibberish_ota_test_nonce_b.json"),
    ))?;

    let mut engine_a = ChunkEngine::new();
    let mut engine_b = ChunkEngine::new();

    // Drain initial startup logs
    sleep(Duration::from_millis(500));
    let _ = transport_a.poll_stream();
    let _ = transport_b.poll_stream();

    // =========================================================================
    // Test 1: Dongle A (TX) -> 802.15.4 RF Airwaves -> Dongle B (RX)
    // =========================================================================
    println!("\n[2] Executing Test 1: Dongle A (TX) -> 2.425 GHz Ch 15 RF -> Dongle B (RX)...");
    let test_msg_a = Secret::new(
        "Gibberish-M6-Test: Encrypted clipboard packet from Node A to Node B over raw 802.15.4 airwaves!"
            .to_string(),
    );

    let packets_a = ChunkEngine::fragment_and_encrypt(
        &test_msg_a,
        swarm_tag,
        node_a_id,
        &swarm_master_key,
        &mut nonce_mgr_a,
    )
    .map_err(|e| format!("{:?}", e))?;

    println!(
        "    Fragmented message into {} ciphertext chunk(s). Injecting via [0xAA, 0x55, 0x72, <wire>] framing...",
        packets_a.len()
    );

    for pkt in &packets_a {
        transport_a.send_packet(pkt)?;
        sleep(Duration::from_millis(30));
    }

    println!("    Listening on Dongle B for Over-The-Air RF packet reception and #PKT# framing...");
    let start_a = Instant::now();
    let mut received_on_b: Option<Secret<String>> = None;

    while start_a.elapsed() < Duration::from_secs(6) {
        let (packets, logs) = transport_b.poll_stream();
        for log in logs {
            println!("    [Dongle B Log] {}", log);
        }

        for wire_pkt in packets {
            println!(
                "    [Dongle B Wire RX] Received #PKT# from 0x{:08X}, MsgID: {:08X}, Chunk: {}/{}",
                wire_pkt.src_node_id,
                wire_pkt.packet.header.msg_id,
                wire_pkt.packet.header.chunk_idx,
                wire_pkt.packet.header.total_chunks
            );

            if let Some(reassembled) = engine_b
                .ingest_packet(wire_pkt.src_node_id, &wire_pkt.packet, &swarm_master_key)
                .map_err(|e| format!("{:?}", e))?
            {
                received_on_b = Some(reassembled);
                break;
            }
        }

        if received_on_b.is_some() {
            break;
        }
        sleep(Duration::from_millis(50));
    }

    let result_b = received_on_b.expect("FAIL: Dongle B did not receive or reassemble packet from Dongle A!");
    assert_eq!(result_b.expose_secret(), test_msg_a.expose_secret());
    println!("    ✓ SUCCESS: Dongle B decrypted and reassembled: \"{}\"", result_b.expose_secret());

    // =========================================================================
    // Test 2: Dongle B (TX) -> 802.15.4 RF Airwaves -> Dongle A (RX)
    // =========================================================================
    println!("\n[3] Executing Test 2: Dongle B (TX) -> 2.425 GHz Ch 15 RF -> Dongle A (RX)...");
    let test_msg_b = Secret::new(
        "Gibberish-M6-Reply: Acknowledged! Node B transmitting back to Node A across RF airwaves!"
            .to_string(),
    );

    let packets_b = ChunkEngine::fragment_and_encrypt(
        &test_msg_b,
        swarm_tag,
        node_b_id,
        &swarm_master_key,
        &mut nonce_mgr_b,
    )
    .map_err(|e| format!("{:?}", e))?;

    println!(
        "    Fragmented reply into {} ciphertext chunk(s). Injecting into Dongle B...",
        packets_b.len()
    );

    for pkt in &packets_b {
        transport_b.send_packet(pkt)?;
        sleep(Duration::from_millis(30));
    }

    println!("    Listening on Dongle A for Over-The-Air RF packet reception and #PKT# framing...");
    let start_b = Instant::now();
    let mut received_on_a: Option<Secret<String>> = None;

    while start_b.elapsed() < Duration::from_secs(6) {
        let (packets, logs) = transport_a.poll_stream();
        for log in logs {
            println!("    [Dongle A Log] {}", log);
        }

        for wire_pkt in packets {
            println!(
                "    [Dongle A Wire RX] Received #PKT# from 0x{:08X}, MsgID: {:08X}, Chunk: {}/{}",
                wire_pkt.src_node_id,
                wire_pkt.packet.header.msg_id,
                wire_pkt.packet.header.chunk_idx,
                wire_pkt.packet.header.total_chunks
            );

            if let Some(reassembled) = engine_a
                .ingest_packet(wire_pkt.src_node_id, &wire_pkt.packet, &swarm_master_key)
                .map_err(|e| format!("{:?}", e))?
            {
                received_on_a = Some(reassembled);
                break;
            }
        }

        if received_on_a.is_some() {
            break;
        }
        sleep(Duration::from_millis(50));
    }

    let result_a = received_on_a.expect("FAIL: Dongle A did not receive or reassemble packet from Dongle B!");
    assert_eq!(result_a.expose_secret(), test_msg_b.expose_secret());
    println!("    ✓ SUCCESS: Dongle A decrypted and reassembled: \"{}\"", result_a.expose_secret());

    println!("\n============================================================");
    println!(" >>> PHYSICAL OVER-THE-AIR RF MESH SYNC FULLY VERIFIED! <<<");
    println!("============================================================\n");

    Ok(())
}
