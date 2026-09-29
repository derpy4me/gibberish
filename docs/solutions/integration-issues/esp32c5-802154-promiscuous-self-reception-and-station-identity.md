---
title: "ESP32-C5 802.15.4 Promiscuous Self-Reception Loopback Suppression and Hardware Station Identity"
date: "2026-09-23"
category: "integration-issues"
module: "firmware"
problem_type: "integration_issue"
component: "radio"
severity: "high"
symptoms:
  - "Infinite broadcast echo loops where an ESP32-C5 node re-processes its own transmitted 802.15.4 frames as incoming traffic"
  - "Host companion daemon experiencing clipboard ping-pong storms between local and remote stations"
  - "USB-CDC serial buffer saturation and radio airtime exhaustion on Channel 15 (2.425 GHz)"
  - "Cryptographic subkey mismatch when host daemons assume static node IDs instead of hardware MAC-derived station IDs"
root_cause: "logic_error"
resolution_type: "code_fix"
tags:
  - "esp32-c5"
  - "ieee802154"
  - "promiscuous-rx"
  - "loopback-suppression"
  - "hardware-mac"
  - "t-dongle-c5"
  - "no-std"
  - "rust"
---

# ESP32-C5 802.15.4 Promiscuous Self-Reception Loopback Suppression and Hardware Station Identity

> Superseded (2026-09-29): production frames may carry no MAC or MAC-derived bytes in the MAC header, so the source-MAC self-echo discard and MAC-derived station identity described below are replaced by a redesign with no MAC bytes on the air. See docs/plans/2026-09-29-0837-feat-production-opsec-radio-contract-plan.md (R5, R18, KTD1). The premise that the radio hears its own transmissions is also unproven (see Problem).

## Problem

When operating the ESP32-C5 (LilyGO T-Dongle-C5) in raw IEEE 802.15.4 promiscuous mesh mode (`cfg.promiscuous = true; cfg.rx_when_idle = true;`) on Channel 15 (2.425 GHz), the firmware discards frames whose source bytes match its own MAC. The original hypothesis was that the RF transceiver captures its own transmitted broadcast frames directly off the internal antenna coupler; this is unverified (the 802.15.4 radio is half-duplex, the filter existed from the initial commit so there is no before/after evidence, and a later doc traces the observed echoes to a second dongle, see the airwave-echo doc). To verify: run one dongle with the filter disabled and count its own `msg_id`s on RX. 

In a distributed encrypted mesh with desktop host companion daemons (`gibberish-daemon`) communicating over USB-CDC serial links, unsuppressed self-receptions trigger catastrophic feedback loops: Dongle A transmits a broadcast frame, immediately receives its own packet from the radio buffer, and relays it over USB-CDC to Host A. Host A treats it as a newly received remote frame from an external peer and re-emits or synchronizes it, generating infinite clipboard ping-pong storms, USB buffer overruns, and RF channel saturation. Furthermore, statically hardcoding station identifiers or fallback keys across nodes breaks cryptographic origin binding and message deduplication.

## Symptoms

- **Infinite Self-Echo Ping-Pong Storms**: When a user copies text on Host A or sends a broadcast frame, the local ESP32-C5 transmits it over RF and immediately receives it back. The daemon logs rapid-fire reciprocal updates (`Transmitted -> Received -> Transmitted`) at dozens of events per second until serial buffers overflow.
- **RF Channel Congestion on Channel 15**: Broadcast amplification saturates the 250 kbps PHY layer, triggering packet collisions, Clear Channel Assessment (CCA) backoff timeouts, and dropped packets (`telemetry.dropped_count`).
- **USB-CDC Buffer Saturation & Firmware Lockup**: The CDC serial ring buffer fills with self-reflected packets, stalling UART/USB execution and preventing the firmware from servicing real-time ST7735 LCD updates or user button presses.
- **Station Identity & Cryptographic Subkey Mismatch**: If the host companion daemon assumes a fixed default station ID (`0xBEBCE5B8`) instead of binding to the dongle's true physical MAC address, multi-dongle environments fail to decrypt packets because keyed-BLAKE3 sender subkeys (`derive_sender_subkey`, `crates/gibberish-crypto/src/ratchet.rs:47-52`) diverge from the transmitting node's actual hardware identity.

## What Didn't Work

- **Host-Side Clipboard Content Hash Comparison Only**:
  Relying solely on host clipboard change hashes (`last_clipboard_hash == new_hash`) failed because intermediate chunk framing, vector clocks, or nonces mutated metadata across successive transmission rounds. Even with a short provenance suppression window, multi-chunk messages spanning several frames slipped through between chunk deliveries.
- **Filtering by PAN ID or Short Address in Radio Hardware Registers**:
  Setting `cfg.promiscuous = false` and relying on 802.15.4 hardware destination address matching stopped self-reception, but broke broadcast mesh networking completely: in an ad-hoc mesh with dynamic multi-hop routing, packets must be broadcast to `ADDR_BROADCAST (0xFFFF)` with `PAN_ID_BROADCAST (0xFFFF)`. Filtering out broadcasts in hardware prevented nodes from receiving legitimate traffic from neighboring peers.
