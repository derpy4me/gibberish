---
title: Project Gibberish - Off-Grid RF Telemetry Sink & Multi-Tier Build Profiles Plan
type: feat
date: 2026-09-22
topic: gibberish-telemetry-sink
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

## Goal Capsule

- **Objective:** Build an autonomous, pure off-grid IEEE 802.15.4 RF telemetry sink and tiered build profiles (`debug` vs `prod`) for Project Gibberish. A central development machine with a plugged-in C5 dongle overhears in-band telemetry beacons broadcast by all peer dongles (Linux, macOS, Windows, Android) over 2.4 GHz airwaves, visualizing a real-time fleet health dashboard without touching Wi-Fi, routers, or IP networks.
- **Means:**
  1. In-band 802.15.4 telemetry frames (`FLAG_TELEMETRY = 0x0020`) encoding compact scalar telemetry (`ClosedTelemetry`) and diagnostic descriptors into standard 114-byte wire payloads.
  2. Cargo feature matrix and profile tiers:
     - **Debug Build Profile (`debug-telemetry`)**: Frequent (5s) telemetry bursts, verbose unencrypted diagnostic telemetry (queue depth breakdown, drop reasons, RF RSSI/LQI, task timing) for effortless lab troubleshooting. Payload data and identity keys remain strictly encrypted with `Secret<T>` redaction.
     - **Production Build Profile (`prod`)**: Rate-limited (60s) telemetry bursts, minimal scalar metrics only, stealth radio discipline, and zero metadata leakage.
  3. Firmware non-blocking telemetry emitter integrated into the 200ms TDM cycle during the `RadioSlot::MeshActive` window.
  4. Central host companion fleet sink (`gibberish-sink` / `gibberishd --sink`) monitoring serial streams from the local dongle, decoding peer RF telemetry, writing timestamped logs to `/tmp/gibberish/fleet.log`, and rendering a live terminal fleet dashboard.
- **Product Authority:** In-band 2.4 GHz RF mesh telemetry is canonical. External Wi-Fi, LAN, and cloud dependencies are explicitly out of scope per user direction.
- **Open Blockers:** None.

---

## Product Contract

### Summary

Project **Gibberish** operates as a zero-trust, off-grid swarm. When testing multiple dongles deployed across laptops (macOS, Windows, Linux) and mobile phones (Android USB-OTG), engineers need centralized visibility into fleet health without violating the core off-grid, air-gapped identity of the system.

This feature enables **In-Band RF Telemetry**: every dongle periodically transmits an authenticated telemetry packet over 802.15.4 Channel 15. The development workstation’s local dongle acts as an RF receiver, capturing all peer telemetry out of the air and streaming it to a centralized live fleet monitor. Two distinct build tiers give engineers maximum diagnostic verbosity during development while preserving zero-trust cryptographic stealth in production.

### Problem Frame

Debugging decentralized mesh nodes in the field is difficult when nodes lack displays or when operators cannot easily tail serial ports on locked-down mobile devices or remote laptops. Introducing an IP/Wi-Fi logging bridge would compromise air-gapped security, add networking failure modes, and defeat the purpose of an autonomous RF mesh. 

Furthermore, a single static logging configuration forces a bad compromise: verbose debugging exposes too much device metadata over the air for production use, while production stealth makes lab troubleshooting blind.

### Key Decisions

- **Pure Off-Grid RF Telemetry Over 802.15.4** (session-settled: user-directed — chosen over Wi-Fi/LAN IP socket sink: telemetry transmits entirely over the airwaves on Channel 15; zero Wi-Fi, router, or internet access required). Governs R1, R2, R5.
- **TTL=1 Non-Relaying Broadcast & User Preemption** (session-settled: architectural review with Claude Senior Engineer — chosen over multi-hop flood relay: telemetry frames use TTL=1 and intermediate nodes never re-broadcast them, eliminating O(N²) airtime congestion. User clipboard and chat packets strictly preempt telemetry in the outbound queue). Governs R2, R11.
- **RNG Transmission Jitter** (session-settled: architectural review with Claude Senior Engineer — chosen over fixed interval: applies ±500ms pseudorandom jitter via hardware RNG to eliminate synchronous RF beacon collisions across multiple nodes). Governs R4, R12.
- **Tiered Build Profiles (`debug` vs `prod`)** (session-settled: user-directed — chosen over single runtime toggle: compile-time cargo feature flags eliminate dead debug code in production while giving debug builds rich unencrypted diagnostic visibility). Governs R3, R4, R6.
- **Guaranteed Payload Cryptographic Separation** (session-settled: user-directed — even in debug builds where node and device metadata are unencrypted for troubleshooting, all message payloads, session keys, and clipboard contents remain strictly encrypted with ChaCha20-Poly1305 and wrapped in `Secret<T>`). Governs R7, R8.
- **Dedicated Central Fleet Sink Monitor (`gibberish-sink`)** (session-settled: architectural review — implemented as a standalone binary in `apps/gibberish-daemon` that multiplexes local dongles and formats peer telemetry into a live ASCII/ANSI fleet table and `/tmp/gibberish/fleet.log`). Governs R9, R10.

