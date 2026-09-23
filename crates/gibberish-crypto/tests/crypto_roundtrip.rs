use gibberish_crypto::ratchet::*;
use gibberish_crypto::secrecy::Secret;

#[test]
fn test_chacha20_roundtrip_with_implicit_nonce() {
    let key = Secret::new([0x42u8; 32]);
    let msg_id = 12345;
    let chunk_idx = 2;
    let ratchet_counter = 77;
    let plaintext = b"Hello, Gibberish encrypted mesh swarm! This is 80B chunk.";

    let ciphertext = encrypt_chunk(&key, msg_id, chunk_idx, ratchet_counter, plaintext)
        .expect("encryption failed");

    // Ciphertext must be 96 bytes (80B body + 16B tag)
    assert_eq!(ciphertext.len(), 96);

    let decrypted = decrypt_chunk(&key, msg_id, chunk_idx, ratchet_counter, &ciphertext)
        .expect("decryption failed");

    assert_eq!(&decrypted[..plaintext.len()], plaintext);

    // Tamper with ciphertext tag
    let mut tampered = ciphertext;
    tampered[95] ^= 0xFF;
    let err = decrypt_chunk(&key, msg_id, chunk_idx, ratchet_counter, &tampered);
    assert_eq!(err, Err(CryptoError::AuthenticationFailed));

    // Wrong ratchet counter should fail authentication because nonce differs
    let wrong_counter = decrypt_chunk(&key, msg_id, chunk_idx, ratchet_counter + 1, &ciphertext);
    assert_eq!(wrong_counter, Err(CryptoError::AuthenticationFailed));
}

#[test]
fn test_sender_key_chain_stepping() {
    let seed = Secret::new([0x13u8; 32]);
    let mut chain = SenderKeyChain::new(seed);

    assert_eq!(chain.counter(), 0);
    let (key0, step0) = chain.step();
    assert_eq!(step0, 0);

    let (key1, step1) = chain.step();
    assert_eq!(step1, 1);

    assert_ne!(key0.expose_secret(), key1.expose_secret());
    assert_eq!(chain.counter(), 2);
}

#[test]
fn test_x25519_diffie_hellman_shared_secret() {
    let alice_priv = [1u8; 32];
    let bob_priv = [2u8; 32];

    let alice = KeyPair::from_secret_bytes(alice_priv);
    let bob = KeyPair::from_secret_bytes(bob_priv);

    let alice_shared = alice.diffie_hellman(bob.public_key());
    let bob_shared = bob.diffie_hellman(alice.public_key());

    assert_eq!(alice_shared.expose_secret(), bob_shared.expose_secret());
}

#[test]
fn test_network_tag_derivation() {
    let swarm_key = [0x77u8; 32];
    let tag1 = derive_network_tag(&swarm_key);
    let tag2 = derive_network_tag(&swarm_key);
    assert_eq!(tag1, tag2);

    let other_key = [0x78u8; 32];
    let tag3 = derive_network_tag(&other_key);
    assert_ne!(tag1, tag3);
}
