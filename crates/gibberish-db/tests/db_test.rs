use gibberish_db::{
    ContactRecord, DatabaseStore, MessageRecord, MessageStatus, OutboxRecord, OutboxStatus,
    TrustState,
};

#[test]
fn test_contact_crud_and_trust_transition() {
    let store = DatabaseStore::open_in_memory().expect("failed to open in-memory db");

    let mut pubkey = [0u8; 32];
    pubkey[0] = 0xAA;
    pubkey[31] = 0xFF;

    let contact = ContactRecord {
        node_id: 0xBEBD82B4,
        alias: "Alice_Station".to_string(),
        pubkey,
        trust_state: TrustState::Unverified,
        last_seen: 1000,
        rssi: -72,
        lqi: 180,
    };

    // Upsert
    store.upsert_contact(&contact).expect("upsert failed");

    // Get
    let fetched = store
        .get_contact(0xBEBD82B4)
        .expect("get failed")
        .expect("contact missing");
    assert_eq!(fetched.node_id, 0xBEBD82B4);
    assert_eq!(fetched.alias, "Alice_Station");
    assert_eq!(fetched.pubkey, pubkey);
    assert_eq!(fetched.trust_state, TrustState::Unverified);
    assert_eq!(fetched.rssi, -72);
    assert_eq!(fetched.lqi, 180);

    // List
    let contacts = store.list_contacts().expect("list failed");
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].pubkey, pubkey);

    // Transition to Verified
    let changed = store
        .set_trust_state(0xBEBD82B4, TrustState::Verified)
        .expect("set trust failed");
    assert!(changed);

    let updated = store
        .get_contact(0xBEBD82B4)
        .expect("get failed")
        .expect("contact missing");
    assert_eq!(updated.trust_state, TrustState::Verified);

    // Update metrics
    store
        .update_contact_metrics(0xBEBD82B4, -65, 210, 1050)
        .expect("metrics update failed");
    let updated_metrics = store.get_contact(0xBEBD82B4).unwrap().unwrap();
    assert_eq!(updated_metrics.rssi, -65);
    assert_eq!(updated_metrics.lqi, 210);
    assert_eq!(updated_metrics.last_seen, 1050);
}

#[test]
fn test_message_persistence_and_pagination() {
    let store = DatabaseStore::open_in_memory().expect("failed to open in-memory db");

    for i in 1..=10 {
        let msg = MessageRecord {
            id: format!("msg-{}", i),
            convo_id: "#all".to_string(),
            sender_node_id: 0x11112222,
            timestamp: 1000 + i as i64,
            text: format!("Broadcast payload number {}", i),
            status: MessageStatus::Transmitted,
        };
        store.insert_message(&msg).expect("insert message failed");
    }

    // Pagination: limit 5, offset 0
    let page1 = store.list_messages("#all", 5, 0).expect("list page 1 failed");
    assert_eq!(page1.len(), 5);
    assert_eq!(page1[0].id, "msg-1");
    assert_eq!(page1[4].id, "msg-5");

    // Pagination: limit 5, offset 5
    let page2 = store.list_messages("#all", 5, 5).expect("list page 2 failed");
    assert_eq!(page2.len(), 5);
    assert_eq!(page2[0].id, "msg-6");
    assert_eq!(page2[4].id, "msg-10");

    // Update message status
    let updated = store
        .update_message_status("msg-1", MessageStatus::Delivered)
        .expect("update status failed");
    assert!(updated);
    let fetched = store.get_message("msg-1").unwrap().unwrap();
    assert_eq!(fetched.status, MessageStatus::Delivered);
}

#[test]
fn test_outbox_lifecycle_and_crash_reconciliation() {
    let temp_dir = tempfile::tempdir().expect("create temp dir failed");
    let db_path = temp_dir.path().join("test_gibberish.db");

    {
        let store = DatabaseStore::open(&db_path).expect("open db failed");

        let outbox_item1 = OutboxRecord {
            id: "dtn-1".to_string(),
            dest_node_id: 0xC3A109F2,
            payload: vec![1, 2, 3, 4],
            queued_at: 1000,
            retry_count: 0,
            ttl_secs: 172800, // 48h
            status: OutboxStatus::Pending,
        };
        let outbox_item2 = OutboxRecord {
            id: "dtn-2".to_string(),
            dest_node_id: 0xC3A109F2,
            payload: vec![5, 6, 7, 8],
            queued_at: 1005,
            retry_count: 1,
            ttl_secs: 172800,
            status: OutboxStatus::Sending, // Simulate sending state during crash
        };

        store.insert_outbox(&outbox_item1).expect("insert outbox failed");
        store.insert_outbox(&outbox_item2).expect("insert outbox failed");

        // Verify dtn-2 is in sending state before crash
        let item2 = store.get_outbox("dtn-2").unwrap().unwrap();
        assert_eq!(item2.status, OutboxStatus::Sending);
    } // DB closed, simulating abnormal termination / restart

    // Re-open DB
    {
        let store = DatabaseStore::open(&db_path).expect("reopen db failed");

        // Crash reconciliation must have reset dtn-2 from Sending back to Pending
        let item2 = store.get_outbox("dtn-2").unwrap().unwrap();
        assert_eq!(item2.status, OutboxStatus::Pending);

        // List pending for node
        let pending = store
            .list_pending_outbox_for_node(0xC3A109F2)
            .expect("list pending failed");
        assert_eq!(pending.len(), 2);
    }
}

#[test]
fn test_dtn_outbox_ttl_eviction() {
    let store = DatabaseStore::open_in_memory().expect("open db failed");

    // Insert corresponding message in messages table
    let msg = MessageRecord {
        id: "dtn-expired-1".to_string(),
        convo_id: "0xC3A109F2".to_string(),
        sender_node_id: 0x01,
        timestamp: 1000,
        text: "Direct message that will expire".to_string(),
        status: MessageStatus::Queued,
    };
    store.insert_message(&msg).expect("insert msg failed");

    let outbox_entry = OutboxRecord {
        id: "dtn-expired-1".to_string(),
        dest_node_id: 0xC3A109F2,
        payload: vec![0xDE, 0xAD],
        queued_at: 1000,
        retry_count: 5,
        ttl_secs: 48 * 3600, // 48 hours = 172,800
        status: OutboxStatus::Pending,
    };
    store.insert_outbox(&outbox_entry).expect("insert outbox failed");

    // Check before expiry (e.g. at 1000 + 100,000)
    let evicted_early = store
        .evict_expired_outbox(1000 + 100_000)
        .expect("evict early failed");
    assert!(evicted_early.is_empty());

    // Check after expiry (e.g. at 1000 + 172,801)
    let evicted = store
        .evict_expired_outbox(1000 + 172_801)
        .expect("evict expired failed");
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0], "dtn-expired-1");

    // Verify outbox state transitioned to failed
    let outbox_record = store.get_outbox("dtn-expired-1").unwrap().unwrap();
    assert_eq!(outbox_record.status, OutboxStatus::Failed);

    // Verify message state in chat thread transitioned to failed
    let msg_record = store.get_message("dtn-expired-1").unwrap().unwrap();
    assert_eq!(msg_record.status, MessageStatus::Failed);
}
