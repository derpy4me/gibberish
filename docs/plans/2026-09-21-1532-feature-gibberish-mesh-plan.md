---
title: Project Gibberish - Encrypted Mesh & Decentralized Sync Plan
type: feat
date: 2026-09-21
topic: gibberish-mesh
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

## Goal Capsule

- **Objective:** Build "Gibberish" — a zero-trust, off-grid encrypted mesh communications, decentralized clipboard synchronization, and sneakernet system running on LilyGO T-Dongle-C5 hardware in bare-metal Rust (`no_std`), with seamless cross-platform client support across macOS, Windows, Linux, Android, and iOS.
- **Means:** 
  1. Bare-metal Rust firmware on ESP32-C5 implementing raw IEEE 802.15.4 mesh radio with Time-Division Multiplexed (TDM) BLE 5.0 coexistence, a dual-mode storage engine (authoritative 32 KB SRAM live ring with dynamic MicroSD FAT32 pre-allocated container sink), and sliding Bloom filter deduplication.
  2. Zero-Trust Blind Dongle architecture: Dongles only route and store opaque ciphertext chunks ("gibberish"); all encryption keys (X25519 ECDH, ChaCha20-Poly1305, Sender Keys) live strictly on user host devices with type-level `Secret<T>` redaction.
  3. Structured Cargo workspace isolating RISC-V `no_std` firmware from desktop `std` daemons, accompanied by a `justfile` build matrix and `cargo-deny` compile-time security guardrails.
  4. Portable Rust host daemon (`gibberishd`) for desktop clipboard monitoring and USB-CDC communications, paired with a Web/PWA interface (Web Bluetooth / Web Serial) and a lightweight iOS Swift BLE wrapper.
- **Product Authority:** P2P direct messaging, group chat, broadcast channels, decentralized encrypted clipboard sync, dynamic MicroSD sneakernet NAS (with graceful RAM-only fallback), and closed scalar telemetry are in scope. High-bitrate video/audio streaming, centralized cloud infrastructure, and proprietary Apple USB accessories are explicitly out of scope.
- **Open Blockers:** None.

---

## Product Contract

### Summary

Project **Gibberish** transforms LilyGO T-Dongle-C5 USB dongles into an autonomous, encrypted swarm. The dongles scream unintelligible encrypted packets into the 2.4 GHz airwaves over raw IEEE 802.15.4 radio. To eavesdroppers or rogue radio sniffers, the traffic is mathematically indistinguishable from random noise. To authorized peers, the packets assemble into end-to-end encrypted direct chats, multi-user group discussions, real-time shared clipboard buffers, and multi-gigabyte sneakernet files replicated across an onboard MicroSD card.

If an SD card is missing or removed, the dongle smoothly operates in **Ephemeral RAM-Only Relay Mode**, routing live mesh traffic and clipboards from internal SRAM with zero downtime.

### Problem Frame

Modern private communication and cross-device sync tools rely heavily on active Wi-Fi infrastructure, local subnets, centralized cloud relays, or matching hardware ecosystems. In air-gapped security labs, conferences, field deployments, or corporate networks with strict client isolation, devices cannot communicate directly. Furthermore, leaving a hardware dongle plugged into a shared machine or losing it in transit poses a severe security risk if the dongle itself stores plaintext or private keys. 

Gibberish solves this by decoupling the hardware transport from cryptographic trust: the dongle is a "blind relay" that stores and forwards encrypted ciphertext chunks. Even if physically stolen or interrogated, the dongle reveals nothing.

### Key Decisions

