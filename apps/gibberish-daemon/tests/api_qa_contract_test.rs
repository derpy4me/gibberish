use futures_util::{SinkExt, StreamExt};
use gibberish_daemon::ipc::{IpcServer, JsonRpcRequest, JsonRpcResponse};
use gibberish_db::{ContactRecord, DatabaseStore, TrustState};
use serde_json::Value;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

async fn spawn_test_server() -> (SocketAddr, DatabaseStore, IpcServer) {
    let db = DatabaseStore::open_in_memory().expect("open memory db");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);

    let server = IpcServer::with_db(addr, db.clone());
    let server_clone = server.clone();

    tokio::spawn(async move {
        let _ = server_clone.run().await;
    });

    // Retry connecting until server is up (up to 500ms)
    let url = format!("ws://{}", addr);
    for _ in 0..20 {
        if let Ok((ws, _)) = connect_async(&url).await {
            drop(ws);
            break;
        }
        sleep(Duration::from_millis(25)).await;
    }

    (addr, db, server)
}

#[tokio::test]
async fn test_jsonrpc_contract_malformed_and_invalid_request() {
    let (addr, _, _) = spawn_test_server().await;
    let url = format!("ws://{}", addr);

    let (mut ws, _) = connect_async(&url).await.expect("connect failed");

    // 1. Send completely malformed non-JSON string
    ws.send(Message::Text("{invalid_json_payload".to_string()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let text = msg.to_text().unwrap();
    let res: JsonRpcResponse = serde_json::from_str(text).expect("parse error response");

    assert_eq!(res.jsonrpc, "2.0");
    assert_eq!(res.id, 0);
    assert!(res.result.is_none());
    let err = res.error.expect("error object expected");
    assert_eq!(err.code, -32600);
    assert_eq!(err.message, "Invalid Request");

    // 2. Send JSON object that does not conform to JsonRpcRequest schema
    ws.send(Message::Text(r#"{"hello": "world"}"#.to_string()))
        .await
        .unwrap();

    let msg2 = ws.next().await.unwrap().unwrap();
    let text2 = msg2.to_text().unwrap();
    let res2: JsonRpcResponse = serde_json::from_str(text2).expect("parse error response");
    assert_eq!(res2.jsonrpc, "2.0");
    assert_eq!(res2.id, 0);
    let err2 = res2.error.expect("error object expected");
    assert_eq!(err2.code, -32600);
}

#[tokio::test]
async fn test_jsonrpc_contract_method_not_found() {
    let (addr, _, _) = spawn_test_server().await;
    let url = format!("ws://{}", addr);

    let (mut ws, _) = connect_async(&url).await.expect("connect failed");

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 777,
        method: "non_existent_rpc_method".to_string(),
        params: None,
    };
    ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let res: JsonRpcResponse = serde_json::from_str(msg.to_text().unwrap()).unwrap();

    assert_eq!(res.jsonrpc, "2.0");
    assert_eq!(res.id, 777);
    assert!(res.result.is_none());
    let err = res.error.expect("error expected for unknown method");
    assert_eq!(err.code, -32601);
    assert!(
        err.message.contains("Method not found"),
        "Unexpected error message: {}",
        err.message
    );
}

#[tokio::test]
async fn test_jsonrpc_contract_parameter_validation_errors() {
    let (addr, _, _) = spawn_test_server().await;
    let url = format!("ws://{}", addr);

    let (mut ws, _) = connect_async(&url).await.expect("connect failed");

    // 1. send_dm missing params entirely
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 101,
        method: "send_dm".to_string(),
        params: None,
    };
    ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
        .await
        .unwrap();
    let res: JsonRpcResponse = serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(res.id, 101);
    assert_eq!(res.error.unwrap().code, -32602);

    // 2. send_dm missing dest_node_id
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 102,
        method: "send_dm".to_string(),
        params: Some(serde_json::json!({ "text": "missing dest" })),
    };
    ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
        .await
        .unwrap();
    let res: JsonRpcResponse = serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(res.id, 102);
    assert_eq!(res.error.unwrap().code, -32602);

    // 3. send_dm missing text
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 103,
        method: "send_dm".to_string(),
        params: Some(serde_json::json!({ "dest_node_id": "0x1234" })),
    };
    ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
        .await
        .unwrap();
    let res: JsonRpcResponse = serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(res.id, 103);
    assert_eq!(res.error.unwrap().code, -32602);

    // 4. send_swarm missing text
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 104,
        method: "send_swarm".to_string(),
        params: Some(serde_json::json!({})),
    };
    ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
        .await
        .unwrap();
    let res: JsonRpcResponse = serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(res.id, 104);
    assert_eq!(res.error.unwrap().code, -32602);

    // 5. list_messages missing convo_id
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 105,
        method: "list_messages".to_string(),
        params: Some(serde_json::json!({ "limit": 10 })),
    };
    ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
        .await
        .unwrap();
    let res: JsonRpcResponse = serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(res.id, 105);
    assert_eq!(res.error.unwrap().code, -32602);
}

