# Project Gibberish 📡🔒

> **Zero-Trust, Off-Grid Encrypted Mesh Communications & Sneakernet System**  
> Running on bare-metal Rust on LilyGO T-Dongle-C5 (ESP32-C5 RISC-V) with cross-platform companion daemons for Linux and macOS.

---

## ⚡ What is Gibberish?

Gibberish transforms USB dongles into an autonomous, off-grid encrypted swarm. The dongles scream high-entropy ciphertext into the 2.4 GHz airwaves over raw **IEEE 802.15.4** radio (Channel 15, 2.425 GHz). 

- **To eavesdroppers or radio sniffers:** The traffic is mathematically indistinguishable from random noise or smart-home sensor pings. No Wi-Fi beacons, no standard network headers.
- **To authorized swarm peers:** The packets assemble into end-to-end encrypted direct messages, real-time decentralized clipboard sync, and offline sneakernet files replicated across onboard MicroSD cards.

### Core Architecture Principles

1. **Zero-Trust Blind Dongle**: The hardware dongle never receives, stores, or generates private identity keys or decrypted plaintext. All cryptography happens exclusively in host memory (`Secret<T>` with `ZeroizeOnDrop`). Even if physically stolen or interrogated, the dongle reveals nothing.
2. **Pure RF Airwaves**: Operates on raw IEEE 802.15.4 Channel 15 (2.425 GHz). Zero Wi-Fi association, zero cellular, zero cloud servers, zero internet reliance.
3. **Dual-Mode Storage**: Authoritative 32 KB SRAM live ring buffer handles real-time mesh routing. If a MicroSD card is inserted, it dynamically logs and archives to a pre-allocated FAT32 container (`CHUNKS.BIN`). If removed, the dongle falls back to RAM-only relay mode with zero interruption.
4. **Cross-Platform Host Integration**: Host companion daemon (`gibberishd`) provides bidirectional clipboard synchronization with native Wayland (`wl-copy`/`wl-paste`), X11, and macOS (`pbcopy`/`pbpaste`) support, plus loopback echo suppression.

---

## 🛠️ Hardware Specification: LilyGO T-Dongle-C5

| Component | Specification | Details / Pinout |
| :--- | :--- | :--- |
| **SoC** | Espressif ESP32-C5 | Single-core 32-bit RISC-V @ 240 MHz |
| **Radio** | 2.4 GHz IEEE 802.15.4 | Channel 15 (2.425 GHz), 250 kbps, CSMA/CA |
| **Display** | 0.96" IPS ST7735 LCD | 160x80 color SPI display (MOSI: 2, SCK: 6, CS: 10, DC: 3, RST: 1, BL: 0) |
| **LED** | APA102 DotStar RGB | Clock: GPIO 4, Data: GPIO 5 |
| **Button** | User Pushbutton | GPIO 28 (active low with internal pull-up) |
| **Storage** | MicroSD Card Slot | SPI mode shared on SPI2 (MISO: GPIO 7, CS: GPIO 23) |
| **Host Link** | USB 2.0 CDC-ACM | High-speed binary framing [0xAA, 0x55, 0x72, <payload>] |

---

## 📁 Repository Structure

```text
gibberish/
├── apps/
│   ├── gibberish-firmware/     # Bare-metal no_std RISC-V Rust firmware (esp-hal)
│   └── gibberish-daemon/       # Desktop host daemon in std Rust (Linux & macOS)
├── crates/
│   ├── gibberish-crypto/       # X25519, ChaCha20-Poly1305, HKDF, Nonce ratchet
│   ├── gibberish-protocol/     # 127-byte PHY frame serialization & Network Tag
│   └── gibberish-storage/      # 32KB SRAM ring & pre-allocated FAT32 container
├── docs/
│   ├── plans/                  # Unified planning documents & milestone history
│   └── solutions/              # Problem-solution records & root cause fixes
├── tests/
│   └── integration-sim/        # Headless multi-node mesh simulator
├── CONCEPTS.md                 # Formal domain concepts & security contracts
├── deny.toml                   # Compile-time guardrails banning plaintext keys in firmware
└── justfile                    # Unified build & flash recipes
```

---

## 🚀 Quickstart

### Prerequisites

- **Rust toolchain**: `rustup` with native host target
- **Embedded target**: `rustup target add riscv32imac-unknown-none-elf`
- **Flashing utility**: `cargo install espflash`

---

### 1. Flash the Hardware Dongle

Connect the LilyGO T-Dongle-C5 to your computer via USB:

```bash
# Flash the firmware in production RAM-only mode
cd apps/gibberish-firmware
cargo run --release --bin gibberish-firmware --no-default-features --features prod_ram

# Or if a MicroSD card is inserted:
cargo run --release --bin gibberish-firmware --no-default-features --features prod_sd
```

---

### 2. Run the Desktop Companion Daemon

#### On Linux (Wayland / Hyprland / X11):
```bash
cargo run --release -p gibberish-daemon -- --auto-sync
```

#### On macOS (Apple Silicon M1/M2/M3 or Intel):
```bash
cargo run --release -p gibberish-daemon -- --auto-sync
```

Both daemons automatically discover connected dongles (`/dev/ttyACM*` or `/dev/cu.usbmodem*`), query the hardware station node ID, and begin live encrypted clipboard synchronization across the 802.15.4 airwaves.

---

## 🗺️ Roadmap & Milestones

- [x] **Milestone 1**: Project bootstrap & isolated Cargo workspace architecture.
- [x] **Milestone 2**: Core cryptographic engine (`gibberish-crypto`) with ChaCha20-Poly1305 AEAD, HKDF subkeys, and type-level `Secret<T>`.
- [x] **Milestone 3**: Dual-mode storage engine (`gibberish-storage`) with SRAM ring buffer and power-loss recovery.
- [x] **Milestone 4**: Hardware bringup (ST7735 LCD, APA102 LED, Button GPIO 28, IEEE 802.15.4 radio).
- [x] **Milestone 5**: Closed-schema scalar telemetry sink and desktop fleet monitoring.
- [x] **Milestone 6**: Physical cross-machine encrypted mesh clipboard synchronization (Linux ↔ M1 Mac).
- [ ] **Milestone 7**: ST7735 LCD live mesh dashboard (animated packet throughput, peer count, RSSI indicators).
- [ ] **Milestone 8**: MicroSD sneakernet vault & multi-megabyte file chunking over mesh.
- [ ] **Milestone 9**: BLE 5.0 GATT peripheral service with slotted TDM radio coexistence.
- [ ] **Milestone 10**: Cross-platform Web/PWA client (Web Bluetooth / Web Serial) & iOS bridge.

---

## 🔒 Security Design

- **Cipher Suite**: ChaCha20-Poly1305 AEAD (IETF RFC 8439) with X25519 Diffie-Hellman key exchange.
- **Per-Sender Subkeys**: Each sender derives a unique subkey via `HKDF(swarm_key, local_node_id)`, preventing keystream collisions.
- **Implicit Nonces**: 96-bit nonces are computed deterministically from `(msg_id, chunk_idx, ratchet_counter)` and are never transmitted over the air.
- **Admission Tag**: Frames carry a 64-bit cryptographic Network Admission Tag (`BLAKE3-MAC(master_key, "GIBBERISH-NET")`) to reject unauthorized radio noise before memory allocation.
- **Provenance Echo Suppression**: Senders record BLAKE3 hashes of local transmissions with a 500ms suppression window to eliminate loopback clipboard echoes.

---

## 📜 License

Licensed under the MIT License. See [LICENSE](LICENSE) for details.
