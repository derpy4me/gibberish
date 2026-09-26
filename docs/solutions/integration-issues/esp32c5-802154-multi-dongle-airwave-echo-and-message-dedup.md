---
title: "ESP32-C5 802.15.4 Multi-Dongle Airwave Echo Suppression and Sub-Second Message ID Deduplication"
date: "2026-09-25"
category: "integration-issues"
module: "gibberish-daemon"
problem_type: "integration_issue"
component: "messaging"
severity: "medium"
symptoms:
  - "Broadcast messages sent on #all swarm appear reflected as an incoming message from the transmitting station's Node ID"
  - "Sent broadcast messages intermittently display asterisk (*) without expected delivery confirmation"
  - "Rapidly transmitted chat messages sent within the same second are dropped by the client UI"
root_cause: "logic_error"
resolution_type: "code_fix"
tags:
  - "esp32-c5"
  - "802154"
  - "mesh"
  - "echo-suppression"
  - "deduplication"
  - "json-rpc"
  - "slint"
  - "multi-dongle"
---

# ESP32-C5 802.15.4 Multi-Dongle Airwave Echo Suppression and Sub-Second Message ID Deduplication

## Problem

When developing and running multiple IEEE 802.15.4 radio dongles (LilyGO T-Dongle-C5) attached to a single host machine or operating within close RF proximity on Channel 15 (2.425 GHz), broadcast frames transmitted by the primary dongle are physically received over the airwaves by the second dongle. 

The companion daemon (`gibberish-daemon`) reassembles the incoming radio frame from the second dongle, recognizes the decrypted payload, and forwards it to the client UI as a newly arrived message from the transmitting node (`0xBEBCE5B8`) with delivery status `[OK]`. This causes the sender to see an unwanted duplicate echo of their own message directly beneath their `<Me> *` line. Furthermore, because incoming messages were assigned message IDs based solely on integer Unix seconds (`m-{timestamp}` and `rx-{timestamp}`), rapid messages or immediate radio reflections occurring in the same second collided, causing the client's `messages_model` deduplication filter to silently drop legitimate messages.

## Symptoms

- **Duplicate Self-Echo in Chat**: Immediately after transmitting a broadcast message on `#all` (Swarm Broadcast), an identical line appears in the Slint GUI formatted as `<0xBEBCE5B8> [message text] [OK]`, mimicking a response from another station.
- **Intermittent Missing Echoes / Status Reflections**: Some sent messages showed the echo bounce-back while others remained marked with `*` (transmitted without ACK), creating confusion as to whether the transmission succeeded.
- **Silently Dropped Rapid Messages**: Sending multiple chat messages within 1–2 seconds resulted in only the first message appearing in the GUI conversation thread; subsequent messages were dropped from the UI despite being recorded in the database.
- **RF Airtime Contention**: Rapid bursts of chat frames occasionally collided with aggressive 5-second autonomous debug telemetry beacons broadcast by the neighboring dongle on the same USB controller.

## What Didn't Work

- **Relying Exclusively on Firmware-Level MAC Self-Discard**:
  The ESP32-C5 firmware already discards frames where the IEEE 802.15.4 MAC Header (MHR) Source Short Address matches its own local MAC address (`raw.data[8..12] == self.local_mac[4..8]`). However, this only suppresses self-reception on the *transmitting* dongle. Dongle 2 has a distinct hardware MAC address and legitimately receives Dongle 1's RF airwave broadcast. Firmware-level loopback filtering cannot detect that Dongle 1 and Dongle 2 share the same host machine.
- **Second-Resolution Timestamp Message IDs**:
  Generating message IDs as `format!("m-{}", ts)` and `format!("rx-{}", now)` using `now.as_secs()` caused ID collisions whenever two events transpired in the same Unix second. In `controller.rs:248-260`, the deduplication logic checks `if m.id == msg.id { exists = true; }`, dropping any message sharing the second-level timestamp.
- **Silencing Swarm Broadcasts in the Client**:
  Suppressing all incoming messages on `#all` where `sender_node_id != 0` would prevent receiving legitimate broadcast messages from other physical peers in the mesh.

## Solution

The fix introduces host-level multi-dongle loopback suppression in `gibberish-daemon` and monotonic sub-second message ID generation across the IPC boundary.

