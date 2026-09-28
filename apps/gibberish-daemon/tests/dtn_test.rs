use gibberish_daemon::dtn_outbox::{
    AsymmetricPolicy, DtnOutboxEngine, IngestDeduplicator, DEFAULT_OUTBOX_TTL_SECS,
};
use gibberish_db::{DatabaseStore, MessageRecord, MessageStatus, OutboxStatus};
use gibberish_protocol::{FLAG_ACK_REQ, FLAG_CLIPBOARD, FLAG_DIRECT, FLAG_GROUP};
use std::time::Duration;

#[test]
fn test_asymmetric_policy_zero_broadcast_acks_r10_r13() {
    // 100 consecutive #all Swarm messages produce exactly ZERO ACK responses (R10, R13)
    let broadcast_flags = FLAG_GROUP | FLAG_CLIPBOARD;
    for _ in 0..100 {
        let emit_ack = AsymmetricPolicy::should_emit_ack(broadcast_flags);
        assert!(!emit_ack, "Swarm broadcast frame must NEVER emit ACK response");
    }

    // Direct message with ACK requested MUST emit ACK
    let direct_flags = FLAG_DIRECT | FLAG_ACK_REQ;
    assert!(
        AsymmetricPolicy::should_emit_ack(direct_flags),
        "Direct message with FLAG_ACK_REQ must trigger ACK"
    );

    // Direct message without ACK requested does not emit ACK
    assert!(
        !AsymmetricPolicy::should_emit_ack(FLAG_DIRECT),
        "Direct message without FLAG_ACK_REQ must not emit ACK"
    );

    // Protocol gate: reject FLAG_ACK_REQ attached to broadcast
    assert!(
        AsymmetricPolicy::validate_outbound_flags(FLAG_GROUP | FLAG_ACK_REQ).is_err(),
        "Must reject FLAG_ACK_REQ on broadcast frames"
    );
    assert!(
        AsymmetricPolicy::validate_outbound_flags(FLAG_GROUP).is_ok(),
        "Clean broadcast flags must validate cleanly"
    );
}

#[test]
fn test_offline_dtn_queue_and_beacon_flush_f2() {
    let db = DatabaseStore::open_in_memory().expect("open memory db");
    let mut engine = DtnOutboxEngine::new(db.clone());

    let dest_node_id = 0xC3A109F2;
    let msg_id = "msg-offline-101";

    // Insert corresponding message in chat store
    let chat_msg = MessageRecord {
        id: msg_id.to_string(),
        convo_id: format!("0x{:08X}", dest_node_id),
        sender_node_id: 0x01,
        timestamp: 1000,
        text: "Offline dispatched directive".to_string(),
        status: MessageStatus::Queued,
    };
    db.insert_message(&chat_msg).expect("insert msg failed");

    // Peer is offline: queue in DTN outbox
    let queued = engine
        .queue_message(
            msg_id,
            dest_node_id,
            b"Encrypted payload bytes".to_vec(),
            DEFAULT_OUTBOX_TTL_SECS,
            1000,
        )
        .expect("queue failed");
    assert_eq!(queued.status, OutboxStatus::Pending);

    // Peer is not recently heard
    assert!(!engine.is_peer_recently_heard(dest_node_id));

    // Radio listener overhears peer announcement beacon (F2)
    let flush_batch = engine
        .on_peer_beacon_overheard(dest_node_id)
        .expect("beacon flush failed");
    assert_eq!(flush_batch.len(), 1);
    assert_eq!(flush_batch[0].id, msg_id);

    // Outbox record transitioned to Sending
    let sending_item = db.get_outbox(msg_id).unwrap().unwrap();
    assert_eq!(sending_item.status, OutboxStatus::Sending);

    // Peer is now recognized as recently active
    assert!(engine.is_peer_recently_heard(dest_node_id));

    // Recipient returns SACK / delivery receipt (F2)
    let acked = engine
        .on_delivery_ack_received(msg_id)
        .expect("ack received failed");
    assert!(acked);

    // Status in outbox is Sent
    let sent_item = db.get_outbox(msg_id).unwrap().unwrap();
    assert_eq!(sent_item.status, OutboxStatus::Sent);

    // Status in messages table is Delivered ([OK])
    let delivered_msg = db.get_message(msg_id).unwrap().unwrap();
    assert_eq!(delivered_msg.status, MessageStatus::Delivered);
    assert_eq!(delivered_msg.status.symbol(), "[OK]");
}

#[test]
fn test_dtn_outbox_48h_ttl_eviction_ae3() {
    let db = DatabaseStore::open_in_memory().expect("open memory db");
    let engine = DtnOutboxEngine::new(db.clone());

    let dest_node_id = 0xC3A109F2;
    let msg_id = "msg-ttl-expire-202";

    // Insert message in messages table
    let chat_msg = MessageRecord {
        id: msg_id.to_string(),
        convo_id: format!("0x{:08X}", dest_node_id),
        sender_node_id: 0x01,
        timestamp: 1000,
        text: "Expiring offline message".to_string(),
        status: MessageStatus::Queued,
    };
    db.insert_message(&chat_msg).expect("insert msg failed");

    // Queue in DTN outbox with 48h TTL (172,800s)
    engine
        .queue_message(
            msg_id,
            dest_node_id,
            b"Payload".to_vec(),
            48 * 3600,
            1000, // queued at t=1000
        )
        .expect("queue failed");

    // 24 hours elapse (t = 1000 + 86,400): not expired
    let evicted_24h = engine.evict_expired(1000 + 86400).expect("evict failed");
    assert!(evicted_24h.is_empty());

    // 48 hours + 1s elapse (t = 1000 + 172,801): must evict (AE3)
    let evicted_48h = engine.evict_expired(1000 + 172801).expect("evict failed");
    assert_eq!(evicted_48h.len(), 1);
    assert_eq!(evicted_48h[0], msg_id);

    // Outbox record is marked Failed
    let outbox_record = db.get_outbox(msg_id).unwrap().unwrap();
    assert_eq!(outbox_record.status, OutboxStatus::Failed);

    // Message status in chat thread transitioned to Failed [!]
    let msg_record = db.get_message(msg_id).unwrap().unwrap();
    assert_eq!(msg_record.status, MessageStatus::Failed);
    assert_eq!(msg_record.status.symbol(), "[!]");
}

#[test]
fn test_ingest_deduplicator() {
    let mut dedup = IngestDeduplicator::new(Duration::from_secs(10));

    // First arrival
    assert!(!dedup.check_and_insert(0x11112222, 1));

    // Immediate duplicate
    assert!(dedup.check_and_insert(0x11112222, 1));

    // Different sequence number from same node
    assert!(!dedup.check_and_insert(0x11112222, 2));

    // Same sequence number from different node
    assert!(!dedup.check_and_insert(0x33334444, 1));
}