### Requirements

#### Wire Protocol & Frame Layout (`gibberish-protocol`)
- **R1.** The protocol shall define `FLAG_TELEMETRY = 0x0020` in `MeshHeader.flags` to differentiate telemetry broadcasts from standard mesh packets.
- **R2.** Telemetry wire frames shall fit within the standard 114-byte mesh wire payload (18B header + 96B payload) and carry `ttl = 1`. Intermediate mesh nodes shall consume telemetry locally and shall NOT re-broadcast it across the mesh.
- **R11.** User message and clipboard packets shall strictly preempt telemetry frames in the CSMA/CA outbound backoff queue; telemetry emission is best-effort and must yield to user traffic.
- **R12.** Telemetry emission timers shall apply a ±500ms pseudorandom jitter derived from hardware RNG (`esp_hal::rng::Rng`) to prevent synchronous phase-locked RF beacon collisions across multiple nodes.

#### Build Profiles & Feature Matrix (`gibberish-firmware`)
- **R3.** The firmware shall support two mutually exclusive Cargo feature profiles: `debug-telemetry` (default for development) and `prod`.
- **R4.** In `debug-telemetry` mode:
  - Telemetry broadcast interval shall be **5 seconds**.
  - Telemetry payload shall include: Uptime, RX packet count, TX packet count, Drop count, SRAM ring usage, MicroSD storage status, last diagnostic event code, last peer RSSI, last peer LQI, free heap, and node build version string.
  - Telemetry metadata shall be serialized unencrypted for rapid sniffer inspection.
- **R5.** In `prod` mode:
  - Telemetry broadcast interval shall be **60 seconds**.
  - Telemetry payload shall be strictly limited to minimal scalar fields (`ClosedTelemetry`: uptime, rx, tx, drops, sram used, storage mode, event code, rssi) and encrypted with the swarm ratchet key.
- **R6.** Switching between profiles shall be supported via top-level `justfile` recipes: `just build-firmware-debug` and `just build-firmware-prod`.

#### Cryptographic Boundaries
- **R7.** The firmware and protocol shall enforce that `debug-telemetry` never compromises user clipboard or message payloads; clipboard chunks must remain 96-byte ChaCha20-Poly1305 authenticated ciphertext blobs under all build profiles.
- **R8.** All cryptographic keys and plaintext buffers in the companion daemon shall remain wrapped in `Secret<T>` with zeroization on drop.

#### Central Fleet Sink & Dashboard (`gibberish-daemon`)
- **R9.** The companion tool shall provide a dedicated binary `gibberish-sink` (executable via `just sink` or `cargo run -p gibberish-daemon --bin gibberish-sink`) that auto-discovers connected `/dev/ttyACM*` dongles.
- **R10.** The sink shall parse incoming `FLAG_TELEMETRY` packets received over the air, record historical metrics per Node ID, append timestamped entries to `/tmp/gibberish/fleet.log`, and render an ANSI terminal dashboard displaying:
  - Node ID (Hex)
  - Build Tier (`DEBUG` / `PROD`)
  - Storage Mode (`FAT32 Active` / `RAM Only`)
  - Packet Counts (`RX`, `TX`, `Drops`)
  - RF Signal (`RSSI dBm`, `LQI`)
  - Uptime (Formatted HH:MM:SS)
  - Last Seen (Relative seconds ago with stale indicator)

### Actors

