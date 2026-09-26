//! WebSocket JSON-RPC Desktop Transport Client (R16, R17, KTD2).

use crate::controller::{ChatMessageItem, UiEvent, UiEventSender};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
}

#[derive(Clone)]
pub struct DesktopIpcTransport {
    url: String,
    ui_sender: UiEventSender,
    outbound_tx: tokio::sync::mpsc::UnboundedSender<String>,
    outbound_rx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<String>>>>,
    req_counter: Arc<AtomicU64>,
}

impl DesktopIpcTransport {
    pub fn new(url: &str, ui_sender: UiEventSender) -> Self {
        let (outbound_tx, outbound_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            url: url.to_string(),
            ui_sender,
            outbound_tx,
            outbound_rx: Arc::new(Mutex::new(Some(outbound_rx))),
            req_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Spawns background connection loop that maintains WebSocket connection to gibberishd.
    pub fn start(&self) {
        let transport = self.clone();
        tokio::spawn(async move {
            let rx_opt = {
                let mut guard = transport.outbound_rx.lock().await;
                guard.take()
            };
            if let Some(rx) = rx_opt {
                transport.run_loop(rx).await;
            } else {
                log::error!("DesktopIpcTransport already started");
            }
        });
    }

    pub fn send_swarm(&self, text: &str) -> Result<(), &'static str> {
        let id = self.req_counter.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: "send_swarm".to_string(),
            params: Some(serde_json::json!({ "text": text })),
        };
        let json_str = serde_json::to_string(&req).map_err(|_| "Serialization error")?;
        self.outbound_tx
            .send(json_str)
            .map_err(|_| "Transport channel closed")?;
        Ok(())
    }