#[tokio::test]
async fn test_jsonrpc_contract_happy_path_and_schema_validation() {
    let (addr, db, _) = spawn_test_server().await;
    let url = format!("ws://{}", addr);

    let (mut ws, _) = connect_async(&url).await.expect("connect failed");

    // 1. 'status' contract
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 201,
        method: "status".to_string(),
        params: None,
    };
    ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
        .await
        .unwrap();
    let res: JsonRpcResponse = serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(res.id, 201);
    let status_val = res.result.expect("status result");
    assert_eq!(status_val["status"], "connected");
    assert_eq!(status_val["dongle_attached"], true);
    assert_eq!(status_val["dongle_mode"], "RAM_ONLY");
    assert_eq!(status_val["mesh_active"], true);

    // 2. 'version' contract
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 202,
        method: "version".to_string(),
        params: None,
    };
    ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
        .await
        .unwrap();
    let res: JsonRpcResponse = serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(res.id, 202);
    let ver_val = res.result.expect("version result");
    assert_eq!(ver_val["version"], "0.1.0");
    assert_eq!(ver_val["app"], "gibberishd");

    // 3. 'send_dm' with integer node_id
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 203,
        method: "send_dm".to_string(),
        params: Some(serde_json::json!({
            "dest_node_id": 0xAABBCCDDu32,
            "text": "Encrypted handshake payload"
        })),
    };
    ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
        .await
        .unwrap();

    let mut dm_res: Option<JsonRpcResponse> = None;
    for _ in 0..2 {
        let msg = ws.next().await.unwrap().unwrap();
        if let Ok(res) = serde_json::from_str::<JsonRpcResponse>(msg.to_text().unwrap()) {
            dm_res = Some(res);
        }
    }
    let res = dm_res.expect("dm response");
    assert_eq!(res.id, 203);
    let dm_val = res.result.expect("dm result");
    assert!(dm_val["message_id"].as_str().unwrap().starts_with("dm-"));
    assert_eq!(dm_val["status"], "queued");

    // 4. 'list_messages' pagination contract
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 204,
        method: "list_messages".to_string(),
        params: Some(serde_json::json!({
            "convo_id": "0xAABBCCDD",
            "limit": 10,
            "offset": 0
        })),
    };
    ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
        .await
        .unwrap();
    let res: JsonRpcResponse = serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(res.id, 204);
    let msgs = res.result.expect("messages array");
    assert_eq!(msgs.as_array().unwrap().len(), 1);
    assert_eq!(msgs[0]["text"], "Encrypted handshake payload");
    assert_eq!(msgs[0]["status"], "queued");

    // 5. 'list_contacts' and 'verify_contact'
    let contact = ContactRecord {
        node_id: 0xAABBCCDD,
        alias: "Bob_Relay".to_string(),
        pubkey: [0x77; 32],
        trust_state: TrustState::Unverified,
        last_seen: 1500,
        rssi: -65,
        lqi: 200,
    };
    db.upsert_contact(&contact).unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 205,
        method: "list_contacts".to_string(),
        params: None,
    };
    ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
        .await
        .unwrap();
    let res: JsonRpcResponse = serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(res.id, 205);
    let contacts = res.result.expect("contacts array");
    assert_eq!(contacts.as_array().unwrap().len(), 1);
    assert_eq!(contacts[0]["alias"], "Bob_Relay");
    assert_eq!(contacts[0]["trust_state"], "unverified");

    // Toggle verified: true
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 206,
        method: "verify_contact".to_string(),
        params: Some(serde_json::json!({
            "node_id": "0xAABBCCDD",
            "verified": true
        })),
    };
    ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
        .await
        .unwrap();

    let mut verify_res: Option<JsonRpcResponse> = None;
    for _ in 0..2 {
        let msg = ws.next().await.unwrap().unwrap();
        if let Ok(res) = serde_json::from_str::<JsonRpcResponse>(msg.to_text().unwrap()) {
            verify_res = Some(res);
        }
    }
    let res = verify_res.expect("verify response");
    assert_eq!(res.id, 206);
    let v_val = res.result.expect("verify result");
    assert_eq!(v_val["verified"], true);
    assert_eq!(v_val["updated"], true);
}

