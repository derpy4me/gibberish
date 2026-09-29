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
  - "Single build configuration forcing compromise between verbose lab troubleshooting and a quiet production build"
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

> Superseded (2026-09-29): production dongles must emit no autonomous beacons or telemetry, diagnostic telemetry frame types exist only in debug builds, dongle health reaches only the local host, and no production frame may carry the hardware MAC or a MAC-derived value. See docs/plans/2026-09-29-0837-feat-production-opsec-radio-contract-plan.md (R2, R3, R4, R5, KTD1). The mechanisms below describe the pre-contract implementation; the "What Didn't Work" failure stories are design rationale (the doc and the fix landed in one commit, so no failing version is in git history; unverified).

## Problem

In Project Gibberish (an off-grid encrypted communication mesh running on the ESP32-C5 / LilyGO T-Dongle-C5 in bare-metal `no_std` Rust), nodes were designed to broadcast autonomous diagnostic health telemetry over raw IEEE 802.15.4 Channel 15 (2.425 GHz) to central listener sinks (`gibberish-sink`) without external infrastructure (no Wi-Fi, no cellular, no cloud brokers).

Implementing autonomous in-band telemetry over the same RF channel used by high-priority encrypted user messages and clipboard data introduced four systemic failure modes:

1. **Broadcast Storms & Mesh Deduplication Contamination**: Telemetry beacons forwarded by intermediate mesh routers cause exponential broadcast amplification. Furthermore, inserting frequent telemetry frames into the sliding bloom filter deduplication cache can push out actual encrypted user and clipboard messages, causing dropped communications or memory ring exhaustion.
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
  Tracking received telemetry frame identifiers in `SlidingBloomFilter::insert` was expected to pollute the filter (a 10,240-bit Bloom filter plus a 64-entry LRU sized for 1,024 packets, `apps/gibberish-firmware/src/radio/dedup.rs:1-9`) with ephemeral sequence numbers. Legitimate user packets sharing hash buckets were erroneously flagged as duplicates and discarded.
- **Fixed-Interval Periodic Timers**:
  Configuring a static 5-second or 60-second telemetry timer (`telem_elapsed_ms >= 5000`) was expected to let nodes become phase-locked, transmitting at the same moments (the TDM mesh slot is 168 ms of a 200 ms cycle, `radio/coex.rs:10-13`; 5 ms is only the main-loop tick, `main.rs:225`).
- **First-Come, First-Served Transmission Queue**:
  Using a basic FIFO transmission queue for all radio packets allowed low-priority diagnostic telemetry to block urgent user-initiated broadcasts (such as BLE pairing authorizations or encrypted clipboard sync packets).
- **Tracking Fleet Nodes by `msg_id`**:
  Attempting to identify swarm nodes using the 32-bit packet `header.msg_id` in the daemon sink failed because `msg_id` is an incrementing sequence counter per message. Each beacon created a new ghost entry in the fleet manager table.

## Solution

The solution implements an integrated, non-polluting off-grid RF telemetry subsystem and resilient host sink architecture consisting of four core components:

### 1. Isolated Telemetry Framing with Single-Hop (`TTL = 1`) Enforcement

Telemetry packets are explicitly flagged with `FLAG_TELEMETRY = 0x0020` and configured with `ttl = 1` and `total_chunks = 1`.

On reception, the telemetry frame kinds are handled before any deduplication or routing logic. Telemetry packets trigger a 100ms magenta LED pulse, update link health metrics (RSSI), print structured serial lines, and immediately execute `continue;`. They are **never** added to the sliding bloom filter, **never** enqueued into the SRAM ring buffer, and **never** forwarded over the mesh.

Current receive path (`apps/gibberish-firmware/src/main.rs:453-495`; abridged). The original `FLAG_TELEMETRY` check with `packet.payload[20]` and `DebugTelemetryPayload::deserialize` was replaced in adf775e by `FrameKind` dispatch:

```rust
match rx_frame.kind {
    FrameKind::StaticMetadata(_beacon) => {
        telem_flash_ticks = 20; // 100ms magenta visual indicator
        telemetry.rx_packet_count = telemetry.rx_packet_count.saturating_add(1);
        // ... log_info!("[Telemetry RX] Node: {:08X}, Tier: {:?}, Storage: {:?}, Epoch: {}, ...") ...
        continue;
    }
    FrameKind::CompactDelta(_delta) => {
        telem_flash_ticks = 20;
        // ... log_info!("[Telemetry RX] Node: {:08X}, Tier: {:?}, Storage: {:?}, Uptime: {}s, ...") ...
        continue;
    }
    FrameKind::Mesh(packet) => { /* bloom_filter.contains / insert, on_overhear, SRAM ring */ }
}
```

### 2. User Traffic Preemption in CSMA/CA Backoff Controller

In `BackoffController` (`apps/gibberish-firmware/src/radio/ieee802154.rs:201-325`), pending transmissions are segregated into a HIGH queue (chat, clipboard, SACK) and a LOW queue (telemetry). `schedule_low` only arms a backoff when no high-priority traffic is pending, and `schedule_high` aborts an active low-priority backoff:

```rust
pub fn schedule_low(&mut self, header: MeshHeader, payload: &[u8], jitter_ms: u16) {
    // ... enqueue in the LOW queue (newest overwrites oldest when full) ...
    // If no high-priority traffic is pending and not already backing off, arm low backoff
    if self.high_count == 0 && !self.is_low_active {
        self.is_low_active = true;
        self.backoff_remaining_ms = jitter_ms.clamp(15, 60);
    }
}
```

(The earlier `schedule_telemetry` method that checked a single `pending_tx` slot was removed in adf775e.) Overheard transmissions from neighboring nodes only trigger overhearing cancellation via `BackoffController::on_overhear` in the `FrameKind::Mesh` arm (`main.rs:500,504`), so telemetry frames never cancel user data transmissions.

### 3. Anti-Collision Hardware RNG Jitter

Periodic timers use the ESP32-C5 on-chip hardware Random Number Generator (`esp_hal::rng::Rng`) to inject dynamic anti-collision jitter on both the emission intervals and the CSMA/CA contention backoff. The current cadence (`apps/gibberish-firmware/src/main.rs:196-218`) is an RFC 6206 Trickle timer (10-60 s intervals, random point in the second half of each interval) for compact deltas, plus a static metadata beacon every 165-195 s. The earlier `calc_next_telem_interval` with `#[cfg(feature = "prod")]` (5 s / 60 s) was removed in adf775e; the `prod` feature no longer exists (`Cargo.toml` has `default = []` and `debug-telemetry`).

```rust
// Static Metadata Beacon cadence: 180s ± 15s TRNG jitter (165s..=195s)
let calc_next_static_interval = |rng: &esp_hal::rng::Rng| -> u32 {
    165_000 + (rng.random() % 30_001)
};

// Contention backoff jitter: 15..=60ms
let telem_jitter = (hw_rng.random() % 46) as u16 + 15;
backoff.schedule_low(telem_hdr, &delta_buf, telem_jitter);
```

### 4. Multi-Tier Telemetry Payloads (`Debug` vs `Prod`)

The protocol defines `TelemetryTier::Debug = 0xDB` and `TelemetryTier::Prod = 0x50` (`crates/gibberish-protocol/src/frame.rs:349-350`). Current firmware sends variable-length frames rather than the original 96-byte envelope: a 28-byte `StaticMetadataBeacon` and a 24-byte `CompactDeltaPayload` (`frame.rs:558,662`), with the beacon's `build_tier` set from the `debug-telemetry` feature. These frames are not free of identifying data: every frame's MAC header carries the MAC tail (`radio/ieee802154.rs:104-117`), the beacon carries a MAC-derived node ID (`full_mac` is 4 zero bytes plus the 4-byte MAC-derived ID, `main.rs:67-69`), and a constant network tag is present. The earlier claim that the Prod tier had "zero sensitive data leakage" was false and is what the OPSEC contract above removes.