### 1. Multi-Dongle Host-Side Airwave Loopback Suppression

In [`apps/gibberish-daemon/src/main.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-daemon/src/main.rs), the daemon maintains a hash set of all hardware Node IDs discovered across all locally connected USB serial dongles:

```rust
let mut local_dongle_ids = std::collections::HashSet::new();
if local_node_id != 0 {
    local_dongle_ids.insert(local_node_id);
}

// In the serial polling loop:
if let Some(detected_id) = parse_dongle_node_id(&line) {
    local_dongle_ids.insert(detected_id);
    if !user_specified_node_id && is_primary_dongle && detected_id != local_node_id {
        local_node_id = detected_id;
        ipc.set_local_node_id(local_node_id);
    }
}
```

When an incoming mesh message is reassembled by any dongle's transport, the daemon checks if the source node ID belongs to any attached local dongle:

```rust
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
```

### 2. Monotonic Sub-Second Sequence IDs

In [`apps/gibberish-daemon/src/ipc.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-daemon/src/ipc.rs) and [`apps/gibberish-client/src/transport.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-client/src/transport.rs), message IDs include explicit monotonic counters:

```rust
// In gibberish-daemon main.rs:
static RX_MSG_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
let seq = RX_MSG_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
let msg_id = format!("rx-{}-{}", now, seq);
ipc.notify_rx_message(&msg_id, "#all", wire_pkt.src_node_id, &text_str, now, "[OK]");

// In gibberish-client transport.rs:
let notif_id = notif.params.get("id").and_then(|v| v.as_str());
let item_id: slint::SharedString = match notif_id {
    Some(id_str) => id_str.into(),
    None => {
        static FALLBACK_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let c = FALLBACK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("m-{}-{}", ts, c).into()
    }
};
```

### 3. Dedicated Telemetry Channel & Preemption Issue

To resolve underlying half-duplex RF collisions between background telemetry beacons and active chat transmissions, [Issue #24](https://github.com/derpy4me/gibberish/issues/24) was opened and documented in `docs/ideation/tracked-ideas.md` to implement active-transmission quiet windows and throttle production telemetry cadence to 30–60s.

## Why This Works

1. **Host-Wide Identity Awareness**: By aggregating all connected dongle IDs into `local_dongle_ids`, the daemon recognizes loopbacks regardless of which specific dongle transmitted and which overheard the frame on the RF channel.
2. **Deterministic UI Ingestion**: Monotonic sequence counters guarantee that messages created within the same second generate unique keys (`m-{ts}-{seq}`), preserving rapid back-to-back chat messages while still deduplicating true transport retransmissions.
3. **Zero Wire Overhead**: Loopback suppression requires no protocol alterations, extra packet headers, or cryptographic renegotiations; it is evaluated host-side from the physical 802.15.4 MAC address bytes.

## Prevention

1. **Never Assume Single-Dongle Topology in Mesh Development**:
   Development and QA environments frequently attach multiple transceivers to a single workstation. Companion daemons must maintain an aggregate registry of local radio interfaces rather than assuming a single static `local_node_id`.
2. **Never Use Whole-Second Timestamps as Primary Cache Keys**:
   Asynchronous messaging systems easily process multiple events in sub-second windows. Message IDs must always incorporate either nanosecond precision, monotonic sequence counters, or cryptographic hashes (e.g. `blake3(payload)`).
3. **Segregate Background Diagnostic Cadences from Interactive Traffic**:
   Autonomous health beacons should yield during active user transmission windows (preemption) to keep half-duplex airwaves clear for interactive bursts.

## Related Issues

- [Issue #24: feat(radio): Out-of-band / dedicated telemetry channel and transmission preemption](https://github.com/derpy4me/gibberish/issues/24)
- [Issue #15: feat(chat): #all Swarm broadcast channel and pairwise ratcheted 1-to-1 DMs](https://github.com/derpy4me/gibberish/issues/15)
- [Solution: ESP32-C5 802.15.4 Promiscuous Self-Reception Loopback Suppression and Hardware Station Identity](file:///home/tscott/Work/esp32/gibberish/docs/solutions/integration-issues/esp32c5-802154-promiscuous-self-reception-and-station-identity.md)
