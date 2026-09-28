use gibberish_db::{
    ContactRecord, DatabaseStore, MessageRecord, MessageStatus, OutboxRecord, OutboxStatus,
    TrustState,
};
use rusqlite::params;
use std::thread;

#[test]
fn test_schema_constraints_not_null_and_unique() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let db_path = temp_dir.path().join("constraints_test.db");

    // Initialize schema via DatabaseStore
    let _store = DatabaseStore::open(&db_path).expect("open db");

    // Open direct rusqlite connection to test SQL constraints
    let conn = rusqlite::Connection::open(&db_path).expect("open direct connection");

    // 1. NOT NULL constraint on contacts.alias
    let err_alias = conn.execute(
        "INSERT INTO contacts (node_id, alias, pubkey, trust_state, last_seen, rssi, lqi)
         VALUES (1, NULL, X'0102', 'unverified', 1000, -70, 150);",
        [],
    );
    assert!(
        err_alias.is_err(),
        "Expected NOT NULL constraint violation on contacts.alias"
    );

    // 2. NOT NULL constraint on contacts.pubkey
    let err_pubkey = conn.execute(
        "INSERT INTO contacts (node_id, alias, pubkey, trust_state, last_seen, rssi, lqi)
         VALUES (1, 'Alice', NULL, 'unverified', 1000, -70, 150);",
        [],
    );
    assert!(
        err_pubkey.is_err(),
        "Expected NOT NULL constraint violation on contacts.pubkey"
    );

    // 3. NOT NULL constraint on messages.convo_id
    let err_convo = conn.execute(
        "INSERT INTO messages (id, convo_id, sender_node_id, timestamp, text, status)
         VALUES ('m1', NULL, 1, 1000, 'hello', 'transmitted');",
        [],
    );
    assert!(
        err_convo.is_err(),
        "Expected NOT NULL constraint violation on messages.convo_id"
    );

    // 4. NOT NULL constraint on outbox.payload
    let err_payload = conn.execute(
        "INSERT INTO outbox (id, dest_node_id, payload, queued_at, retry_count, ttl_secs, status)
         VALUES ('o1', 1, NULL, 1000, 0, 3600, 'pending');",
        [],
    );
    assert!(
        err_payload.is_err(),
        "Expected NOT NULL constraint violation on outbox.payload"
    );

    // 5. PRIMARY KEY / UNIQUE constraint violation on messages.id without ON CONFLICT
    conn.execute(
        "INSERT INTO messages (id, convo_id, sender_node_id, timestamp, text, status)
         VALUES ('msg-dup', '#all', 1, 1000, 'first', 'transmitted');",
        [],
    )
    .expect("first insert should succeed");

    let err_dup = conn.execute(
        "INSERT INTO messages (id, convo_id, sender_node_id, timestamp, text, status)
         VALUES ('msg-dup', '#all', 2, 1005, 'second', 'transmitted');",
        [],
    );
    assert!(
        err_dup.is_err(),
        "Expected UNIQUE constraint violation on duplicate message ID"
    );

    // 6. Verify _schema_migrations applied_at timestamp is properly recorded
    let applied_at: i64 = conn
        .query_row(
            "SELECT applied_at FROM _schema_migrations WHERE version = 1;",
            [],
            |row| row.get(0),
        )
        .expect("query migration timestamp");
    assert!(
        applied_at > 1_000_000_000,
        "Migration timestamp must be a real epoch timestamp (> 1_000_000_000), got: {}",
        applied_at
    );
}

