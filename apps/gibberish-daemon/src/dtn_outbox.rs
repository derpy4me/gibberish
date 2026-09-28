//! Hybrid Asymmetric Delivery, Beacon-Triggered DTN Outbox & TTL Eviction (R13, R14, R15, KTD4, KTD5).

use gibberish_db::{DatabaseStore, DbError, OutboxRecord, OutboxStatus};
use gibberish_protocol::{FLAG_ACK_REQ, FLAG_DIRECT, FLAG_GROUP};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

pub const DEFAULT_OUTBOX_TTL_SECS: u64 = 48 * 3600; // 48 hours = 172,800s
pub const PEER_RECENT_ACTIVITY_WINDOW: Duration = Duration::from_secs(60);

/// Asymmetric delivery policy gatekeeper (R10, R13, KTD4).
pub struct AsymmetricPolicy;

impl AsymmetricPolicy {
    /// Validates outbound frame flags against asymmetric transport rules.
    /// Strictly forbids FLAG_ACK_REQ on broadcast frames (FLAG_GROUP).
    pub fn validate_outbound_flags(flags: u16) -> Result<(), &'static str> {
        if (flags & FLAG_GROUP) != 0 && (flags & FLAG_ACK_REQ) != 0 {
            return Err("FLAG_ACK_REQ is forbidden on swarm broadcast frames");
        }
        Ok(())
    }

    /// Determines whether an inbound frame should trigger an ACK frame across the mesh.
    /// Broadcasts (#all) NEVER trigger ACKs, preventing broadcast implosion.
    pub fn should_emit_ack(flags: u16) -> bool {
        // Group broadcasts never emit ACKs regardless of other bits
        if (flags & FLAG_GROUP) != 0 {
            return false;
        }

        // Direct messages only emit ACKs if FLAG_ACK_REQ is explicitly requested
        (flags & FLAG_DIRECT) != 0 && (flags & FLAG_ACK_REQ) != 0
    }
}

/// Opportunistic Beacon-Triggered DTN Outbox Engine (R14, R15, KTD5).
pub struct DtnOutboxEngine {
    db: DatabaseStore,
    peer_last_seen: HashMap<u32, Instant>,
    dedup: IngestDeduplicator,
}

impl DtnOutboxEngine {
    pub fn new(db: DatabaseStore) -> Self {
        Self {
            db,
            peer_last_seen: HashMap::new(),
            dedup: IngestDeduplicator::new(Duration::from_secs(600)),
        }
    }

    pub fn db(&self) -> &DatabaseStore {
        &self.db
    }

    pub fn deduplicator_mut(&mut self) -> &mut IngestDeduplicator {
        &mut self.dedup
    }

    /// Records overhearing a peer announcement beacon or packet, updating peer recency.
    pub fn record_peer_heard(&mut self, node_id: u32) {
        self.peer_last_seen.insert(node_id, Instant::now());
    }

    /// Checks if a peer was heard within the recent activity window.
    pub fn is_peer_recently_heard(&self, node_id: u32) -> bool {
        if let Some(last_seen) = self.peer_last_seen.get(&node_id) {
            last_seen.elapsed() < PEER_RECENT_ACTIVITY_WINDOW
        } else {
            false
        }
    }

    /// Queues a 1-to-1 direct message into the persistent DTN outbox.
    pub fn queue_message(
        &self,
        id: &str,
        dest_node_id: u32,
        payload: Vec<u8>,
        ttl_secs: u64,
        queued_at: i64,
    ) -> Result<OutboxRecord, DbError> {
        let record = OutboxRecord {
            id: id.to_string(),
            dest_node_id,
            payload,
            queued_at,
            retry_count: 0,
            ttl_secs,
            status: OutboxStatus::Pending,
        };
        self.db.insert_outbox(&record)?;
        Ok(record)
    }

    /// Triggers opportunistic burst flush when an announcement beacon from a peer is overheard (F2).
    /// Retrieves all pending outbox records for that peer, updates their status to `Sending`,
    /// and returns them for transmission over the mesh radio.
    pub fn on_peer_beacon_overheard(&mut self, dest_node_id: u32) -> Result<Vec<OutboxRecord>, DbError> {
        self.record_peer_heard(dest_node_id);

        let pending = self.db.list_pending_outbox_for_node(dest_node_id)?;
        if pending.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<&str> = pending.iter().map(|r| r.id.as_str()).collect();
        self.db.mark_outbox_sending(&ids)?;

        Ok(pending)
    }

    /// Matches an incoming SACK / delivery receipt, updating outbox and message status to Delivered (F2).
    pub fn on_delivery_ack_received(&self, msg_id: &str) -> Result<bool, DbError> {
        self.db.mark_outbox_sent(&[msg_id])?;
        Ok(true)
    }

    /// Enforces the 48-hour TTL eviction policy on the DTN outbox ring (AE3, R15).
    /// Evicts expired pending or sending messages, transitioning them to Failed in both outbox and messages.
    pub fn evict_expired(&self, current_time: i64) -> Result<Vec<String>, DbError> {
        self.db.evict_expired_outbox(current_time)
    }
}

/// Ingest deduplicator tracking recent `(src_node_id, seq_num)` pairs.
pub struct IngestDeduplicator {
    window: Duration,
    seen: HashMap<(u32, u32), Instant>,
    order: VecDeque<((u32, u32), Instant)>,
}

impl IngestDeduplicator {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            seen: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Returns true if this packet was already processed within the deduplication window.
    pub fn check_and_insert(&mut self, src_node_id: u32, seq_num: u32) -> bool {
        let now = Instant::now();
        self.prune(now);

        let key = (src_node_id, seq_num);
        if let std::collections::hash_map::Entry::Vacant(e) = self.seen.entry(key) {
            e.insert(now);
            self.order.push_back((key, now));
            false
        } else {
            true // Duplicate detected
        }
    }

    fn prune(&mut self, now: Instant) {
        while let Some(&(key, timestamp)) = self.order.front() {
            if now.duration_since(timestamp) > self.window {
                self.order.pop_front();
                self.seen.remove(&key);
            } else {
                break;
            }
        }
    }
}
