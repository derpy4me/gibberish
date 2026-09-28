use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustState {
    Unverified,
    Verified,
}

impl TrustState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TrustState::Unverified => "unverified",
            TrustState::Verified => "verified",
        }
    }

    pub fn from_str_val(s: &str) -> Self {
        match s {
            "verified" => TrustState::Verified,
            _ => TrustState::Unverified,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactRecord {
    pub node_id: u32,
    pub alias: String,
    pub pubkey: [u8; 32],
    pub trust_state: TrustState,
    pub last_seen: i64,
    pub rssi: i16,
    pub lqi: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageStatus {
    Transmitted,
    Delivered,
    Queued,
    Failed,
}

impl MessageStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageStatus::Transmitted => "transmitted",
            MessageStatus::Delivered => "delivered",
            MessageStatus::Queued => "queued",
            MessageStatus::Failed => "failed",
        }
    }

    pub fn from_str_val(s: &str) -> Self {
        match s {
            "delivered" => MessageStatus::Delivered,
            "queued" => MessageStatus::Queued,
            "failed" => MessageStatus::Failed,
            _ => MessageStatus::Transmitted,
        }
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            MessageStatus::Transmitted => "*",
            MessageStatus::Delivered => "[OK]",
            MessageStatus::Queued => "[Q]",
            MessageStatus::Failed => "[!]",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageRecord {
    pub id: String,
    pub convo_id: String,
    pub sender_node_id: u32,
    pub timestamp: i64,
    pub text: String,
    pub status: MessageStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxStatus {
    Pending,
    Sending,
    Sent,
    Failed,
}

impl OutboxStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutboxStatus::Pending => "pending",
            OutboxStatus::Sending => "sending",
            OutboxStatus::Sent => "sent",
            OutboxStatus::Failed => "failed",
        }
    }

    pub fn from_str_val(s: &str) -> Self {
        match s {
            "sending" => OutboxStatus::Sending,
            "sent" => OutboxStatus::Sent,
            "failed" => OutboxStatus::Failed,
            _ => OutboxStatus::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxRecord {
    pub id: String,
    pub dest_node_id: u32,
    pub payload: Vec<u8>,
    pub queued_at: i64,
    pub retry_count: u32,
    pub ttl_secs: u64,
    pub status: OutboxStatus,
}