#[test]
fn test_index_usage_and_teeth_proof() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let db_path = temp_dir.path().join("index_perf_test.db");

    let store = DatabaseStore::open(&db_path).expect("open db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw conn");

    // 1. Verify indexes exist in sqlite_master
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index';")
        .expect("prepare index query");
    let indexes: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .expect("query indexes")
        .filter_map(Result::ok)
        .collect();

    assert!(
        indexes.contains(&"idx_messages_convo".to_string()),
        "idx_messages_convo index missing"
    );
    assert!(
        indexes.contains(&"idx_outbox_dest_status".to_string()),
        "idx_outbox_dest_status index missing"
    );

    // Seed some messages
    for i in 0..100 {
        let msg = MessageRecord {
            id: format!("msg-perf-{}", i),
            convo_id: "#all".to_string(),
            sender_node_id: 0x1234,
            timestamp: 1000 + i,
            text: format!("Message content {}", i),
            status: MessageStatus::Transmitted,
        };
        store.insert_message(&msg).expect("insert msg");
    }

    // 2. EXPLAIN QUERY PLAN on messages pagination query with index intact
    let plan_with_index: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT id, convo_id, sender_node_id, timestamp, text, status 
             FROM messages WHERE convo_id = ?1 ORDER BY timestamp ASC LIMIT 10 OFFSET 0;",
            params!["#all"],
            |row| row.get(3), // 'detail' column in sqlite EXPLAIN QUERY PLAN
        )
        .expect("explain query plan with index");

    assert!(
        plan_with_index.contains("idx_messages_convo"),
        "Query plan must utilize idx_messages_convo index. Actual plan: {}",
        plan_with_index
    );

    // 3. Teeth test: DROP the index in test connection and assert the planner falls back to SCAN
    conn.execute("DROP INDEX idx_messages_convo;", [])
        .expect("drop index");

    let plan_without_index: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT id, convo_id, sender_node_id, timestamp, text, status 
             FROM messages WHERE convo_id = ?1 ORDER BY timestamp ASC LIMIT 10 OFFSET 0;",
            params!["#all"],
            |row| row.get(3),
        )
        .expect("explain query plan without index");

    assert!(
        plan_without_index.contains("SCAN messages"),
        "Query plan without index must fall back to table SCAN. Actual plan: {}",
        plan_without_index
    );
}

#[test]
fn test_transaction_rollback_and_isolation() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let db_path = temp_dir.path().join("tx_rollback_test.db");

    let store = DatabaseStore::open(&db_path).expect("open db");
    let mut conn = rusqlite::Connection::open(&db_path).expect("open direct connection");

    // Begin a transaction, perform writes, and deliberately abort/rollback
    {
        let tx = conn.transaction().expect("start transaction");
        tx.execute(
            "INSERT INTO messages (id, convo_id, sender_node_id, timestamp, text, status)
             VALUES ('msg-abort', '#all', 1, 1000, 'doomed text', 'transmitted');",
            [],
        )
        .expect("insert inside tx");

        // Roll back transaction
        tx.rollback().expect("rollback tx");
    }

    // Assert that the record was not committed to the database
    let fetched = store
        .get_message("msg-abort")
        .expect("get message query failed");
    assert!(
        fetched.is_none(),
        "Aborted transaction must not leak data into store"
    );
}

