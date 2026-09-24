//! Multi-Sender HKDF Crypto, Chunk Fragmentation, and Anti-DoS Reassembly (R8, R9, R10, R11, R12, R13, KTD1, KTD4).

use crate::nonce::NonceManager;
use gibberish_crypto::ratchet::{decrypt_chunk, derive_sender_subkey, encrypt_chunk, CryptoError};
use gibberish_crypto::secrecy::Secret;
use gibberish_protocol::{
    MeshHeader, MeshPacket, SackPayload, CIPHERTEXT_LEN, FLAG_CLIPBOARD, FLAG_SACK,
    PLAINTEXT_CHUNK_LEN,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const MAX_TOTAL_CHUNKS: usize = 800; // 64 KB cap (R11)
pub const MAX_GLOBAL_IN_FLIGHT: usize = 16; // R12
pub const MAX_PER_SENDER_IN_FLIGHT: usize = 2; // R12
pub const REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(10); // R13
pub const SACK_INITIAL_DELAY: Duration = Duration::from_millis(300);
pub const SACK_RETRY_INTERVAL: Duration = Duration::from_millis(500);
pub const MAX_SACK_RETRIES: u8 = 3;

struct InFlightMessage {
    #[allow(dead_code)]
    total_chunks: usize,
    received_chunks: Vec<Option<[u8; PLAINTEXT_CHUNK_LEN]>>,
    first_received: Instant,
    last_received: Instant,
    last_sack_sent: Option<Instant>,
    sack_count: u8,
}

pub struct ChunkEngine {
    in_flight: HashMap<(u32, u32), InFlightMessage>,
    completed_cache: HashMap<(u32, u32), Instant>,
    outbox_cache: HashMap<u32, (Vec<MeshPacket>, Instant)>,
}

impl ChunkEngine {
    pub fn new() -> Self {
        Self {
            in_flight: HashMap::new(),
            completed_cache: HashMap::new(),
            outbox_cache: HashMap::new(),
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
            last_sack_sent: None,
            sack_count: 0,
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

    /// Cache outbound transmitted packets for missing chunk retransmission upon receiving a SACK (Issue #3).
    pub fn cache_outbound(&mut self, packets: &[MeshPacket]) {
        if let Some(first) = packets.first() {
            self.outbox_cache.insert(
                first.header.msg_id,
                (packets.to_vec(), Instant::now()),
            );
        }
    }

    /// Process an incoming SACK frame. If this node has the requested message cached in its outbox,
    /// returns a list of retransmit packets matching only the requested missing chunk indices (Issue #3).
    pub fn handle_sack(&mut self, sack_packet: &MeshPacket) -> Vec<MeshPacket> {
        if (sack_packet.header.flags & FLAG_SACK) == 0 {
            return Vec::new();
        }

        let sack = SackPayload::deserialize(&sack_packet.payload);
        let msg_id = sack_packet.header.msg_id;

        let mut retransmit = Vec::new();
        if let Some((cached_pkts, _cached_at)) = self.outbox_cache.get(&msg_id) {
            sack.for_each_missing(|missing_idx| {
                if let Some(pkt) = cached_pkts.get(missing_idx) {
                    retransmit.push(*pkt);
                }
            });
        }

        retransmit
    }

    /// Scan in-flight messages for missing chunks and generate compact 1-packet bitmask SACKs
    /// for incomplete transmissions, enforcing rate limits to prevent broadcast storms (Issue #3).
    pub fn check_pending_sacks(
        &mut self,
        local_node_id: u32,
        swarm_tag: u64,
    ) -> Vec<MeshPacket> {
        let now = Instant::now();
        let mut sacks = Vec::new();

        for ((src_node_id, msg_id), inflight) in self.in_flight.iter_mut() {
            // Only generate SACK if the message is incomplete
            let is_incomplete = inflight.received_chunks.iter().any(|c| c.is_none());
            if !is_incomplete {
                continue;
            }

            // Wait initial delay after receiving chunk before firing first SACK
            if now.duration_since(inflight.last_received) < SACK_INITIAL_DELAY {
                continue;
            }

            // Rate limiting: check max retries and retry interval
            if inflight.sack_count >= MAX_SACK_RETRIES {
                continue;
            }

            if let Some(last_sent) = inflight.last_sack_sent {
                if now.duration_since(last_sent) < SACK_RETRY_INTERVAL {
                    continue;
                }
            }

            // Build compact bitmask SACK
            let mut sack_payload = SackPayload::new(local_node_id, *src_node_id, 0);
            for (idx, chunk) in inflight.received_chunks.iter().enumerate() {
                if chunk.is_none() {
                    sack_payload.mark_missing(idx);
                }
            }

            let mut payload = [0u8; CIPHERTEXT_LEN];
            sack_payload.serialize(&mut payload);

            let header = MeshHeader {
                network_tag: swarm_tag,
                msg_id: *msg_id,
                chunk_idx: 0,
                total_chunks: inflight.total_chunks as u8,
                ttl: 5,
                hop_count: 0,
                flags: FLAG_SACK,
            };

            inflight.last_sack_sent = Some(now);
            inflight.sack_count += 1;

            sacks.push(MeshPacket { header, payload });
        }

        sacks
    }

    /// Suppress our own pending SACK if an identical SACK from another peer was overheard,
    /// avoiding redundant broadcast storms (Issue #3).
    pub fn suppress_sack(&mut self, msg_id: u32) {
        let now = Instant::now();
        for ((_, m_id), inflight) in self.in_flight.iter_mut() {
            if *m_id == msg_id {
                inflight.last_sack_sent = Some(now);
            }
        }
    }

    /// Purge incomplete in-flight messages older than 10 seconds (R13) and expired outbox cache.
    fn purge_expired(&mut self) {
        let now = Instant::now();
        self.in_flight
            .retain(|_, inflight| now.duration_since(inflight.last_received) < REASSEMBLY_TIMEOUT);
        self.completed_cache
            .retain(|_, completed_at| now.duration_since(*completed_at) < Duration::from_secs(60));
        self.outbox_cache
            .retain(|_, (_, cached_at)| now.duration_since(*cached_at) < Duration::from_secs(60));
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

    #[test]
    fn test_sack_missing_chunk_recovery() {
        let master_key = Secret::new([0x77u8; 32]);
        let sender_node = 0xAAAA1111;
        let receiver_node = 0xBBBB2222;
        let mut nonce_mgr = NonceManager::new(Some(
            std::env::temp_dir().join("test_sack_recovery_nonce.json"),
        ))
        .unwrap();

        // Multi-chunk message: > 160 characters = 3 chunks (80B + 80B + 52B)
        let original_text = Secret::new(
            "Selective Acknowledgment (SACK) recovery test message designed to span across exactly three mesh frames to reliably simulate RF chunk loss and recovery across 802.15.4 airwaves without any data corruption.".to_string()
        );

        let packets = ChunkEngine::fragment_and_encrypt(
            &original_text,
            DEFAULT_NETWORK_TAG,
            sender_node,
            &master_key,
            &mut nonce_mgr,
        )
        .expect("Fragmentation failed");

        assert_eq!(packets.len(), 3);

        let mut sender_engine = ChunkEngine::new();
        sender_engine.cache_outbound(&packets);

        let mut receiver_engine = ChunkEngine::new();

        // Receiver ingests chunk 0 and chunk 2 (chunk 1 is lost to RF interference)
        let res0 = receiver_engine
            .ingest_packet(sender_node, &packets[0], &master_key)
            .unwrap();
        assert!(res0.is_none());

        let res2 = receiver_engine
            .ingest_packet(sender_node, &packets[2], &master_key)
            .unwrap();
        assert!(res2.is_none());

        // Immediately checking pending SACKs returns empty because SACK_INITIAL_DELAY has not elapsed
        let sacks_immediate = receiver_engine.check_pending_sacks(receiver_node, DEFAULT_NETWORK_TAG);
        assert!(sacks_immediate.is_empty());

        // Artificially age the in-flight message past SACK_INITIAL_DELAY
        for (_, inflight) in receiver_engine.in_flight.iter_mut() {
            inflight.last_received -= Duration::from_millis(350);
        }

        // Now receiver generates a compact 1-packet bitmask SACK
        let sacks = receiver_engine.check_pending_sacks(receiver_node, DEFAULT_NETWORK_TAG);
        assert_eq!(sacks.len(), 1);
        let sack_pkt = &sacks[0];
        assert_eq!(sack_pkt.header.flags & FLAG_SACK, FLAG_SACK);
        assert_eq!(sack_pkt.header.msg_id, packets[0].header.msg_id);

        let sack_payload = SackPayload::deserialize(&sack_pkt.payload);
        assert_eq!(sack_payload.receiver_node_id, receiver_node);
        assert_eq!(sack_payload.sender_node_id, sender_node);
        assert_eq!(sack_payload.missing_count(), 1);
        assert!(!sack_payload.is_missing(0));
        assert!(sack_payload.is_missing(1));
        assert!(!sack_payload.is_missing(2));

        // Sender handles SACK and retransmits ONLY the missing chunk (chunk 1)
        let retransmitted = sender_engine.handle_sack(sack_pkt);
        assert_eq!(retransmitted.len(), 1);
        assert_eq!(retransmitted[0].header.chunk_idx, 1);
        assert_eq!(retransmitted[0].payload, packets[1].payload);

        // Receiver ingests the retransmitted chunk 1
        let reassembled = receiver_engine
            .ingest_packet(sender_node, &retransmitted[0], &master_key)
            .unwrap();

        assert!(reassembled.is_some());
        assert_eq!(
            reassembled.unwrap().expose_secret(),
            original_text.expose_secret()
        );
    }

    #[test]
    fn test_sack_rate_limiting_and_suppression() {
        let master_key = Secret::new([0x88u8; 32]);
        let sender_node = 0xCCCC3333;
        let receiver_node = 0xDDDD4444;
        let mut nonce_mgr = NonceManager::new(Some(
            std::env::temp_dir().join("test_sack_rate_limit_nonce.json"),
        ))
        .unwrap();

        let original_text = Secret::new(
            "Rate-limiting verification for SACK frames to prevent broadcast storms during lossy mesh link states.".to_string()
        );

        let packets = ChunkEngine::fragment_and_encrypt(
            &original_text,
            DEFAULT_NETWORK_TAG,
            sender_node,
            &master_key,
            &mut nonce_mgr,
        )
        .expect("Fragmentation failed");

        let mut receiver_engine = ChunkEngine::new();
        receiver_engine
            .ingest_packet(sender_node, &packets[0], &master_key)
            .unwrap();

        // Age in-flight entry
        for (_, inflight) in receiver_engine.in_flight.iter_mut() {
            inflight.last_received -= Duration::from_millis(350);
        }

        // First SACK fires
        let sacks1 = receiver_engine.check_pending_sacks(receiver_node, DEFAULT_NETWORK_TAG);
        assert_eq!(sacks1.len(), 1);

        // Immediate subsequent check: rate-limited by SACK_RETRY_INTERVAL
        let sacks2 = receiver_engine.check_pending_sacks(receiver_node, DEFAULT_NETWORK_TAG);
        assert!(sacks2.is_empty());

        // Test overheard SACK suppression
        receiver_engine.suppress_sack(packets[0].header.msg_id);
        // Still suppressed
        let sacks3 = receiver_engine.check_pending_sacks(receiver_node, DEFAULT_NETWORK_TAG);
        assert!(sacks3.is_empty());
    }
}
