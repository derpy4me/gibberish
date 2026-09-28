use crate::schema::{
    ContactRecord, MessageRecord, MessageStatus, OutboxRecord, OutboxStatus, TrustState,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Database lock poisoned")]
    LockPoisoned,
    #[error("Invalid pubkey length: expected 32 bytes, got {0}")]
    InvalidPubkeyLength(usize),
}

#[derive(Clone)]
pub struct DatabaseStore {
    conn: Arc<Mutex<Connection>>,
}

impl DatabaseStore {
    /// Open SQLite database at given path, configuring WAL mode and running migrations.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;
        Self::initialize_connection(conn)
    }

    /// Open in-memory SQLite database (primarily for testing).
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        Self::initialize_connection(conn)
    }

    fn initialize_connection(conn: Connection) -> Result<Self, DbError> {
        // Enforce performance and concurrency PRAGMAs
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        // Run migrations strictly once before accepting queries
        store.run_migrations()?;

        // Perform startup crash reconciliation
        store.reconcile_crashed_outbox()?;

        Ok(store)
    }

    /// Access database connection, recovering gracefully from lock poisoning if a previous thread panicked.
    fn get_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn run_migrations(&self) -> Result<(), DbError> {
        let conn = self.get_conn();

        conn.execute(
            "CREATE TABLE IF NOT EXISTS _schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at INTEGER NOT NULL
            );",
            [],
        )?;

        let current_version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM _schema_migrations;",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if current_version < 1 {
            let tx = conn.unchecked_transaction()?;

            tx.execute(
                "CREATE TABLE IF NOT EXISTS contacts (
                    node_id INTEGER PRIMARY KEY,
                    alias TEXT NOT NULL,
                    pubkey BLOB NOT NULL,
                    trust_state TEXT NOT NULL,
                    last_seen INTEGER NOT NULL,
                    rssi INTEGER NOT NULL,
                    lqi INTEGER NOT NULL
                );",
                [],
            )?;

            tx.execute(
                "CREATE TABLE IF NOT EXISTS messages (
                    id TEXT PRIMARY KEY,
                    convo_id TEXT NOT NULL,
                    sender_node_id INTEGER NOT NULL,
                    timestamp INTEGER NOT NULL,
                    text TEXT NOT NULL,
                    status TEXT NOT NULL
                );",
                [],
            )?;

            tx.execute(
                "CREATE INDEX IF NOT EXISTS idx_messages_convo 
                 ON messages (convo_id, timestamp ASC);",
                [],
            )?;

            tx.execute(
                "CREATE TABLE IF NOT EXISTS outbox (
                    id TEXT PRIMARY KEY,
                    dest_node_id INTEGER NOT NULL,
                    payload BLOB NOT NULL,
                    queued_at INTEGER NOT NULL,
                    retry_count INTEGER NOT NULL,
                    ttl_secs INTEGER NOT NULL,
                    status TEXT NOT NULL
                );",
                [],
            )?;

            tx.execute(
                "CREATE INDEX IF NOT EXISTS idx_outbox_dest_status 
                 ON outbox (dest_node_id, status);",
                [],
            )?;

            tx.execute(
                "INSERT INTO _schema_migrations (version, applied_at) VALUES (1, ?1);",
                params![chrono_now()],
            )?;

            tx.commit()?;
        }

        Ok(())
    }

    /// Reconciles dangling 'sending' status rows in outbox back to 'pending' on startup.
    pub fn reconcile_crashed_outbox(&self) -> Result<usize, DbError> {
        let conn = self.get_conn();
        let count = conn.execute(
            "UPDATE outbox SET status = 'pending' WHERE status = 'sending';",
            [],
        )?;
        Ok(count)
    }

    // --- Contacts API ---

    pub fn upsert_contact(&self, contact: &ContactRecord) -> Result<(), DbError> {
        let conn = self.get_conn();
        conn.execute(
            "INSERT INTO contacts (node_id, alias, pubkey, trust_state, last_seen, rssi, lqi)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(node_id) DO UPDATE SET
                alias = excluded.alias,
                pubkey = excluded.pubkey,
                last_seen = excluded.last_seen,
                rssi = excluded.rssi,
                lqi = excluded.lqi;",
            params![
                contact.node_id as i64,
                contact.alias,
                &contact.pubkey[..],
                contact.trust_state.as_str(),
                contact.last_seen,
                contact.rssi as i32,
                contact.lqi as i32,
            ],
        )?;
        Ok(())
    }

    pub fn get_contact(&self, node_id: u32) -> Result<Option<ContactRecord>, DbError> {
        let conn = self.get_conn();
        let mut stmt = conn.prepare(
            "SELECT node_id, alias, pubkey, trust_state, last_seen, rssi, lqi
             FROM contacts WHERE node_id = ?1;",
        )?;

        let contact = stmt
            .query_row(params![node_id as i64], |row| {
                let node_id: i64 = row.get(0)?;
                let alias: String = row.get(1)?;
                let pubkey_bytes: Vec<u8> = row.get(2)?;
                let trust_str: String = row.get(3)?;
                let last_seen: i64 = row.get(4)?;
                let rssi: i32 = row.get(5)?;
                let lqi: i32 = row.get(6)?;

                let mut pubkey = [0u8; 32];
                if pubkey_bytes.len() == 32 {
                    pubkey.copy_from_slice(&pubkey_bytes);
                }

                Ok(ContactRecord {
                    node_id: node_id as u32,
                    alias,
                    pubkey,
                    trust_state: TrustState::from_str_val(&trust_str),
                    last_seen,
                    rssi: rssi as i16,
                    lqi: lqi as u8,
                })
            })
            .optional()?;

        Ok(contact)
    }

    pub fn list_contacts(&self) -> Result<Vec<ContactRecord>, DbError> {
        let conn = self.get_conn();
        let mut stmt = conn.prepare(
            "SELECT node_id, alias, pubkey, trust_state, last_seen, rssi, lqi
             FROM contacts ORDER BY last_seen DESC;",
        )?;

        let rows = stmt.query_map([], |row| {
            let node_id: i64 = row.get(0)?;
            let alias: String = row.get(1)?;
            let pubkey_bytes: Vec<u8> = row.get(2)?;
            let trust_str: String = row.get(3)?;
            let last_seen: i64 = row.get(4)?;
            let rssi: i32 = row.get(5)?;
            let lqi: i32 = row.get(6)?;

            let mut pubkey = [0u8; 32];
            if pubkey_bytes.len() == 32 {
                pubkey.copy_from_slice(&pubkey_bytes);
            }

            Ok(ContactRecord {
                node_id: node_id as u32,
                alias,
                pubkey,
                trust_state: TrustState::from_str_val(&trust_str),
                last_seen,
                rssi: rssi as i16,
                lqi: lqi as u8,
            })
        })?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    }

    pub fn set_trust_state(&self, node_id: u32, trust: TrustState) -> Result<bool, DbError> {
        let conn = self.get_conn();
        let rows_affected = conn.execute(
            "UPDATE contacts SET trust_state = ?1 WHERE node_id = ?2;",
            params![trust.as_str(), node_id as i64],
        )?;
        Ok(rows_affected > 0)
    }

    pub fn update_contact_metrics(
        &self,
        node_id: u32,
        rssi: i16,
        lqi: u8,
        timestamp: i64,
    ) -> Result<(), DbError> {
        let conn = self.get_conn();
        conn.execute(
            "UPDATE contacts SET rssi = ?1, lqi = ?2, last_seen = ?3 WHERE node_id = ?4;",
            params![rssi as i32, lqi as i32, timestamp, node_id as i64],
        )?;
        Ok(())
    }

    // --- Messages API ---

    pub fn insert_message(&self, msg: &MessageRecord) -> Result<(), DbError> {
        let conn = self.get_conn();
        conn.execute(
            "INSERT INTO messages (id, convo_id, sender_node_id, timestamp, text, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                status = excluded.status,
                text = excluded.text;",
            params![
                msg.id,
                msg.convo_id,
                msg.sender_node_id as i64,
                msg.timestamp,
                msg.text,
                msg.status.as_str(),
            ],
        )?;
        Ok(())
    }

    pub fn update_message_status(&self, id: &str, status: MessageStatus) -> Result<bool, DbError> {
        let conn = self.get_conn();
        let rows = conn.execute(
            "UPDATE messages SET status = ?1 WHERE id = ?2;",
            params![status.as_str(), id],
        )?;
        Ok(rows > 0)
    }

    pub fn get_message(&self, id: &str) -> Result<Option<MessageRecord>, DbError> {
        let conn = self.get_conn();
        let mut stmt = conn.prepare(
            "SELECT id, convo_id, sender_node_id, timestamp, text, status
             FROM messages WHERE id = ?1;",
        )?;

        let msg = stmt
            .query_row(params![id], |row| {
                let id: String = row.get(0)?;
                let convo_id: String = row.get(1)?;
                let sender: i64 = row.get(2)?;
                let timestamp: i64 = row.get(3)?;
                let text: String = row.get(4)?;
                let status_str: String = row.get(5)?;

                Ok(MessageRecord {
                    id,
                    convo_id,
                    sender_node_id: sender as u32,
                    timestamp,
                    text,
                    status: MessageStatus::from_str_val(&status_str),
                })
            })
            .optional()?;

        Ok(msg)
    }

    pub fn list_messages(
        &self,
        convo_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<MessageRecord>, DbError> {
        let conn = self.get_conn();
        let mut stmt = conn.prepare(
            "SELECT id, convo_id, sender_node_id, timestamp, text, status
             FROM messages WHERE convo_id = ?1
             ORDER BY timestamp ASC
             LIMIT ?2 OFFSET ?3;",
        )?;

        let rows = stmt.query_map(params![convo_id, limit as i64, offset as i64], |row| {
            let id: String = row.get(0)?;
            let convo_id: String = row.get(1)?;
            let sender: i64 = row.get(2)?;
            let timestamp: i64 = row.get(3)?;
            let text: String = row.get(4)?;
            let status_str: String = row.get(5)?;

            Ok(MessageRecord {
                id,
                convo_id,
                sender_node_id: sender as u32,
                timestamp,
                text,
                status: MessageStatus::from_str_val(&status_str),
            })
        })?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    }

    // --- Outbox API ---

    pub fn insert_outbox(&self, record: &OutboxRecord) -> Result<(), DbError> {
        let conn = self.get_conn();
        conn.execute(
            "INSERT INTO outbox (id, dest_node_id, payload, queued_at, retry_count, ttl_secs, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                payload = excluded.payload,
                retry_count = excluded.retry_count,
                status = excluded.status;",
            params![
                record.id,
                record.dest_node_id as i64,
                record.payload,
                record.queued_at,
                record.retry_count as i64,
                record.ttl_secs as i64,
                record.status.as_str(),
            ],
        )?;
        Ok(())
    }

    pub fn get_outbox(&self, id: &str) -> Result<Option<OutboxRecord>, DbError> {
        let conn = self.get_conn();
        let mut stmt = conn.prepare(
            "SELECT id, dest_node_id, payload, queued_at, retry_count, ttl_secs, status
             FROM outbox WHERE id = ?1;",
        )?;

        let record = stmt
            .query_row(params![id], |row| {
                let id: String = row.get(0)?;
                let dest: i64 = row.get(1)?;
                let payload: Vec<u8> = row.get(2)?;
                let queued_at: i64 = row.get(3)?;
                let retry_count: i64 = row.get(4)?;
                let ttl_secs: i64 = row.get(5)?;
                let status_str: String = row.get(6)?;

                Ok(OutboxRecord {
                    id,
                    dest_node_id: dest as u32,
                    payload,
                    queued_at,
                    retry_count: retry_count as u32,
                    ttl_secs: ttl_secs as u64,
                    status: OutboxStatus::from_str_val(&status_str),
                })
            })
            .optional()?;

        Ok(record)
    }

    pub fn list_pending_outbox_for_node(
        &self,
        dest_node_id: u32,
    ) -> Result<Vec<OutboxRecord>, DbError> {
        let conn = self.get_conn();
        let mut stmt = conn.prepare(
            "SELECT id, dest_node_id, payload, queued_at, retry_count, ttl_secs, status
             FROM outbox WHERE dest_node_id = ?1 AND status IN ('pending', 'sending')
             ORDER BY queued_at ASC;",
        )?;

        let rows = stmt.query_map(params![dest_node_id as i64], |row| {
            let id: String = row.get(0)?;
            let dest: i64 = row.get(1)?;
            let payload: Vec<u8> = row.get(2)?;
            let queued_at: i64 = row.get(3)?;
            let retry_count: i64 = row.get(4)?;
            let ttl_secs: i64 = row.get(5)?;
            let status_str: String = row.get(6)?;

            Ok(OutboxRecord {
                id,
                dest_node_id: dest as u32,
                payload,
                queued_at,
                retry_count: retry_count as u32,
                ttl_secs: ttl_secs as u64,
                status: OutboxStatus::from_str_val(&status_str),
            })
        })?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    }

    pub fn list_all_outbox(&self) -> Result<Vec<OutboxRecord>, DbError> {
        let conn = self.get_conn();
        let mut stmt = conn.prepare(
            "SELECT id, dest_node_id, payload, queued_at, retry_count, ttl_secs, status
             FROM outbox ORDER BY queued_at ASC;",
        )?;

        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let dest: i64 = row.get(1)?;
            let payload: Vec<u8> = row.get(2)?;
            let queued_at: i64 = row.get(3)?;
            let retry_count: i64 = row.get(4)?;
            let ttl_secs: i64 = row.get(5)?;
            let status_str: String = row.get(6)?;

            Ok(OutboxRecord {
                id,
                dest_node_id: dest as u32,
                payload,
                queued_at,
                retry_count: retry_count as u32,
                ttl_secs: ttl_secs as u64,
                status: OutboxStatus::from_str_val(&status_str),
            })
        })?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    }

    pub fn update_outbox_status(&self, id: &str, status: OutboxStatus) -> Result<bool, DbError> {
        let conn = self.get_conn();
        let rows = conn.execute(
            "UPDATE outbox SET status = ?1 WHERE id = ?2;",
            params![status.as_str(), id],
        )?;
        Ok(rows > 0)
    }

    pub fn mark_outbox_sending(&self, ids: &[&str]) -> Result<(), DbError> {
        let conn = self.get_conn();
        for id in ids {
            conn.execute(
                "UPDATE outbox SET status = 'sending' WHERE id = ?1;",
                params![id],
            )?;
        }
        Ok(())
    }

    pub fn mark_outbox_sent(&self, ids: &[&str]) -> Result<(), DbError> {
        let conn = self.get_conn();
        for id in ids {
            conn.execute(
                "UPDATE outbox SET status = 'sent' WHERE id = ?1;",
                params![id],
            )?;
            // Also update messages table if corresponding message exists
            conn.execute(
                "UPDATE messages SET status = 'delivered' WHERE id = ?1;",
                params![id],
            )?;
        }
        Ok(())
    }

    pub fn increment_retry_count(&self, id: &str) -> Result<(), DbError> {
        let conn = self.get_conn();
        conn.execute(
            "UPDATE outbox SET retry_count = retry_count + 1 WHERE id = ?1;",
            params![id],
        )?;
        Ok(())
    }

    /// Evicts expired outbox items where `queued_at + ttl_secs < current_time`.
    /// Updates outbox status to `failed` and messages status to `failed`.
    /// Returns vector of evicted message IDs.
    pub fn evict_expired_outbox(&self, current_time: i64) -> Result<Vec<String>, DbError> {
        let conn = self.get_conn();
        let mut stmt = conn.prepare(
            "SELECT id FROM outbox 
             WHERE status IN ('pending', 'sending') 
               AND (queued_at + ttl_secs) < ?1;",
        )?;

        let ids: Vec<String> = stmt
            .query_map(params![current_time], |row| row.get(0))?
            .filter_map(Result::ok)
            .collect();

        for id in &ids {
            conn.execute(
                "UPDATE outbox SET status = 'failed' WHERE id = ?1;",
                params![id],
            )?;
            conn.execute(
                "UPDATE messages SET status = 'failed' WHERE id = ?1;",
                params![id],
            )?;
        }

        Ok(ids)
    }
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
