---
title: "ESP32-C5 802.15.4 Dual-Dongle Mesh Direct Routing, Echo Suppression, and UI Synchronization"
date: "2026-09-26"
category: "integration-issues"
module: "gibberish-daemon"
problem_type: "integration_issue"
component: "messaging"
severity: "high"
symptoms:
  - "Messages sent directly to peer node ID show up in Swarm broadcast channel instead of the direct node conversation"
  - "Messages sent from Linux to Mac directly to node do not appear on Mac"
  - "Desktop client displays RAM-only storage mode even when an active MicroSD card is mounted on the physical dongle"
  - "Conversation thread appears blank upon initial client boot until user interacts with channels"
  - "Local chat input disappears upon transmission with a perceptible delay before reappearing in the conversation view"
root_cause: "logic_error"
resolution_type: "code_fix"
tags:
  - "esp32-c5"
  - "802154"
  - "mesh"
  - "routing"
  - "echo-suppression"
  - "slint"
  - "ipc"
  - "microsd"
---

# ESP32-C5 802.15.4 Dual-Dongle Mesh Direct Routing, Echo Suppression, and UI Synchronization

> Superseded (2026-09-29), in part: the MAC-based echo suppression (local dongle IDs taken from the MAC-derived node ID in frame headers and firmware logs) and the claim of "forward compatibility with existing relay firmware" do not survive the OPSEC radio contract, which removes MAC bytes from production frames and makes one breaking frame-format change. See docs/plans/2026-09-29-0837-feat-production-opsec-radio-contract-plan.md (R5, R18, KTD1). The routing, cold-boot, and optimistic-UI fixes below still describe current code.

## Problem

When testing multi-station mesh deployments between two independent host machines (Linux x86_64 and macOS Apple Silicon) equipped with LilyGO T-Dongle-C5 IEEE 802.15.4 radios, several protocol, routing, and UI synchronization failures occurred:

1. **Storage Mode Telemetry Stalled in RAM**: The Slint desktop client persistently displayed "RAM ONLY" storage mode even when an active MicroSD card was formatted (FAT32) and mounted on the physical USB dongle.
2. **Cold-Boot Blank Conversation View**: Upon launching the Slint GUI, the conversation view appeared empty until the operator manually clicked or toggled between sidebar channels.
3. **Swarm/Direct Channel Cross-Talk**: Direct 1-to-1 messages sent to a specific node address (`0xBEBD82B4`) were delivered to the recipient's `#all` Swarm Broadcast channel rather than the node's dedicated conversation thread.
4. **Peer Transmissions Dropped by Echo Suppression**: Messages sent from the Linux workstation failed to appear on the macOS peer because the receiving daemon misclassified legitimate over-the-air packets from the remote station as local hardware echoes and silently discarded them.
5. **UI Latency on Sent Messages**: Local chat input disappeared upon hitting Enter with a noticeable lag before appearing in the message list, giving the perception of a dropped transmission.

## Symptoms

- **Station Storage Reporting Lag**: Slint header bar showed `Storage: RAM_ONLY` on Node A despite the firmware log line "MicroSD card detected & initialized in SPI mode (SDHC/SDSC active)" (`apps/gibberish-firmware/src/main.rs:119`).
- **Empty Cold-Boot View**: Cold startup of `gibberish-client` left the chat pane blank even though SQLite contained stored messages.
- **Direct Messages Misrouted to `#all`**: A direct message sent to `0xBEBD82B4` appeared in `#all` on the recipient station.
- **Airwave Loopback Suppressed on Remote Stations**: Daemon logs on the Mac node reported:
  `[Airwave Loopback Suppressed] Overheard own transmission from Node 0xBEBCE5B8; skipping duplicate chat insertion`
  even though `0xBEBCE5B8` was the physical Linux node transmitting nearby (the "10 feet away" test setup is unverified).
- **UI Sluggishness**: Operators experienced an interactive disconnect where typed text vanished from the input field upon pressing Send but did not immediately render in the chat thread.

## What Didn't Work

