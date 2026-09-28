//! Airwave Contact Discovery, Trust State Machine & SAS/QR Verification (R6, R7, R8, R9, KTD6).

use bip39::Language;
use gibberish_db::{ContactRecord, DatabaseStore, DbError, TrustState};
use hkdf::Hkdf;
use qrcode::render::svg;
use qrcode::QrCode;
use sha2::Sha256;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum IdentityError {
    #[error("Database error: {0}")]
    Db(#[from] DbError),
    #[error("QR generation error: {0}")]
    Qr(#[from] qrcode::types::QrError),
    #[error("Contact not found for node ID: 0x{0:08X}")]
    ContactNotFound(u32),
}

/// Derives a deterministic 4-word Short Authentication String (SAS) from two public keys.
/// Uses HKDF-SHA256(ikm = min(pk1, pk2) || max(pk1, pk2), info = "gibberish-sas-v1") mapped into BIP-39.
pub fn derive_sas_words(pk1: &[u8; 32], pk2: &[u8; 32]) -> [String; 4] {
    let (min_pk, max_pk) = if pk1 <= pk2 {
        (pk1, pk2)
    } else {
        (pk2, pk1)
    };

    let mut ikm = [0u8; 64];
    ikm[0..32].copy_from_slice(min_pk);
    ikm[32..64].copy_from_slice(max_pk);

    let hk = Hkdf::<Sha256>::new(None, &ikm);
    let mut okm = [0u8; 8];
    hk.expand(b"gibberish-sas-v1", &mut okm)
        .expect("8 bytes is valid HKDF expansion length");

    let wordlist = Language::English.word_list();

    let idx0 = u16::from_be_bytes([okm[0], okm[1]]) as usize % 2048;
    let idx1 = u16::from_be_bytes([okm[2], okm[3]]) as usize % 2048;
    let idx2 = u16::from_be_bytes([okm[4], okm[5]]) as usize % 2048;
    let idx3 = u16::from_be_bytes([okm[6], okm[7]]) as usize % 2048;

    [
        wordlist[idx0].to_string(),
        wordlist[idx1].to_string(),
        wordlist[idx2].to_string(),
        wordlist[idx3].to_string(),
    ]
}

/// Formats the 4-word SAS mnemonic as a space-separated string.
pub fn format_sas_words(words: &[String; 4]) -> String {
    words.join(" ")
}

/// Generates an SVG representation of an identity verification QR code.
pub fn generate_verification_qr_svg(
    node_id: u32,
    alias: &str,
    pubkey: &[u8; 32],
) -> Result<String, IdentityError> {
    let mut hex_pk = String::with_capacity(64);
    for b in pubkey {
        use std::fmt::Write;
        let _ = write!(&mut hex_pk, "{:02x}", b);
    }

    let payload = format!(
        "gibberish://verify?node_id=0x{:08X}&alias={}&pubkey={}",
        node_id, alias, hex_pk
    );

    let code = QrCode::new(payload.as_bytes())?;
    let svg_string = code.render::<svg::Color>().build();
    Ok(svg_string)
}

/// Ingests an airwave announcement beacon, populating or updating the contact in the database.
/// Defaults newly discovered contacts to Unverified (Amber) trust status (AE1).
pub fn ingest_announcement_beacon(
    db: &DatabaseStore,
    node_id: u32,
    alias: &str,
    pubkey: &[u8; 32],
    rssi: i16,
    lqi: u8,
    timestamp: i64,
) -> Result<ContactRecord, IdentityError> {
    // Preserve existing verified trust state if previously established
    let trust_state = if let Ok(Some(existing)) = db.get_contact(node_id) {
        existing.trust_state
    } else {
        TrustState::Unverified
    };

    let contact = ContactRecord {
        node_id,
        alias: alias.to_string(),
        pubkey: *pubkey,
        trust_state,
        last_seen: timestamp,
        rssi,
        lqi,
    };

    db.upsert_contact(&contact)?;
    Ok(contact)
}

/// Transitions a contact to Verified (Green shield) status in the persistent database (AE2).
pub fn verify_contact_identity(db: &DatabaseStore, node_id: u32) -> Result<bool, IdentityError> {
    let updated = db.set_trust_state(node_id, TrustState::Verified)?;
    if !updated {
        return Err(IdentityError::ContactNotFound(node_id));
    }
    Ok(true)
}
