---
title: "ESP32-C5 Pure Off-Grid IEEE 802.15.4 Telemetry Sink and Multi-Tier Build Profiles"
date: "2026-09-22"
category: "integration-issues"
module: "firmware"
problem_type: "integration_issue"
component: "radio"
severity: "high"
symptoms:
  - "Lack of centralized fleet health visibility across heterogeneous field devices without compromising air-gapped off-grid isolation"
  - "Periodic telemetry transmissions causing RF channel collisions across simultaneously powered nodes"
  - "Outbound telemetry frames stalling or competing with high-priority user clipboard and chat traffic in CSMA/CA queue"
  - "Intermediate mesh nodes experiencing airtime congestion and storage pollution from relayed diagnostic frames"
  - "Single build configuration forcing compromise between verbose lab troubleshooting and production cryptographic stealth"
root_cause: "config_error"
resolution_type: "code_fix"
tags:
  - "esp32-c5"
  - "t-dongle-c5"
  - "ieee802154"
  - "telemetry"
  - "fleet-sink"
  - "cargo-features"
  - "bare-metal"
  - "no-std"
  - "rust"
---

# ESP32-C5 Pure Off-Grid IEEE 802.15.4 Telemetry Sink and Multi-Tier Build Profiles

## Problem

In Project Gibberish (an off-grid encrypted communication mesh running on the ESP32-C5 / LilyGO T-Dongle-C5 in bare-metal `no_std` Rust), nodes must broadcast autonomous diagnostic health telemetry over raw IEEE 802.15.4 Channel 15 (2.425 GHz) to central listener sinks (`gibberish-sink`) without external infrastructure (no Wi-Fi, no cellular, no cloud brokers).

Implementing autonomous in-band telemetry over the same RF channel used by high-priority encrypted user messages and clipboard data introduced four systemic failure modes:

1. **Broadcast Storms & Mesh Deduplication Contamination**: Telemetry beacons forwarded by intermediate mesh routers cause exponential broadcast amplification. Furthermore, inserting frequent telemetry frames into the sliding bloom filter deduplication cache pushes out actual encrypted user and clipboard messages, causing dropped communications or memory ring exhaustion.
2. **RF Channel Contention & User Traffic Starvation**: Autonomous, periodic telemetry emissions compete for the same 802.15.4 half-duplex radio channel as interactive user traffic. Without strict prioritization, autonomous telemetry packets block or delay critical user payloads.
3. **Synchronous Phase-Locked Beacon Collisions**: Fixed periodic telemetry timers across multiple nodes cause transmission schedules to drift into lockstep, resulting in persistent over-the-air collisions on Channel 15.
4. **Node Misidentification & Sink Crash on Corrupted Frames**: Desktop companion daemons monitoring the serial CDC stream (`/dev/ttyACM*`) easily misattribute nodes if ephemeral message sequence numbers are treated as node identifiers, or crash/hang when raw serial streams interleave debug logs, terminal escape codes, or malformed frames.

## Symptoms

- **Mesh Radio Flooding**: Telemetry packets rebroadcast across all nodes until hop exhaustion, saturating the 2.425 GHz channel and triggering false packet drop events (`telemetry.dropped_count`).
- **User Message Loss via Bloom Filter Cache Eviction**: High-frequency telemetry packets (`msg_id`, `chunk_idx`) rapidly consume capacity in the node's `SlidingBloomFilter`, leading to premature eviction of genuine encrypted mesh packets or unnecessary retransmissions.
- **Transmitter Starvation**: When a user presses the physical BOOT button or transmits a clipboard chunk via USB-CDC, the packet is delayed or dropped because an autonomous telemetry beacon occupied the single-packet transmission slot in the backoff controller.
- **Repeated Over-the-Air Packet Collisions**: Two or more dongles powered on simultaneously regularly fail to deliver telemetry packets to the sink due to synchronized periodic transmission timers.
- **Fleet Dashboard Chaos**: The desktop sink dashboard (`gibberish-sink`) displays flickering, phantom node IDs (e.g., treating sequence IDs `00000001`, `00000002` as new hardware nodes) and encounters parsing panics when timestamps or device port tags precede telemetry log lines.

## What Didn't Work

- **Standard Multi-Hop Mesh Forwarding (`TTL > 1`) for Telemetry**:
  Treating telemetry beacons as ordinary mesh packets caused every listening node to decrement TTL and schedule forwarding with randomized jitter. In a cluster of dongles, a single periodic beacon triggered a cascade of duplicate transmissions that choked the 250 kbps PHY layer.
- **Inserting Telemetry into Deduplication Bloom Filters**:
  Tracking received telemetry frame identifiers in `SlidingBloomFilter::insert` quickly polluted the 256-entry filter with ephemeral sequence numbers. Legitimate user packets sharing hash buckets were erroneously flagged as duplicates and discarded.
