//! Swarm Broadcast (#all) & Pairwise Ratcheted 1-to-1 DMs (R10, R11, R12, KTD4).

use gibberish_crypto::ratchet::{
    decrypt_chunk, derive_sender_subkey, encrypt_chunk, CryptoError, KeyPair, SenderKeyChain,
};
use gibberish_crypto::secrecy::Secret;
use gibberish_db::{DatabaseStore, DbError, MessageRecord, MessageStatus};
use gibberish_protocol::{
    CIPHERTEXT_LEN, FLAG_ACK_REQ, FLAG_DIRECT, FLAG_GROUP, PLAINTEXT_CHUNK_LEN,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ChatError {
    #[error("Crypto error: {0:?}")]
    Crypto(CryptoError),
    #[error("Database error: {0}")]
    Db(#[from] DbError),
    #[error("Message payload exceeds 80B single chunk limit: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("Invalid UTF-8 plaintext payload")]
    InvalidUtf8,
    #[error("Broadcast messages disallow FLAG_ACK_REQ")]
    BroadcastAckReqForbidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedFramePayload {
    pub convo_id: String,
    pub dest_node_id: u32,
    pub flags: u16,
    pub msg_id: u32,
    pub counter: u64,
    pub ciphertext: [u8; CIPHERTEXT_LEN],
    pub status: MessageStatus,
}

/// Encrypts an open Swarm broadcast message for `#all` using the shared Swarm Master Key.
/// Strictly enforces FLAG_GROUP and disallows FLAG_ACK_REQ (R10, R13, KTD4).
pub fn encrypt_swarm_broadcast(
    swarm_key: &Secret<[u8; 32]>,
    local_node_id: u32,
    msg_id: u32,
    text: &str,
) -> Result<EncryptedFramePayload, ChatError> {
    let bytes = text.as_bytes();
    if bytes.len() > PLAINTEXT_CHUNK_LEN {
        return Err(ChatError::PayloadTooLarge(bytes.len()));
    }

    let subkey = derive_sender_subkey(swarm_key, local_node_id);
    let ciphertext = encrypt_chunk(&subkey, msg_id, 0, 0, bytes).map_err(ChatError::Crypto)?;

    let flags = FLAG_GROUP; // NO FLAG_ACK_REQ on swarm broadcasts!
    if (flags & FLAG_ACK_REQ) != 0 {
        return Err(ChatError::BroadcastAckReqForbidden);
    }

    Ok(EncryptedFramePayload {
        convo_id: "#all".to_string(),
        dest_node_id: 0xFFFFFFFF, // Broadcast address
        flags,
        msg_id,
        counter: 0,
        ciphertext,
        status: MessageStatus::Transmitted,
    })
}

/// Decrypts an incoming Swarm broadcast frame from `#all`.
pub fn decrypt_swarm_broadcast(
    swarm_key: &Secret<[u8; 32]>,
    src_node_id: u32,
    msg_id: u32,
    ciphertext: &[u8; CIPHERTEXT_LEN],
) -> Result<String, ChatError> {
    let subkey = derive_sender_subkey(swarm_key, src_node_id);
    let plaintext_bytes =
        decrypt_chunk(&subkey, msg_id, 0, 0, ciphertext).map_err(ChatError::Crypto)?;

    // Trim trailing zeroes if null-padded
    let len = plaintext_bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(PLAINTEXT_CHUNK_LEN);
    let s = std::str::from_utf8(&plaintext_bytes[..len]).map_err(|_| ChatError::InvalidUtf8)?;
    Ok(s.to_string())
}

/// Encrypts a 1-to-1 direct message using pairwise X25519 ECDH + SenderKeyChain ratcheting.
/// Strictly attaches FLAG_DIRECT and FLAG_ACK_REQ (R11, R12, KTD4).
pub fn encrypt_direct_message(
    local_keypair: &KeyPair,
    remote_pubkey: &[u8; 32],
    dest_node_id: u32,
    msg_id: u32,
    text: &str,
    is_peer_online: bool,
) -> Result<EncryptedFramePayload, ChatError> {
    let bytes = text.as_bytes();
    if bytes.len() > PLAINTEXT_CHUNK_LEN {
        return Err(ChatError::PayloadTooLarge(bytes.len()));
    }

    // Derive pairwise shared secret via Diffie-Hellman
    let shared_secret = local_keypair.diffie_hellman(remote_pubkey);
    let mut chain = SenderKeyChain::new(shared_secret);
    let (msg_key, counter) = chain.step();

    let ciphertext =
        encrypt_chunk(&msg_key, msg_id, 0, counter, bytes).map_err(ChatError::Crypto)?;

    let flags = FLAG_DIRECT | FLAG_ACK_REQ;
    let initial_status = if is_peer_online {
        MessageStatus::Transmitted
    } else {
        MessageStatus::Queued
    };

    Ok(EncryptedFramePayload {
        convo_id: format!("0x{:08X}", dest_node_id),
        dest_node_id,
        flags,
        msg_id,
        counter,
        ciphertext,
        status: initial_status,
    })
}

/// Decrypts an incoming 1-to-1 direct message frame.
pub fn decrypt_direct_message(
    local_keypair: &KeyPair,
    remote_pubkey: &[u8; 32],
    msg_id: u32,
    counter: u64,
    ciphertext: &[u8; CIPHERTEXT_LEN],
) -> Result<String, ChatError> {
    let shared_secret = local_keypair.diffie_hellman(remote_pubkey);
    let mut chain = SenderKeyChain::new(shared_secret);

    // Step chain up to counter
    let mut current_key = Secret::new([0u8; 32]);
    for _ in 0..=counter {
        let (k, _) = chain.step();
        current_key = k;
    }

    let plaintext_bytes =
        decrypt_chunk(&current_key, msg_id, 0, counter, ciphertext).map_err(ChatError::Crypto)?;

    let len = plaintext_bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(PLAINTEXT_CHUNK_LEN);
    let s = std::str::from_utf8(&plaintext_bytes[..len]).map_err(|_| ChatError::InvalidUtf8)?;
    Ok(s.to_string())
}

/// Saves message to the local SQLite database and returns the record.
pub fn persist_chat_message(
    db: &DatabaseStore,
    id: &str,
    convo_id: &str,
    sender_node_id: u32,
    timestamp: i64,
    text: &str,
    status: MessageStatus,
) -> Result<MessageRecord, ChatError> {
    let rec = MessageRecord {
        id: id.to_string(),
        convo_id: convo_id.to_string(),
        sender_node_id,
        timestamp,
        text: text.to_string(),
        status,
    };
    db.insert_message(&rec)?;
    Ok(rec)
}
