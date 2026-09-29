# Project Gibberish 📡🔒

> **Off-Grid Mesh Communications Prototype**  
> Running on bare-metal Rust on LilyGO T-Dongle-C5 (ESP32-C5 RISC-V) with cross-platform companion daemons for Linux and macOS.

> **Security status: not secure.** The current build uses a hardcoded development swarm key (`[0x55; 32]`, in `apps/gibberish-daemon/src/main.rs`) that is public in the source, so anyone can decrypt every message. Frames also carry a constant network tag and a MAC-derived source address in cleartext, and production firmware still broadcasts periodic beacons and telemetry. Do not rely on it for confidentiality or covert operation. See `docs/plans/2026-09-29-0837-feat-production-opsec-radio-contract-plan.md` for the planned production design.

---

## ⚡ What is Gibberish?

Gibberish turns USB dongles into an off-grid swarm. The dongles broadcast AEAD-encrypted payloads (currently under the hardcoded development key noted above) over raw **IEEE 802.15.4** radio (Channel 15, 2.425 GHz). 

- **To eavesdroppers or radio sniffers:** Payloads are AEAD ciphertext, but frame headers are cleartext: a standard 802.15.4 MAC header with fixed frame-control bytes and a MAC-derived source address, followed by a constant network tag. The traffic is identifiable as Gibberish traffic, and with the current hardcoded key the payloads are readable too.
- **To swarm peers:** The packets assemble into direct messages (encrypted with the shared swarm key, so not end-to-end between two peers) and real-time decentralized clipboard sync. Offline sneakernet file transfer is planned, not implemented (see Roadmap).

### Core Architecture Principles

1. **Zero-Trust Blind Dongle**: The hardware dongle never receives, stores, or generates private identity keys or decrypted plaintext. All cryptography happens exclusively in host memory (`Secret<T>` with `ZeroizeOnDrop`). The dongle does expose a MAC-derived node ID, and any ciphertext it holds decrypts with the public development key.
2. **Pure RF Airwaves**: Operates on raw IEEE 802.15.4 Channel 15 (2.425 GHz). Zero Wi-Fi association, zero cellular, zero cloud servers, zero internet reliance.
3. **Dual-Mode Storage**: Authoritative SRAM live ring buffer (256 packets) handles real-time mesh routing. If a MicroSD card is inserted, the firmware writes raw ciphertext sectors directly to the card, starting at LBA 10 with checkpoints at LBA 1 and 2. There is no filesystem or `CHUNKS.BIN` file, and on a formatted card this overwrites the filesystem's early sectors, so use a card whose contents you do not need. If a write fails (e.g. the card is removed), the dongle falls back to RAM-only relay mode and stays there until reboot.
4. **Cross-Platform Host Integration**: Host companion daemon (`gibberish-daemon`) provides bidirectional clipboard synchronization with native Wayland (`wl-copy`/`wl-paste`), X11, and macOS (`pbcopy`/`pbpaste`) support, plus loopback echo suppression.

---

## 🛠️ Hardware Specification: LilyGO T-Dongle-C5

| Component | Specification | Details / Pinout |
| :--- | :--- | :--- |
| **SoC** | Espressif ESP32-C5 | Single-core 32-bit RISC-V (chip maximum 240 MHz; runtime clock unverified) |
| **Radio** | 2.4 GHz IEEE 802.15.4 | Channel 15 (2.425 GHz), 250 kbps, CSMA/CA |
| **Display** | 0.96" IPS ST7735 LCD | 160x80 color SPI display (MOSI: 2, SCK: 6, CS: 10, DC: 3, RST: 1, BL: 0) |
| **LED** | APA102 DotStar RGB | Clock: GPIO 4, Data: GPIO 5 |
| **Button** | User Pushbutton | GPIO 28 (active low with internal pull-up) |
| **Storage** | MicroSD Card Slot | SPI mode shared on SPI2 (MISO: GPIO 7, CS: GPIO 23) |
| **Host Link** | USB CDC-ACM (full-speed) | Host-to-dongle binary framing `[0xAA, 0x55, 0x72, <payload>]`; dongle-to-host binary frames `[0xAA, 0x55, len, crc16, payload]` (ASCII `#PKT#` lines in `debug-telemetry` builds) |

---

## 📁 Repository Structure