- **A1. Field Tester / Developer:** Deploys dongles across various machines (Linux, macOS, Windows, Android) and uses the dev workstation to monitor live mesh health.
- **A2. Central Sink Node:** A LilyGO T-Dongle-C5 plugged into the development machine acting as a passive RF receiver for peer telemetry frames.
- **A3. Rogue RF Sniffer:** An external eavesdropper analyzing 2.4 GHz airwaves. In `prod` mode, traffic is indistinguishable from random noise; in `debug` mode, node health metadata is visible but user content remains unreadable.

### Key Flows

- **F1: Autonomous Periodic Telemetry Beaconing**
  1. Dongle timer expires (5s in debug, 60s in prod).
  2. Main loop serializes current node health into a `FLAG_TELEMETRY` packet.
  3. Packet is scheduled with low-priority CSMA/CA backoff and broadcasted over IEEE 802.15.4 Channel 15.
- **F2: Central Sink Reception & TUI Display**
  1. Central sink dongle receives RF frame with `FLAG_TELEMETRY`.
  2. Dongle emits `[Radio RX] Telemetry from: <NodeID>` over USB CDC.
  3. Host `gibberish-sink` parses frame, updates node state table, flushes to `/tmp/gibberish/fleet.log`, and refreshes the console dashboard.
- **F3: Build Profile Selection**
  1. Developer runs `just build-firmware-debug` or `just build-firmware-prod`.
  2. Cargo selects the appropriate feature flags and compiles the release binary.

### Acceptance Examples

- **AE1 (Debug Build Telemetry):** Two dongles flashed with `debug-telemetry`. Dongle A sends a telemetry beacon every 5s. Dongle B catches the frame, logs it, and `gibberish-sink` displays Node A with `Tier: DEBUG`, detailed drop metrics, and RSSI.
- **AE2 (Production Stealth):** Dongle flashed with `prod`. It broadcasts at most once every 60s. Serial console output is muted. RF sniffer sees only high-entropy encrypted ciphertext.
- **AE3 (Zero-Trust Security Boundary):** In `debug` mode, clipboard text is broadcasted. A packet sniffer inspecting the 114-byte frame verifies that bytes 18..114 contain authenticated ChaCha20-Poly1305 ciphertext; zero plaintext characters are transmitted.

---

## Planning Contract

### High-Level Technical Design

```
+-------------------------------------------------------------------------+
|                              RF AIRWAVES                                |
|                     IEEE 802.15.4 Channel 15 (2.425 GHz)                 |
+------------------------------------+------------------------------------+
                                     ^
                                     | 2.4 GHz RF Packets
                                     | (FLAG_TELEMETRY = 0x0020)
       +-----------------------------+-----------------------------+
       |                                                           |
+------+-----------------------------+   +-------------------------+------+
|     REMOTE PEER NODE (e.g. Android)|   | CENTRAL SINK NODE (Dev Laptop) |
|   LilyGO T-Dongle-C5 (Node BEBD82B4|   | LilyGO T-Dongle-C5 (Node BEBCE)|
|                                    |   |                                |
| +--------------------------------+ |   | +----------------------------+ |
| | Telemetry Emitter Task         | |   | | Radio RX (MeshActive Slot) | |
| |  - Debug: 5s interval (Plain)  | |   | |  - Captures Peer Telemetry | |
| |  - Prod: 60s interval (Cipher) | |   | +--------------+-------------+ |
| +----------------+---------------+ |                    |               |
|                  |                 |                    | USB CDC       |
|                  v                 |                    v               |
| +--------------------------------+ |   +----------------+-------------+ |
| | 802.15.4 Radio Transceiver     | |   | gibberish-sink daemon        | |
| |  - Channel 15, +20 dBm         | |   |  - ANSI Live Fleet Matrix    | |
| +--------------------------------+ |   |  - /tmp/gibberish/fleet.log  | |
+------------------------------------+   +------------------------------+ |
                                         +--------------------------------+
```

#### Wire Frame Schemas

1. **Standard Telemetry Header (`MeshHeader`)**:
   - `network_tag`: 64-bit Network Admission Tag (`DEFAULT_NETWORK_TAG` or `SWARM_NETWORK_TAG`)
   - `msg_id`: Monotonic telemetry sequence
   - `flags`: `FLAG_TELEMETRY` (`0x0020`)
   - `ttl`: 1 (direct broadcast) or 3 (mesh routed)