    pub fn send_dm(&self, dest_node_id: u32, text: &str) -> Result<(), &'static str> {
        let id = self.req_counter.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: "send_dm".to_string(),
            params: Some(serde_json::json!({
                "dest_node_id": format!("0x{:08X}", dest_node_id),
                "text": text,
            })),
        };
        let json_str = serde_json::to_string(&req).map_err(|_| "Serialization error")?;
        self.outbound_tx
            .send(json_str)
            .map_err(|_| "Transport channel closed")?;
        Ok(())
    }

    pub fn verify_contact(&self, node_id: u32, verified: bool) -> Result<(), &'static str> {
        let id = self.req_counter.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: "verify_contact".to_string(),
            params: Some(serde_json::json!({
                "node_id": format!("0x{:08X}", node_id),
                "verified": verified,
            })),
        };
        let json_str = serde_json::to_string(&req).map_err(|_| "Serialization error")?;
        self.outbound_tx
            .send(json_str)
            .map_err(|_| "Transport channel closed")?;
        Ok(())
    }

    pub fn get_status(&self) -> Result<(), &'static str> {
        let id = self.req_counter.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: "status".to_string(),
            params: None,
        };
        let json_str = serde_json::to_string(&req).map_err(|_| "Serialization error")?;
        self.outbound_tx
            .send(json_str)
            .map_err(|_| "Transport channel closed")?;
        Ok(())
    }

    pub fn list_contacts(&self) -> Result<(), &'static str> {
        let id = self.req_counter.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: "list_contacts".to_string(),
            params: None,
        };
        let json_str = serde_json::to_string(&req).map_err(|_| "Serialization error")?;
        self.outbound_tx
            .send(json_str)
            .map_err(|_| "Transport channel closed")?;
        Ok(())
    }

    pub fn list_messages(
        &self,
        convo_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(), &'static str> {
        let id = self.req_counter.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: "list_messages".to_string(),
            params: Some(serde_json::json!({
                "convo_id": convo_id,
                "limit": limit,
                "offset": offset,
            })),
        };
        let json_str = serde_json::to_string(&req).map_err(|_| "Serialization error")?;
        self.outbound_tx
            .send(json_str)
            .map_err(|_| "Transport channel closed")?;
        Ok(())
    }

    pub async fn run_loop(&self, mut outbound_rx: tokio::sync::mpsc::UnboundedReceiver<String>) {
        loop {
            log::info!("Connecting to gibberishd at {}", self.url);
            match connect_async(&self.url).await {
                Ok((ws_stream, _)) => {
                    log::info!("Connected to gibberishd WebSocket IPC");
                    let _ = self.ui_sender.send(UiEvent::TelemetryUpdated {
                        node_id: "0xBEBD82B4".to_string(),
                        storage_mode: "SD ACTIVE".to_string(),
                        storage_stats: "SRAM: 40/256 KB".to_string(),
                        tx: 0,
                        rx: 0,
                        channel: 15,
                        avg_lqi: 200,
                        status: "CONNECTED".to_string(),
                    });

                    // Query live daemon status and contacts on connect
                    let _ = self.get_status();
                    let _ = self.list_contacts();

                    let (mut ws_write, mut ws_read) = ws_stream.split();

                    loop {
                        tokio::select! {
                            Some(outbound) = outbound_rx.recv() => {
                                if ws_write.send(Message::Text(outbound)).await.is_err() {
                                    break;
                                }
                            }
                            Some(msg_res) = ws_read.next() => {
                                match msg_res {
                                    Ok(Message::Text(text)) => {
                                        self.handle_inbound_message(&text);
                                    }
                                    Ok(Message::Close(_)) | Err(_) => {
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            else => break,
                        }
                    }

                    log::warn!("Disconnected from gibberishd, reconnecting...");
                    let _ = self.ui_sender.send(UiEvent::TelemetryUpdated {
                        node_id: "0xBEBD82B4".to_string(),
                        storage_mode: "SD ACTIVE".to_string(),
                        storage_stats: "SRAM: 40/256 KB".to_string(),
                        tx: 0,
                        rx: 0,
                        channel: 15,
                        avg_lqi: 0,
                        status: "DISCONNECTED".to_string(),
                    });
                }
                Err(e) => {
                    log::debug!("Failed to connect to gibberishd: {}", e);
                }
            }

            sleep(Duration::from_millis(1500)).await;
        }
    }

    fn handle_inbound_message(&self, text: &str) {
        // Try parsing as notification first
        if let Ok(notif) = serde_json::from_str::<JsonRpcNotification>(text) {
            match notif.method.as_str() {
                "rx_message" => {
                    let convo = notif
                        .params
                        .get("convo_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("#all");
                    let sender = notif
                        .params
                        .get("sender_node_id")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let body = notif
                        .params
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let ts = notif
                        .params
                        .get("timestamp")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let status = notif
                        .params
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("*");
                    let notif_id = notif.params.get("id").and_then(|v| v.as_str());
                    let item_id: slint::SharedString = match notif_id {
                        Some(id_str) => id_str.into(),
                        None => {
                            static FALLBACK_COUNTER: std::sync::atomic::AtomicU64 =
                                std::sync::atomic::AtomicU64::new(1);
                            let c = FALLBACK_COUNTER
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            format!("m-{}-{}", ts, c).into()
                        }
                    };

                    let _ = self.ui_sender.send(UiEvent::MessageReceived(ChatMessageItem {
                        id: item_id,
                        convo_id: convo.into(),
                        sender: if sender == 0 {
                            "Me".into()
                        } else {
                            format!("0x{:08X}", sender).into()
                        },
                        text: body.into(),
                        timestamp: format_ts(ts).into(),
                        status: status.into(),
                        is_outgoing: sender == 0,
                    }));
                }
                "node_discovered" => {
                    let node_id = notif
                        .params
                        .get("node_id")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    let alias = notif
                        .params
                        .get("alias")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let trust = notif
                        .params
                        .get("trust_state")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unverified");
                    let rssi = notif
                        .params
                        .get("rssi")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(-80) as i16;
                    let lqi = notif
                        .params
                        .get("lqi")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(150) as u8;

                    let _ = self.ui_sender.send(UiEvent::StationDiscovered {
                        node_id: format!("0x{:08X}", node_id),
                        alias: alias.to_string(),
                        rssi: format!("{} dBm", rssi),
                        lqi: format!("LQI {}", lqi),
                        trust_state: trust.to_string(),
                    });
                }
                "delivery_ack" => {
                    let msg_id = notif
                        .params
                        .get("message_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let status = notif
                        .params
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("[OK]");
                    let _ = self.ui_sender.send(UiEvent::MessageStatusUpdated {
                        id: msg_id.to_string(),
                        status: status.to_string(),
                    });
                }
                "telemetry_update" => {
                    let node_id = notif
                        .params
                        .get("node_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("0xBEBD82B4");
                    let storage_mode = notif
                        .params
                        .get("storage_mode")
                        .and_then(|v| v.as_str())
                        .unwrap_or("SD ACTIVE");
                    let tx = notif
                        .params
                        .get("tx_packets")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0) as i32;
                    let rx = notif
                        .params
                        .get("rx_packets")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0) as i32;
                    let lqi = notif
                        .params
                        .get("lqi")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(200) as i32;

                    let _ = self.ui_sender.send(UiEvent::TelemetryUpdated {
                        node_id: node_id.to_string(),
                        storage_mode: storage_mode.to_string(),
                        storage_stats: "SRAM: 40/256 KB".to_string(),
                        tx,
                        rx,
                        channel: 15,
                        avg_lqi: lqi,
                        status: "CONNECTED".to_string(),
                    });
                }
                _ => {}
            }
            return;
        }

        // Try parsing as RPC response (e.g. status, list_contacts or list_messages result)
        if let Ok(res) = serde_json::from_str::<JsonRpcResponse>(text) {
            if let Some(val) = res.result {
                if let Some(obj) = val.as_object() {
                    // Check if it's status response
                    if let Some(node_id_val) = obj.get("local_node_id").or_else(|| obj.get("node_id")) {
                        let node_id = node_id_val.as_str().unwrap_or("0xBEBD82B4");
                        let storage_mode = obj
                            .get("storage_mode")
                            .or_else(|| obj.get("dongle_mode"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("SD ACTIVE");
                        let attached = obj
                            .get("dongle_attached")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        let _ = self.ui_sender.send(UiEvent::TelemetryUpdated {
                            node_id: node_id.to_string(),
                            storage_mode: storage_mode.to_string(),
                            storage_stats: "SRAM: 40/256 KB".to_string(),
                            tx: 0,
                            rx: 0,
                            channel: 15,
                            avg_lqi: 200,
                            status: if attached {
                                "CONNECTED".to_string()
                            } else {
                                "NO DONGLE".to_string()
                            },
                        });
                    }
                } else if let Some(arr) = val.as_array() {
                    for item in arr {
                        // Check if it's a ContactRecord
                        if let Some(node_id_val) = item.get("node_id") {
                            let node_id = node_id_val.as_u64().unwrap_or(0) as u32;
                            let alias = item.get("alias").and_then(|v| v.as_str()).unwrap_or("");
                            let trust = item
                                .get("trust_state")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unverified");
                            let rssi =
                                item.get("rssi").and_then(|v| v.as_i64()).unwrap_or(-80) as i16;
                            let lqi = item.get("lqi").and_then(|v| v.as_u64()).unwrap_or(150) as u8;

                            let _ = self.ui_sender.send(UiEvent::StationDiscovered {
                                node_id: format!("0x{:08X}", node_id),
                                alias: alias.to_string(),
                                rssi: format!("{} dBm", rssi),
                                lqi: format!("LQI {}", lqi),
                                trust_state: trust.to_string(),
                            });
                        }
                        // Check if it's a MessageRecord
                        else if let Some(text_val) = item.get("text") {
                            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let convo = item
                                .get("convo_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("#all");
                            let sender = item
                                .get("sender_node_id")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            let body = text_val.as_str().unwrap_or("");
                            let ts = item
                                .get("timestamp")
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0);
                            let status = item
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("*");

                            let _ =
                                self.ui_sender.send(UiEvent::MessageReceived(ChatMessageItem {
                                    id: id.into(),
                                    convo_id: convo.into(),
                                    sender: if sender == 0 {
                                        "Me".into()
                                    } else {
                                        format!("0x{:08X}", sender).into()
                                    },
                                    text: body.into(),
                                    timestamp: format_ts(ts).into(),
                                    status: status.into(),
                                    is_outgoing: sender == 0,
                                }));
                        }
                    }
                }
            }
        }
    }
}

fn format_ts(ts: i64) -> String {
    let secs = ts % 86400;
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", hours, mins, s)
}