- **Fixed-Interval Periodic Timers**:
  Configuring a static 5-second or 60-second telemetry timer (`telem_elapsed_ms >= 5000`) caused nodes to become phase-locked, continually attempting to transmit in the exact same 5ms time-division multiplexing (TDM) radio slot.
- **First-Come, First-Served Transmission Queue**:
  Using a basic FIFO transmission queue for all radio packets allowed low-priority diagnostic telemetry to block urgent user-initiated broadcasts (such as BLE pairing authorizations or encrypted clipboard sync packets).
- **Tracking Fleet Nodes by `msg_id`**:
  Attempting to identify swarm nodes using the 32-bit packet `header.msg_id` in the daemon sink failed because `msg_id` is an incrementing sequence counter per message. Each beacon created a new ghost entry in the fleet manager table.

## Solution

The solution implements an integrated, non-polluting off-grid RF telemetry subsystem and resilient host sink architecture consisting of four core components:

### 1. Isolated Telemetry Framing with Single-Hop (`TTL = 1`) Enforcement

Telemetry packets are explicitly flagged with `FLAG_TELEMETRY = 0x0020` and configured with `ttl = 1` and `total_chunks = 1`.

On reception, the radio receiver checks for `FLAG_TELEMETRY` before any deduplication or routing logic. Telemetry packets trigger a 100ms magenta LED pulse, update link health metrics (RSSI/LQI), print structured serial lines, and immediately execute `continue;`. They are **never** added to the sliding bloom filter, **never** enqueued into the SRAM ring buffer, and **never** forwarded over the mesh.

```rust
// In apps/gibberish-firmware/src/main.rs:
// Check if this is a TELEMETRY frame
// Telemetry frames have TTL=1, are consumed locally, and MUST NOT pollute the
// mesh deduplication bloom filter or cancel pending user packets via overhearing.
if (packet.header.flags & FLAG_TELEMETRY) != 0 {
    telem_flash_ticks = 20; // 100ms magenta visual indicator
    telemetry.rx_packet_count = telemetry.rx_packet_count.saturating_add(1);
    telemetry.last_event = DiagnosticEventCode::RadioRxOk;
    telemetry.last_rssi = rx.rssi;

    if packet.payload[20] == (TelemetryTier::Debug as u8) {
        let dbg = DebugTelemetryPayload::deserialize(&packet.payload);
        let node_id = if dbg.node_id() != 0 { dbg.node_id() } else { rx.src_node_id };
        println!(
            "[Telemetry RX] Node: {:08X}, Tier: {:?}, Storage: {:?}, Uptime: {}s, SRAM: {}/256, Drops: {}, RX: {}, TX: {}, RSSI: {} dBm, LQI: {}",
            node_id, dbg.build_tier, dbg.storage_mode, dbg.uptime_secs, dbg.sram_used,
            dbg.drop_count, dbg.rx_count, dbg.tx_count, rx.rssi, rx.lqi
        );
    }
    // Local consumption without mesh relay (TTL=1) or SRAM ring pollution
    continue;
}
```

### 2. User Traffic Preemption in CSMA/CA Backoff Controller

In `BackoffController`, pending transmissions are segregated by priority. The `schedule_telemetry` method strictly yields whenever user traffic is currently scheduled:

```rust
// In apps/gibberish-firmware/src/radio/ieee802154.rs:
/// Schedule telemetry packet only if no high-priority user traffic is pending.
pub fn schedule_telemetry(&mut self, packet: MeshPacket, jitter_ms: u16) -> bool {
    if let Some(ref p) = self.pending_tx {
        // If pending packet does NOT have FLAG_TELEMETRY, it is user traffic: yield!
        if (p.header.flags & gibberish_protocol::FLAG_TELEMETRY) == 0 {
            return false; // Yield to user traffic
        }
    }
    self.schedule_tx(packet, jitter_ms);
    true
}
```

Furthermore, overheard transmissions from neighboring nodes only trigger overhearing cancellation in `BackoffController::on_overhear` for non-telemetry mesh frames, preventing telemetry packets from canceling user data transmissions.

### 3. Anti-Collision Hardware RNG Jitter

Periodic timers use the ESP32-C5 on-chip hardware Random Number Generator (`esp_hal::rng::Rng`) to inject dynamic anti-collision jitter on both the emission intervals and the CSMA/CA contention backoff:

