//! Embedded SQLite Storage Layer for Project Gibberish (R19, KTD3).
//!
//! Enforces Single-Writer, Single-Owner SQLite storage with WAL mode,
//! startup crash reconciliation, and schema migrations.

pub mod schema;
pub mod store;

pub use schema::{ContactRecord, MessageRecord, MessageStatus, OutboxRecord, OutboxStatus, TrustState};
pub use store::{DatabaseStore, DbError};