2. **Debug Telemetry Payload (Unencrypted 96B)**:
   - `[0..4]`: Uptime seconds (u32)
   - `[4..8]`: RX count (u32)
   - `[8..12]`: TX count (u32)
   - `[12..16]`: Drop count (u32)
   - `[16..18]`: SRAM used (u16)
   - `[18..19]`: Storage mode (u8: 0=RAM, 1=SD)
   - `[19..20]`: Last event code (u8)
   - `[20..21]`: Build tier (u8: 0xDB for Debug, 0xPR for Prod)
   - `[21..22]`: Free heap KB (u8)
   - `[22..23]`: Last RSSI (i8)
   - `[23..24]`: Last LQI (u8)
   - `[24..32]`: Node MAC tail (8 bytes)
   - `[32..96]`: Zero-padded diagnostic reserve

3. **Production Telemetry Payload (Encrypted 96B)**:
   - 80-byte `ClosedTelemetry` struct encrypted with ChaCha20-Poly1305 + 16-byte Poly1305 authentication tag.

### Assumptions

1. Both physical LilyGO T-Dongle-C5 devices remain plugged into `/dev/ttyACM0` and `/dev/ttyACM1` during development.
2. The central sink daemon runs on Linux with ANSI terminal color support.
3. Node clocks are local monotonic; timestamps on the central sink are assigned using the dev machine's system clock upon RF packet reception.

### Sequencing

- **Phase 1 (Protocol & Features):** U1 $\rightarrow$ U2
- **Phase 2 (Firmware Emission):** U3
- **Phase 3 (Host Sink & Live Dashboard):** U4 $\rightarrow$ U5

---

## Implementation Units

### U1. Protocol Telemetry Wire Framing
- **Target:** `crates/gibberish-protocol`
- **Files:**
  - `crates/gibberish-protocol/src/frame.rs`
  - `crates/gibberish-protocol/src/lib.rs`
- **Description:** Define `FLAG_TELEMETRY = 0x0020`. Create `DebugTelemetryPayload` (unencrypted diagnostic layout) and `TelemetryTier` enum (`Debug = 0xDB`, `Prod = 0xPR`). Implement serialization and deserialization into 96-byte payload arrays with unit tests.
- **Test Scenarios:**
  - *Roundtrip:* Serialize `DebugTelemetryPayload` into 96 bytes and deserialize; verify field fidelity.
  - *Flag Mask:* Verify `FLAG_TELEMETRY` does not collide with `FLAG_DIRECT`, `FLAG_GROUP`, `FLAG_CLIPBOARD`, `FLAG_SNEAKERNET`, or `FLAG_ACK_REQ`.

### U2. Cargo Feature Flags & Build Profiles
- **Target:** `apps/gibberish-firmware`, `gibberish/justfile`
- **Files:**
  - `apps/gibberish-firmware/Cargo.toml`
  - `justfile`
- **Description:** Define Cargo features in `gibberish-firmware`: `features = { default = ["debug-telemetry"], "debug-telemetry" = [], "prod" = [] }`. Update root `justfile` with targets:
  - `just build-debug`
  - `just build-prod`
  - `just flash-debug PORT=/dev/ttyACM0`
  - `just flash-prod PORT=/dev/ttyACM0`
- **Test Scenarios:**
  - *Compilation Check:* Build with `--no-default-features --features prod` and `--features debug-telemetry`; verify both build cleanly with zero warnings.

### U3. Firmware Telemetry Emitter & Rate Arbiter
- **Target:** `apps/gibberish-firmware`
- **Files:**
  - `apps/gibberish-firmware/src/main.rs`
- **Description:** Implement periodic telemetry emission in the main loop. In `debug-telemetry` profile: emit `DebugTelemetryPayload` every 5 seconds (with ±500ms hardware RNG jitter). In `prod` profile: emit compact `ClosedTelemetry` every 60 seconds (with ±5s jitter).
  - Outbound priority: User chat/clipboard packets strictly preempt telemetry. If `backoff` has a pending transmission, telemetry is skipped until the channel is idle.
  - Inbound handling: When receiving a `FLAG_TELEMETRY` frame:
    - Output structured CDC string: `[Telemetry RX] Node: {:08X}, Tier: {:?}, Uptime: {}s, SRAM: {}/256, Drops: {}, RSSI: {} dBm, LQI: {}`.
    - Flash LED momentarily in magenta to signify telemetry packet reception.
    - Consume locally: Do NOT re-broadcast across the mesh (TTL=1) and do NOT push to `sram_ring` or MicroSD to prevent storage pollution.