#[tokio::test]
async fn test_broadcast_notifications_contract_and_fanout() {
    let (addr, _, server) = spawn_test_server().await;
    let url = format!("ws://{}", addr);

    // Connect two independent clients
    let (mut client1, _) = connect_async(&url).await.expect("c1 connect");
    let (mut client2, _) = connect_async(&url).await.expect("c2 connect");

    // Trigger delivery_ack notification on server
    server.notify_delivery_ack("dm-test-ack-1", "delivered");

    // Both clients must receive the delivery_ack notification
    let msg1 = client1.next().await.unwrap().unwrap();
    let val1: Value = serde_json::from_str(msg1.to_text().unwrap()).unwrap();
    assert_eq!(val1["jsonrpc"], "2.0");
    assert_eq!(val1["method"], "delivery_ack");
    assert_eq!(val1["params"]["message_id"], "dm-test-ack-1");
    assert_eq!(val1["params"]["status"], "delivered");
    // Per JSON-RPC 2.0 spec, notifications MUST NOT have an id member
    assert!(val1.get("id").is_none());

    let msg2 = client2.next().await.unwrap().unwrap();
    let val2: Value = serde_json::from_str(msg2.to_text().unwrap()).unwrap();
    assert_eq!(val2["jsonrpc"], "2.0");
    assert_eq!(val2["method"], "delivery_ack");
    assert_eq!(val2["params"]["message_id"], "dm-test-ack-1");
    assert!(val2.get("id").is_none());
}

#[tokio::test]
async fn test_burst_requests_and_large_payload() {
    let (addr, db, _) = spawn_test_server().await;
    let url = format!("ws://{}", addr);

    let (mut ws, _) = connect_async(&url).await.expect("connect");

    // 1. Large 32KB payload test for send_swarm
    let large_text = "A".repeat(32 * 1024);
    let req_large = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 301,
        method: "send_swarm".to_string(),
        params: Some(serde_json::json!({ "text": large_text })),
    };
    ws.send(Message::Text(serde_json::to_string(&req_large).unwrap()))
        .await
        .unwrap();

    let mut swarm_res: Option<JsonRpcResponse> = None;
    for _ in 0..2 {
        let msg = ws.next().await.unwrap().unwrap();
        if let Ok(res) = serde_json::from_str::<JsonRpcResponse>(msg.to_text().unwrap()) {
            swarm_res = Some(res);
        }
    }
    let res = swarm_res.expect("large swarm res");
    assert_eq!(res.id, 301);
    assert_eq!(res.result.unwrap()["status"], "transmitted");

    // Verify stored in DB with exact length
    let stored = db.list_messages("#all", 10, 0).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].text.len(), 32 * 1024);

    // 2. Burst of 30 sequential status calls
    for i in 1..=30 {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 400 + i,
            method: "status".to_string(),
            params: None,
        };
        ws.send(Message::Text(serde_json::to_string(&req).unwrap()))
            .await
            .unwrap();

        let msg = ws.next().await.unwrap().unwrap();
        let res: JsonRpcResponse = serde_json::from_str(msg.to_text().unwrap()).unwrap();
        assert_eq!(res.id, 400 + i);
        assert_eq!(res.result.unwrap()["mesh_active"], true);
    }
}