#[test]
fn test_outbox_crash_reconciliation_multi_state() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let db_path = temp_dir.path().join("crash_recovery_states.db");

    let store = DatabaseStore::open(&db_path).expect("open db");

    let records = vec![
        OutboxRecord {
            id: "out-pending-1".to_string(),
            dest_node_id: 0x1111,
            payload: vec![1],
            queued_at: 1000,
            retry_count: 0,
            ttl_secs: 3600,
            status: OutboxStatus::Pending,
        },
        OutboxRecord {
            id: "out-sending-1".to_string(),
            dest_node_id: 0x2222,
            payload: vec![2],
            queued_at: 1001,
            retry_count: 1,
            ttl_secs: 3600,
            status: OutboxStatus::Sending,
        },
        OutboxRecord {
            id: "out-sending-2".to_string(),
            dest_node_id: 0x3333,
            payload: vec![3],
            queued_at: 1002,
            retry_count: 2,
            ttl_secs: 3600,
            status: OutboxStatus::Sending,
        },
        OutboxRecord {
            id: "out-sent-1".to_string(),
            dest_node_id: 0x4444,
            payload: vec![4],
            queued_at: 1003,
            retry_count: 0,
            ttl_secs: 3600,
            status: OutboxStatus::Sent,
        },
        OutboxRecord {
            id: "out-failed-1".to_string(),
            dest_node_id: 0x5555,
            payload: vec![5],
            queued_at: 1004,
            retry_count: 5,
            ttl_secs: 3600,
            status: OutboxStatus::Failed,
        },
    ];

    for r in &records {
        store.insert_outbox(r).expect("insert outbox record");
    }

    // Execute crash reconciliation
    let reconciled_count = store
        .reconcile_crashed_outbox()
        .expect("reconcile crash failed");
    assert_eq!(
        reconciled_count, 2,
        "Only the 2 'sending' records should be reset"
    );

    // Verify final states
    let pending_1 = store.get_outbox("out-pending-1").unwrap().unwrap();
    assert_eq!(pending_1.status, OutboxStatus::Pending);

    let sending_1 = store.get_outbox("out-sending-1").unwrap().unwrap();
    assert_eq!(
        sending_1.status,
        OutboxStatus::Pending,
        "Crashed sending item 1 must be reset to pending"
    );

    let sending_2 = store.get_outbox("out-sending-2").unwrap().unwrap();
    assert_eq!(
        sending_2.status,
        OutboxStatus::Pending,
        "Crashed sending item 2 must be reset to pending"
    );

    let sent_1 = store.get_outbox("out-sent-1").unwrap().unwrap();
    assert_eq!(
        sent_1.status,
        OutboxStatus::Sent,
        "Sent records must remain sent"
    );

    let failed_1 = store.get_outbox("out-failed-1").unwrap().unwrap();
    assert_eq!(
        failed_1.status,
        OutboxStatus::Failed,
        "Failed records must remain failed"
    );
}

#[test]
fn test_ttl_eviction_boundary_and_state_isolation() {
    let store = DatabaseStore::open_in_memory().expect("open memory db");

    // Seed outbox items with varying TTLs
    let items = vec![
        // item 1: expires at 1000 + 100 = 1100
        ("item-1", 1000, 100, OutboxStatus::Pending),
        // item 2: expires at 1000 + 200 = 1200
        ("item-2", 1000, 200, OutboxStatus::Pending),
        // item 3: expires at 1000 + 100 = 1100, but is already SENT
        ("item-3-sent", 1000, 100, OutboxStatus::Sent),
        // item 4: expires at 1000 + 100 = 1100, but is already FAILED
        ("item-4-failed", 1000, 100, OutboxStatus::Failed),
    ];

    for (id, queued_at, ttl, status) in items {
        let msg = MessageRecord {
            id: id.to_string(),
            convo_id: "0x1234".to_string(),
            sender_node_id: 1,
            timestamp: queued_at,
            text: format!("Text for {}", id),
            status: MessageStatus::Queued,
        };
        store.insert_message(&msg).expect("insert msg");

        let outbox = OutboxRecord {
            id: id.to_string(),
            dest_node_id: 0x1234,
            payload: vec![1, 2],
            queued_at,
            retry_count: 0,
            ttl_secs: ttl,
            status,
        };
        store.insert_outbox(&outbox).expect("insert outbox");
    }

    // Boundary check 1: at t = 1099, nothing expired yet
    let evicted_1099 = store.evict_expired_outbox(1099).expect("evict 1099");
    assert!(evicted_1099.is_empty());

    // Boundary check 2: at t = 1100, (1000 + 100) < 1100 is FALSE (exact boundary)
    let evicted_1100 = store.evict_expired_outbox(1100).expect("evict 1100");
    assert!(evicted_1100.is_empty());

    // Boundary check 3: at t = 1101, item-1 is expired, but item-3-sent and item-4-failed are NOT touched
    let evicted_1101 = store.evict_expired_outbox(1101).expect("evict 1101");
    assert_eq!(evicted_1101, vec!["item-1".to_string()]);

    // Check item-1 outbox and message status
    assert_eq!(
        store.get_outbox("item-1").unwrap().unwrap().status,
        OutboxStatus::Failed
    );
    assert_eq!(
        store.get_message("item-1").unwrap().unwrap().status,
        MessageStatus::Failed
    );

    // Verify item-3-sent remained Sent
    assert_eq!(
        store.get_outbox("item-3-sent").unwrap().unwrap().status,
        OutboxStatus::Sent
    );

    // Boundary check 4: at t = 1201, item-2 is expired
    let evicted_1201 = store.evict_expired_outbox(1201).expect("evict 1201");
    assert_eq!(evicted_1201, vec!["item-2".to_string()]);
    assert_eq!(
        store.get_outbox("item-2").unwrap().unwrap().status,
        OutboxStatus::Failed
    );
}

