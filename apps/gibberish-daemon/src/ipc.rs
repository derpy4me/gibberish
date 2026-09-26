//! Local WebSocket JSON-RPC Server on 127.0.0.1:4483 (R16, R17, KTD1, KTD3).

use futures_util::{SinkExt, StreamExt};
use gibberish_db::{
    DatabaseStore, MessageRecord, MessageStatus, OutboxRecord, OutboxStatus,
    TrustState,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

pub const IPC_PORT: u16 = 4483;

static MSG_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundMeshMessage {
    pub convo_id: String,
    pub dest_node_id: Option<u32>,
    pub text: String,
    pub message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Clone)]
pub struct IpcServer {
    addr: SocketAddr,
    db: DatabaseStore,
    broadcast_tx: broadcast::Sender<String>,
    outbound_tx: tokio::sync::mpsc::UnboundedSender<OutboundMeshMessage>,
    local_node_id: Arc<AtomicU32>,
    storage_mode: Arc<RwLock<String>>,
    dongle_attached: Arc<AtomicBool>,
}

impl Default for IpcServer {
    fn default() -> Self {
        Self::new()
    }
}

impl IpcServer {
    pub fn new() -> Self {
        let (server, _) = Self::channel();
        server
    }

    pub fn channel() -> (Self, tokio::sync::mpsc::UnboundedReceiver<OutboundMeshMessage>) {
        let db = DatabaseStore::open("/tmp/gibberish/store.db")
            .unwrap_or_else(|_| DatabaseStore::open_in_memory().expect("open memory db"));
        let (broadcast_tx, _) = broadcast::channel(512);
        let (outbound_tx, outbound_rx) = tokio::sync::mpsc::unbounded_channel();

        let server = Self {
            addr: SocketAddr::from(([127, 0, 0, 1], IPC_PORT)),
            db,
            broadcast_tx,
            outbound_tx,
            local_node_id: Arc::new(AtomicU32::new(0)),
            storage_mode: Arc::new(RwLock::new("RAM_ONLY".to_string())),
            dongle_attached: Arc::new(AtomicBool::new(true)),
        };
        (server, outbound_rx)
    }

