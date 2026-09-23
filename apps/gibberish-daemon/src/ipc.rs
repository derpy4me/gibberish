//! Local WebSocket JSON-RPC Server on 127.0.0.1:4483 (R19).

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

pub const IPC_PORT: u16 = 4483;

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

pub struct IpcServer {
    addr: SocketAddr,
}

impl IpcServer {
    pub fn new() -> Self {
        Self {
            addr: SocketAddr::from(([127, 0, 0, 1], IPC_PORT)),
        }
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(self.addr).await?;
        println!("WebSocket IPC listening on ws://{}", self.addr);

        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(handle_connection(stream));
        }

        Ok(())
    }
}

async fn handle_connection(stream: TcpStream) {
    if let Ok(mut ws_stream) = accept_async(stream).await {
        while let Some(msg) = ws_stream.next().await {
            if let Ok(Message::Text(text)) = msg {
                if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(&text) {
                    let res = handle_rpc(req);
                    if let Ok(res_str) = serde_json::to_string(&res) {
                        let _ = ws_stream.send(Message::Text(res_str)).await;
                    }
                }
            }
        }
    }
}

fn handle_rpc(req: JsonRpcRequest) -> JsonRpcResponse {
    match req.method.as_str() {
        "status" => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(serde_json::json!({
                "status": "connected",
                "dongle_attached": true,
                "dongle_mode": "RAM_ONLY",
                "mesh_active": true
            })),
            error: None,
        },
        "version" => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(serde_json::json!({ "version": "0.1.0", "app": "gibberishd" })),
            error: None,
        },
        _ => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: None,
            error: Some(format!("Unknown method '{}'", req.method)),
        },
    }
}
