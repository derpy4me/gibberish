//! Cryptographic vault and key management for Project Gibberish (R3, R4, R11, R12, R30).
//!
//! Note: This crate is used strictly by host daemons and mobile/web clients.
//! Under Zero-Trust architecture, the ESP32-C5 firmware MUST NOT depend on this crate.

pub mod ratchet;
pub mod secrecy;

pub use ratchet::{
    decrypt_chunk, derive_implicit_nonce, derive_network_tag, derive_sender_subkey, encrypt_chunk,
    CryptoError, KeyPair, SenderKeyChain,
};
pub use secrecy::Secret;
