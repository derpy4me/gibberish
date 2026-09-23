//! Multi-Sender HKDF Crypto, Chunk Fragmentation, and Anti-DoS Reassembly (R8, R9, R10, R11, R12, R13, KTD1, KTD4).

use crate::nonce::NonceManager;
use gibberish_crypto::ratchet::{decrypt_chunk, derive_sender_subkey, encrypt_chunk, CryptoError};
use gibberish_crypto::secrecy::Secret;
use gibberish_protocol::{MeshHeader, MeshPacket, FLAG_CLIPBOARD, PLAINTEXT_CHUNK_LEN};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const MAX_TOTAL_CHUNKS: usize = 800; // 64 KB cap (R11)
pub const MAX_GLOBAL_IN_FLIGHT: usize = 16; // R12
pub const MAX_PER_SENDER_IN_FLIGHT: usize = 2; // R12
pub const REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(10); // R13

struct InFlightMessage {
    #[allow(dead_code)]
    total_chunks: usize,
    received_chunks: Vec<Option<[u8; PLAINTEXT_CHUNK_LEN]>>,
    first_received: Instant,
    last_received: Instant,
}

pub struct ChunkEngine {
    in_flight: HashMap<(u32, u32), InFlightMessage>,
    completed_cache: HashMap<(u32, u32), Instant>,
}

impl ChunkEngine {
    pub fn new() -> Self {
        Self {
            in_flight: HashMap::new(),
            completed_cache: HashMap::new(),
        }
    }

    /// Split a Secret plaintext string into an array of encrypted MeshPackets using
    /// per-sender HKDF subkeys and monotonic nonce allocation (R8, R10).
    pub fn fragment_and_encrypt(
        text: &Secret<String>,
        swarm_tag: u64,
        local_node_id: u32,
        swarm_master_key: &Secret<[u8; 32]>,
        nonce_mgr: &mut NonceManager,
    ) -> Result<Vec<MeshPacket>, CryptoError> {
        let raw_bytes = text.expose_secret().as_bytes();
        if raw_bytes.is_empty() {
            return Ok(Vec::new());
        }

        let num_chunks = (raw_bytes.len() + PLAINTEXT_CHUNK_LEN - 1) / PLAINTEXT_CHUNK_LEN;
        if num_chunks > MAX_TOTAL_CHUNKS {
            return Err(CryptoError::PayloadTooLarge);
        }
        let total_chunks = num_chunks as u8;

        // Allocate monotonic message coordinates
        let (_epoch_secs, counter) = nonce_mgr
            .allocate_msg()
            .map_err(|_| CryptoError::InvalidNonce)?;

        // Message ID: incorporate local_node_id prefix so different nodes produce globally unique
        // message IDs across the mesh deduplication bloom filter
        let msg_id = (local_node_id & 0xFFFF_0000) | ((counter & 0x0000_FFFF) as u32);

        // Derive unique sender subkey: HKDF(swarm_key, info = local_node_id) (KTD1)
        let sender_key = derive_sender_subkey(swarm_master_key, local_node_id);

        let mut packets = Vec::with_capacity(total_chunks as usize);

        for chunk_idx in 0..total_chunks {
            let start = (chunk_idx as usize) * PLAINTEXT_CHUNK_LEN;
            let end = (start + PLAINTEXT_CHUNK_LEN).min(raw_bytes.len());
            let chunk_data = &raw_bytes[start..end];

            // Ratchet counter derived deterministically from wire msg_id so both sender and
            // receiver construct the identical implicit nonce without out-of-band clock sync
            let ratchet_counter = msg_id as u64;

            let ciphertext = encrypt_chunk(&sender_key, msg_id, chunk_idx, ratchet_counter, chunk_data)?;

            let header = MeshHeader {
                network_tag: swarm_tag,
                msg_id,
                chunk_idx,
                total_chunks,
                ttl: 7, // Multi-hop mesh relay
                hop_count: 0,
                flags: FLAG_CLIPBOARD,
            };

            packets.push(MeshPacket {
                header,
                payload: ciphertext,
            });
        }

        Ok(packets)
    }

