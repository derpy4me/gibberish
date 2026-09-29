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

> Correction (2026-09-29): BLE is not implemented. `esp-radio` is built with `["esp32c5", "ieee802154", "unstable"]` (no `ble` feature; `apps/gibberish-firmware/Cargo.toml:13`), no BLE controller is ever initialized, and the `RadioSlot::BleCompanion` arm in `apps/gibberish-firmware/src/main.rs:563-565` is an empty placeholder. The TDM arbiter only reserves a 32 ms-per-200 ms BLE slot in which the 802.15.4 mesh work idles. Read the BLE parts below as design intent, not shipped behavior.

## Problem

When developing bare-metal Rust (`no_std`) multi-radio applications on the ESP32-C5 (such as Project Gibberish) that plan to combine IEEE 802.15.4 mesh radio with Bluetooth Low Energy (BLE 5.0; not yet implemented) alongside cross-platform host daemons, multiple build and runtime failures arise:
1. Enabling coexistence in `esp-radio` reportedly halts compilation with a fatal error: `COEX is enabled but Wi-Fi is not` (unverified; no build log exists).
2. Including embedded `no_std` RISC-V firmware in a unified Cargo workspace with `std` desktop crates causes package boundary collisions (`current package believes it's in a workspace when it's not`) and panic profile conflicts (`panic = "abort"` required by bare-metal versus `panic = "unwind"` on host).
3. Physical interaction fails when assuming standard ESP32-C3 GPIO 9 for the BOOT button, because the LilyGO T-Dongle-C5 wires the user button to GPIO 28.
4. Attempting block storage I/O on the shared SPI2 bus panics and crashes when a MicroSD card is not physically inserted.

## Symptoms

- `cargo build --release` in the firmware crate reportedly fails (unverified; no build log exists) with:
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

- **Enabling `coex` in `esp-radio` Features**: Passing `features = ["esp32c5", "ieee802154", "coex"]` to `esp-radio` in `Cargo.toml`. In `esp-radio` 1.0.0-beta.1, the `coex` feature flag is reportedly gated on the Wi-Fi stack; with Wi-Fi omitted the compiler throws a compile-time assertion failure (unverified; not reproduced since).
- **Monolithic Multi-Target Cargo Workspace**: Defining `apps/gibberish-firmware` under the root `[workspace.members]` alongside desktop crates (`gibberish-daemon`). Because Cargo unifies dependencies across the workspace, target-specific panic strategies, `std`/`no_std` feature flags, and target-runner settings collide.
- **Polling GPIO 9 for Physical User Input**: Following the ESP32-C3 convention where BOOT is on GPIO 9 (the ESP32-S3 BOOT button is on GPIO 0, not 9). On the LilyGO T-Dongle-C5, GPIO 9 is not routed to the external push button.
- **Unconditional MicroSD Initialization**: Calling SPI SD block device initialization without card detection or falling back to a dummy driver, causing infinite bus timeouts or panics on empty sockets.

## Solution

### 1. Software Slotted TDM Schedule Without Wi-Fi

In `apps/gibberish-firmware/Cargo.toml`, omit `coex` and enable `unstable` alongside `ieee802154` (there is no `ble` feature in the current build):

```toml
[dependencies.esp-radio]
version = "1.0.0-beta.1"
features = ["esp32c5", "ieee802154", "unstable"]
```

A software Time-Division Multiplexed (TDM) schedule with 2 ms guard bands reserves a BLE slot: a 200 ms cycle of 0-168 ms 802.15.4 mesh, 168-170 ms guard, 170-198 ms BLE companion, 198-200 ms guard. Real code: `TdmArbiter` in `apps/gibberish-firmware/src/radio/coex.rs:22-70`, advanced by `advance(delta_ms)` from the main loop (which assumes a nominal 5 ms per iteration, `main.rs:225,228`):

```rust
pub fn advance(&mut self, delta_ms: u32) -> RadioSlot {
    // ...
    self.current_time_ms = (self.current_time_ms + delta_ms) % TDM_CYCLE_MS;
    // < 168 => Ieee802154Mesh, < 170 => GuardBand, < 198 => BleCompanion, else GuardBand
}
```

