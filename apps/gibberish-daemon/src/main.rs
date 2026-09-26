//! Project Gibberish Desktop Companion Daemon (`gibberishd`) (R14, R15, R16, R17, R18, KTD6).

use gibberish_crypto::ratchet::derive_network_tag;
use gibberish_crypto::secrecy::Secret;
use gibberish_daemon::chunk::ChunkEngine;
use gibberish_daemon::clipboard::ClipboardManager;
use gibberish_daemon::fleet;
use gibberish_daemon::ipc::IpcServer;
use gibberish_daemon::nonce::NonceManager;
use gibberish_daemon::transport::{self, parse_dongle_node_id, SerialTransport};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use tokio::time::sleep;

struct DualLogger {
    file: Option<File>,
}

impl DualLogger {
    fn new(path: &str) -> Self {
        if let Some(parent) = Path::new(path).parent() {
            let _ = fs::create_dir_all(parent);
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok();

        Self { file }
    }

    fn log(&mut self, msg: &str) {
        println!("{}", msg);
        let _ = std::io::stdout().flush();
        if let Some(ref mut f) = self.file {
            let _ = writeln!(f, "{}", msg);
            let _ = f.flush();
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse CLI arguments
    let mut specified_ports: Vec<String> = Vec::new();
    let mut log_path = "/tmp/gibberish/daemon.log".to_string();
    let mut local_node_id: u32 = 0xBEBCE5B8; // Default Node A fallback
    let mut user_specified_node_id = false;
    let mut auto_sync = true;
    let mut push_message: Option<String> = None;
    let mut check_status = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--port" || arg == "-p" {
            if let Some(p) = args.next() {
                for port in p.split(',') {
                    let mut trimmed = port.trim();
                    if let Some(stripped) = trimmed.strip_prefix("port=") {
                        trimmed = stripped;
                    }
                    if let Some(stripped) = trimmed.strip_prefix("--port=") {
                        trimmed = stripped;
                    }
                    if !trimmed.is_empty() {
                        specified_ports.push(trimmed.to_string());
                    }
                }
            }
        } else if let Some(p) = arg.strip_prefix("--port=") {
            for port in p.split(',') {
                let mut trimmed = port.trim();
                if let Some(stripped) = trimmed.strip_prefix("port=") {
                    trimmed = stripped;
                }
                if !trimmed.is_empty() {
                    specified_ports.push(trimmed.to_string());
                }
            }
        } else if arg == "--log-file" || arg == "-l" {
            if let Some(l) = args.next() {
                log_path = l;
            }
        } else if arg == "--node-id" {
            if let Some(id_str) = args.next() {
                let clean = id_str.trim_start_matches("0x");
                if let Ok(id) = u32::from_str_radix(clean, 16) {
                    local_node_id = id;
                    user_specified_node_id = true;
                }
            }
        } else if arg == "--auto-sync" {
            auto_sync = true;
        } else if arg == "--no-sync" {
            auto_sync = false;
        } else if arg == "push" {
            if let Some(msg) = args.next() {
                push_message = Some(msg);
            }
        } else if arg == "status" {
            check_status = true;
        }
    }

    // CLI Command: status check
    if check_status {
        println!("Checking Gibberish local hardware and daemon status...");
        if let Some(dongle_path) = SerialTransport::find_dongle() {
            println!("  Hardware Dongle: DETECTED ({})", dongle_path.display());
        } else {
            println!("  Hardware Dongle: NONE DETECTED");
        }
        println!("  Local Station ID: 0x{:08X}", local_node_id);
        println!("  RF Mesh Channel: 15 (2.425 GHz)");
        return Ok(());
    }

    let mut log = DualLogger::new(&log_path);
    log.log("===============================================");
    log.log(" Gibberish Desktop Companion Daemon (gibberishd)");
    log.log(" Zero-Trust Encrypted Mesh & Clipboard Sync    ");
    log.log("===============================================\n");
    log.log(&format!("Logging to stdout and {}\n", log_path));
    log.log(&format!("Local Station Node ID: 0x{:08X}", local_node_id));

    // Default Swarm Master Key (type-level Secret<T> with zeroize)
    let swarm_master_key = Secret::new([0x55u8; 32]);
    let swarm_tag = derive_network_tag(swarm_master_key.expose_secret());
    log.log("Swarm Master Key loaded with type-level Secret<T> redaction.");
    log.log(&format!("Derived Network Tag: 0x{:016X}", swarm_tag));

    // Initialize Monotonic Nonce Manager (R6, R7, KTD3)
    let mut nonce_mgr = NonceManager::new(None)
        .map_err(|e| format!("Failed to initialize NonceManager: {}", e))?;
    log.log(&format!("Monotonic Nonce Manager active (Epoch: {})", nonce_mgr.current_epoch()));

    // Initialize Anti-DoS Chunk Engine (R8-R13, KTD1, KTD4)
    let mut chunk_engine = ChunkEngine::new();

    // Auto-discover serial dongles if none specified (Linux: ttyACM*, macOS: cu.usbmodem*)
    if specified_ports.is_empty() {
        if let Ok(entries) = std::fs::read_dir("/dev") {
            for entry in entries.flatten() {
                if let Ok(name) = entry.file_name().into_string() {
                    // Match Linux (ttyACM) and macOS (cu.usbmodem preferred on macOS)
                    if name.starts_with("ttyACM") || name.starts_with("cu.usbmodem") {
                        specified_ports.push(format!("/dev/{}", name));
                    }
                }
            }
        }
        // Fallback: query serialport crate if /dev scanning found nothing
        if specified_ports.is_empty() {
            if let Ok(ports) = serialport::available_ports() {
                for p in ports {
                    if p.port_name.contains("ACM") || p.port_name.contains("usbmodem") {
                        specified_ports.push(p.port_name);
                    }
                }
            }
        }
        specified_ports.sort();
    }

    let mut transports: Vec<(String, transport::SerialTransport)> = Vec::new();
    for path in &specified_ports {
        match transport::SerialTransport::open(path) {
            Ok(t) => {
                log.log(&format!("Connected to C5 Hardware Dongle at {}", path));
                transports.push((path.clone(), t));
            }
            Err(e) => {
                log.log(&format!("Notice: Could not open {}: {}", path, e));
            }
        }
    }
    log.log(&format!("Active Hardware Dongles Monitored: {}\n", transports.len()));

    // If user did not explicitly specify --node-id, probe initial dongle logs
    if !user_specified_node_id {
        for (port, t) in &mut transports {
            let (_pkts, logs) = t.poll_stream();
            for line in logs {
                if let Some(detected_id) = parse_dongle_node_id(&line) {
                    local_node_id = detected_id;
                    log.log(&format!(
                        "[Auto-Discovery] Detected local Dongle Node ID from hardware on {}: 0x{:08X}",
                        port, detected_id
                    ));
                    break;
                }
            }
        }
    }

    // CLI Command: One-shot Push Message
    if let Some(msg) = push_message {
        log.log(&format!("Broadcasting one-shot message via mesh: \"{}\"", msg));
        let secret_msg = Secret::new(msg);
        let packets = ChunkEngine::fragment_and_encrypt(
            &secret_msg,
            swarm_tag,
            local_node_id,
            &swarm_master_key,
            &mut nonce_mgr,
        )
        .map_err(|e| format!("Crypto error: {:?}", e))?;

        chunk_engine.cache_outbound(&packets);

        if let Some((primary_port, t)) = transports.first_mut() {
            for pkt in &packets {
                t.send_packet(pkt)?;
            }
            log.log(&format!(
                "Successfully transmitted {} chunks over {} to RF airwaves!",
                packets.len(),
                primary_port
            ));
        } else {
            log.log("Error: No hardware dongle available to transmit message.");
        }
        return Ok(());
    }

    // Spawn WebSocket IPC Server on 127.0.0.1:4483
    let (ipc, mut outbound_rx) = IpcServer::channel();
    ipc.set_local_node_id(local_node_id);
    ipc.set_dongle_attached(!transports.is_empty());
    let ipc_server = ipc.clone();
    tokio::spawn(async move {
        if let Err(e) = ipc_server.run().await {
            eprintln!("IPC Server error: {}", e);
        }
    });

    // Initialize OS Clipboard Manager with 500ms provenance hash suppression (R14, R15, KTD5)
    let mut clipboard_mgr = ClipboardManager::new();

    if auto_sync {
        log.log("OS clipboard auto-synchronization enabled (--auto-sync).\n");
    } else {
        log.log("OS clipboard auto-synchronization disabled (--no-sync).\n");
    }

    let primary_port = transports.first().map(|(p, _)| p.clone());
    let mut local_dongle_ids = std::collections::HashSet::new();
    if local_node_id != 0 {
        local_dongle_ids.insert(local_node_id);
    }

    // Main daemon synchronization loop
    loop {
        sleep(Duration::from_millis(50)).await;

        // 1. Inbound processing: poll stream from all connected hardware dongles
        for (port, t) in &mut transports {
            let (packets, logs) = t.poll_stream();
            let is_primary_dongle = primary_port.as_ref() == Some(port);

            for line in logs {
                if let Some(detected_id) = parse_dongle_node_id(&line) {
                    local_dongle_ids.insert(detected_id);
                    if !user_specified_node_id
                        && is_primary_dongle
                        && detected_id != local_node_id
                    {
                        log.log(&format!(
                            "[Auto-Discovery] Updated local Dongle Node ID from hardware: 0x{:08X}",
                            detected_id
                        ));
                        local_node_id = detected_id;
                        ipc.set_local_node_id(local_node_id);
                    }
                }

                // Ingest telemetry beacon frames (R9, R10)
                if let Some(telem) = fleet::parse_telemetry_line(&line) {
                    let clean_id = telem.node_id.trim_start_matches("0x");
                    if let Ok(peer_id) = u32::from_str_radix(clean_id, 16) {
                        if peer_id == local_node_id {
                            ipc.set_storage_mode(&telem.storage_mode);
                            ipc.broadcast_event(
                                "telemetry_update",
                                serde_json::json!({
                                    "node_id": format!("0x{:08X}", local_node_id),
                                    "storage_mode": telem.storage_mode,
                                    "rx_packets": telem.rx_count,
                                    "tx_packets": telem.tx_count,
                                    "lqi": telem.lqi,
                                    "rssi": telem.rssi,
                                }),
                            );
                        } else {
                            let alias = format!("Station-{:04X}", peer_id & 0xFFFF);
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as i64;
                            let _ = ipc.db().upsert_contact(&gibberish_db::ContactRecord {
                                node_id: peer_id,
                                alias: alias.clone(),
                                pubkey: [0u8; 32],
                                trust_state: gibberish_db::TrustState::Unverified,
                                last_seen: now,
                                rssi: telem.rssi as i16,
                                lqi: telem.lqi,
                            });
                            ipc.notify_node_discovered(
                                peer_id,
                                &alias,
                                "unverified",
                                telem.rssi as i16,
                                telem.lqi,
                            );
                            log.log(&format!(
                                "[Peer Discovered] Node 0x{:08X} (Storage: {}, LQI: {}, RSSI: {} dBm)",
                                peer_id, telem.storage_mode, telem.lqi, telem.rssi
                            ));
                        }
                    }
                }

                log.log(&format!("[Dongle {}] {}", port, line));
            }

            for wire_pkt in packets {
                // Check if this is a Selective Acknowledgment (SACK) frame (Issue #3)
                if (wire_pkt.packet.header.flags & gibberish_protocol::FLAG_SACK) != 0 {
                    chunk_engine.suppress_sack(wire_pkt.packet.header.msg_id);
                    let retransmit = chunk_engine.handle_sack(&wire_pkt.packet);
                    if !retransmit.is_empty() {
                        log.log(&format!(
                            "[SACK Retransmit] Peer 0x{:08X} requested {} missing chunks for MsgID 0x{:08X}. Retransmitting...",
                            wire_pkt.src_node_id,
                            retransmit.len(),
                            wire_pkt.packet.header.msg_id,
                        ));
                        for pkt in &retransmit {
                            let _ = t.send_packet(pkt);
                            sleep(Duration::from_millis(20)).await;
                        }
                    }
                    continue;
                }

                match chunk_engine.ingest_packet(
                    wire_pkt.src_node_id,
                    &wire_pkt.packet,
                    &swarm_master_key,
                ) {
                    Ok(Some(decrypted_text)) => {
                        let text_str = decrypted_text.expose_secret().clone();
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;
                        log.log(&format!(
                            "[Mesh RX Decrypted] Reassembled message from Peer Node 0x{:08X} ({} chars)",
                            wire_pkt.src_node_id,
                            text_str.len()
                        ));

                        // Suppress over-the-air loopback echoes from local dongles attached to this host
                        let is_self_echo = local_dongle_ids.contains(&wire_pkt.src_node_id)
                            || (local_node_id != 0 && wire_pkt.src_node_id == local_node_id);
                        if is_self_echo {
                            log.log(&format!(
                                "[Airwave Loopback Suppressed] Overheard own transmission from Node 0x{:08X}; skipping duplicate chat insertion",
                                wire_pkt.src_node_id
                            ));
                            continue;
                        }

                        static RX_MSG_COUNTER: std::sync::atomic::AtomicU64 =
                            std::sync::atomic::AtomicU64::new(1);
                        let seq = RX_MSG_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let msg_id = format!("rx-{}-{}", now, seq);

                        // Persist to database and push to UI via IPC
                        let msg_rec = gibberish_db::MessageRecord {
                            id: msg_id.clone(),
                            convo_id: "#all".to_string(),
                            sender_node_id: wire_pkt.src_node_id,
                            timestamp: now,
                            text: text_str.clone(),
                            status: gibberish_db::MessageStatus::Delivered,
                        };
                        let _ = ipc.db().insert_message(&msg_rec);
                        ipc.notify_rx_message(&msg_id, "#all", wire_pkt.src_node_id, &text_str, now, "[OK]");

                        if auto_sync {
                            if let Err(e) = clipboard_mgr.write_clipboard(&decrypted_text) {
                                log.log(&format!("[Clipboard Write Warning] {}", e));
                            } else {
                                log.log("[Clipboard Sync] Successfully updated local OS clipboard with remote mesh text!");
                            }
                        }
                    }
                    Ok(None) => {
                        // Chunk buffered; awaiting remaining chunks
                    }
                    Err(e) => {
                        log.log(&format!(
                            "[Mesh RX Warning] Dropped chunk from Node 0x{:08X}: {:?}",
                            wire_pkt.src_node_id, e
                        ));
                    }
                }
            }
        }

        // 2. Outbound processing: drain UI IPC messages from send_swarm / send_dm
        while let Ok(outbound_msg) = outbound_rx.try_recv() {
            log.log(&format!(
                "[Outbound IPC] Transmitting chat message ({}) to RF mesh...",
                outbound_msg.message_id
            ));
            let secret_msg = Secret::new(outbound_msg.text);
            match ChunkEngine::fragment_and_encrypt(
                &secret_msg,
                swarm_tag,
                local_node_id,
                &swarm_master_key,
                &mut nonce_mgr,
            ) {
                Ok(packets) => {
                    chunk_engine.cache_outbound(&packets);
                    if let Some((primary_port, t)) = transports.first_mut() {
                        for pkt in &packets {
                            if let Err(e) = t.send_packet(pkt) {
                                log.log(&format!(
                                    "Failed to transmit chunk over {}: {}",
                                    primary_port, e
                                ));
                            }
                            sleep(Duration::from_millis(20)).await;
                        }
                        log.log(&format!(
                            "[Mesh TX] Successfully sent {} chunks for {} over {}",
                            packets.len(),
                            outbound_msg.message_id,
                            primary_port
                        ));
                    } else {
                        log.log(&format!(
                            "Warning: No hardware dongle attached to transmit outbound message {}",
                            outbound_msg.message_id
                        ));
                    }
                }
                Err(e) => {
                    log.log(&format!("Encryption / Chunking error on outbound IPC: {:?}", e));
                }
            }
        }

        // 3. Outbound processing: monitor OS clipboard for new local copies
        if auto_sync {
            if let Some(text) = clipboard_mgr.read_clipboard() {
                log.log(&format!(
                    "Local clipboard copy captured ({} bytes)! Redacted: {:?}",
                    text.expose_secret().len(),
                    text
                ));

                match ChunkEngine::fragment_and_encrypt(
                    &text,
                    swarm_tag,
                    local_node_id,
                    &swarm_master_key,
                    &mut nonce_mgr,
                ) {
                    Ok(packets) => {
                        log.log(&format!(
                            "Encrypted into {} ciphertext chunks (96B each). Broadcasting to 802.15.4 mesh...",
                            packets.len()
                        ));

                        chunk_engine.cache_outbound(&packets);

                        if let Some((primary_port, t)) = transports.first_mut() {
                            for pkt in &packets {
                                if let Err(e) = t.send_packet(pkt) {
                                    log.log(&format!("Failed to transmit chunk over {}: {}", primary_port, e));
                                }
                                sleep(Duration::from_millis(30)).await;
                            }
                            log.log(&format!(
                                "Transmitted {} chunks to primary dongle ({}) via length-prefixed framing!",
                                packets.len(),
                                primary_port
                            ));
                        } else {
                            log.log("Warning: No hardware dongle attached to transmit outbound clipboard update.");
                        }
                    }
                    Err(e) => {
                        log.log(&format!("Encryption / Chunking error: {:?}", e));
                    }
                }
            }
        }

        // 3. Selective Acknowledgment (SACK): poll for missing chunks and broadcast recovery bitmasks (Issue #3)
        let pending_sacks = chunk_engine.check_pending_sacks(local_node_id, swarm_tag);
        if !pending_sacks.is_empty() {
            if let Some((_primary_port, t)) = transports.first_mut() {
                for sack_pkt in &pending_sacks {
                    let sack_payload = gibberish_protocol::SackPayload::deserialize(&sack_pkt.payload);
                    log.log(&format!(
                        "[SACK Recovery] Broadcasted NACK bitmask for MsgID 0x{:08X} (missing {} chunks) to Peer 0x{:08X}",
                        sack_pkt.header.msg_id,
                        sack_payload.missing_count(),
                        sack_payload.sender_node_id
                    ));
                    let _ = t.send_packet(sack_pkt);
                }
            }
        }
    }
}