### 5. MAC-Derived Node ID & Resilient Fleet Sink Parser

In `apps/gibberish-daemon/src/fleet.rs`, the sink identifies nodes by the hex `Node:` field parsed from the firmware's text log lines (`fleet.rs:108-200`), rather than mutable sequence numbers. That ID is the 32-bit MAC-derived node ID; the `node_mac_tail[4..8]` extraction is `DebugTelemetryPayload::node_id()` in `crates/gibberish-protocol/src/frame.rs:506-512`, not `fleet.rs`.

The parser (`parse_telemetry_line`) strips prefix noise (timestamp prefixes, CDC port identifiers) and parses key-value pairs safely, feeding the in-memory `FleetManager` which writes append-only disk logs to `/tmp/gibberish/fleet.log` and renders a live ANSI terminal dashboard.

## Why This Works

1. **Deterministic Bounded Blast Radius**:
   Setting `ttl = 1` ensures telemetry frames are transmitted strictly as single-hop local broadcasts. Even in dense swarms, no node ever forwards a packet marked with `FLAG_TELEMETRY`, guaranteeing that network overhead remains $O(N)$ with respect to node count rather than $O(N \cdot e^{\text{hops}})$.
2. **Preservation of Mesh Deduplication State**:
   Bypassing the sliding bloom filter ensures that diagnostic frames consume zero entries in `SlidingBloomFilter`. The bloom filter remains dedicated to routing encrypted user payloads, removing telemetry as a source of false-positive deduplication drops (a Bloom filter still has an inherent false-positive rate).
3. **Loss-Tolerant Non-Blocking Telemetry Scheduling**:
   Telemetry is inherently loss-tolerant. By designing `schedule_telemetry` to immediately yield (`return false`) when user traffic occupies the transmitter, telemetry does not queue ahead of user messages (added latency was not measured; a low-priority frame already on air still occupies the half-duplex radio).
4. **Collision De-correlation via Hardware Entropy**:
   Introducing hardware RNG jitter to the Trickle and beacon intervals (see Section 3) alongside 15–60ms contention backoff breaks the periodicity of node clocks, preventing continuous destructive interference.
5. **Stable Node Identity (superseded)**:
   Deriving `node_id` from the Station MAC address in eFuse gave a stable node identity across reboots and sequence increments, but it also put a MAC-derived value on the air. OPSEC plan R5/KTD1 forbid that in production frames.

## Prevention

To prevent broadcast saturation, cache pollution, and telemetry contention in future mesh and embedded radio designs:

- **Flag-Gated Processing Pipelines**: Always evaluate packet control flags (`FLAG_TELEMETRY`, `FLAG_ROUTING`, etc.) at the earliest point of ingress. Keep diagnostic and operational protocols in isolated data paths from user payload pipelines.
- **Strict `TTL = 1` for Diagnostics**: Diagnostic beacons and link-probing packets must be single-hop by default. Multi-hop telemetry collection should only occur via explicit pull requests or centralized aggregator nodes.
- **Never Route Ephemeral Diagnostics Through Deduplication Caches**: High-frequency periodic data will poison LRU caches and bloom filters designed for lower-frequency transactional traffic.
- **Contention Preemption by Design**: Implement priority levels in transmission schedulers. Loss-tolerant status frames must immediately yield or drop when user-interactive frames are queued.
- **Hardware Entropy on All Periodic Emitters**: Never use static periodic delays in wireless mesh nodes. Always add hardware RNG jitter proportional to the interval (typically $\pm 10\%$) to avoid accidental phase locking.
- **Decouple Node Identity from Packet Sequences**: Use a stable identifier as the primary key in tracking tables, never transient packet sequence counters. (Superseded in part: a hardware MAC or MAC-derived ID must not be transmitted in production frames, OPSEC plan R5.)
- **Sanitize and Prefix-Tolerate Serial Parsers**: When parsing human-readable CDC diagnostic streams on host daemons, use token-delimited prefix matching and strip non-alphanumeric noise to guarantee resilience against log interleaving.
