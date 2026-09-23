---
title: "ESP32-C5 Dual-Radio Coexistence (802.15.4 + BLE) and Multi-Target Cargo Workspace Isolation"
date: "2026-09-21"
category: "integration-issues"
module: "firmware"
problem_type: "integration_issue"
component: "radio"
symptoms:
  - "esp-radio compilation error: COEX is enabled but Wi-Fi is not"
  - "cargo build error: current package believes it's in a workspace when it's not"
  - "Profile mismatch between no_std firmware (panic = abort) and desktop host daemons (panic = unwind)"
  - "Physical BOOT button press unhandled when mapped to GPIO 9 instead of GPIO 28 on LilyGO T-Dongle-C5"
  - "Block device read/write panics when MicroSD card is absent on shared SPI2 bus"
root_cause: "config_error"
resolution_type: "code_fix"
severity: "high"
tags:
  - "esp32-c5"
  - "t-dongle-c5"
  - "esp-radio"
  - "ieee802154"
  - "ble"
  - "coexistence"
  - "cargo-workspace"
  - "no-std"
---

# ESP32-C5 Dual-Radio Coexistence (802.15.4 + BLE) and Multi-Target Cargo Workspace Isolation

## Problem

When developing bare-metal Rust (`no_std`) multi-radio applications on the ESP32-C5 (such as Project Gibberish) that combine IEEE 802.15.4 mesh radio with Bluetooth Low Energy (BLE 5.0) alongside cross-platform host daemons, multiple build and runtime failures arise:
1. Enabling coexistence in `esp-radio` halts compilation with a fatal error: `COEX is enabled but Wi-Fi is not`.
2. Including embedded `no_std` RISC-V firmware in a unified Cargo workspace with `std` desktop crates causes package boundary collisions (`current package believes it's in a workspace when it's not`) and panic profile conflicts (`panic = "abort"` required by bare-metal versus `panic = "unwind"` on host).
3. Physical interaction fails when assuming standard ESP32-C3 GPIO 9 for the BOOT button, because the LilyGO T-Dongle-C5 wires the user button to GPIO 28.
4. Attempting block storage I/O on the shared SPI2 bus panics and crashes when a MicroSD card is not physically inserted.

## Symptoms

- `cargo build --release` in the firmware crate fails with:
  ```text
  error: COEX is enabled but Wi-Fi is not
  ```
- Cargo commands fail with workspace membership confusion:
  ```text
  error: current package believes it's in a workspace when it's not: workspace: /path/to/gibberish/Cargo.toml
  ```
- Multi-target builds error on profile settings: `panic = "abort"` cannot be set on workspace dependencies when a host target requires unwinding.
- Boot button interrupts or polling loops fail to detect button presses on the LilyGO T-Dongle-C5 when listening on GPIO 9.
- Firmware panics immediately upon boot when no MicroSD card is inserted into the onboard slot.

## What Didn't Work

- **Enabling `coex` in `esp-radio` Features**: Passing `features = ["esp32c5", "ble", "ieee802154", "coex"]` to `esp-radio` in `Cargo.toml`. In `esp-radio` 1.0.0-beta.1, the `coex` feature flag is explicitly gated on the Wi-Fi stack. When Wi-Fi is omitted to conserve RAM and avoid 802.11 overhead, the compiler throws a compile-time assertion failure.
- **Monolithic Multi-Target Cargo Workspace**: Defining `apps/gibberish-firmware` under the root `[workspace.members]` alongside desktop crates (`gibberish-daemon`). Because Cargo unifies dependencies across the workspace, target-specific panic strategies, `std`/`no_std` feature flags, and target-runner settings collide.
- **Polling GPIO 9 for Physical User Input**: Following standard ESP32-C3 / ESP32-S3 schematics where BOOT is on GPIO 9. On the LilyGO T-Dongle-C5, GPIO 9 is not routed to the external push button.
- **Unconditional MicroSD Initialization**: Calling SPI SD block device initialization without card detection or falling back to a dummy driver, causing infinite bus timeouts or panics on empty sockets.

## Solution

### 1. Software Slotted TDM Coexistence Without Wi-Fi

In `apps/gibberish-firmware/Cargo.toml`, omit `coex` and enable `unstable` alongside `ieee802154` and `ble`:

```toml
[dependencies.esp-radio]
version = "1.0.0-beta.1"
features = ["esp32c5", "ble", "ieee802154", "unstable"]
```

Implement Time-Division Multiplexed (TDM) scheduling in software with a 2ms guard band between 802.15.4 RX slots and BLE advertising/GATT windows:

