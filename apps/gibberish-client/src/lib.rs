//! Slint Native Mesh Messaging Client for Project Gibberish (R1, R2, R3, R4, R5).

pub mod chat;
pub mod controller;
pub mod identity;
pub mod transport;

pub use chat::{
    decrypt_direct_message, decrypt_swarm_broadcast, encrypt_direct_message,
    encrypt_swarm_broadcast, persist_chat_message, ChatError, EncryptedFramePayload,
};
pub use controller::{
    ChatMessageItem, MainWindow, SlintController, StationItem, UiEvent, UiEventSender,
};
pub use identity::{
    derive_sas_words, format_sas_words, generate_verification_qr_svg, ingest_announcement_beacon,
    verify_contact_identity, IdentityError,
};
pub use transport::DesktopIpcTransport;