- **Higher-Layer Sliding Bloom Filter Deduplication**:
  Relying on the firmware's payload bloom filter (`SlidingBloomFilter`) to catch self-echoes caused severe cache pollution. Outgoing frames inserted into the bloom filter pushed out legitimate remote packet hashes, causing the node to miss valid incoming retransmissions from distant routers.
- **Static Configuration Flags for Station Node IDs**:
  Hardcoding Station Node IDs via CLI flags (`--node-id 0xBEBCE5B8`) or compile-time constants worked in single-device tests, but immediately broke when swapping dongles between machines (e.g., Linux vs macOS) or plugging in dual local dongles for integration testing.

## Solution

The solution combines two architectural mechanisms: **low-level MAC self-reception loopback suppression in firmware** and **dynamic hardware station identity discovery in the host daemon**.

### 1. Firmware Low-Level MAC Loopback Filtering

In [`poll_rx_frame`, `apps/gibberish-firmware/src/radio/ieee802154.rs:131-134`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-firmware/src/radio/ieee802154.rs#L131-L134), the low-level frame receiver compares four bytes of the frame header (`raw.data[8..12]`, i.e. PSDU bytes 7..11, since `raw.data[0]` is the length) against the lower 4 bytes of the ESP32-C5's factory hardware MAC. These are not a standard IEEE 802.15.4 Source Short Address: the frame control field 0x0841 declares no source addressing (`radio/ieee802154.rs:104-117`), and a short address is 2 bytes. The 4-byte MAC tail is a non-standard field that standard parsers treat as payload. The check runs after `parse_variable_phy_frame` succeeds:

```rust
// In RadioManager::poll_rx_frame() at apps/gibberish-firmware/src/radio/ieee802154.rs:126-134 (abridged)
while let Some(raw) = self.radio.raw_received() {
    let len = raw.data[0] as usize;
    if len >= MHR_LEN + MESH_HEADER_LEN + 2 && len < raw.data.len() {
        if let Ok((header, payload)) = parse_variable_phy_frame(&raw.data[1..1 + len]) {
            // Loopback check: ignore packets transmitted by this node
            let src_matches_local = raw.data[8..12] == self.local_mac[4..8];
            if src_matches_local {
                continue;
            }
            // ... RSSI/LQI extraction, src_node_id from raw.data[8..12], FrameKind dispatch ...
        }
    }
}
```

(The earlier `poll_rx()` form with `parse_phy_frame_swarm` and `CIPHERTEXT_LEN` was replaced by `poll_rx_frame` + `parse_variable_phy_frame` in adf775e.)

### 2. Dynamic Hardware Station Identity Probing in Host Daemon

In [`apps/gibberish-daemon/src/main.rs:182-195`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-daemon/src/main.rs#L182-L195), the companion daemon inspects serial CDC initialization logs from connected dongles upon connection, automatically binding its encryption engine to the physical ESP32-C5's hardware station Node ID:

```rust
// In apps/gibberish-daemon/src/main.rs
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
```

## Why This Works

1. **Zero-Overhead Early Discard**:
   Dropping loopback frames at the physical MAC layer (`raw.data[8..12] == self.local_mac[4..8]`) occurs after the frame header is parsed but before USB serial transmission. The dongle never performs AEAD decryption (the firmware crate must not depend on `gibberish-crypto`, `apps/gibberish-firmware/Cargo.toml:18-20`). No serial bandwidth is wasted, and the host daemon is never exposed to self-reflected traffic.
2. **Promiscuous Broadcast Preserved**:
   Because loopback filtering inspects the IEEE 802.15.4 frame header's Source Address field rather than restricting Destination Addresses, the node can remain in fully promiscuous broadcast mode (`ADDR_BROADCAST = 0xFFFF`) to receive frames from all valid peers across the airwaves.
3. **Hardware-Derived Node IDs (superseded)**:
   Deriving Node IDs from ESP32-C5 factory MAC bytes gives per-board uniqueness without provisioning, and the daemon can bind to the ID that dongle logs at startup. OPSEC plan R5/KTD1 remove MAC-derived values from production frames, and R18 has the daemon learn its identity without relying on dongle debug output.

## Prevention

1. **Source-Address Self-Echo Discard in Embedded Promiscuous Receivers (superseded for production; MAC bytes are no longer allowed on the air, so self-echo handling must be redesigned)**:
   Whenever an 802.15.4, BLE, or raw sub-GHz radio is configured with `promiscuous = true` and `rx_when_idle = true`, the receive ISR or polling loop must explicitly compare the source address against local hardware identity before dispatching packets up the stack.
2. **Decouple Broadcast Addressing from Node Filtering**:
   Keep radio PHY configuration in promiscuous mode with broadcast PAN/Address support, and enforce topology and loopback rules at the immediate MAC framing boundary.
3. **Verify Physical Bidirectional Airwaves in CI/Hardware Lab**:
   Test mesh networking with at least two physical boards running on distinct USB ports. Verify that Node A transmitting to Node B does not trigger an echo response on Node A's serial monitor.
