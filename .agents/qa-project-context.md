# QA Project Context: Gibberish Mesh Communicator

## Product
- **Name:** Gibberish
- **Type:** Embedded / IoT Mesh Communicator & Desktop Client
- **Description:** Encrypted off-grid mesh communicator for ESP32-C5 LilyGO T-Dongle-C5 over IEEE 802.15.4 radio, featuring cross-platform Slint native desktop client and daemon.
- **URLs / Endpoints:**
  - Local IPC: ws://127.0.0.1:4483 (WebSocket JSON-RPC 2.0)
  - Hardware Serial Port: /dev/ttyACM0 (USB CDC-ACM at 115200 baud)
- **Key User Flows:**
  - Station Discovery & Beacon Broadcast: Node broadcasts an unencrypted static metadata beacon over airwaves; peers parse the node ID and update the contacts store. The beacon carries no alias or public key: the daemon invents an alias (`Station-XXXX`) and stores an all-zero pubkey. The production OPSEC plan removes beacons.
  - SAS 4-Word / QR Identity Verification: User selects contact, reviews derived BIP-39 4-word Short Authentication String and visual QR code, and toggles trust state to Verified. Caveat: the UI flow exists, but keys are hardcoded or placeholders (no key exchange), so the words do not prove contact authenticity.
  - Swarm Broadcast Messaging: User sends broadcast message to `#all`; daemon encapsulates with `FLAG_GROUP` and broadcasts over airwaves while persisting to local store. Encrypted with the hardcoded swarm key; multi-chunk broadcasts still produce chunk-level NACK bitmask (SACK) frames, so "no ACKs" is not strictly true.
  - Direct Messaging: User sends a 1-to-1 DM; daemon encrypts with ChaCha20-Poly1305 (`FLAG_DIRECT`) using the swarm-key sender subkey (so any swarm member can decrypt it), inserts a pending outbox row, and transmits immediately over RF. Pairwise X25519 ratchet code exists only in `apps/gibberish-client/src/chat.rs` and is called only from tests. Nothing marks the message delivered.
  - DTN Asymmetric Outbox & Beacon Flush (not wired): Library-only logic in `dtn_outbox.rs`, exercised by `apps/gibberish-daemon/tests/dtn_test.rs`; no runtime caller flushes the outbox on a recipient beacon.
  - Outbox TTL Eviction (not wired): The 48-hour TTL logic and constant exist and are tested, but nothing calls eviction at runtime, so nothing is ever evicted.
  - Daemon Crash Recovery: On startup after abnormal termination, dangling outbox items in sending state are safely reset to pending without message loss or duplicate lockup. (Store logic and test exist; at runtime nothing currently sets the `sending` state.)
  - Slint UI Event Streaming: Slint native GUI client establishes WebSocket connection to daemon, submits JSON-RPC calls, and receives push notifications (`rx_message`, `node_discovered`; `delivery_ack` has a client handler but the daemon never emits it) via coalesced Tokio async queue.

## Tech Stack
### Frontend (apps/gibberish-client)
- **Framework:** Slint 1.18 (native UI toolkit)
- **Language:** Rust 2021
- **Styling:** Monospace terminal / cyberpunk dark palette
- **Async Runtime:** Tokio 1.x (declared minimum 1.40.0, locked 1.53.1) + bounded wake-coalescing event queue

### Backend Daemon (apps/gibberish-daemon)
- **Framework:** Tokio 1.x asynchronous runtime (declared minimum 1.40.0, locked 1.53.1)
- **Language:** Rust 2021
- **IPC Protocol:** JSON-RPC 2.0 over WebSocket (tokio-tungstenite 0.24) on 127.0.0.1:4483
- **Serial Transport:** serialport 4.5.1 for USB CDC-ACM framing

### Embedded Firmware (apps/gibberish-firmware)
- **Platform:** ESP32-C5 (RISC-V `riscv32imac-unknown-none-elf`), `no_std`
- **HAL:** esp-hal 1.2.1, esp-radio 1.0.0-beta.1, esp-alloc, esp-bootloader-esp-idf, esp-backtrace, esp-println
- **Workspace:** excluded from the root Cargo workspace; build and check from `apps/gibberish-firmware`
- **Radio:** IEEE 802.15.4 2.4GHz raw transceiver framing

### Storage Layer (crates/gibberish-db)
- **Engine:** SQLite 3 via rusqlite 0.32 (bundled)
- **Concurrency Mode:** WAL mode, single-writer invariant, synchronous NORMAL, busy timeout 5000ms
- **Migration Strategy:** Versioned schema migrations table (`_schema_migrations`)

### Core Libraries
- **Cryptography (crates/gibberish-crypto):** chacha20poly1305 0.10, x25519-dalek 2.0, blake3 1.5, subtle 2.6 (declared but unused in code)
- **Wire Protocol (crates/gibberish-protocol):** postcard 1.0, serde 1.0 (`crc32fast` 1.4 belongs to `crates/gibberish-storage`)