```text
gibberish/
├── apps/
│   ├── gibberish-firmware/     # Bare-metal no_std RISC-V Rust firmware (esp-hal; excluded from the root workspace)
│   ├── gibberish-daemon/       # Desktop host daemon in std Rust (Linux & macOS)
│   └── gibberish-client/       # Slint native desktop chat client
├── clients/                    # Web and iOS client stubs
├── crates/
│   ├── gibberish-crypto/       # X25519, ChaCha20-Poly1305, keyed-BLAKE3 KDF, nonce ratchet
│   ├── gibberish-db/           # SQLite store for the daemon
│   ├── gibberish-protocol/     # 127-byte PHY frame serialization & Network Tag
│   └── gibberish-storage/      # SRAM ring & raw-sector SD block log
├── docs/
│   ├── ideation/               # Tracked architectural ideas & brainstorm backlog
│   ├── plans/                  # Unified planning documents & milestone history
│   └── solutions/              # Problem-solution records & root cause fixes
├── tests/
│   └── integration-sim/        # Single-process crypto/chunk/storage round-trip simulation
├── tools/                      # Helper scripts
├── run_daemon.sh
├── CONCEPTS.md                 # Formal domain concepts & security contracts
├── deny.toml                   # cargo-deny policy (bans x25519-dalek; see justfile check-deny)
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
cd apps/gibberish-firmware
cargo run --release --bin gibberish-firmware
```

Or use `just flash`. Add `--features debug-telemetry` (or `just flash-debug`) for serial debug output. Storage mode (SD or RAM-only) is detected at runtime; see the SD card warning above.

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

Both daemons automatically discover connected dongles (`/dev/ttyACM*` or `/dev/cu.usbmodem*`), and begin live clipboard synchronization across the 802.15.4 airwaves (encrypted with the public development key). The daemon reads the station node ID from dongle debug log lines, which production firmware does not print, so with production firmware pass `--node-id` explicitly.

---

## 🗺️ Roadmap & Milestones

- [x] **Milestone 1**: Project bootstrap & isolated Cargo workspace architecture.
- [x] **Milestone 2**: Core cryptographic engine (`gibberish-crypto`) with ChaCha20-Poly1305 AEAD, keyed-BLAKE3 sender subkeys, and type-level `Secret<T>`.
- [x] **Milestone 3**: Dual-mode storage engine (`gibberish-storage`) with SRAM ring buffer and power-loss recovery.
- [x] **Milestone 4**: Hardware bringup (ST7735 LCD, APA102 LED, Button GPIO 28, IEEE 802.15.4 radio).
- [x] **Milestone 5**: Closed-schema scalar telemetry sink and desktop fleet monitoring. (Superseded for production by the production OPSEC plan, which removes production telemetry; it works only with `debug-telemetry` firmware.)
- [x] **Milestone 6**: Physical cross-machine encrypted mesh clipboard synchronization (Linux ↔ M1 Mac). (Unverified here: the evidence is a write-up in `docs/solutions`, with no logs in the repo.)
- [x] **Milestone 7**: ST7735 LCD live mesh dashboard (packet counters, peers with RSSI, sync badge).
- [ ] **Milestone 8**: MicroSD sneakernet vault & multi-megabyte file chunking over mesh.
- [ ] **Milestone 9**: BLE 5.0 GATT peripheral service with slotted TDM radio coexistence.
- [ ] **Milestone 10**: Cross-platform Web/PWA client (Web Bluetooth / Web Serial) & iOS bridge. (Not done: `clients/` holds stubs. A Slint native desktop client shipped separately in `apps/gibberish-client`.)

---

## 🔒 Security Design

- **Cipher Suite**: ChaCha20-Poly1305 AEAD (IETF RFC 8439). X25519 primitives exist in `gibberish-crypto`, but no key exchange runs in the daemon's live path.
- **Per-Sender Subkeys**: Each sender derives a subkey from the swarm key and its node ID with a keyed-BLAKE3 derivation (not HKDF). The swarm key is the hardcoded `[0x55; 32]`, so any swarm member can derive every sender's key. The 16-bit `msg_id` also wraps after 65,536 messages, repeating nonces under the same key.
- **Implicit Nonces**: 96-bit nonces are computed deterministically from `(msg_id, chunk_idx, ratchet_counter)` and are never transmitted over the air.
- **Admission Tag**: Frames carry a fixed 64-bit network tag used as a filter to reject unrelated radio noise. Firmware accepts two fixed constants (the ASCII string "GIBBERIS" and a swarm tag), so the tag is a public constant, not authentication.
- **Provenance Echo Suppression**: The receiving daemon records the BLAKE3 hash of remote text it writes to the local clipboard and drops matching clipboard events for 500ms, to avoid loopback echoes.

---

## 📜 License

The workspace `Cargo.toml` declares `MIT OR Apache-2.0`. There is no `LICENSE` file in the repository yet.
