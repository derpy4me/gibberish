//! Cryptographic primitive operations: ChaCha20-Poly1305 AEAD, implicit nonce derivation,
//! Sender Keys group ratcheting, and X25519 ECDH (R4, R11, R12, KTD4).

use crate::secrecy::Secret;
use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, Tag};
use gibberish_protocol::{CIPHERTEXT_LEN, PLAINTEXT_CHUNK_LEN};
use x25519_dalek::{PublicKey, StaticSecret};

#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    AuthenticationFailed,
    InvalidKeyLength,
    InvalidNonce,
    PayloadTooLarge,
}

/// Derive a deterministic 96-bit (12-byte) implicit nonce from session key and packet coords (KTD4).
/// Never transmitted on-wire to preserve the 127-byte PHY MTU.
pub fn derive_implicit_nonce(
    session_key: &[u8; 32],
    msg_id: u32,
    chunk_idx: u8,
    ratchet_counter: u64,
) -> [u8; 12] {
    let mut hasher = blake3::Hasher::new_keyed(session_key);
    hasher.update(b"GIBBERISH-NONCE-V1");
    hasher.update(&msg_id.to_be_bytes());
    hasher.update(&[chunk_idx]);
    hasher.update(&ratchet_counter.to_be_bytes());

    let output = hasher.finalize();
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&output.as_bytes()[0..12]);
    nonce
}

/// Derive a 64-bit Network Admission Tag from a Swarm Master Key using BLAKE3 (R15).
pub fn derive_network_tag(swarm_key: &[u8; 32]) -> u64 {
    let mut hasher = blake3::Hasher::new_keyed(swarm_key);
    hasher.update(b"GIBBERISH-NETWORK-ADMISSION-TAG-V1");
    let output = hasher.finalize();
    u64::from_be_bytes(output.as_bytes()[0..8].try_into().unwrap())
}

/// Derive a 32-byte Sender Subkey from a Swarm Master Key and 32-bit Node ID (KTD1).
pub fn derive_sender_subkey(swarm_key: &Secret<[u8; 32]>, node_id: u32) -> Secret<[u8; 32]> {
    let mut hasher = blake3::Hasher::new_keyed(swarm_key.expose_secret());
    hasher.update(b"GIBBERISH-SENDER-SUBKEY-V1");
    hasher.update(&node_id.to_be_bytes());
    let subkey_bytes = *hasher.finalize().as_bytes();
    Secret::new(subkey_bytes)
}

/// Encrypt an 80-byte plaintext chunk into a 96-byte ciphertext chunk (80B body + 16B Poly1305 tag).
pub fn encrypt_chunk(
    session_key: &Secret<[u8; 32]>,
    msg_id: u32,
    chunk_idx: u8,
    ratchet_counter: u64,
    plaintext: &[u8],
) -> Result<[u8; CIPHERTEXT_LEN], CryptoError> {
    if plaintext.len() > PLAINTEXT_CHUNK_LEN {
        return Err(CryptoError::PayloadTooLarge);
    }

    let raw_nonce = derive_implicit_nonce(session_key.expose_secret(), msg_id, chunk_idx, ratchet_counter);
    let nonce = Nonce::from_slice(&raw_nonce);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(session_key.expose_secret()));

    let mut buffer = [0u8; PLAINTEXT_CHUNK_LEN];
    buffer[..plaintext.len()].copy_from_slice(plaintext);

    let tag = cipher
        .encrypt_in_place_detached(nonce, b"", &mut buffer)
        .map_err(|_| CryptoError::AuthenticationFailed)?;

    let mut out = [0u8; CIPHERTEXT_LEN];
    out[0..PLAINTEXT_CHUNK_LEN].copy_from_slice(&buffer);
    out[PLAINTEXT_CHUNK_LEN..CIPHERTEXT_LEN].copy_from_slice(tag.as_slice());

    Ok(out)
}

/// Decrypt a 96-byte ciphertext chunk back to an 80-byte plaintext buffer.
pub fn decrypt_chunk(
    session_key: &Secret<[u8; 32]>,
    msg_id: u32,
    chunk_idx: u8,
    ratchet_counter: u64,
    ciphertext: &[u8; CIPHERTEXT_LEN],
) -> Result<[u8; PLAINTEXT_CHUNK_LEN], CryptoError> {
    let raw_nonce = derive_implicit_nonce(session_key.expose_secret(), msg_id, chunk_idx, ratchet_counter);
    let nonce = Nonce::from_slice(&raw_nonce);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(session_key.expose_secret()));

    let mut buffer = [0u8; PLAINTEXT_CHUNK_LEN];
    buffer.copy_from_slice(&ciphertext[0..PLAINTEXT_CHUNK_LEN]);
    let tag = Tag::from_slice(&ciphertext[PLAINTEXT_CHUNK_LEN..CIPHERTEXT_LEN]);

    cipher
        .decrypt_in_place_detached(nonce, b"", &mut buffer, tag)
        .map_err(|_| CryptoError::AuthenticationFailed)?;

    Ok(buffer)
}

/// Sender Key Ratchet Chain for $O(1)$ group message encryption (R12, KTD3).
#[derive(Clone)]
pub struct SenderKeyChain {
    chain_key: Secret<[u8; 32]>,
    counter: u64,
}

impl SenderKeyChain {
    pub fn new(initial_seed: Secret<[u8; 32]>) -> Self {
        Self {
            chain_key: initial_seed,
            counter: 0,
        }
    }

    pub fn counter(&self) -> u64 {
        self.counter
    }

    /// Advance the ratchet chain by one step, producing a distinct message encryption key
    /// and ratcheting the forward-secret chain key forward.
    pub fn step(&mut self) -> (Secret<[u8; 32]>, u64) {
        let current_step = self.counter;

        // Derive message key: K_msg = BLAKE3-KDF(ChainKey, "msg" || counter)
        let mut msg_hasher = blake3::Hasher::new_keyed(self.chain_key.expose_secret());
        msg_hasher.update(b"SENDER-KEY-MSG");
        msg_hasher.update(&current_step.to_be_bytes());
        let msg_key_bytes = *msg_hasher.finalize().as_bytes();

        // Advance chain key: K_next = BLAKE3-KDF(ChainKey, "next")
        let mut next_hasher = blake3::Hasher::new_keyed(self.chain_key.expose_secret());
        next_hasher.update(b"SENDER-KEY-NEXT");
        let next_chain_bytes = *next_hasher.finalize().as_bytes();

        self.chain_key = Secret::new(next_chain_bytes);
        self.counter = self.counter.saturating_add(1);

        (Secret::new(msg_key_bytes), current_step)
    }
}

/// Pairwise X25519 ECDH keypair for 1-on-1 ratcheting and key establishment (R11).
pub struct KeyPair {
    private_key: Secret<[u8; 32]>,
    public_key: [u8; 32],
}

impl KeyPair {
    pub fn from_secret_bytes(bytes: [u8; 32]) -> Self {
        let static_secret = StaticSecret::from(bytes);
        let pub_key = PublicKey::from(&static_secret);
        Self {
            private_key: Secret::new(bytes),
            public_key: *pub_key.as_bytes(),
        }
    }

    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    pub fn diffie_hellman(&self, peer_public: &[u8; 32]) -> Secret<[u8; 32]> {
        let static_secret = StaticSecret::from(*self.private_key.expose_secret());
        let peer_pub = PublicKey::from(*peer_public);
        let shared = static_secret.diffie_hellman(&peer_pub);
        Secret::new(*shared.as_bytes())
    }
}