## Test Stack
### Unit / Integration
- **Framework:** Native Rust built-in test runner (`cargo test`)
- **Test Directory:** `tests/` in each crate/app and co-located `src/` unit tests
- **Coverage Tool:** cargo-llvm-cov / cargo-tarpaulin

### Simulation Testing (tests/integration-sim)
- **Framework:** Single-process crypto/chunk/SD-storage round-trip simulation between two logical nodes (no radio or mesh simulation)
- **Config / Tests:** tests/integration-sim/src/

### Database Testing (crates/gibberish-db)
- **Framework:** Dedicated SQLite integration tests with in-memory and temporary file databases
- **Test Directory:** crates/gibberish-db/tests/

### API / IPC Testing (apps/gibberish-daemon)
- **Framework:** WebSocket JSON-RPC integration test suite with ephemeral TCP ports
- **Test Directory:** apps/gibberish-daemon/tests/

### Static Analysis & Lints
- **Tools:** `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo deny check` (neither verified; `cargo deny check` runs at the root workspace, which excludes firmware, and `deny.toml` bans `x25519-dalek`, which `gibberish-crypto` depends on, so it likely fails)

## CI/CD
- **Platform:** Local Justfile workflows only. There is no CI: no `.github/` directory or other CI config exists.
- **Gate Checks (run manually):**
  - `cargo test --workspace` (all host unit and integration tests must pass)
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings` (strict zero warnings)
  - `cd apps/gibberish-firmware && cargo check --release` (firmware cross-compile gate; the firmware is excluded from the workspace, so `cargo check -p gibberish-firmware` from the root fails)

## Environments
### Local Host Development
- **URL / Port:** ws://127.0.0.1:4483
- **Characteristics:** In-memory or local temp SQLite DB (`/tmp/gibberish/store.db`, the hard-coded path the daemon also uses in normal runs), mock RF or loopback simulation

### Hardware-in-the-Loop (HIL)
- **Hardware:** LilyGO T-Dongle-C5 plugged into host via USB CDC-ACM (`/dev/ttyACM0`)
- **Characteristics:** Physical IEEE 802.15.4 2.4GHz radio transmission, real RF propagation and RSSI/LQI readings

## Quality Goals
- **Unit Test Coverage Target:** >85% on cryptographic primitives, protocol framing, and database operations
- **Flakiness Threshold:** <0.5% (zero allowable async race conditions or deadlocks)
- **Max Test Suite Duration:**
  - Workspace Unit Tests: <30 seconds
  - Simulation & DB Tests: <45 seconds
- **Firmware Resource Budget:** Zero heap allocations during steady-state mesh packet routing; binary size fits within ESP32-C5 SRAM/Flash limits
- **Client Latency Target:** Event bridge wake-to-render latency <16ms (60 FPS)

## Risk Areas
| Area | Risk Level | Business Impact | Notes |
|------|-----------|----------------|-------|
| IEEE 802.15.4 Airwave Congestion | Critical | Message loss / unreliability | Packet storms in dense swarms require CSMA/CA and backoff jitter |
| DTN Outbox Crash Recovery | Critical | Loss of unsent messages or stuck state | Dangling sending states across process crashes must resolve to pending |
| SQLite Single-Writer Invariant | High | Database lock contention / panic | Concurrency model requires single writer; poisoned locks must recover gracefully |
| Slint UI Event Loop Starvation | High | Frozen or laggy desktop client | Rapid bursts of incoming packets must coalesce without overwhelming UI thread |
| Firmware Memory Leak / Crash | Critical | Bricked or unresponsive dongle | ESP32-C5 no_std environment has no OS supervisor; panics halt radio |

## Team
- **QA Engineers:** 0 (Solo Developer / Pair-Programming Model)
- **Total Developers:** 1
- **Dev/QA Ratio:** Solo / Zero dedicated QA (Devs own 100% of automated tests and validation)
- **Process:** Compound Engineering & iterative milestone delivery
- **QA Involvement:** Shift-left automated testing built alongside each milestone feature

## Conventions
### Test Files
- **Naming Pattern:** mostly `*_test.rs` / `*_tests.rs` for integration test files (some differ, e.g. `crypto_roundtrip.rs`), `mod tests` for unit tests inside `src/`
- **Location:** `tests/` directory at crate/app root for integration tests, inline in `src/` for unit tests

### Data & Serialization
- **Wire Serialization:** hand-written big-endian byte layouts for IEEE 802.15.4 airwave headers and payloads; `postcard` is used only for `ClosedTelemetry` on USB CDC
- **IPC Protocol:** JSON-RPC 2.0 specification over WebSocket
- **Database Schema:** Explicit SQLite types, snake_case strings for persisted status enums

### Error Handling & Safety
- **Error Types:** `thiserror` enums in `gibberish-db` and the client (`gibberish-crypto` and `gibberish-protocol` use plain enums), explicit status codes for JSON-RPC
- **Panics:** Goal is zero panics in runtime paths; some `unwrap`/`expect` calls remain in non-test source (e.g. `ratchet.rs`, `identity.rs`, `ipc.rs`, firmware SPI init)