#[test]
fn test_poison_lock_resilience() {
    let store = DatabaseStore::open_in_memory().expect("open memory db");
    let store_clone = store.clone();

    // Spawn a thread that panics while using store
    let handle = thread::spawn(move || {
        let contact = ContactRecord {
            node_id: 0x9999,
            alias: "PanicTester".to_string(),
            pubkey: [0x55; 32],
            trust_state: TrustState::Unverified,
            last_seen: 1000,
            rssi: -80,
            lqi: 120,
        };
        store_clone.upsert_contact(&contact).unwrap();
        panic!("Simulated thread panic holding mutex!");
    });

    // The thread panics
    let _ = handle.join();

    // Subsequent operation on the main thread must NOT panic and should read the committed contact
    let contact = store
        .get_contact(0x9999)
        .expect("must recover gracefully from poisoned mutex")
        .expect("contact should exist");
    assert_eq!(contact.alias, "PanicTester");

    // Perform another write after the poison event
    let msg = MessageRecord {
        id: "msg-post-poison".to_string(),
        convo_id: "#all".to_string(),
        sender_node_id: 0x9999,
        timestamp: 2000,
        text: "Survived poison".to_string(),
        status: MessageStatus::Transmitted,
    };
    store
        .insert_message(&msg)
        .expect("write after poison must succeed");

    let read_msg = store
        .get_message("msg-post-poison")
        .unwrap()
        .expect("message must exist");
    assert_eq!(read_msg.text, "Survived poison");
}