- **Project Name "Gibberish"** (session-settled: user-directed — chosen over *WhisperMesh*, *KvetchNet*, and *Malarkey*: reflects the reality that nodes scream high-entropy ciphertext into the RF spectrum that only authorized recipients can decode). Governs R1, R2.
- **Zero-Trust Blind Dongle Architecture** (session-settled: user-directed — chosen over Hardware Enclave: all master identity keys, group keys, and plaintext stay exclusively on user host devices; the C5 dongle only processes opaque ciphertext chunks). Governs R3, R4, R11.
- **Dual-Mode Storage Architecture: Authoritative SRAM Live Ring + Asynchronous MicroSD Sink** (session-settled: user-directed & architectural review — internal 32 KB SRAM ring buffer is always authoritative for real-time routing; MicroSD is an optional, non-blocking archival sink. Missing SD cards fall back seamlessly to RAM-Only mode). Governs R7, R8, R26, R28.
- **FAT32 Pre-allocated Container Layout for Sneakernet Interoperability** (session-settled: architectural review — chosen over raw sector partitioning: cards are standard PC-readable FAT32. High-speed mesh appends write to pre-allocated `CHUNKS.BIN` to prevent FAT table corruption, while `VAULT/` holds direct PC-readable encrypted files). Governs R8, R29.
- **Type-Level Secrecy & Sanitized Panics** (session-settled: architectural review — chosen over manual developer redaction: all sensitive material wraps in `Secret<T>` with `ZeroizeOnDrop` and opaque `Debug` output; panics dump static metadata only). Governs R4, R30.
- **Closed-Schema Scalar Telemetry** (session-settled: architectural review — chosen over free-text log strings: telemetry packets use statically typed structs with integer metrics and enum event codes, making secret leakage structurally impossible). Governs R31, R32.
- **Dedicated Multi-Target Cargo Workspace** (session-settled: architectural review — chosen over monolithic workspace: isolates `gibberish-firmware` into its own RISC-V workspace to eliminate profile collisions (`panic = "abort"` vs `panic = "unwind"`), with root `justfile` and `cargo-deny` guardrails). Governs R19, R33.
- **QR Device Linking + 12-Word Paper Backup** (session-settled: user-directed — chosen over Passphrase-only: allows 10-second camera pairing between phone and desktop, with a BIP-39 12-word seed for offline disaster recovery). Governs R5, R6.
- **Visual LCD Passkey + Physical Button Authorization for BLE** (session-settled: user-directed — unknown BLE devices attempting to connect trigger a dynamic 6-digit PIN on the ST7735 LCD and require a physical button tap on verified GPIO 28). Governs R9, R10.
- **Sender Keys for Scalable Group Encryption** (session-settled: architectural review — group members distribute their Sender Key once over 1-on-1 ratchets; subsequent group broadcasts are $O(1)$ single-transmission packets). Governs R12, R13.
- **TDM Radio Coexistence (BLE + 802.15.4)** (session-settled: architectural review — 200ms scheduling cycle with 2ms guard bands: 85% 802.15.4 RX, 15% BLE advertising/sync). Governs R1, R9, R14.
- **64-bit Network Admission Tag (BLAKE3-MAC)** (session-settled: architectural review — provides a $2^{64}$ security margin against RF brute-force injection before touching RAM or MicroSD). Governs R8, R15.
- **Explicit Push Clipboard with Auto-Sync Toggle** (session-settled: user-directed — default hotkey/tray push avoids broadcasting passwords, with a user toggle for trusted sessions). Governs R17, R18.

### Requirements

#### Radio & Mesh Layer (ESP32-C5 Firmware)
- **R1.** The C5 firmware shall implement a Time-Division Multiplexed (TDM) radio arbiter operating on 2.4 GHz IEEE 802.15.4 Channel 15 and Bluetooth Low Energy 5.0 with a 2ms guard band between slots.
- **R2.** Raw 802.15.4 packets shall adhere to the 127-byte PHY MTU, providing an exact payload budget of 96 bytes for encrypted ciphertext (80 bytes usable plaintext + 16 bytes Poly1305 auth tag) after deducting MAC header (11B), FCS CRC-16 (2B), and 64-bit Mesh header (18B).
- **R14.** Mesh headers shall include an 8-byte Network Tag, 4-byte Message ID, 1-byte Chunk Index, 1-byte Total Chunks, 1-byte TTL, 1-byte Hop Count, and 2-byte Flags.
- **R15.** The C5 shall verify an 8-byte pre-shared Network Authentication Tag on all incoming 802.15.4 frames and silently discard unauthorized frames prior to RAM buffer allocation or MicroSD write.
- **R16.** The C5 shall maintain an in-memory Sliding Bloom Filter (sized for $p < 0.01$ false-positive rate over 1,024 recent packets) and fixed-capacity LRU cache to drop duplicate packets without runtime heap allocation.
- **R22.** Retransmissions shall use randomized contention backoff (15–60ms); if a node overhears a peer broadcasting the identical chunk ID during backoff, it cancels its own transmission.

#### Storage & Sneakernet Layer (Dynamic MicroSD + RAM Fallback)
- **R7.** The C5 firmware shall probe for MicroSD card presence on boot via SPI command handshake (CMD0/CMD8/ACMD41 with 300ms timeout) and track connection status via non-blocking status polling.
- **R28.** When no MicroSD card is present, or if an inserted card fails/is yanked, the firmware shall operate in **Ephemeral RAM-Only Relay Mode**, buffering the latest 256 packets (~32 KB) in internal SRAM and maintaining full live mesh routing and BLE clipboard relaying with zero crashes or stalls.
- **R8.** When a MicroSD card ($\ge 2\text{ GB}$) is detected, it shall mount as a standard FAT32 volume and manage three storage zones:
  - Zone 1: `GIBBERISH/CHUNKS.BIN` — Pre-allocated circular append container (scaled to min(25% card space, 2 GB)) for high-speed mesh sync and gossip reassembly.
  - Zone 2: `GIBBERISH/VAULT/` — Directory for large multi-megabyte encrypted sneakernet files dropped from PCs or phones.
  - Zone 3: `GIBBERISH/DIAG.LOG` — Pre-allocated circular telemetry and panic diagnostic log.