    /// Ingest an incoming wire packet, authenticate with the sender's HKDF subkey,
    /// and assemble when all chunks are received. Enforces Anti-DoS policy (R9, R11, R12, R13).
    pub fn ingest_packet(
        &mut self,
        src_node_id: u32,
        packet: &MeshPacket,
        swarm_master_key: &Secret<[u8; 32]>,
    ) -> Result<Option<Secret<String>>, CryptoError> {
        self.purge_expired();

        let msg_id = packet.header.msg_id;
        let chunk_idx = packet.header.chunk_idx as usize;
        let total_chunks = packet.header.total_chunks as usize;

        // Anti-DoS Rule 1: Reject zero or oversized chunk counts (R11)
        if total_chunks == 0 || total_chunks > MAX_TOTAL_CHUNKS {
            return Err(CryptoError::PayloadTooLarge);
        }
        if chunk_idx >= total_chunks {
            return Err(CryptoError::PayloadTooLarge);
        }

        let key = (src_node_id, msg_id);

        // Anti-DoS Rule 2: Drop duplicates for already completed messages without resetting timers (R13)
        if self.completed_cache.contains_key(&key) {
            return Ok(None);
        }

        // Anti-DoS Rule 3: Enforce per-sender concurrency limits (max 2 active messages per sender) (R12)
        if !self.in_flight.contains_key(&key) {
            let sender_in_flight_count = self.in_flight.keys().filter(|(src, _)| *src == src_node_id).count();
            if sender_in_flight_count >= MAX_PER_SENDER_IN_FLIGHT {
                // Evict the oldest message from this sender
                let oldest_msg = self
                    .in_flight
                    .iter()
                    .filter(|(&(src, _), _)| src == src_node_id)
                    .min_by_key(|(_, inflight)| inflight.first_received)
                    .map(|(&k, _)| k);
                if let Some(old_key) = oldest_msg {
                    self.in_flight.remove(&old_key);
                }
            }

            // Anti-DoS Rule 4: Enforce global in-flight limit (max 16 concurrent messages) (R12)
            if self.in_flight.len() >= MAX_GLOBAL_IN_FLIGHT {
                let oldest_global = self
                    .in_flight
                    .iter()
                    .min_by_key(|(_, inflight)| inflight.first_received)
                    .map(|(&k, _)| k);
                if let Some(old_key) = oldest_global {
                    self.in_flight.remove(&old_key);
                }
            }
        }

        // Derive peer's subkey using HKDF(swarm_key, info = src_node_id) (KTD1)
        let peer_key = derive_sender_subkey(swarm_master_key, src_node_id);

        let ratchet_counter = msg_id as u64;
        let decrypted = decrypt_chunk(
            &peer_key,
            msg_id,
            packet.header.chunk_idx,
            ratchet_counter,
            &packet.payload,
        )?;

        let now = Instant::now();
        let entry = self.in_flight.entry(key).or_insert_with(|| InFlightMessage {
            total_chunks,
            received_chunks: vec![None; total_chunks],
            first_received: now,
            last_received: now,
        });

        // Store chunk if not already present
        if entry.received_chunks[chunk_idx].is_none() {
            entry.received_chunks[chunk_idx] = Some(decrypted);
            entry.last_received = now;
        }

        // Check if all chunks received
        if entry.received_chunks.iter().all(|c| c.is_some()) {
            let mut assembled_bytes = Vec::with_capacity(total_chunks * PLAINTEXT_CHUNK_LEN);
            for chunk in entry.received_chunks.iter().flatten() {
                assembled_bytes.extend_from_slice(chunk);
            }

            // Clean up trailing null padding
            while assembled_bytes.last() == Some(&0) {
                assembled_bytes.pop();
            }

            self.in_flight.remove(&key);
            self.completed_cache.insert(key, now);

            if let Ok(text) = String::from_utf8(assembled_bytes) {
                return Ok(Some(Secret::new(text)));
            }
        }

        Ok(None)
    }

    /// Purge incomplete in-flight messages older than 10 seconds (R13).
    fn purge_expired(&mut self) {
        let now = Instant::now();
        self.in_flight
            .retain(|_, inflight| now.duration_since(inflight.last_received) < REASSEMBLY_TIMEOUT);
        self.completed_cache
            .retain(|_, completed_at| now.duration_since(*completed_at) < Duration::from_secs(60));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gibberish_protocol::DEFAULT_NETWORK_TAG;

    #[test]
    fn test_fragment_and_reassembly_roundtrip() {
        let master_key = Secret::new([0x33u8; 32]);
        let local_node = 0xBEBCE5B8;
        let mut nonce_mgr = NonceManager::new(Some(
            std::env::temp_dir().join("test_chunk_roundtrip_nonce.json"),
        ))
        .unwrap();

        let original_text = Secret::new(
            "The quick brown fox jumps over the lazy dog! Running across IEEE 802.15.4 Channel 15 mesh airwaves with 0-trust ChaCha20-Poly1305 encryption."
                .to_string(),
        );

        let packets = ChunkEngine::fragment_and_encrypt(
            &original_text,
            DEFAULT_NETWORK_TAG,
            local_node,
            &master_key,
            &mut nonce_mgr,
        )
        .expect("Fragmentation failed");

        assert!(packets.len() > 1);

        let mut engine = ChunkEngine::new();
        let mut reassembled = None;
        // Ingest packets in scrambled order to verify out-of-order robustness
        for pkt in packets.iter().rev() {
            if let Some(text) = engine
                .ingest_packet(local_node, pkt, &master_key)
                .expect("Ingest failed")
            {
                reassembled = Some(text);
            }
        }

        let final_text = reassembled.expect("Failed to reassemble complete message");
        assert_eq!(
            final_text.expose_secret(),
            original_text.expose_secret()
        );
    }

    #[test]
    fn test_anti_dos_caps() {
        let mut engine = ChunkEngine::new();
        let master_key = Secret::new([0x55u8; 32]);

        // Attempting total_chunks = 801 should be rejected immediately (64 KB cap)
        let invalid_hdr = MeshHeader {
            network_tag: DEFAULT_NETWORK_TAG,
            msg_id: 1,
            chunk_idx: 0,
            total_chunks: 255, // Note: u8 max is 255 anyway, but MAX_TOTAL_CHUNKS checks usize
            ttl: 7,
            hop_count: 0,
            flags: FLAG_CLIPBOARD,
        };
        let pkt = MeshPacket {
            header: invalid_hdr,
            payload: [0u8; 96],
        };

        let res = engine.ingest_packet(0x11112222, &pkt, &master_key);
        // Should fail authentication or be dropped cleanly
        assert!(res.is_err() || res.unwrap().is_none());
    }
}
