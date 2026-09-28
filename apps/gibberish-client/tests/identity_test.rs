use gibberish_client::{
    derive_sas_words, format_sas_words, generate_verification_qr_svg, ingest_announcement_beacon,
    verify_contact_identity,
};
use gibberish_db::{DatabaseStore, TrustState};

#[test]
fn test_sas_derivation_deterministic_and_commutative() {
    let mut pk_alice = [0u8; 32];
    pk_alice[0] = 0xAA;
    pk_alice[31] = 0x01;

    let mut pk_bob = [0u8; 32];
    pk_bob[0] = 0xBB;
    pk_bob[31] = 0x02;

    let words_ab = derive_sas_words(&pk_alice, &pk_bob);
    let words_ba = derive_sas_words(&pk_bob, &pk_alice);

    // Commutative: Alice and Bob see identical words regardless of order
    assert_eq!(words_ab, words_ba);
    assert_eq!(words_ab.len(), 4);

    let formatted = format_sas_words(&words_ab);
    assert!(!formatted.is_empty());
    assert_eq!(formatted.split_whitespace().count(), 4);
}

#[test]
fn test_sas_mismatch_on_impersonation_ae2() {
    // AE2: Attacker node Eve broadcasts alias Alice using Key_Eve
    let mut pk_alice = [0u8; 32];
    pk_alice[0] = 0x01;
    let mut pk_bob = [0u8; 32];
    pk_bob[0] = 0x02;
    let mut pk_eve = [0u8; 32];
    pk_eve[0] = 0x66; // Impersonator key

    let real_alice_bob_words = derive_sas_words(&pk_alice, &pk_bob);
    let eve_bob_words = derive_sas_words(&pk_eve, &pk_bob);

    // Words computed on Bob's screen with Eve MUST NOT match real Alice's screen
    assert_ne!(
        real_alice_bob_words, eve_bob_words,
        "Impersonator key must produce mismatched SAS words"
    );
}

#[test]
fn test_qr_code_svg_generation() {
    let mut pk = [0u8; 32];
    pk[0] = 0xDE;
    pk[31] = 0xAD;

    let svg = generate_verification_qr_svg(0xBEBD82B4, "Alice", &pk).expect("QR generation failed");
    assert!(svg.contains("<svg"), "Output must contain SVG tag");
    assert!(svg.contains("</svg>"), "Output must be closed SVG");
}

#[test]
fn test_airwave_contact_discovery_and_trust_transition_ae1() {
    let db = DatabaseStore::open_in_memory().expect("open memory db");

    let mut pk = [0u8; 32];
    pk[0] = 0x42;

    // AE1: Node appears via radio announcement without prior verification
    let contact = ingest_announcement_beacon(
        &db,
        0xBEBD82B4,
        "Alice_Node",
        &pk,
        -75,
        185,
        1000,
    )
    .expect("ingest beacon failed");

    // Guardrail: must default to Unverified (Amber)
    assert_eq!(contact.trust_state, TrustState::Unverified);

    let stored = db.get_contact(0xBEBD82B4).unwrap().unwrap();
    assert_eq!(stored.trust_state, TrustState::Unverified);

    // Transition to Verified (Green) after verbal SAS confirmation or QR scan
    let verified = verify_contact_identity(&db, 0xBEBD82B4).expect("verify failed");
    assert!(verified);

    let updated = db.get_contact(0xBEBD82B4).unwrap().unwrap();
    assert_eq!(updated.trust_state, TrustState::Verified);

    // Overhearing a subsequent announcement MUST preserve Verified status
    let re_announced = ingest_announcement_beacon(
        &db,
        0xBEBD82B4,
        "Alice_Node",
        &pk,
        -70,
        200,
        1050,
    )
    .expect("re-announce failed");
    assert_eq!(re_announced.trust_state, TrustState::Verified);
}
