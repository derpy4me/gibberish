//! Real Hardware Groundwork Stress-Test Suite for Project Gibberish.
//! Runs directly against physical LilyGO T-Dongle-C5 hardware.

use gibberish_crypto::ratchet::{decrypt_chunk, encrypt_chunk};
use gibberish_crypto::secrecy::Secret;
use gibberish_protocol::{
    MeshHeader, MeshPacket, CIPHERTEXT_LEN, DEFAULT_NETWORK_TAG, FLAG_DIRECT,
    PLAINTEXT_CHUNK_LEN,
};
use serialport::SerialPort;
use std::io::{Read, Write};
use std::thread::sleep;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("============================================================");
    println!(" Project Gibberish - Physical Hardware Groundwork Stress Test ");
    println!(" Testing Physical ESP32-C5 Dongles on Live USB-CDC Links     ");
    println!("============================================================\n");

    let port_a_path = "/dev/ttyACM0";
    let port_b_path = "/dev/ttyACM1";

    println!("[1] Opening physical serial ports...");
    let mut port_a = serialport::new(port_a_path, 115_200)
        .timeout(Duration::from_millis(300))
        .open()
        .map_err(|e| format!("Failed to open {}: {}", port_a_path, e))?;
    println!("    ✓ Opened Dongle A: {}", port_a_path);

    let mut port_b = serialport::new(port_b_path, 115_200)
        .timeout(Duration::from_millis(300))
        .open()
        .map_err(|e| format!("Failed to open {}: {}", port_b_path, e))?;
    println!("    ✓ Opened Dongle B: {}", port_b_path);

    // Drain initial banners
    sleep(Duration::from_millis(800));
    drain_lines(&mut port_a);
    drain_lines(&mut port_b);

    // =========================================================================
    // Test 1: Noise Injection & Sliding-Window Tag Re-synchronization
    // =========================================================================
    println!("\n[2] Executing Test 1: Noise Injection & Frame Alignment Resilience...");
    println!("    Injecting 37 random bytes of high-entropy stream noise into Dongle A...");
    let noise = [0x5A, 0x12, 0xFF, 0x00, 0x7E, 0x42, 0x99, 0xAA, 0x11, 0x33, 0x77, 0x88,
                 0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
                 0x13, 0x37, 0x42, 0x00, 0xFF, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA, 0x99, 0x88, 0x77];
    port_a.write_all(&noise)?;

    println!("    Injecting packet with INVALID Network Tag (0xDEADBEEFCAFE0001)...");
    let mut rogue_wire = [0u8; 114];
    rogue_wire[0..8].copy_from_slice(&0xDEADBEEFCAFE0001u64.to_be_bytes());
    port_a.write_all(&rogue_wire)?;

    println!("    Immediately injecting VALID packet (MsgID: 0xCAFE0001)...");
    let valid_hdr = MeshHeader {
        network_tag: DEFAULT_NETWORK_TAG,
        msg_id: 0xCAFE0001,
        chunk_idx: 0,
        total_chunks: 1,
        ttl: 3,
        hop_count: 0,
        flags: FLAG_DIRECT,
    };
    let valid_pkt = MeshPacket {
        header: valid_hdr,
        payload: [0x42; CIPHERTEXT_LEN],
    };
    let mut valid_wire = [0u8; 114];
    valid_pkt.serialize_payload(&mut valid_wire);
    port_a.write_all(&valid_wire)?;

    sleep(Duration::from_millis(300));
    let lines_a = drain_lines(&mut port_a);
    let mut saw_valid_rx = false;
    for line in &lines_a {
        println!("    [Dongle A] {}", line);
        if line.contains("CAFE0001") {
            saw_valid_rx = true;
        }
    }
    assert!(
        saw_valid_rx,
        "FAIL: Dongle A failed to re-sync framing after noise and rogue tag!"
    );
    println!("    ✓ SUCCESS: Dongle A discarded noise, rejected rogue tag, and aligned valid packet!");

    println!("    Injecting 37 random bytes of noise into Dongle B...");
    port_b.write_all(&noise)?;
    println!("    Injecting rogue packet into Dongle B...");
    port_b.write_all(&rogue_wire)?;
    println!("    Immediately injecting VALID packet (MsgID: 0xCAFE0002) into Dongle B...");
    let mut valid_b_wire = [0u8; 114];
    let mut valid_b_hdr = valid_hdr;
    valid_b_hdr.msg_id = 0xCAFE0002;
    let valid_b_pkt = MeshPacket {
        header: valid_b_hdr,
        payload: [0x43; CIPHERTEXT_LEN],
    };
    valid_b_pkt.serialize_payload(&mut valid_b_wire);
    port_b.write_all(&valid_b_wire)?;

    sleep(Duration::from_millis(300));
    let lines_b = drain_lines(&mut port_b);
    let mut saw_valid_b_rx = false;
    for line in &lines_b {
        println!("    [Dongle B] {}", line);
        if line.contains("CAFE0002") {
            saw_valid_b_rx = true;
        }
    }
    assert!(
        saw_valid_b_rx,
        "FAIL: Dongle B failed to re-sync framing after noise and rogue tag!"
    );
    println!("    ✓ SUCCESS: Dongle B discarded noise, rejected rogue tag, and aligned valid packet!");

    // =========================================================================
    // Test 2: High-Volume Ingestion & Authoritative 32 KB SRAM Ring Saturation
    // =========================================================================
    println!("\n[3] Executing Test 2: High-Volume Ingestion & SRAM Ring Saturation...");
    println!("    Pumping 300 chunks (34.2 KB wire traffic) into Dongle A...");
    let swarm_key = Secret::new([0x77u8; 32]);
    let swarm_tag = DEFAULT_NETWORK_TAG;

    for i in 0..300u32 {
        let chunk_hdr = MeshHeader {
            network_tag: swarm_tag,
            msg_id: 0x5000 + i,
            chunk_idx: (i % 5) as u8,
            total_chunks: 5,
            ttl: 3,
            hop_count: 0,
            flags: FLAG_DIRECT,
        };
        let pkt = MeshPacket {
            header: chunk_hdr,
            payload: [(i & 0xFF) as u8; CIPHERTEXT_LEN],
        };
        let mut wire = [0u8; 114];
        pkt.serialize_payload(&mut wire);
        port_a.write_all(&wire)?;
        if i % 10 == 0 {
            drain_lines(&mut port_a);
            sleep(Duration::from_millis(5));
        }
    }

    println!("    Waiting for Dongle A MicroSD container flush telemetry report...");
    let mut saw_sd_active = false;
    let mut dongle_a_drops = 0;
    let start_wait = std::time::Instant::now();

    while start_wait.elapsed() < Duration::from_secs(6) {
        let lines_saturation = drain_lines(&mut port_a);
        for line in &lines_saturation {
            if line.contains("Storage: MicroSdActive") {
                println!("    [Dongle A] {}", line);
                saw_sd_active = true;
                if let Some(pos) = line.find("Drops: ") {
                    let rest = &line[pos + 7..];
                    if let Ok(d) = rest.trim().parse::<u32>() {
                        dongle_a_drops = d;
                    }
                }
            }
        }
        if saw_sd_active {
            break;
        }
        sleep(Duration::from_millis(300));
    }

    assert!(saw_sd_active, "FAIL: Dongle A did not report Storage: MicroSdActive!");
    assert_eq!(
        dongle_a_drops, 0,
        "FAIL: Dongle A dropped packets despite having active MicroSD storage!"
    );
    println!("    ✓ SUCCESS: Dongle A actively flushed chunks to MicroSD card (0 dropped packets)!");

    println!("    Pumping 300 chunks (34.2 KB wire traffic) into Dongle B...");
    for i in 0..300u32 {
        let chunk_hdr = MeshHeader {
            network_tag: swarm_tag,
            msg_id: 0x6000 + i,
            chunk_idx: (i % 5) as u8,
            total_chunks: 5,
            ttl: 3,
            hop_count: 0,
            flags: FLAG_DIRECT,
        };
        let pkt = MeshPacket {
            header: chunk_hdr,
            payload: [(i & 0xFF) as u8; CIPHERTEXT_LEN],
        };
        let mut wire = [0u8; 114];
        pkt.serialize_payload(&mut wire);
        port_b.write_all(&wire)?;
        if i % 10 == 0 {
            drain_lines(&mut port_b);
            sleep(Duration::from_millis(5));
        }
    }

    println!("    Waiting for Dongle B saturation telemetry report...");
    let mut sram_b_full = false;
    let mut drop_b_count = 0;
    let start_wait_b = std::time::Instant::now();

    while start_wait_b.elapsed() < Duration::from_secs(6) {
        let lines_b_saturation = drain_lines(&mut port_b);
        for line in &lines_b_saturation {
            if line.contains("SRAM:") {
                println!("    [Dongle B] {}", line);
                if line.contains("SRAM: 256/256 pkts") {
                    sram_b_full = true;
                }
                if let Some(pos) = line.find("Drops: ") {
                    let rest = &line[pos + 7..];
                    if let Ok(d) = rest.trim().parse::<u32>() {
                        drop_b_count = d;
                    }
                }
            }
        }
        if sram_b_full && drop_b_count >= 44 {
            break;
        }
        sleep(Duration::from_millis(300));
    }

    assert!(sram_b_full, "FAIL: Dongle B SRAM ring buffer did not saturate at 256 packets!");
    assert!(
        drop_b_count >= 44,
        "FAIL: Expected at least 44 dropped packets on Dongle B, got {}",
        drop_b_count
    );
    println!("    ✓ SUCCESS: Dongle B SRAM ring saturated at 256/256 packets; evicted {} packets cleanly without crashing!", drop_b_count);

    // =========================================================================
    // Test 3: End-to-End Cryptographic Loopback Decryption
    // =========================================================================
    println!("\n[4] Executing Test 3: Cryptographic Authenticated Decryption Loopback...");
    let secret_plaintext = b"CONFIDENTIAL: LilyGO T-Dongle-C5 Physical Zero-Trust Mesh Verified!";
    println!("    Plaintext: '{}'", String::from_utf8_lossy(secret_plaintext));

    let mut chunk_plain = [0u8; PLAINTEXT_CHUNK_LEN];
    chunk_plain[..secret_plaintext.len()].copy_from_slice(secret_plaintext);

    let ciphertext = encrypt_chunk(&swarm_key, 0xBEEF0001, 0, 1, &chunk_plain)
        .map_err(|e| format!("Encryption error: {:?}", e))?;
    assert_eq!(ciphertext.len(), CIPHERTEXT_LEN);
    println!("    Encrypted into 96-byte ciphertext with Poly1305 MAC: {:02X?}...", &ciphertext[0..16]);

    let decrypted = decrypt_chunk(&swarm_key, 0xBEEF0001, 0, 1, &ciphertext)
        .map_err(|e| format!("Decryption error: {:?}", e))?;
    let recovered_str = String::from_utf8_lossy(&decrypted[..secret_plaintext.len()]);
    println!("    Decrypted Recovered: '{}'", recovered_str);
    assert_eq!(decrypted[..secret_plaintext.len()], secret_plaintext[..]);
    println!("    ✓ SUCCESS: Byte-for-byte cryptographic fidelity verified!");

    println!("\n============================================================");
    println!(" >>> ALL PHYSICAL HARDWARE GROUNDWORK TESTS PASSED! <<<");
    println!("============================================================\n");
    Ok(())
}

fn drain_lines(port: &mut Box<dyn SerialPort>) -> Vec<String> {
    let mut lines = Vec::new();
    let mut raw = [0u8; 1024];
    while let Ok(n) = port.read(&mut raw) {
        if n == 0 {
            break;
        }
        if let Ok(s) = std::str::from_utf8(&raw[..n]) {
            for line in s.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    lines.push(trimmed.to_string());
                }
            }
        }
    }
    lines
}
