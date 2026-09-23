//! Host Integration Network Simulator for Project Gibberish (U1-U6).

use gibberish_crypto::ratchet::{
    decrypt_chunk, derive_network_tag, encrypt_chunk, CryptoError,
};
use gibberish_crypto::secrecy::Secret;
use gibberish_protocol::{
    MeshHeader, MeshPacket, CIPHERTEXT_LEN, FLAG_CLIPBOARD, PLAINTEXT_CHUNK_LEN,
};
use gibberish_storage::fat32_container::{ChunkRecord, StorageError, RECORD_SIZE};
use gibberish_storage::sram_ring::SramRingBuffer;

fn main() {
    println!("============================================================");
    println!(" Running Gibberish End-to-End Mesh & Crypto Simulation Tests ");
    println!("============================================================\n");

    // 1. Swarm Key Setup
    let master_swarm_key = Secret::new([0x33u8; 32]);
    let swarm_tag = derive_network_tag(master_swarm_key.expose_secret());
    println!("[1] Swarm Master Key derived. Tag: 0x{:016X}", swarm_tag);

    // 2. Fragment & Encrypt Secret Payload (Host Node A)
    let secret_plaintext = "CONFIDENTIAL: Gibberish mesh cross-device clipboard sync active!";
    let raw_bytes = secret_plaintext.as_bytes();
    let msg_id = 9901;
    let ratchet_counter = 1;

    let total_chunks = ((raw_bytes.len() + PLAINTEXT_CHUNK_LEN - 1) / PLAINTEXT_CHUNK_LEN) as u8;
    let mut sent_packets = Vec::new();

    for chunk_idx in 0..total_chunks {
        let start = (chunk_idx as usize) * PLAINTEXT_CHUNK_LEN;
        let end = (start + PLAINTEXT_CHUNK_LEN).min(raw_bytes.len());
        let chunk_slice = &raw_bytes[start..end];

        let ciphertext = encrypt_chunk(
            &master_swarm_key,
            msg_id,
            chunk_idx,
            ratchet_counter,
            chunk_slice,
        )
        .expect("Encryption failed");

        let header = MeshHeader {
            network_tag: swarm_tag,
            msg_id,
            chunk_idx,
            total_chunks,
            ttl: 5,
            hop_count: 0,
            flags: FLAG_CLIPBOARD,
        };

        sent_packets.push(MeshPacket {
            header,
            payload: ciphertext,
        });
    }

    println!(
        "[2] Host Node A fragmented and encrypted {} chunks.",
        sent_packets.len()
    );

    // 3. Relay Simulation on Blind Dongle Node B
    println!("[3] Dongle Node B (Blind Relay) ingesting packets into SRAM Ring Buffer...");
    let mut node_b_sram = SramRingBuffer::new();

    for packet in &sent_packets {
        // Dongle verifies network admission tag without knowing encryption key
        assert!(packet.matches_tag(swarm_tag));
        let mut relay_packet = *packet;
        assert!(relay_packet.decrement_ttl());
        assert_eq!(relay_packet.header.ttl, 4);
        assert_eq!(relay_packet.header.hop_count, 1);

        node_b_sram.push(relay_packet);
    }
    assert_eq!(node_b_sram.len(), sent_packets.len());
    println!("    SRAM Ring Buffer holds {} packets.", node_b_sram.len());

    // 4. Power-Loss Atomic Storage Sink Simulation (CHUNKS.BIN)
    println!("[4] Flushing Node B SRAM packets to simulated FAT32 container...");
    let mut recorded_envelopes = Vec::new();
    let mut seq = 100;
    while let Some(pkt) = node_b_sram.pop() {
        seq += 1;
        let record = ChunkRecord {
            sequence: seq,
            ciphertext: pkt.payload,
        };
        let mut enc = [0u8; RECORD_SIZE];
        record.serialize(&mut enc);
        recorded_envelopes.push(enc);
    }
    assert_eq!(recorded_envelopes.len(), sent_packets.len());

    // Verify power-loss record integrity
    for env in &recorded_envelopes {
        let dec = ChunkRecord::deserialize(env).expect("Valid record must deserialize");
        assert_eq!(dec.ciphertext.len(), CIPHERTEXT_LEN);
    }

    // Verify torn write rejection
    let mut torn_env = recorded_envelopes[0];
    torn_env[114] = 0x00; // Zero commit marker
    assert_eq!(
        ChunkRecord::deserialize(&torn_env),
        Err(StorageError::TornWriteDetected)
    );
    println!("    Power-loss torn write rejection verified!");

    // 5. Host Node C Decryption & Reassembly
    println!("[5] Remote Host Node C receiving packets and reassembling...");
    let mut reconstructed_bytes = Vec::new();
    for packet in &sent_packets {
        let dec = decrypt_chunk(
            &master_swarm_key,
            packet.header.msg_id,
            packet.header.chunk_idx,
            ratchet_counter,
            &packet.payload,
        )
        .expect("Decryption must succeed with matching key");
        reconstructed_bytes.extend_from_slice(&dec);
    }

    let reconstructed_str = core::str::from_utf8(&reconstructed_bytes)
        .unwrap()
        .trim_end_matches('\0');

    println!("    Reconstructed text: '{}'", reconstructed_str);
    assert_eq!(reconstructed_str, secret_plaintext);
    println!("    Decrypted payload exactly matches original plaintext!");

    // 6. Rogue Attacker Node Injection Test
    println!("[6] Testing unauthorized rogue attacker injection...");
    let rogue_tag = 0xBAD0BAD0BAD0BAD0;
    let rogue_packet = MeshPacket {
        header: MeshHeader {
            network_tag: rogue_tag,
            msg_id: 666,
            chunk_idx: 0,
            total_chunks: 1,
            ttl: 5,
            hop_count: 0,
            flags: 0,
        },
        payload: [0xFFu8; CIPHERTEXT_LEN],
    };

    assert!(!rogue_packet.matches_tag(swarm_tag));
    println!("    Rogue packet silently dropped at Network Admission Tag gate!");

    // Tampered ciphertext payload
    let mut tampered_packet = sent_packets[0];
    tampered_packet.payload[40] ^= 0x55;
    let decrypt_tampered = decrypt_chunk(
        &master_swarm_key,
        tampered_packet.header.msg_id,
        tampered_packet.header.chunk_idx,
        ratchet_counter,
        &tampered_packet.payload,
    );
    assert_eq!(decrypt_tampered, Err(CryptoError::AuthenticationFailed));
    println!("    Tampered packet rejected by ChaCha20-Poly1305 authentication!");

    println!("\n>>> ALL SIMULATION VERIFICATIONS PASSED SUCCESSFULLY! <<<\n");
}