- **R23.** Chunk writes inside `CHUNKS.BIN` shall be power-loss atomic using record-level framing: `[Magic: GIBB][Seq 8B][Len 2B][Ciphertext 96B][CRC32 4B][Commit: 0xAA55]`. Torn writes on sudden USB unplugging shall be detected and discarded via CRC32.
- **R26.** The storage engine shall maintain ping-pong Checkpoint Sectors (LBA 1 and 2) recording monotonic sequence numbers and Head/Tail offsets. Boot-time recovery shall load the latest valid checkpoint and scan at most 64 KB of envelopes to locate the log head in <15ms.
- **R29.** The MicroSD writer shall run as an asynchronous background task decoupled from the radio loop via the 32 KB SRAM ring buffer. Under MicroSD flash write stalls (up to 250ms), the radio loop shall continue uninterrupted, dropping oldest archival chunks if the ring saturates.

#### Security, Telemetry & Cryptography
- **R3.** The C5 dongle shall never receive, generate, or store private identity keys or decrypted plaintext.
- **R4.** Host clients shall execute all cryptographic operations using X25519 ECDH, ChaCha20-Poly1305, and BLAKE3 / HKDF.
- **R30.** All sensitive buffers (plaintext, keys, seed words) in host code shall be wrapped in `Secret<T>` newtypes backed by `zeroize::ZeroizeOnDrop`, with `Debug` printing hardcoded to `<REDACTED>`. Panic handlers on firmware and daemon shall strip dynamic memory buffers.
- **R31.** Telemetry transmitted from the dongle over USB-CDC shall be encoded via `postcard` into a closed, statically typed struct containing only scalar integers and a `DiagnosticEventCode` enum, with zero dynamic free-text strings.
- **R9.** The C5 firmware shall operate a BLE GATT peripheral service with encrypted transport.
- **R10.** When an unbonded BLE client attempts pairing, the C5 shall display a dynamic 6-digit numeric passkey on the 0.96" ST7735 LCD, flash the APA102 LED in amber, and require a physical press of GPIO 28 to authorize bonding.
- **R11.** 1-on-1 private messaging shall use pairwise Double Ratchet sessions established via X25519.
- **R12.** Group messaging and group clipboard channels shall use Sender Keys (one ratchet per participant, broadcast to the group).
- **R5.** Client applications shall support device enrollment via dynamic QR code exchange between paired host devices.
- **R6.** Client applications shall support offline disaster recovery via standard BIP-39 12-word mnemonic phrases.

