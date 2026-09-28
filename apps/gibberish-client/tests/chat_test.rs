use gibberish_client::{
    decrypt_direct_message, decrypt_swarm_broadcast, encrypt_direct_message,
    encrypt_swarm_broadcast, persist_chat_message,
};
use gibberish_crypto::ratchet::KeyPair;
use gibberish_crypto::secrecy::Secret;
use gibberish_db::{DatabaseStore, MessageStatus};
use gibberish_protocol::{FLAG_ACK_REQ, FLAG_DIRECT, FLAG_GROUP};

#[test]
fn test_swarm_broadcast_encryption_roundtrip_f3() {
    let swarm_key = Secret::new([0x55; 32]);
    let local_node_id = 0xBEBCE5B8;
    let msg_id = 42;
    let text = "Emergency: Swarm Channel Alert #all";

    // Encrypt broadcast
    let frame = encrypt_swarm_broadcast(&swarm_key, local_node_id, msg_id, text)
        .expect("encrypt broadcast failed");

    // Verify F3 & R13: FLAG_GROUP set, FLAG_ACK_REQ strictly absent
    assert_eq!(frame.flags & FLAG_GROUP, FLAG_GROUP);
    assert_eq!(frame.flags & FLAG_ACK_REQ, 0, "FLAG_ACK_REQ must be zero for broadcast");
    assert_eq!(frame.status, MessageStatus::Transmitted);
    assert_eq!(frame.convo_id, "#all");

    // Decrypt on peer node
    let decrypted = decrypt_swarm_broadcast(&swarm_key, local_node_id, msg_id, &frame.ciphertext)
        .expect("decrypt broadcast failed");
    assert_eq!(decrypted, text);
}

#[test]
fn test_pairwise_ratcheted_dm_encryption_roundtrip() {
    let alice_keypair = KeyPair::from_secret_bytes([0x01; 32]);
    let bob_keypair = KeyPair::from_secret_bytes([0x02; 32]);

    let _alice_node_id: u32 = 0xAAAA0001;
    let bob_node_id: u32 = 0xBBBB0002;
    let msg_id = 99;
    let text = "Classified pairwise dispatch to Bob";

    // Alice sends to Bob (online)
    let frame = encrypt_direct_message(
        &alice_keypair,
        bob_keypair.public_key(),
        bob_node_id,
        msg_id,
        text,
        true, // online
    )
    .expect("encrypt DM failed");

    // Verify R11, R12: FLAG_DIRECT and FLAG_ACK_REQ set
    assert_eq!(frame.flags & FLAG_DIRECT, FLAG_DIRECT);
    assert_eq!(frame.flags & FLAG_ACK_REQ, FLAG_ACK_REQ);
    assert_eq!(frame.status, MessageStatus::Transmitted);
    assert_eq!(frame.convo_id, "0xBBBB0002");

    // Bob decrypts from Alice
    let decrypted = decrypt_direct_message(
        &bob_keypair,
        alice_keypair.public_key(),
        msg_id,
        frame.counter,
        &frame.ciphertext,
    )
    .expect("decrypt DM failed");
    assert_eq!(decrypted, text);
}

#[test]
fn test_pairwise_offline_dm_initial_status_queued() {
    let alice_keypair = KeyPair::from_secret_bytes([0x11; 32]);
    let bob_keypair = KeyPair::from_secret_bytes([0x22; 32]);

    let frame = encrypt_direct_message(
        &alice_keypair,
        bob_keypair.public_key(),
        0xDEADBEEF,
        1,
        "Offline queued message",
        false, // offline
    )
    .expect("encrypt DM failed");

    // R14: Offline peer produces Queued status ([Q])
    assert_eq!(frame.status, MessageStatus::Queued);
    assert_eq!(frame.status.symbol(), "[Q]");
}

#[test]
fn test_chat_message_persistence_in_db() {
    let db = DatabaseStore::open_in_memory().expect("open memory db");

    let rec = persist_chat_message(
        &db,
        "msg-test-1",
        "#all",
        0xBEBCE5B8,
        1700000000,
        "Stored swarm message",
        MessageStatus::Transmitted,
    )
    .expect("persist failed");

    assert_eq!(rec.id, "msg-test-1");
    assert_eq!(rec.status, MessageStatus::Transmitted);

    let messages = db.list_messages("#all", 10, 0).expect("list messages failed");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "Stored swarm message");
}
