use futures_util::{SinkExt, StreamExt};
use gibberish_daemon::ipc::{IpcServer, JsonRpcRequest, JsonRpcResponse};
use gibberish_db::{ContactRecord, DatabaseStore, TrustState};
use std::net::SocketAddr;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

async fn spawn_test_server() -> (SocketAddr, DatabaseStore, IpcServer) {
    let db = DatabaseStore::open_in_memory().expect("open memory db");
    // Pick port 0 to let OS assign an ephemeral port
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

    // Small delay to ensure server is listening
    sleep(Duration::from_millis(50)).await;

    (addr, db, server)
}

#[tokio::test]
async fn test_rpc_status_and_version() {
    let (addr, _, _) = spawn_test_server().await;
    let url = format!("ws://{}", addr);

    let (mut ws, _) = connect_async(&url).await.expect("connect failed");

    // Test 'status'
    let req_status = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 1,
        method: "status".to_string(),
        params: None,
    };
    ws.send(Message::Text(serde_json::to_string(&req_status).unwrap()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let text = msg.to_text().unwrap();
    let res: JsonRpcResponse = serde_json::from_str(text).unwrap();
    assert_eq!(res.id, 1);
    assert!(res.error.is_none());
    assert_eq!(res.result.unwrap()["mesh_active"], true);

    // Test 'version'
    let req_ver = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 2,
        method: "version".to_string(),
        params: None,
    };
    ws.send(Message::Text(serde_json::to_string(&req_ver).unwrap()))
        .await
        .unwrap();

    let msg2 = ws.next().await.unwrap().unwrap();
    let res2: JsonRpcResponse = serde_json::from_str(msg2.to_text().unwrap()).unwrap();
    assert_eq!(res2.id, 2);
    assert_eq!(res2.result.unwrap()["app"], "gibberishd");
}

#[tokio::test]
async fn test_rpc_unknown_method_error() {
    let (addr, _, _) = spawn_test_server().await;
    let url = format!("ws://{}", addr);

    let (mut ws, _) = connect_async(&url).await.expect("connect failed");

    let req_invalid = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 99,
        method: "non_existent_method".to_string(),
        params: None,
    };
    ws.send(Message::Text(serde_json::to_string(&req_invalid).unwrap()))
        .await
        .unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    let res: JsonRpcResponse = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(res.id, 99);
    assert!(res.error.is_some());
    let err = res.error.unwrap();
    assert_eq!(err.code, -32601);
}

#[tokio::test]
async fn test_rpc_messaging_and_contact_lifecycle() {
    let (addr, db, _server) = spawn_test_server().await;
    let url = format!("ws://{}", addr);

    let (mut ws, _) = connect_async(&url).await.expect("connect failed");

    // 1. Send Swarm Broadcast
    let req_swarm = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 10,
        method: "send_swarm".to_string(),
        params: Some(serde_json::json!({
            "text": "Hello Swarm #all!"
        })),
    };
    ws.send(Message::Text(serde_json::to_string(&req_swarm).unwrap()))
        .await
        .unwrap();

    // Consume responses: note notification may arrive or response first
    let mut swarm_res: Option<JsonRpcResponse> = None;
    for _ in 0..2 {
        let msg = ws.next().await.unwrap().unwrap();
        let text = msg.to_text().unwrap();
        if let Ok(res) = serde_json::from_str::<JsonRpcResponse>(text) {
            swarm_res = Some(res);
        }
    }
    let res = swarm_res.expect("expected response for send_swarm");
    assert_eq!(res.id, 10);
    assert_eq!(res.result.unwrap()["status"], "transmitted");

    // Verify persisted in db
    let msgs = db.list_messages("#all", 10, 0).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].text, "Hello Swarm #all!");

    // 2. Send 1-to-1 DM (dest 0xBEBD82B4)
    let req_dm = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 11,
        method: "send_dm".to_string(),
        params: Some(serde_json::json!({
            "dest_node_id": "0xBEBD82B4",
            "text": "Secret DM to Alice"
        })),
    };
    ws.send(Message::Text(serde_json::to_string(&req_dm).unwrap()))
        .await
        .unwrap();

    let mut dm_res: Option<JsonRpcResponse> = None;
    for _ in 0..2 {
        let msg = ws.next().await.unwrap().unwrap();
        let text = msg.to_text().unwrap();
        if let Ok(res) = serde_json::from_str::<JsonRpcResponse>(text) {
            dm_res = Some(res);
        }
    }
    let res_dm = dm_res.expect("expected response for send_dm");
    assert_eq!(res_dm.id, 11);
    assert_eq!(res_dm.result.unwrap()["status"], "queued");

    // 3. Populate a contact in DB and verify via RPC
    let contact = ContactRecord {
        node_id: 0xBEBD82B4,
        alias: "Alice".to_string(),
        pubkey: [0x42; 32],
        trust_state: TrustState::Unverified,
        last_seen: 1000,
        rssi: -70,
        lqi: 190,
    };
    db.upsert_contact(&contact).unwrap();

    let req_verify = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: 12,
        method: "verify_contact".to_string(),
        params: Some(serde_json::json!({
            "node_id": "0xBEBD82B4",
            "verified": true
        })),
    };
    ws.send(Message::Text(serde_json::to_string(&req_verify).unwrap()))
        .await
        .unwrap();

    let mut verify_res: Option<JsonRpcResponse> = None;
    for _ in 0..2 {
        let msg = ws.next().await.unwrap().unwrap();
        let text = msg.to_text().unwrap();
        if let Ok(res) = serde_json::from_str::<JsonRpcResponse>(text) {
            verify_res = Some(res);
        }
    }
    let res_verify = verify_res.expect("expected verify response");
    assert_eq!(res_verify.id, 12);
    assert_eq!(res_verify.result.unwrap()["verified"], true);

    // Verify DB updated
    let updated_contact = db.get_contact(0xBEBD82B4).unwrap().unwrap();
    assert_eq!(updated_contact.trust_state, TrustState::Verified);
}