```rust
pub struct TdmCoexArbiter {
    current_slot: RadioSlot,
    slot_deadline_ticks: u64,
}

impl TdmCoexArbiter {
    pub fn poll_schedule(&mut self, now_ticks: u64) -> RadioAction {
        if now_ticks >= self.slot_deadline_ticks {
            self.switch_slot(now_ticks);
        }
        RadioAction::RunCurrent(self.current_slot)
    }
}
```

### 2. Multi-Target Workspace Isolation

Exclude the RISC-V bare-metal firmware crate from the root `Cargo.toml` and declare a standalone `[workspace]` in the firmware crate:

In root `Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = [
    "crates/gibberish-protocol",
    "crates/gibberish-crypto",
    "crates/gibberish-storage",
    "apps/gibberish-daemon",
    "tests/integration-sim",
]
exclude = [
    "apps/gibberish-firmware",
]
```

In `apps/gibberish-firmware/Cargo.toml`:
```toml
[package]
name = "gibberish-firmware"
version = "0.1.0"
edition = "2021"

[workspace]

[profile.release]
opt-level = "s"
lto = "fat"
codegen-units = 1
panic = "abort"
```

### 3. Correct Button GPIO Mapping (GPIO 28)

Configure the physical button on **GPIO 28** with an internal pull-up resistor:

```rust
let button_pin = io.pins.gpio28;
let button = Input::new(button_pin, Pull::Up);
```

### 4. Dynamic MicroSD Card Detection & Ephemeral SRAM Fallback

Implement a dual-mode storage engine that checks for block device responsiveness. When no card responds or when bus errors occur, seamlessly switch to `StorageMode::RamOnly` backed by an internal 32 KB circular buffer:

```rust
pub enum DynamicStorageManager<D: BlockDevice> {
    MicroSd(Fat32ContainerManager<D>),
    RamOnly(SramRingBuffer),
}

impl<D: BlockDevice> DynamicStorageManager<D> {
    pub fn new_ram_only() -> Self {
        Self::RamOnly(SramRingBuffer::new())
    }

    pub fn append_chunk(&mut self, chunk: &[u8; CHUNK_RECORD_LEN]) -> Result<(), StorageError> {
        match self {
            Self::MicroSd(sd) => sd.append_chunk(chunk),
            Self::RamOnly(ring) => {
                ring.push(chunk);
                Ok(())
            }
        }
    }
}
```

## Why This Works

1. **Decoupled Radio Controllers**: The hardware 2.4 GHz RF front-end on the ESP32-C5 supports switching between 802.15.4 and BLE. The compilation failure occurs solely because `esp-radio`'s built-in hardware coexistence arbiter (`esp_coex`) assumes the Wi-Fi scheduler is active. Omitting `coex` allows both `Ieee802154` and `ble::controller` drivers to be initialized independently, while a software TDM loop multiplexes channel time safely.
2. **Cargo Workspace Boundaries**: Setting `workspace.exclude` on the root workspace stops Cargo from attempting to unify profile settings across disparate compilation targets. The standalone `[workspace]` in `apps/gibberish-firmware` allows `panic = "abort"` and RISC-V optimization flags (`opt-level = "s"`, `lto = "fat"`) to apply without breaking host crates that need unwinding.
3. **Hardware Pin Routing**: The LilyGO schematic routes the physical tactile switch to GPIO 28 (which also serves as an active-low boot strap). Driving it with an internal pull-up correctly yields a logic low reading upon tactile depression.
4. **Resilient Non-Blocking Storage**: An authoritative SRAM live ring buffer maintains zero-latency packet routing regardless of physical SD card availability, hot-unplugging, or SPI bus transients.

## Prevention

- When using `esp-radio` for non-Wi-Fi radio combinations (802.15.4 + BLE), do not enable the `coex` feature flag; use explicit software TDM scheduling.
- Separate `no_std` embedded crates from `std` host tools in Cargo workspaces by explicitly listing embedded crates in `workspace.exclude` and marking them with `[workspace]`.
- Always verify hardware tactile button GPIO pinouts against schematics or working reference code rather than relying on family-default pin assumptions (e.g. GPIO 28 on T-Dongle-C5 vs GPIO 9 on ESP32-C3).
- Always pair block device drivers with non-panicking initialization checks and RAM-backed fallback storage to ensure continuous headless operation.

## Related Issues

- [LilyGO T-Dongle-C5 ST7735 LCD, Active-Low Backlight, and IEEE 802.15.4 FCS Integration](file:///home/tscott/Work/esp32/docs/solutions/integration-issues/lilygo-tdongle-c5-st7735-backlight-and-radio-fcs.md)
- [ESP32-C5 Bare-Metal Rust Setup and Factory Firmware Backup](file:///home/tscott/Work/esp32/docs/solutions/build-errors/esp32c5-rust-baremetal-bootstrapping.md)