In the `BleCompanion` slot the main loop does nothing (`main.rs:563-565` is an empty comment), so the mesh radio simply idles for 32 of every 200 ms.

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
    "crates/gibberish-db",
    "apps/gibberish-daemon",
    "apps/gibberish-client",
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
let btn = Input::new(
    peripherals.GPIO28,
    InputConfig::default().with_pull(Pull::Up),
);
```

(`apps/gibberish-firmware/src/main.rs:111-114`.)

### 4. Dynamic MicroSD Card Detection & Ephemeral SRAM Fallback

Implement a dual-mode storage engine that checks for block device responsiveness. When no card responds, or a write errors, it switches to `StorageModeStatus::RamOnly`; live packets stay in a separate internal SRAM ring (256 slots of `Option<MeshPacket>`, roughly 30 KB; `crates/gibberish-storage/src/sram_ring.rs`). Real code: `DynamicStorageManager` in `crates/gibberish-storage/src/fat32_container.rs:170-176` is a struct, not an enum:

```rust
pub struct DynamicStorageManager<D: BlockDevice> {
    device: Option<D>,
    mode: StorageModeStatus,
    current_sequence: u64,
    current_lba: u32,
    torn_writes_recovered: u32,
}
```

`new_ram_only()` builds it with `device: None`; `new_with_device(dev)` recovers checkpoints; `flush_from_sram(&mut SramRingBuffer)` (line 268) drains the ring to the card and drops to RamOnly on a write error (line 314). The firmware picks the constructor at boot in `apps/gibberish-firmware/src/main.rs:117-126`.

## Why This Works

1. **Decoupled Radio Controllers**: The hardware 2.4 GHz RF front-end on the ESP32-C5 supports switching between 802.15.4 and BLE. The compilation failure is attributed (unverified) to `esp-radio`'s built-in coexistence arbiter assuming the Wi-Fi scheduler is active. The firmware omits `coex` and initializes only the `Ieee802154` driver; no BLE controller is initialized. The software TDM schedule merely reserves a slot for a future BLE companion link.
2. **Cargo Workspace Boundaries**: Setting `workspace.exclude` on the root workspace stops Cargo from attempting to unify profile settings across disparate compilation targets. The standalone `[workspace]` in `apps/gibberish-firmware` allows `panic = "abort"` and RISC-V optimization flags (`opt-level = "s"`, `lto = "fat"`) to apply without breaking host crates that need unwinding.
3. **Hardware Pin Routing**: The LilyGO schematic routes the physical tactile switch to GPIO 28 (which also serves as an active-low boot strap). Driving it with an internal pull-up correctly yields a logic low reading upon tactile depression.
4. **Resilient Non-Blocking Storage**: An SRAM ring buffer holds live packets independent of the SD card, and `flush_from_sram` switches the manager to RamOnly on a write error (`fat32_container.rs:314`). No latency was measured and no hot-unplug test exists.

## Prevention

- When using `esp-radio` without Wi-Fi, do not enable the `coex` feature flag (reported to fail the build; unverified); the project uses a software TDM schedule.
- Separate `no_std` embedded crates from `std` host tools in Cargo workspaces by explicitly listing embedded crates in `workspace.exclude` and marking them with `[workspace]`.
- Always verify hardware tactile button GPIO pinouts against schematics or working reference code rather than relying on family-default pin assumptions (e.g. GPIO 28 on T-Dongle-C5 vs GPIO 9 on ESP32-C3 and GPIO 0 on ESP32-S3).
- Always pair block device drivers with non-panicking initialization checks and RAM-backed fallback storage to ensure continuous headless operation.

## Related Issues

- [LilyGO T-Dongle-C5 ST7735 LCD, Active-Low Backlight, and IEEE 802.15.4 FCS Integration](file:///home/tscott/Work/esp32/gibberish/docs/solutions/integration-issues/lilygo-tdongle-c5-st7735-backlight-and-radio-fcs.md)
- [ESP32-C5 Bare-Metal Rust Setup and Factory Firmware Backup](file:///home/tscott/Work/esp32/gibberish/docs/solutions/build-errors/esp32c5-rust-baremetal-bootstrapping.md)