- **Expanding `MeshHeader` to Include `dest_node_id` on the Wire**:
  Attempting to add a 4-byte `dest_node_id` field to the shared `MeshHeader` structure pushed the overall packet size beyond the IEEE 802.15.4 Physical Layer Maximum Transmission Unit (PHY MTU) of 127 bytes. With an 11-byte MAC Header (MHR), 18-byte `MeshHeader`, 96-byte ciphertext chunk, and 2-byte Frame Check Sequence (FCS), the wire frame already consumed exactly 127 bytes ($11 + 18 + 96 + 2 = 127$). Expanding the header was said to cause a buffer overflow or frame truncation (unverified; no commit or log records this attempt). The 127-byte arithmetic itself holds (`crates/gibberish-protocol/src/frame.rs:6-16`).
- **Filtering Messages Solely by MAC-Layer Promiscuous Checks**:
  The firmware's MAC-tail filter (`raw.data[8..12] == local_mac[4..8]`, a software comparison on non-standard header bytes) drops frames carrying the dongle's own MAC tail (whether the radio ever hears its own transmissions is unproven, see the self-reception doc), but cannot detect whether an overheard packet was transmitted by a peer node or by another dongle on the same host.
- **Seeding Host Daemon Dongle Sets with Static Defaults**:
  `gibberish-daemon` initialized its default station ID to `local_node_id = 0xBEBCE5B8` and pre-seeded `local_dongle_ids.insert(local_node_id)`. When deployed on the second machine (Mac Node `0xBEBD82B4`), the daemon retained Node A's ID in its local suppression table before dynamic auto-discovery had completed. When Node A transmitted, Node B inspected `local_dongle_ids.contains(&wire_pkt.src_node_id)`, returned `true`, and dropped the packet as a self-echo.

## Solution

The fix addresses the full lifecycle across firmware telemetry parsing, payload-enclosed routing, dynamic hardware discovery, and optimistic UI updates.

### 1. Local Diagnostic Heartbeat & Storage Telemetry Parsing