#### Clipboard Synchronization
- **R17.** The client system shall capture and inject system clipboard text and images up to 5 MB (split into 80-byte plaintext chunks).
- **R18.** Clipboard items shall be encrypted under a personal Swarm Key (shared across the user's enrolled devices) or a specific Group Key.
- **R25.** Clipboard sync shall support explicit hotkey/tray push (`Ctrl+Alt+C` or UI action) by default, with a toggle for automatic background synchronization.

#### Cross-Platform Clients
- **R19.** Desktop clients (macOS, Windows, Linux) shall run a standalone Rust daemon (`gibberishd`) that interfaces directly with `/dev/ttyACM0` (or Windows COM port) via USB CDC-ACM and monitors the OS clipboard.
- **R20.** Web & Android clients shall provide a Progressive Web App (PWA) using the Web Bluetooth and Web Serial APIs.
- **R21.** iOS devices shall connect via a lightweight native Swift wrapper bridging iOS CoreBluetooth to the web interface.
- **R33.** The project shall be organized into an isolated-workspace Cargo structure with a top-level `justfile` and `cargo-deny` rules forbidding crypto keys in the firmware tree.

### Actors

- **A1. Desktop User:** Operates a PC or laptop running `gibberishd` with a T-Dongle-C5 plugged into USB.
- **A2. Mobile User:** Operates an iPhone or Android device connected to a portable battery-powered T-Dongle-C5 over Bluetooth LE.
- **A3. Mesh Relay Node:** A standalone C5 dongle plugged into a USB wall charger acting as an autonomous router, using MicroSD if present or RAM-only if absent.
- **A4. Rogue Radio Sniffer / Attacker:** An unauthorized radio device attempting to intercept, decrypt, inject, or flood packets across the 2.4 GHz spectrum.

### Key Flows

- **F1: New Device Enrollment (Desktop to Phone)**  
  1. Desktop client displays an ephemeral encrypted pairing QR code.  
  2. Mobile app scans QR code, establishing an authenticated ECDH session.  
  3. Desktop transmits the encrypted Personal Swarm Keyring to the phone.  
  4. Phone decrypts and stores the keyring in local secure storage (iOS Keychain / Android Keystore).
- **F2: BLE Dongle Pairing & Physical Authorization**  
  1. Phone opens Gibberish app and scans for nearby C5 dongles over BLE.  
  2. Phone initiates connection; C5 generates a random 6-digit PIN, renders it on ST7735 LCD, and pulses APA102 in amber.  
  3. User enters PIN into mobile app and presses the C5's physical button (GPIO 28).  
  4. C5 confirms match, stores BLE bonding keys in non-volatile flash, and transitions LCD to steady green status.
- **F3: Encrypted Clipboard Push & Radio Replication**  
  1. User presses `Ctrl+Alt+C` on Desktop.  
  2. `gibberishd` reads system clipboard, encrypts payload using Personal Swarm Key, and fragments it into 80-byte plaintext chunks (each producing 96 bytes ciphertext with Poly1305 tag).  
  3. Chunks stream over USB CDC-ACM to local C5.  
  4. Local C5 writes chunks to the authoritative 32 KB SRAM ring, signals the background MicroSD task, and broadcasts over 802.15.4.  
  5. Intermediate C5 relay dongles verify Network Tag, record chunks to SRAM/SD, check Bloom filter, and re-broadcast with decremented TTL.  
  6. Remote C5 delivers chunks to Laptop/Phone via USB/BLE; host client reassembles, decrypts, and updates target clipboard.
- **F4: Dynamic MicroSD Hot-Pull / Fallback Transition**  
  1. A C5 is operating in full sneakernet mode with a 32 GB MicroSD card inserted.  
  2. User yanks the MicroSD card or unplugs the dongle while packets are arriving.  
  3. SD write times out after 100ms; background task declares `RAM_ONLY` mode and updates the LCD status to `SD: NONE (RAM ONLY)`.  
  4. Live radio mesh routing and BLE companion syncing continue with zero packet loss from the 32 KB SRAM ring.

### Acceptance Examples

- **AE1 (Zero-Trust Hardware Theft):** An attacker steals a C5 dongle, extracts the MicroSD card, and mounts it on a PC. Result: `CHUNKS.BIN` and `VAULT/` contain only ChaCha20-Poly1305 ciphertext blobs with random nonces; zero plaintext strings, identity keys, or message metadata can be recovered.
- **AE2 (Cardless Operation):** A user boots a C5 dongle with no MicroSD card inserted. Result: Dongle boots in 120ms, displays `SD: NONE (RAM ONLY)` on the LCD, and successfully relays clipboard broadcasts and mesh messages across 802.15.4 and BLE.
- **AE3 (Radio Traffic Deduplication):** Three C5 dongles (A, B, C) are within mutual radio range. Node A broadcasts a 5-chunk clipboard update. Both B and C receive chunk 1. Node B's jitter backoff expires first (18ms) and rebroadcasts chunk 1. Node C overhears B's rebroadcast, notes chunk 1 in its Bloom filter, and cancels its own rebroadcast. Result: Exactly one rebroadcast occurs instead of an exponential loop.
- **AE4 (Physical BLE Authorization):** A stranger in the same room attempts to connect their phone to User's C5 dongle over BLE. The C5 displays a PIN on screen and waits for a button press. The stranger cannot press the physical button. After 30 seconds, the connection times out and the C5 drops the link. Result: Zero unauthorized access to dongle transport.

### Scope Boundaries

- **In-Scope:**
  - Bare-metal `no_std` Rust firmware for LilyGO T-Dongle-C5 with dynamic MicroSD/RAM-only detection.
  - Raw IEEE 802.15.4 packet framing with TDM BLE 5.0 coexistence.
  - Multi-gigabyte FAT32 pre-allocated container storage (`CHUNKS.BIN`, `VAULT/`, `DIAG.LOG`).
  - ST7735 LCD status display (160x80) & APA102 LED animations.
  - Type-level `Secret<T>` zeroization and closed-schema binary telemetry.
  - Host daemon `gibberishd` in Rust for macOS, Linux, and Windows (with native Wayland and X11 clipboard integration).
  - Central Testing Telemetry Sink: Multi-platform test coordinator on development host aggregating telemetry, RSSI, drop rates, and packet events from remote companion nodes (Android, macOS, Windows) over WebSocket / UDP.
  - Web/PWA interface for Chrome, Edge, and Android (Web Bluetooth/Serial).
  - Minimal iOS Swift bridge shell for CoreBluetooth.
  - End-to-end encryption (X25519, ChaCha20-Poly1305, Sender Keys, BIP-39).
- **Deferred for Later:**
  - Automated Wi-Fi 6 high-speed fallback for large multi-gigabyte file transfers.
  - Voice note audio compression / voice messaging.
  - Hardware accelerated post-quantum Kyber/ML-KEM key exchange.
- **Outside This Product's Identity:**
  - Centralized cloud servers, internet bridges, or phone-number-based accounts.
  - Full-fidelity video streaming over mesh.
  - Direct Lightning/USB-C wired client support on iOS (precluded by Apple MFi requirements).

---

## Planning Contract

### Key Technical Decisions (KTDs)

- **KTD1: Slotted Time-Division Multiplexed (TDM) Radio Engine**  
  *Context:* ESP32-C5 has a single 2.4 GHz RF synthesizer shared across Wi-Fi, BLE, and 802.15.4. Running continuous 802.15.4 reception blocks BLE connections.  
  *Decision:* Implement a 200ms scheduling cycle in firmware:  
  - Slot A (0–168ms, 84%): IEEE 802.15.4 RX listening and CSMA/CA mesh transmission.  
  - Guard Band (168–170ms, 2ms): Synthesizer quiet time for PHY retuning.  
  - Slot B (170–198ms, 14%): BLE 5.0 advertising and GATT connection servicing.  
  - Guard Band (198–200ms, 2ms): Retune back to 802.15.4.  
  - When plugged into USB on desktop, BLE sleeps completely, giving 100% duty cycle to 802.15.4.  
  *Rationale:* Eliminates packet truncation during RF synthesizer lock time while keeping BLE discovery latency under 400ms.
- **KTD2: Dynamic Storage Engine (Authoritative SRAM Ring + Asynchronous FAT32 Pre-allocated SD)**  
  *Context:* C5 may boot without an SD card, or an inserted SD card may be pulled mid-stream. SD cards are $\ge 2\text{ GB}$.  
  *Decision:*  
  1. An internal 32 KB fixed-size array in SRAM acts as the authoritative live packet ring buffer (holding the last 256 packets). Live routing and BLE streaming consume from this ring directly with zero disk dependency.  
  2. If an SD card is present, it mounts as FAT32. On first boot, it pre-allocates `GIBBERISH/CHUNKS.BIN` (up to 2 GB) as a circular container. A low-priority background task drains the SRAM ring to `CHUNKS.BIN` in 512-byte sector batches.  
  3. If no SD card is detected or an I/O times out (>100ms), the system falls back to `RAM_ONLY` mode. Live routing never blocks or panics.  
  *Rationale:* Decouples slow/unreliable SD flash latency (up to 250ms stalls) from the microsecond-level radio loop and provides seamless cardless operation.
- **KTD3: Sender Keys for Group Mesh Scalability**  
  *Context:* Transmitting $O(N)$ pairwise ratchet packets for group messages over a 100-byte MTU mesh saturates available bandwidth at group sizes > 5.  
  *Decision:* Group channels use the Sender Key protocol. Each user generates a local ratchet key chain and distributes their public Sender Key to group members via 1-on-1 pairwise sessions upon joining. Subsequent group messages and group clipboard updates are encrypted once with the sender's current ratchet key and broadcast as a single $O(1)$ packet to the entire mesh.  
  *Rationale:* Shrinks group transmission radio footprint from $N \times \text{chunks}$ to $1 \times \text{chunks}$.
- **KTD4: Exact 127-byte 802.15.4 PHY Frame Allocation with Implicit Nonces**  
  *Structure:*  
  - IEEE 802.15.4 MAC Header (MHR): 11 bytes.  
  - Mesh Network Header: 18 bytes (Network Tag 8B, MsgID 4B, ChunkIdx 1B, TotalChunks 1B, TTL 1B, HopCount 1B, Flags 2B).  
  - Encrypted Ciphertext Payload: **96 bytes** (80 bytes usable plaintext + 16 bytes Poly1305 authentication tag).  
  - 96-bit Nonce: Derived deterministically from `BLAKE3_KDF(SessionKey, MsgID || ChunkIdx || RatchetCounter)[:12]`. Not transmitted on-wire.  
  - Hardware FCS CRC-16: 2 bytes (automatically calculated and appended by ESP32-C5 radio hardware).  
  *Total:* Exactly 127 bytes ($11 + 18 + 96 + 2 = 127$).
- **KTD5: Type-Level Secrecy & Closed Telemetry Pipeline**  
  *Context:* Accidental logging or crash dumps can leak copied passwords, private keys, or plaintext messages.  
  *Decision:*  
  1. Plaintext and keys wrap in `Secret<T>` backed by `zeroize::ZeroizeOnDrop`, with opaque `Debug` implementations.  
  2. Panic handlers strip dynamic memory strings, logging only static file/line indicators.  
  3. Telemetry packets use fixed binary structs with scalar integer metrics and an event enum (`DiagnosticEventCode`), with zero string fields.  
  *Rationale:* Guarantees mathematical impossibility of secret leakage into log streams.

### Technical Design

```
+-------------------------------------------------------------------------+
|                              HOST DEVICE                                |
|  (macOS / Windows / Linux / Android / iOS)                             |
|                                                                         |
|  +-------------------------------------------------------------------+  |
|  | Cryptographic Vault (Secret<T>, X25519, ChaCha20, Sender Keys)    |  |
|  +-------------------------------------------------------------------+  |
|  | App UI (Chat, Groups, Clipboard Manager, Keyring Enrollment)     |  |
|  +-------------------------------------------------------------------+  |
|                                   |                                     |
|               USB CDC-ACM (/dev/ttyACM0) or BLE 5.0 GATT                |
+-----------------------------------|-------------------------------------+
                                    |
                                    v
+-------------------------------------------------------------------------+
|                       LILYGO T-DONGLE-C5 FIRMWARE                       |
|                                                                         |
|  +-------------------------+            +----------------------------+  |
|  | USB-Serial-JTAG / BLE   |            | ST7735 LCD (4Hz Dirty-Rect)|  |
|  | Companion Transport     |            | Visual PIN & Telemetry     |  |
|  +-------------------------+            +----------------------------+  |
|               |                                       ^                 |
|               v                                       |                 |
|  +-------------------------------------------------------------------+  |
|  | Authoritative SRAM Ring Buffer (32 KB / 256 Chunks, Anti-Flood)   |  |
|  +-------------------------------------------------------------------+  |
|         |                                            |                  |
|         | (Async Drain / Non-Blocking)               v                  |
|         v                                  +-------------------------+  |
|  +---------------------------+             | TDM Radio Arbiter       |  |
|  | MicroSD FAT32 Engine      |             | 2ms Guard Bands         |  |
|  | CHUNKS.BIN (Pre-Allocated)|             | IEEE 802.15.4 (Ch 15)   |  |
|  | [Fallback: RAM-Only Mode] |             +-------------------------+  |
|  +---------------------------+                          |               |
+---------------------------------------------------------|---------------+
                                                          v
                                                [AIRWAVES: GIBBERISH]
```

### Workspace Structure & Target Matrix

```
esp32/gibberish/
├── Cargo.toml                       # Host Workspace (crates + daemon + sim)
├── justfile                         # Unified developer commands (firmware, daemon, wasm)
├── deny.toml                        # CI guardrail banning crypto keys in firmware
│
├── crates/                          # Shared Libraries (no_std by default, default-features = false)
│   ├── gibberish-protocol/          # Wire framing, 127B packet layout, 64-bit Network Tag
│   ├── gibberish-crypto/            # X25519, ChaCha20-Poly1305, Secret<T>, Sender Keys
│   └── gibberish-storage/           # BlockDevice trait, FAT32 chunk container, checkpointing
│
├── apps/
│   ├── gibberish-firmware/          # Dedicated RISC-V Workspace (no_std, panic = "abort")
│   │   ├── .cargo/config.toml       # Pinned to riscv32imac-unknown-none-elf
│   │   └── Cargo.toml               # Imports ONLY protocol & storage (Zero-Trust)
│   │
│   ├── gibberish-daemon/            # Desktop companion service (std, panic = "unwind")
│   │   └── Cargo.toml               # Imports protocol & crypto + arboard + serialport
│   │
│   └── gibberish-wasm/              # WebAssembly bridge (wasm32-unknown-unknown)
│       └── Cargo.toml               # Imports protocol & crypto + getrandom (js)
│
├── clients/
│   ├── gibberish-web/               # React / TypeScript PWA (Web Bluetooth / Serial)
│   └── gibberish-ios/               # Swift CoreBluetooth wrapper
│
└── tests/
    └── integration-sim/             # Host mock network simulator (x86_64)
```

---

## Implementation Units & Dependency Graph

```
  [U1. Core Protocol & Crypto]
              |
              +-----------------------+
              |                       |
              v                       v
     [U2. Radio & TDM]      [U3. Dual-Mode Storage Engine]
              |                       |
              +-----------+-----------+
                          |
                          v
               [U4. LCD & BLE Gate]
                          |
                          v
               [U5. Desktop Daemon]
                          |
                          v
             [U6. Web/PWA & iOS Client]
```

### U1. Core Protocol & Cryptography Engine (`gibberish-core`)
- **Dependencies:** None (foundation crate).
- **Interface Contract:** Exposes `encrypt_chunk(key, msg_id, chunk_idx, plaintext) -> CiphertextChunk`, `decrypt_chunk(...)`, `SenderKeyGroup`, `Secret<T>`, and `MnemonicKeyring`.
- **Description:** Implement platform-agnostic packet chunking (80B plaintext to 96B ciphertext), implicit 96-bit nonce derivation, framing serialization, X25519 key exchange, ChaCha20-Poly1305 encryption, and Sender Key ratcheting with `ZeroizeOnDrop` secrecy.
- **Files:**
  - `crates/gibberish-protocol/src/lib.rs`
  - `crates/gibberish-protocol/src/frame.rs`
  - `crates/gibberish-crypto/src/lib.rs`
  - `crates/gibberish-crypto/src/secrecy.rs`
  - `crates/gibberish-crypto/src/ratchet.rs`
- **Tests:**
  - `crates/gibberish-crypto/tests/crypto_roundtrip.rs`: Verify X25519 + ChaCha20-Poly1305 roundtrips with implicit nonces.
  - `crates/gibberish-protocol/tests/framing_tests.rs`: Verify 127-byte boundary checks and 64-bit Network Tag validation.

### U2. C5 Hardware Radio & TDM Coexistence Engine (`gibberish-firmware`)
- **Dependencies:** `U1`.
- **Interface Contract:** Exposes `RadioArbiter::poll()` servicing 802.15.4 and BLE GATT slots, and `RadioDriver` trait for mockability.
- **Description:** Implement raw IEEE 802.15.4 frame TX/RX on Channel 15 with Clear Channel Assessment (CCA), random jitter backoff, sliding Bloom filter deduplication, 64-bit Network Tag validation, and slotted TDM arbitration with BLE 5.0 (with 2ms guard bands).
- **Files:**
  - `apps/gibberish-firmware/src/hal/mod.rs`
  - `apps/gibberish-firmware/src/radio/coex.rs`
  - `apps/gibberish-firmware/src/radio/ieee802154.rs`
  - `apps/gibberish-firmware/src/radio/dedup.rs`
- **Tests:**
  - Host mock test verifying Bloom filter false positive rates under 1,024 random hashes without dynamic heap allocation.
  - Bench test between two C5 dongles measuring packet roundtrip time and TDM switching jitter.

### U3. Dual-Mode Storage Engine: SRAM Ring & Dynamic FAT32 MicroSD (`gibberish-firmware`)
- **Dependencies:** `U1`.
- **Interface Contract:** Exposes `StorageManager::write_chunk(chunk)`, `StorageManager::mode() -> StorageMode { RamOnly, MicroSdActive }`, and `gossip::reconcile()`.
- **Description:** Implement the authoritative 32 KB SRAM live packet ring buffer, SPI handshake card presence detection (CMD0/CMD8 with 300ms timeout), FAT32 pre-allocated container management (`CHUNKS.BIN`, `VAULT/`, `DIAG.LOG`), power-loss record framing (`0xAA55`), and graceful `RAM_ONLY` fallback on card absence/yank.
- **Files:**
  - `crates/gibberish-storage/src/lib.rs`
  - `crates/gibberish-storage/src/sram_ring.rs`
  - `crates/gibberish-storage/src/fat32_container.rs`
  - `apps/gibberish-firmware/src/storage/sd_driver.rs`
- **Tests:**
  - Host mock test simulating cardless boot: verify `RAM_ONLY` mode routes 1,000 packets smoothly.
  - Power-loss hot-unplug simulation test: inject simulated mid-write failure, verify recovery detects torn write and truncates cleanly in <15ms.

### U4. Visual LCD Security Gate & BLE Pairing Authorization (`gibberish-firmware`)
- **Dependencies:** `U2`, `U3`.
- **Interface Contract:** Exposes `BleSecurityGate::request_pairing(pin)` and hooks into GPIO 9 button ISR.
- **Description:** Implement ST7735 LCD driver (160x80) with 4 Hz dirty-region line buffering and APA102 LED animations. When an unbonded BLE client connects, display a dynamic 6-digit PIN on the LCD, pulse the LED in amber, and wait for GPIO 9 button press before bonding.
- **Files:**
  - `apps/gibberish-firmware/src/ui/display.rs`
  - `apps/gibberish-firmware/src/ui/led.rs`
  - `apps/gibberish-firmware/src/radio/ble_gate.rs`
  - `apps/gibberish-firmware/src/main.rs`
- **Tests:**
  - Connect via generic BLE scanner app (nRF Connect); verify PIN appears on screen and connection is rejected without physical button click.

### U5. Desktop Companion Daemon (`gibberishd`)
- **Dependencies:** `U1`.
- **Interface Contract:** Exposes local WebSocket JSON-RPC on `127.0.0.1:4483` for frontends.
- **Description:** Implement background Rust daemon for macOS, Windows, and Linux. Manages USB CDC-ACM serial link to C5 dongle, hooks system clipboard for explicit hotkey and auto-sync with `Secret<T>` wrapping, and provides local WebSocket IPC for frontends.
- **Files:**
  - `apps/gibberish-daemon/Cargo.toml`
  - `apps/gibberish-daemon/src/main.rs`
  - `apps/gibberish-daemon/src/clipboard.rs`
  - `apps/gibberish-daemon/src/transport/mod.rs`
  - `apps/gibberish-daemon/src/ipc.rs`
- **Tests:**
  - Clipboard hook test: copy text, verify `gibberishd` emits encrypted chunk stream with redacted logs.
  - Serial transport loopback test against C5 dongle.

### U6. Cross-Platform Web/PWA Client & iOS Bridge (`gibberish-web` & `gibberish-ios`)
- **Dependencies:** `U1` (Wasm), `U5`.
- **Interface Contract:** Web UI connecting to Web Bluetooth, Web Serial, or local daemon WebSocket.
- **Description:** Implement responsive Web/PWA interface supporting Web Bluetooth and Web Serial, QR-code device linking, chat/group messaging UI, and clipboard manager. Include minimal Swift iOS wrapper bridging CoreBluetooth.
- **Files:**
  - `clients/gibberish-web/package.json`
  - `clients/gibberish-web/src/bluetooth.ts`
  - `clients/gibberish-web/src/serial.ts`
  - `clients/gibberish-web/src/ui/App.tsx`
  - `clients/gibberish-ios/BLEBridge.swift`
- **Tests:**
  - Browser pairing test on Chrome/Android via Web Bluetooth.
  - End-to-end message transmission from Web UI through C5 mesh to Desktop daemon.

---

## Verification Contract

### Automated Test Suites
1. **Core Cryptographic Suite:**  
   `cargo test -p gibberish-core`  
   Runs all unit tests for X25519 ECDH, ChaCha20-Poly1305 AEAD, implicit nonce derivation, Sender Key ratchets, and 80-byte chunk reassembly.
2. **Storage & Fallback Mock Suite:**  
   `cargo test -p gibberish-storage`  
   Executes mock block tests: SRAM ring buffer wrap-around, cardless boot fallback, FAT32 pre-allocated container bounds, and torn-write recovery.
3. **Daemon Integration Suite:**  
   `cargo test -p gibberish-daemon`  
   Tests OS clipboard hooking, WebSocket IPC message serialization, and serial CDC mock framing with secret redaction verified.

### Hardware Bench Verification
1. **Cardless Boot & Mesh Test:**  
   Flash C5 without SD card; verify firmware boots, ST7735 LCD displays `SD: NONE (RAM ONLY)`, and dongle successfully relays live 802.15.4 mesh packets.
2. **Multi-Gigabyte SD Mount & Sneakernet Test:**  
   Insert 32 GB MicroSD card formatted FAT32. Boot C5; verify `GIBBERISH/CHUNKS.BIN` is created. Drop a 20 MB encrypted file into `GIBBERISH/VAULT/` from a PC, plug into C5, and read over USB CDC.
3. **Hot-Unplug Resilience Test:**  
   Stream 1,000 chunks to C5; abruptly yank the MicroSD card mid-stream. Verify zero radio loop freeze, instant transition to `RAM_ONLY` mode, and intact log file when inspected on PC.
4. **BLE iPhone Connection & Authorization:**  
   Pair iPhone to battery-powered C5. Confirm 6-digit PIN on ST7735, press button, and verify BLE bonding succeeds.

---

## Definition of Done

- **Code Complete:** All 6 implementation units (U1–U6) authored and compiling with zero warnings.
- **Hardware Grounded:** Firmware verified running on physical LilyGO T-Dongle-C5 hardware with LCD, LED, Button, MicroSD (and cardless fallback), USB-Serial, 802.15.4, and BLE all active.
- **Zero-Trust Verified:** MicroSD card dump and RF sniffing capture verify zero plaintext or unencrypted keys exist outside host personal devices.
- **Cross-Platform Verified:** End-to-end messaging and clipboard sync operational across Linux/macOS desktop and mobile (Android/iOS).
- **Resilience Verified:** 802.15.4 mesh handles out-of-range catchup via MicroSD gossip sync, deduplication prevents broadcast loops, and cardless operation works seamlessly.