    pub fn with_db(addr: SocketAddr, db: DatabaseStore) -> Self {
        let (broadcast_tx, _) = broadcast::channel(512);
        let (outbound_tx, _) = tokio::sync::mpsc::unbounded_channel();
        Self {
            addr,
            db,
            broadcast_tx,
            outbound_tx,
            local_node_id: Arc::new(AtomicU32::new(0)),
            storage_mode: Arc::new(RwLock::new("RAM_ONLY".to_string())),
            dongle_attached: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn set_local_node_id(&self, id: u32) {
        self.local_node_id.store(id, Ordering::Relaxed);
    }

    pub fn set_storage_mode(&self, mode: &str) {
        if let Ok(mut g) = self.storage_mode.write() {
            *g = mode.to_string();
        }
    }

    pub fn set_dongle_attached(&self, attached: bool) {
        self.dongle_attached.store(attached, Ordering::Relaxed);
    }

    pub fn broadcaster(&self) -> broadcast::Sender<String> {
        self.broadcast_tx.clone()
    }

    pub fn db(&self) -> &DatabaseStore {
        &self.db
    }

    pub fn broadcast_event(&self, method: &str, params: serde_json::Value) {
        let notification = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };
        if let Ok(json_str) = serde_json::to_string(&notification) {
            let _ = self.broadcast_tx.send(json_str);
        }
    }

    pub fn notify_rx_message(
        &self,
        id: &str,
        convo_id: &str,
        sender_node_id: u32,
        text: &str,
        timestamp: i64,
        status: &str,
    ) {
        self.broadcast_event(
            "rx_message",
            serde_json::json!({
                "id": id,
                "convo_id": convo_id,
                "sender_node_id": sender_node_id,
                "text": text,
                "timestamp": timestamp,
                "status": status,
            }),
        );
    }

    pub fn notify_node_discovered(
        &self,
        node_id: u32,
        alias: &str,
        trust_state: &str,
        rssi: i16,
        lqi: u8,
    ) {
        self.broadcast_event(
            "node_discovered",
            serde_json::json!({
                "node_id": node_id,
                "alias": alias,
                "trust_state": trust_state,
                "rssi": rssi,
                "lqi": lqi,
            }),
        );
    }

    pub fn notify_delivery_ack(&self, message_id: &str, status: &str) {
        self.broadcast_event(
            "delivery_ack",
            serde_json::json!({
                "message_id": message_id,
                "status": status,
            }),
        );
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let listener = TcpListener::bind(self.addr).await?;
        println!("WebSocket IPC listening on ws://{}", self.addr);

        while let Ok((stream, _)) = listener.accept().await {
            let server_clone = self.clone();
            tokio::spawn(async move {
                handle_connection(stream, server_clone).await;
            });
        }

        Ok(())
    }
}

async fn handle_connection(stream: TcpStream, server: IpcServer) {
    if let Ok(mut ws_stream) = accept_async(stream).await {
        let mut bcast_rx = server.broadcast_tx.subscribe();

        loop {
            tokio::select! {
                // Incoming client request
                msg = ws_stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(&text) {
                                let res = handle_rpc(&server, req);
                                if let Ok(res_str) = serde_json::to_string(&res) {
                                    if ws_stream.send(Message::Text(res_str)).await.is_err() {
                                        break;
                                    }
                                }
                            } else {
                                // Invalid JSON-RPC request format
                                let err_res = JsonRpcResponse {
                                    jsonrpc: "2.0".to_string(),
                                    id: 0,
                                    result: None,
                                    error: Some(JsonRpcError {
                                        code: -32600,
                                        message: "Invalid Request".to_string(),
                                        data: None,
                                    }),
                                };
                                if let Ok(res_str) = serde_json::to_string(&err_res) {
                                    let _ = ws_stream.send(Message::Text(res_str)).await;
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        _ => {}
                    }
                }

                // Broadcast event to client
                bcast_msg = bcast_rx.recv() => {
                    if let Ok(event_text) = bcast_msg {
                        if ws_stream.send(Message::Text(event_text)).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }
}

fn handle_rpc(server: &IpcServer, req: JsonRpcRequest) -> JsonRpcResponse {
    match req.method.as_str() {
        "status" => {
            let node_id = server.local_node_id.load(Ordering::Relaxed);
            let mode = server
                .storage_mode
                .read()
                .map(|m| m.clone())
                .unwrap_or_else(|_| "RAM_ONLY".to_string());
            let attached = server.dongle_attached.load(Ordering::Relaxed);
            let node_hex = if node_id != 0 {
                format!("0x{:08X}", node_id)
            } else {
                "UNKNOWN".to_string()
            };
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(serde_json::json!({
                    "status": "connected",
                    "dongle_attached": attached,
                    "dongle_mode": mode,
                    "storage_mode": mode,
                    "local_node_id": node_hex,
                    "node_id": node_hex,
                    "mesh_active": true
                })),
                error: None,
            }
        }
        "version" => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(serde_json::json!({ "version": "0.1.0", "app": "gibberishd" })),
            error: None,
        },
        "send_dm" => {
            let params = match req.params {
                Some(p) => p,
                None => {
                    return rpc_error(req.id, -32602, "Missing params for send_dm");
                }
            };

            let dest_node_id = if let Some(n) = params.get("dest_node_id") {
                if let Some(num) = n.as_u64() {
                    num as u32
                } else if let Some(s) = n.as_str() {
                    let clean = s.trim_start_matches("0x");
                    u32::from_str_radix(clean, 16).unwrap_or(0)
                } else {
                    return rpc_error(req.id, -32602, "Invalid dest_node_id in send_dm");
                }
            } else {
                return rpc_error(req.id, -32602, "dest_node_id required for send_dm");
            };

            let text = match params.get("text").and_then(|t| t.as_str()) {
                Some(t) => t.to_string(),
                None => return rpc_error(req.id, -32602, "text required for send_dm"),
            };

            let now = now_secs();
            let msg_id = format!("dm-{}", MSG_COUNTER.fetch_add(1, Ordering::Relaxed));
            let convo_id = format!("0x{:08X}", dest_node_id);

            // Persist message marked queued
            let msg_rec = MessageRecord {
                id: msg_id.clone(),
                convo_id: convo_id.clone(),
                sender_node_id: 0, // local node
                timestamp: now,
                text: text.clone(),
                status: MessageStatus::Queued,
            };
            let _ = server.db.insert_message(&msg_rec);

            // Queue in outbox
            let outbox_rec = OutboxRecord {
                id: msg_id.clone(),
                dest_node_id,
                payload: text.clone().into_bytes(),
                queued_at: now,
                retry_count: 0,
                ttl_secs: 48 * 3600,
                status: OutboxStatus::Pending,
            };
            let _ = server.db.insert_outbox(&outbox_rec);

            // Forward to physical RF transport pipeline
            let _ = server.outbound_tx.send(OutboundMeshMessage {
                convo_id: convo_id.clone(),
                dest_node_id: Some(dest_node_id),
                text: text.clone(),
                message_id: msg_id.clone(),
            });

            server.notify_rx_message(&msg_id, &convo_id, 0, &msg_rec.text, now, "[Q]");

            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(serde_json::json!({
                    "message_id": msg_id,
                    "status": "queued"
                })),
                error: None,
            }
        }
        "send_swarm" => {
            let params = match req.params {
                Some(p) => p,
                None => return rpc_error(req.id, -32602, "Missing params for send_swarm"),
            };

            let text = match params.get("text").and_then(|t| t.as_str()) {
                Some(t) => t.to_string(),
                None => return rpc_error(req.id, -32602, "text required for send_swarm"),
            };

            let now = now_secs();
            let msg_id = format!("swarm-{}", MSG_COUNTER.fetch_add(1, Ordering::Relaxed));

            let msg_rec = MessageRecord {
                id: msg_id.clone(),
                convo_id: "#all".to_string(),
                sender_node_id: 0,
                timestamp: now,
                text: text.clone(),
                status: MessageStatus::Transmitted,
            };
            let _ = server.db.insert_message(&msg_rec);

            // Forward to physical RF transport pipeline
            let _ = server.outbound_tx.send(OutboundMeshMessage {
                convo_id: "#all".to_string(),
                dest_node_id: None,
                text: text.clone(),
                message_id: msg_id.clone(),
            });

            server.notify_rx_message(&msg_id, "#all", 0, &text, now, "*");

            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(serde_json::json!({
                    "message_id": msg_id,
                    "status": "transmitted"
                })),
                error: None,
            }
        }
        "list_contacts" => {
            let contacts = server.db.list_contacts().unwrap_or_default();
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(serde_json::to_value(&contacts).unwrap_or(serde_json::json!([]))),
                error: None,
            }
        }
        "verify_contact" => {
            let params = match req.params {
                Some(p) => p,
                None => return rpc_error(req.id, -32602, "Missing params for verify_contact"),
            };

            let node_id = if let Some(n) = params.get("node_id") {
                if let Some(num) = n.as_u64() {
                    num as u32
                } else if let Some(s) = n.as_str() {
                    let clean = s.trim_start_matches("0x");
                    u32::from_str_radix(clean, 16).unwrap_or(0)
                } else {
                    return rpc_error(req.id, -32602, "Invalid node_id in verify_contact");
                }
            } else {
                return rpc_error(req.id, -32602, "node_id required for verify_contact");
            };

            let verified = params
                .get("verified")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            let trust = if verified {
                TrustState::Verified
            } else {
                TrustState::Unverified
            };

            let updated = server
                .db
                .set_trust_state(node_id, trust)
                .unwrap_or(false);

            if let Ok(Some(contact)) = server.db.get_contact(node_id) {
                server.notify_node_discovered(
                    contact.node_id,
                    &contact.alias,
                    contact.trust_state.as_str(),
                    contact.rssi,
                    contact.lqi,
                );
            }

            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(serde_json::json!({
                    "node_id": node_id,
                    "verified": verified,
                    "updated": updated
                })),
                error: None,
            }
        }
        "list_messages" => {
            let params = match req.params {
                Some(p) => p,
                None => return rpc_error(req.id, -32602, "Missing params for list_messages"),
            };

            let convo_id = match params.get("convo_id").and_then(|c| c.as_str()) {
                Some(c) => c,
                None => return rpc_error(req.id, -32602, "convo_id required for list_messages"),
            };

            let limit = params.get("limit").and_then(|l| l.as_u64()).unwrap_or(50) as usize;
            let offset = params.get("offset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;

            let messages = server
                .db
                .list_messages(convo_id, limit, offset)
                .unwrap_or_default();

            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(serde_json::to_value(&messages).unwrap_or(serde_json::json!([]))),
                error: None,
            }
        }
        _ => rpc_error(
            req.id,
            -32601,
            &format!("Method not found: '{}'", req.method),
        ),
    }
}

fn rpc_error(id: u64, code: i32, msg: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: msg.to_string(),
            data: None,
        }),
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