- **Test Scenarios:**
  - *Emitter Timing & Jitter:* Verify timer fires around 5s with varying offsets across successive cycles.
  - *Traffic Preemption:* Queue user clipboard chunk while telemetry timer expires; verify clipboard chunk transmits first and telemetry yields.
  - *Non-Relay Discipline:* Inject `FLAG_TELEMETRY` packet into Node A; verify Node A outputs CDC log but does NOT schedule an RF transmission.

### U4. Central Companion Fleet Sink & Live TUI Dashboard
- **Target:** `apps/gibberish-daemon`
- **Files:**
  - `apps/gibberish-daemon/src/bin/gibberish_sink.rs`
  - `apps/gibberish-daemon/src/fleet.rs`
- **Description:** Implement `gibberish-sink` binary. Automatically connects to available local dongles (`/dev/ttyACM*`). Listens for `[Radio RX]` / `[Telemetry RX]` lines, parses telemetry payloads, maintains an in-memory map of known nodes, writes to `/tmp/gibberish/fleet.log`, and renders an ANSI dashboard updated every second.
- **Dashboard Layout:**
  ```text
  ================================================================================
   PROJECT GIBBERISH - 802.15.4 OFF-GRID FLEET SINK
   Listening on: /dev/ttyACM0, /dev/ttyACM1 | Logging to: /tmp/gibberish/fleet.log
  ================================================================================
  NODE ID   TIER   STORAGE     UPTIME     RX    TX   DROPS  RSSI   LQI  LAST SEEN
  --------------------------------------------------------------------------------
  BEBCE5B8  DEBUG  SD ACTIVE   00:14:22   904   68       0   -19   255    1s ago
  BEBD82B4  DEBUG  RAM ONLY    00:13:58   302   33      46   -19   255    2s ago
  ================================================================================
  ```
- **Test Scenarios:**
  - *Parser Robustness:* Ingest corrupted/truncated telemetry line; verify sink drops line gracefully without crashing.
  - *Stale Node Detection:* Mark nodes unseen for >30s with yellow warning indicator.

### U5. End-to-End Multi-Node Verification Suite
- **Target:** `apps/gibberish-daemon`
- **Files:**
  - `apps/gibberish-daemon/src/bin/fleet_sink_test.rs`
- **Description:** Automated test running against physical hardware:
  1. Flashes Dongle A and Dongle B with `debug-telemetry`.
  2. Runs sink listener on Dongle A.
  3. Triggers telemetry emission from Dongle B over 802.15.4.
  4. Asserts that Dongle A receives Dongle B's telemetry over the air, correctly parses Node ID `BEBD82B4`, and records RSSI.
- **Test Scenarios:**
  - *Live RF Validation:* Confirm physical reception of telemetry frame over Channel 15.

---

## Verification Contract

### Automated Test Suites
1. **Protocol Telemetry Test:**  
   `cargo test -p gibberish-protocol`  
   Validates serialization and flag parsing for `FLAG_TELEMETRY`.
2. **Dual-Profile Compilation Test:**  
   `cargo check --manifest-path apps/gibberish-firmware/Cargo.toml --features debug-telemetry`  
   `cargo check --manifest-path apps/gibberish-firmware/Cargo.toml --no-default-features --features prod`  
   Validates zero-warning compilation on both profiles.
3. **Live Hardware Fleet Test:**  
   `cargo run --release -p gibberish-daemon --bin fleet_sink_test`  
   Validates real 802.15.4 over-the-air telemetry reception between Dongle A and Dongle B.

### Hardware Bench Verification
1. **Live Dashboard Run:**  
   Execute `cargo run -p gibberish-daemon --bin gibberish-sink` and verify both physical dongles populate the live table with real-time RSSI and uptime.

---

## Definition of Done

- **Code Complete:** Units U1–U5 implemented with zero compiler warnings.
- **Pure Off-Grid:** Zero networking dependencies (no sockets, no HTTP/Wi-Fi); 100% of telemetry travels over IEEE 802.15.4 RF airwaves.
- **Build Tiers Functional:** `debug-telemetry` emits rich unencrypted diagnostics every 5s; `prod` emits compact rate-limited telemetry every 60s.
- **Zero-Trust Sealed:** Payload encryption and `Secret<T>` key redaction verified intact across all build tiers.
- **Hardware Grounded:** Live fleet dashboard verified running against physical dongles `/dev/ttyACM0` and `/dev/ttyACM1`.
