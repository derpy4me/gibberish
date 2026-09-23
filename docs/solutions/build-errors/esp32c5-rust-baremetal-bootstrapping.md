---
title: ESP32-C5 Bare-Metal Rust Setup and Factory Firmware Backup
date: 2026-09-17
category: build-errors
module: firmware
problem_type: build_error
component: tooling
symptoms:
  - "Disk quota exceeded (os error 122) during cargo build"
  - "espflash error: ESP-IDF App Descriptor missing in esp-hal application"
  - "esp-println panic: Exactly one of jtag-serial, uart, auto must be enabled"
  - "USB CDC disconnected after hardware flasher RTS reset"
root_cause: config_error
resolution_type: config_change
severity: medium
tags:
  - esp32-c5
  - t-dongle-c5
  - rust
  - esp-hal
  - espflash
  - esptool
  - firmware-backup
---

# ESP32-C5 Bare-Metal Rust Setup and Factory Firmware Backup

## Problem
Bootstrapping bare-metal Rust (`esp-hal` 1.2.1) on the LilyGO T-Dongle-C5 (ESP32-C5 single-core RISC-V) fails with disk quota exhaustion, missing bootloader metadata, and serial output panics. Additionally, preserving and restoring the 16 MB factory firmware image requires specific `esptool` parameters and understanding internal USB PHY reset behavior.

## Symptoms
- Cargo fails during compilation with `error: failed to build archive at /tmp/.cargo/target/debug/deps/...: Disk quota exceeded (os error 122)`.
- `espflash flash` aborts with:
  ```text
  Error: ESP-IDF App Descriptor missing in your esp-hal application.
  You may need to add the esp_bootloader_esp_idf::esp_app_desc!() macro to your application.
  ```
- Adding `jtag-serial` to `esp-println` causes build script panic:
  ```text
  Exactly one of the following features must be enabled: jtag-serial, uart, auto, no-op.
  Currently enabled: jtag-serial, auto.
  ```
- After flashing via `esptool write-flash` with RTS hard reset, the device's internal USB PHY does not re-enumerate until a power cycle or reset button toggle.

## What Didn't Work
- Relying on the global user `~/.cargo/config.toml` which set `target-dir = "/tmp/.cargo/target"`. Because `/tmp` is a `tmpfs` RAM disk (32GB) already holding 24GB of accumulated cargo targets, compiling heavy proc-macro crates (`darling_core`, `syn`, `serde_derive`) exceeded the tmpfs quota.
- Flashing bare `esp-hal` without an app descriptor. While previous chips/toolchains allowed raw ELF flashing, `espflash 4.5.0` requires an ESP-IDF compatible app descriptor structure for ESP32-C5 applications.
- Adding `features = ["esp32c5", "jtag-serial"]` to `esp-println` without `default-features = false`. The default feature set includes `auto` (or `uart`), causing a mutual exclusion panic during `build.rs`.
- Attempting to reset the ESP32-C5 USB-Serial-JTAG via software DTR/RTS ioctl calls (`fcntl.ioctl(fd, TIOCMBIC, TIOCM_DTR_str)`), which throws `OSError: [Errno 71] Protocol error` because the internal USB PHY endpoint stalls when held in bootloader reset mode.

## Solution

### 1. Isolate Build Target Directory
In `.cargo/config.toml` within the project, explicitly set `target-dir = "target"` so builds use the local filesystem disk rather than the global tmpfs:

```toml
[target.riscv32imac-unknown-none-elf]
runner = "espflash flash --monitor"

[build]
rustflags = [
  "-C", "link-arg=-Tlinkall.x",
]
target = "riscv32imac-unknown-none-elf"
target-dir = "target"
```

### 2. Configure Dependencies and App Descriptor
In `Cargo.toml`, include `esp-bootloader-esp-idf` and disable default features on `esp-println`:

```toml
[dependencies]
esp-hal = { version = "1.2.1", features = ["esp32c5", "unstable"] }
esp-backtrace = { version = "0.20.0", features = ["esp32c5", "panic-handler", "println"] }
esp-println = { version = "0.18.0", default-features = false, features = ["esp32c5", "jtag-serial"] }
esp-bootloader-esp-idf = { version = "0.6.0", features = ["esp32c5"] }
```

In `src/main.rs`, declare the app descriptor macro and use `OutputConfig` for GPIO:

```rust
#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::main;
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    // LilyGO T-Dongle-C5 Pinout: GPIO 0 is LCD Backlight
    let mut backlight = Output::new(peripherals.GPIO0, Level::Low, OutputConfig::default());

    println!("Hello from Rust on ESP32-C5!");

    let mut counter: u32 = 0;
    loop {
        counter += 1;
        println!("Rust alive tick #{counter}");
        backlight.toggle();
        delay.delay_millis(500);
    }
}
```

### 3. Full Factory Firmware Backup and Restoration
To safely preserve and restore the stock LilyGO factory demo on the 16 MB flash:

**Backup:**
```bash
esptool --port /dev/ttyACM1 --baud 921600 read-flash 0 0x1000000 firmware_backups/t-dongle-c5-factory-16mb.bin
```

**Restore:**
```bash
esptool --port /dev/ttyACM1 --baud 921600 write-flash 0x0 firmware_backups/t-dongle-c5-factory-16mb.bin
```

After flashing or restoring over native USB CDC, power-cycle the dongle (unplug and replug) or press the reset button to re-initialize the on-chip USB PHY.

## Why This Works
1. **Target Directory**: Overriding `target-dir` locally prevents cargo from inheriting `~/.cargo/config.toml`'s global RAM disk allocation, allowing large Rust proc-macro expansions to complete on persistent storage.
2. **App Descriptor**: `espflash` validates the binary image format against the ESP-IDF standard. `esp_bootloader_esp_idf::esp_app_desc!()` places the necessary metadata in the `.rodata_desc` section so the bootloader recognizes the application entry point.
3. **USB Serial Routing**: On the LilyGO T-Dongle-C5, the USB-A connector wires directly to ESP32-C5 GPIO 13 (USB D-) and GPIO 14 (USB D+), connecting to the built-in USB-Serial-JTAG controller rather than an off-chip UART bridge. `jtag-serial` directs `println!` to this internal FIFO. Setting `default-features = false` ensures only one serial output backend is compiled into the binary.
4. **USB PHY Lifecycle**: Unlike external USB-to-UART bridge ICs (CP2102/CH340) which stay powered and connected to the host PC regardless of MCU state, the ESP32-C5 hosts the USB interface internally. Hardware reset toggles reset the USB PHY, requiring clean USB re-enumeration.

## Prevention
- **Project-Level Cargo Config**: Always include `target-dir = "target"` in embedded project `.cargo/config.toml` files to guard against global RAM disk or shared cache limits.
- **Embedded C5 Template**: Standardize on `esp-hal` 1.2+ with `esp-bootloader-esp-idf` and `esp_println` configured with `default-features = false, features = ["esp32c5", "jtag-serial"]`.
- **Factory Image Hygiene**: Always execute a full flash read (`0 0x1000000` for 16 MB chips) before overwriting new development boards, verifying the sha256 checksum immediately.