#[test]
fn test_multithreaded_concurrent_writes() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let db_path = temp_dir.path().join("concurrent_writes.db");
    let store = DatabaseStore::open(&db_path).expect("open db");

    let mut handles = Vec::new();
    let num_threads = 4;
    let msgs_per_thread = 25;

    for t in 0..num_threads {
        let store_ref = store.clone();
        let handle = thread::spawn(move || {
            for m in 0..msgs_per_thread {
                let msg = MessageRecord {
                    id: format!("t{}-m{}", t, m),
                    convo_id: "#all".to_string(),
                    sender_node_id: t as u32,
                    timestamp: 1000 + m as i64,
                    text: format!("Thread {} Msg {}", t, m),
                    status: MessageStatus::Transmitted,
                };
                store_ref.insert_message(&msg).expect("concurrent insert");
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().expect("thread join failed");
    }

    let all_messages = store
        .list_messages("#all", 200, 0)
        .expect("list messages failed");
    assert_eq!(
        all_messages.len(),
        num_threads * msgs_per_thread,
        "All concurrent messages must be committed without loss"
    );
}

#[test]
fn test_outbox_mark_and_retry_lifecycle() {
    let store = DatabaseStore::open_in_memory().expect("open memory db");

    let msg = MessageRecord {
        id: "msg-track-1".to_string(),
        convo_id: "0x1234".to_string(),
        sender_node_id: 1,
        timestamp: 1000,
        text: "Tracked outbox message".to_string(),
        status: MessageStatus::Queued,
    };
    store.insert_message(&msg).expect("insert msg");

    let outbox = OutboxRecord {
        id: "msg-track-1".to_string(),
        dest_node_id: 0x1234,
        payload: vec![1, 2, 3],
        queued_at: 1000,
        retry_count: 0,
        ttl_secs: 3600,
        status: OutboxStatus::Pending,
    };
    store.insert_outbox(&outbox).expect("insert outbox");

    // 1. Test list_all_outbox
    let all = store.list_all_outbox().expect("list all outbox");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, "msg-track-1");
    assert_eq!(all[0].status, OutboxStatus::Pending);

    // 2. Test mark_outbox_sending
    store
        .mark_outbox_sending(&["msg-track-1"])
        .expect("mark sending");
    let after_sending = store.get_outbox("msg-track-1").unwrap().unwrap();
    assert_eq!(after_sending.status, OutboxStatus::Sending);

    // 3. Test increment_retry_count
    store
        .increment_retry_count("msg-track-1")
        .expect("increment retry 1");
    let after_inc1 = store.get_outbox("msg-track-1").unwrap().unwrap();
    assert_eq!(after_inc1.retry_count, 1);

    store
        .increment_retry_count("msg-track-1")
        .expect("increment retry 2");
    let after_inc2 = store.get_outbox("msg-track-1").unwrap().unwrap();
    assert_eq!(after_inc2.retry_count, 2);

    // 4. Test mark_outbox_sent (must update outbox -> sent AND message -> delivered)
    store.mark_outbox_sent(&["msg-track-1"]).expect("mark sent");
    let outbox_sent = store.get_outbox("msg-track-1").unwrap().unwrap();
    assert_eq!(outbox_sent.status, OutboxStatus::Sent);

    let msg_delivered = store.get_message("msg-track-1").unwrap().unwrap();
    assert_eq!(msg_delivered.status, MessageStatus::Delivered);

    // 5. Test update_outbox_status return values
    let updated = store
        .update_outbox_status("msg-track-1", OutboxStatus::Pending)
        .expect("update status");
    assert!(updated, "Updating existing outbox record must return true");

    let updated_fake = store
        .update_outbox_status("non-existent-id", OutboxStatus::Pending)
        .expect("update status non-existent");
    assert!(
        !updated_fake,
        "Updating non-existent outbox record must return false"
    );
}

#[test]
fn test_status_helpers_and_non_existent_updates() {
    let store = DatabaseStore::open_in_memory().expect("open memory db");

    // 1. Test MessageStatus parsing and symbols
    assert_eq!(
        MessageStatus::from_str_val("queued"),
        MessageStatus::Queued
    );
    assert_eq!(
        MessageStatus::from_str_val("delivered"),
        MessageStatus::Delivered
    );
    assert_eq!(
        MessageStatus::from_str_val("failed"),
        MessageStatus::Failed
    );
    assert_eq!(
        MessageStatus::from_str_val("transmitted"),
        MessageStatus::Transmitted
    );
    assert_eq!(
        MessageStatus::from_str_val("arbitrary"),
        MessageStatus::Transmitted
    );

    assert_eq!(MessageStatus::Transmitted.symbol(), "*");
    assert_eq!(MessageStatus::Delivered.symbol(), "[OK]");
    assert_eq!(MessageStatus::Queued.symbol(), "[Q]");
    assert_eq!(MessageStatus::Failed.symbol(), "[!]");

    // 2. Test update_message_status return values
    let changed_fake = store
        .update_message_status("non-existent-msg", MessageStatus::Delivered)
        .expect("update fake message");
    assert!(
        !changed_fake,
        "Updating non-existent message must return false"
    );

    // 3. Test set_trust_state return values
    let trust_changed_fake = store
        .set_trust_state(0xDEADBEEF, TrustState::Verified)
        .expect("update fake trust");
    assert!(
        !trust_changed_fake,
        "Setting trust state for non-existent contact must return false"
    );
}