In [`apps/gibberish-daemon/src/fleet.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-daemon/src/fleet.rs) and [`apps/gibberish-daemon/src/main.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-daemon/src/main.rs#L281-L301), the daemon parses the firmware's local heartbeat log lines, of the form `[Node XXXXXXXX] Uptime: ...s | Storage: ... | SRAM: n/256 pkts | Drops: n` (`parse_local_heartbeat_line`, `fleet.rs:51-57`; emitted by the firmware at `apps/gibberish-firmware/src/main.rs:669-677`), and notifies the client over JSON-RPC IPC. Note that the `rx_packets`, `tx_packets`, `lqi`, and `rssi` values sent to the UI are hard-coded placeholders (`0`, `0`, `200`, `-45`), not measurements:

```rust
// Parse local dongle diagnostic heartbeats for storage capabilities
if let Some(hb) = fleet::parse_local_heartbeat_line(&line) {
    if is_primary_dongle {
        local_dongle_ids.insert(hb.node_id);
        if !user_specified_node_id && hb.node_id != local_node_id {
            local_node_id = hb.node_id;
            ipc.set_local_node_id(local_node_id);
        }
        ipc.set_storage_mode(&hb.storage_mode);
        ipc.set_storage_stats(&hb.storage_stats);
        ipc.broadcast_event(
            "telemetry_update",
            serde_json::json!({
                "node_id": format!("0x{:08X}", local_node_id),
                "storage_mode": hb.storage_mode,
                "storage_stats": hb.storage_stats,
                "rx_packets": 0,
                "tx_packets": 0,
                "lqi": 200,
                "rssi": -45,
            }),
        );
    }
}
```

### 2. Cold-Boot Swarm History Loading

In [`apps/gibberish-client/src/transport.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-client/src/transport.rs#L201) (added in 23dbb4c), the client's connect handshake queries the daemon for `#all` history alongside status and contacts:

```rust
// Query live daemon status, contacts, and initial message thread on connect
let _ = self.get_status();
let _ = self.list_contacts();
let _ = self.list_messages("#all", 50, 0);
```

### 3. Payload-Enclosed Direct Message Routing (MTU-Constrained Prefix)

Because the 127-byte PHY MTU prohibits adding `dest_node_id` to the wire `MeshHeader`, 1-to-1 direct messages embed a 4-byte big-endian `dest_node_id` prefix inside the encrypted payload chunk. Intermediate blind relay forwarders continue handling 114-byte wire packets opaquely, while endpoints decrypt and inspect the routing prefix.

In [`apps/gibberish-daemon/src/chunk.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-daemon/src/chunk.rs#L116-L136):

```rust
pub fn fragment_and_encrypt_dm(
    dest_node_id: u32,
    text: &Secret<String>,
    swarm_tag: u64,
    local_node_id: u32,
    swarm_master_key: &Secret<[u8; 32]>,
    nonce_mgr: &mut NonceManager,
) -> Result<Vec<MeshPacket>, CryptoError> {
    let mut raw_bytes = Vec::with_capacity(4 + text.expose_secret().len());
    raw_bytes.extend_from_slice(&dest_node_id.to_be_bytes());
    raw_bytes.extend_from_slice(text.expose_secret().as_bytes());
    Self::fragment_and_encrypt_bytes(
        &raw_bytes,
        swarm_tag,
        local_node_id,
        swarm_master_key,
        nonce_mgr,
        FLAG_DIRECT | FLAG_ACK_REQ,
    )
}
```

In [`apps/gibberish-daemon/src/chunk.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-daemon/src/chunk.rs#L303-L318), reassembly parses the destination prefix when `FLAG_DIRECT` is set:

```rust
if (flags & FLAG_DIRECT) != 0 && assembled_bytes.len() >= 4 {
    let dest_id = u32::from_be_bytes([
        assembled_bytes[0],
        assembled_bytes[1],
        assembled_bytes[2],
        assembled_bytes[3],
    ]);
    let body_bytes = &assembled_bytes[4..];
    if let Ok(text) = String::from_utf8(body_bytes.to_vec()) {
        return Ok(Some(IngestedMessage::new(text, flags, Some(dest_id))));
    }
}
```

In [`apps/gibberish-daemon/src/main.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-daemon/src/main.rs#L405-L429), the daemon routes inbound direct messages to the sender's dedicated conversation thread (`0x{src_node_id:08X}`) while ignoring overheard direct messages intended for other mesh nodes:

```rust
let (convo_id, is_for_me) = if (ingested.flags & gibberish_protocol::FLAG_DIRECT) != 0 {
    if let Some(dest_id) = ingested.dest_node_id {
        if dest_id == local_node_id || local_dongle_ids.contains(&dest_id) {
            (format!("0x{:08X}", wire_pkt.src_node_id), true)
        } else {
            log.log(&format!(
                "[Mesh RX Overheard DM] Direct message destined for 0x{:08X} (not for local station 0x{:08X}); skipping",
                dest_id, local_node_id
            ));
            (format!("0x{:08X}", wire_pkt.src_node_id), false)
        }
    } else {
        (format!("0x{:08X}", wire_pkt.src_node_id), false)
    }
} else {
    ("#all".to_string(), true)
};

if !is_for_me {
    continue;
}
```

### 4. Dynamic Hardware Node ID Discovery & Unbiased Echo Suppression

In [`apps/gibberish-daemon/src/main.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-daemon/src/main.rs#L250-L253) (initial value at line 50, seeding at lines 250-253; fb7f290), the default fallback node ID initialization is removed so `local_dongle_ids` starts empty unless the user explicitly provides `--node-id` on the CLI:

```rust
let mut local_node_id: u32 = 0; // Populated strictly from hardware auto-discovery
let mut user_specified_node_id = false;
// ...
let mut local_dongle_ids = std::collections::HashSet::new();
if user_specified_node_id && local_node_id != 0 {
    local_dongle_ids.insert(local_node_id);
}
```

Local dongle IDs are now inserted only when confirmed by attached hardware serial heartbeats (`parse_dongle_node_id` or `parse_local_heartbeat_line`; both parse firmware log text), eliminating false-positive echo suppression on remote peer machines.

### 5. Optimistic UI Chat Reflection

In [`apps/gibberish-client/src/controller.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-client/src/controller.rs#L488-L498), outgoing chat submissions immediately inject a pending message item (`*` for Swarm, `[Q]` for Direct) into the Slint model. When the daemon's IPC acknowledges or delivers the message, the deduplication and status updater reconciles the item without duplication:

```rust
// Immediately reflect message in UI for instant responsiveness
let status = if convo == "#all" { "*" } else { "[Q]" };
let is_outgoing = true;
let _ = sender_for_chat.send(UiEvent::MessageReceived(ChatMessageItem {
    id: format!("local-{}", now).into(),
    convo_id: convo.into(),
    sender: "Me".into(),
    text: txt,
    timestamp: format!("{:02}:{:02}:{:02}", (now % 86400) / 3600, (now % 3600) / 60, now % 60).into(),
    status: status.into(),
    is_outgoing,
    sender_color: derive_sender_color("Me", is_outgoing),
}));
```

## Why This Works

1. **Protocol Framing Within Exact MTU Limits**:
   The IEEE 802.15.4 Physical Layer restricts Maximum Transmission Units to 127 octets. By packing the 4-byte `dest_node_id` inside the encrypted 80-byte plaintext chunk (the 96-byte figure is the ciphertext including the 16-byte Poly1305 tag; the first chunk of a direct message therefore carries 76 bytes of text, `frame.rs:10-14`, `chunk.rs:150`) rather than in the wire header, the unencrypted wire frame stays the same size across broadcast and direct traffic. (An earlier version of this doc claimed this maintains forward compatibility with existing relay firmware; that no longer holds, see the superseded note above.)
2. **Strict Ground-Truth Station Identity**:
   Airwave loopback suppression must be scoped strictly to physical hardware transceivers attached to the local machine. Removing static fallback IDs ensures that a daemon instance running on Station B never inherits Station A's ID, avoiding catastrophic cross-station packet drops.
3. **Decoupled Asynchronous UI Feedback**:
   Decoupling local Slint model updates from network round-trips guarantees that operators receive immediate visual confirmation when typing, but the later status transitions (`[Q]` -> `*` -> `[OK]`) do not reflect mesh delivery state in production: `DtnOutbox::on_delivery_ack_received` is called only from tests (`apps/gibberish-daemon/tests/dtn_test.rs:91`), so no production path marks a sent message delivered.

## Prevention

- **Eliminate Hardcoded Fallback Node IDs**: Daemons and test runners must never default to non-zero hardware IDs; station identities must be auto-discovered (currently by parsing the dongle's firmware log lines, `parse_dongle_node_id` in `apps/gibberish-daemon/src/transport.rs:103`, not USB descriptors) or explicitly passed via CLI flags.
- **Strict MTU Budget Verification**: Any proposed wire protocol changes must calculate the complete PSDU overhead ($11\text{B MHR} + \text{MeshHeader} + \text{Ciphertext} + 2\text{B FCS} \le 127\text{B}$) with automated compile-time or unit-test assertions.
- **Automated Dual-Host Integration Tests**: Test suites should include automated multi-node scripts (like `tools/mac-sync.sh` and `tools/remote-debug-sink.py`) to verify real two-way radio exchanges between physically separated hardware controllers.

## Related Issues

- [`docs/solutions/integration-issues/esp32c5-802154-multi-dongle-airwave-echo-and-message-dedup.md`](file:///home/tscott/Work/esp32/gibberish/docs/solutions/integration-issues/esp32c5-802154-multi-dongle-airwave-echo-and-message-dedup.md) — Predecessor fix for single-host multi-dongle echo suppression.
- [`docs/solutions/integration-issues/esp32c5-802154-promiscuous-self-reception-and-station-identity.md`](file:///home/tscott/Work/esp32/gibberish/docs/solutions/integration-issues/esp32c5-802154-promiscuous-self-reception-and-station-identity.md) — Promiscuous MAC-layer loopback filtering in C5 radio firmware.