```rust
// In apps/gibberish-firmware/src/main.rs:
let calc_next_telem_interval = |rng: &esp_hal::rng::Rng| -> u32 {
    #[cfg(feature = "prod")]
    {
        55_000 + (rng.random() % 10_001) // 60s ± 5s
    }
    #[cfg(not(feature = "prod"))]
    {
        4_500 + (rng.random() % 1_001)   // 5s ± 500ms
    }
};

// Contention backoff jitter: 15..=60ms
let telem_jitter = (hw_rng.random() % 46) as u16 + 15;
let _ = backoff.schedule_telemetry(telem_pkt, telem_jitter);
```

### 4. Multi-Tier Telemetry Payloads (`Debug` vs `Prod`)

The protocol supports dual-tier serialization within the exact 96-byte `CIPHERTEXT_LEN` envelope:

- **Debug Tier (`TelemetryTier::Debug = 0xDB`)**: Unencrypted binary layout packing 32-bit uptime, frame counters, drop counters, SRAM buffer depth, storage operating mode, scalar event codes, free heap KB, RF link metrics (RSSI/LQI), and the 8-byte Station MAC address tail.
- **Prod Tier (`TelemetryTier::Prod = 0x50`)**: Compact scalar struct encoded with `postcard::to_slice` with zero string allocations or sensitive data leakage.

### 5. Hardware MAC Resolution & Resilient Fleet Sink Parser

In `gibberish-daemon/src/fleet.rs`, the sink identifies nodes by their permanent 32-bit hardware Station MAC identifier (`node_id`), extracted from `node_mac_tail[4..8]`, rather than mutable sequence numbers.

The parser (`parse_telemetry_line`) strips prefix noise (timestamp prefixes, CDC port identifiers) and parses key-value pairs safely, feeding the in-memory `FleetManager` which writes append-only disk logs to `/tmp/gibberish/fleet.log` and renders a live ANSI terminal dashboard.

## Why This Works

1. **Deterministic Bounded Blast Radius**:
   Setting `ttl = 1` ensures telemetry frames are transmitted strictly as single-hop local broadcasts. Even in dense swarms, no node ever forwards a packet marked with `FLAG_TELEMETRY`, guaranteeing that network overhead remains $O(N)$ with respect to node count rather than $O(N \cdot e^{\text{hops}})$.
2. **Preservation of Mesh Deduplication State**:
   Bypassing the sliding bloom filter ensures that diagnostic frames consume zero entries in `SlidingBloomFilter`. The bloom filter remains dedicated entirely to routing encrypted user payloads, completely eliminating false-positive deduplication drops.
3. **Loss-Tolerant Non-Blocking Telemetry Scheduling**:
   Telemetry is inherently loss-tolerant. By designing `schedule_telemetry` to immediately yield (`return false`) when user traffic occupies the transmitter, user messages suffer zero added latency from telemetry emissions.
4. **Collision De-correlation via Hardware Entropy**:
   Introducing $\pm 500\text{ms}$ (debug) or $\pm 5\text{s}$ (prod) hardware RNG jitter to beacon intervals alongside 15–60ms contention backoff breaks the periodicity of node clocks, preventing continuous destructive interference.
5. **Exact Hardware Identification**:
   Deriving `node_id` from the Station MAC address burned into eFuse provides a stable, permanent node identity across reboots and message sequence increments.

## Prevention

To prevent broadcast saturation, cache pollution, and telemetry contention in future mesh and embedded radio designs:

- **Flag-Gated Processing Pipelines**: Always evaluate packet control flags (`FLAG_TELEMETRY`, `FLAG_ROUTING`, etc.) at the earliest point of ingress. Keep diagnostic and operational protocols in isolated data paths from user payload pipelines.
- **Strict `TTL = 1` for Diagnostics**: Diagnostic beacons and link-probing packets must be single-hop by default. Multi-hop telemetry collection should only occur via explicit pull requests or centralized aggregator nodes.
- **Never Route Ephemeral Diagnostics Through Deduplication Caches**: High-frequency periodic data will poison LRU caches and bloom filters designed for lower-frequency transactional traffic.
- **Contention Preemption by Design**: Implement priority levels in transmission schedulers. Loss-tolerant status frames must immediately yield or drop when user-interactive frames are queued.
- **Hardware Entropy on All Periodic Emitters**: Never use static periodic delays in wireless mesh nodes. Always add hardware RNG jitter proportional to the interval (typically $\pm 10\%$) to avoid accidental phase locking.
- **Decouple Node Identity from Packet Sequences**: Hardware identifiers (MAC addresses or cryptographic public key hashes) must serve as the primary key in tracking tables, never transient packet sequence counters.
- **Sanitize and Prefix-Tolerate Serial Parsers**: When parsing human-readable CDC diagnostic streams on host daemons, use token-delimited prefix matching and strip non-alphanumeric noise to guarantee resilience against log interleaving.
