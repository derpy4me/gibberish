---
title: "LilyGO T-Dongle-C5 ST7735 LCD, Active-Low Backlight, and IEEE 802.15.4 FCS Integration"
date: "2026-09-17"
category: "integration-issues"
module: "firmware"
problem_type: "integration_issue"
component: "display"
severity: "medium"
symptoms:
  - "ST7735 LCD display remains pitch black with no backlight illumination"
  - "ST7735 pixel writes reportedly misbehave if CS is pulsed high after 0x2C command before pixel streaming (unverified mechanism)"
  - "Onboard microSD slot causes SPI bus contention on MOSI and MISO when SD_CS is floating"
  - "IEEE 802.15.4 packet trailing bytes corrupted by hardware PHY FCS CRC overwrite"
root_cause: "hardware_quirk"
resolution_type: "code_fix"
tags:
  - "esp32-c5"
  - "t-dongle-c5"
  - "st7735"
  - "lcd"
  - "backlight"
  - "active-low"
  - "spi"
  - "ieee802154"
  - "bare-metal"
  - "rust"
  - "esp-hal"
---

# LilyGO T-Dongle-C5 ST7735 LCD, Active-Low Backlight, and IEEE 802.15.4 FCS Integration

## Problem
When developing bare-metal Rust firmware (`esp-hal` 1.2.1) on the LilyGO T-Dongle-C5 (ESP32-C5 single-core RISC-V), the onboard 0.96-inch ST7735 IPS LCD remains completely pitch black despite standard driver initialization. Additionally, SPI pixel streams reportedly experienced display corruption when Chip Select was framed per-byte (unverified), unasserted SD card pins cause SPI bus contention, and 802.15.4 broadcasts silently truncate trailing payload fields due to hardware FCS CRC overwrites.

## Symptoms
- The ST7735 LCD backlight is unlit and completely dark, even after driving `LCD_BL` (GPIO 0) high.
- The display reportedly failed to render pixel rectangles or characters, or rendered erratic noise/shifts, when Chip Select (GPIO 10) was toggled high between the `0x2C` (`RAMWR`) command and subsequent pixel data chunks (unverified; no capture or log of the failing version exists, and the mechanism is not established).
- Read/write operations on SPI2 (MOSI GPIO 2, MISO GPIO 7, SCK GPIO 6) suffer intermittent bus contention or floating signal levels because the co-located microSD card socket CS pin (GPIO 23) is uninitialized.
- In the sibling `c5-light-sync` project (`/home/tscott/Work/esp32/c5-light-sync`), receiving nodes reportedly saw a 16-byte `SyncPacket` with bytes 14 and 15 (RGB blue channel and pattern ID) replaced with unexpected bytes (unverified; c5-light-sync history, no capture exists).

## What Didn't Work
- **Driving `LCD_BL` High**: Following common active-high backlight conventions by calling `lcd_bl.set_high()`. On the T-Dongle-C5 the backlight is active-low; driving GPIO 0 high turns the backlight completely off (`apps/gibberish-firmware/src/ui/display.rs:54-59,78`).
- **Per-Transaction CS Toggling on SPI**: Treating `0x2C` (`RAMWR`) as a standalone command transaction (CS low -> write 0x2C -> CS high) followed by data streaming (CS low -> write pixel buffer -> CS high). This was reported to corrupt the display. The mechanism is unproven: the driver itself raises CS between a command and its parameters for CASET/RASET (`write_cmd`/`write_data`, `apps/gibberish-firmware/src/ui/display.rs:62-74`) and window addressing works.
- **Transmitting Exact Payload Length on 802.15.4 PHY**: Passing exact 16-byte buffers to `radio.transmit_raw(&buf[..16], true)` (c5-light-sync). The ESP32-C5 IEEE 802.15.4 PHY hardware is reported to write its 16-bit CRC over the last two bytes of the transmitted PSDU rather than extending the packet length (unverified against a capture; the fix below is what the code does).

## Solution

### 1. Invert Backlight Polarity (Active-Low GPIO 0)
Configure `GPIO 0` as a push-pull output and drive it `Level::Low` to turn ON the backlight. Real code: [`apps/gibberish-firmware/src/ui/display.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-firmware/src/ui/display.rs) (`set_backlight` at lines 54-60, `init` at lines 77-138); GPIO0 is created at `apps/gibberish-firmware/src/main.rs:88` with `Level::Low`. The same pattern first appeared in the sibling repo `c5-light-sync` (`/home/tscott/Work/esp32/c5-light-sync/src/display.rs`, `set_backlight` at lines 46-53).

```rust
pub fn set_backlight(&mut self, on: bool) {
    if on {
        self.backlight.set_low(); // Active-low on GPIO 0
    } else {
        self.backlight.set_high();
    }
}

pub fn init(&mut self, delay: &mut Delay) {
    self.backlight.set_high(); // OFF during init
    // ... reset and ST7735 init command table ...
    self.write_cmd(0x29); // DISPON
    delay.delay_millis(100);

    self.fill_rect(0, 0, LCD_WIDTH, LCD_HEIGHT, 0x0000);
    // ... cache resets ...
    self.backlight.set_low(); // Backlight ON
}
```

### 2. Keep CS Asserted Across RAMWR and Pixel Streams
Keep Chip Select (GPIO 10) asserted low from the `0x2C` command through the end of the pixel buffer transmission. Real code: `fill_rect` in `apps/gibberish-firmware/src/ui/display.rs:164-198`. (Whether a CS edge mid-RAMWR is actually harmful is not established; see "Why This Works".)

```rust
self.cs.set_low();
self.dc.set_low();
{
    let mut spi = self.spi.borrow_mut();
    let _ = spi.write(&[0x2C]);
    self.dc.set_high();

    let mut remaining = count;
    while remaining > 0 {
        let chunk = remaining.min(32);
        let _ = spi.write(&buf[..(chunk * 2)]);
        remaining -= chunk;
    }
}
self.cs.set_high();
```

### 3. Isolate Shared SPI Bus via SD_CS High
In `apps/gibberish-firmware/src/main.rs:89` (the c5-light-sync equivalent is `/home/tscott/Work/esp32/c5-light-sync/src/main.rs:65`), drive `GPIO 23` (`SD_CS`) high to deselect the microSD socket sharing MOSI (GPIO 2) and MISO (GPIO 7):

```rust
let sd_cs = Output::new(peripherals.GPIO23, Level::High, OutputConfig::default());
```

### 4. Pad 802.15.4 Transmit Buffer with 2 Dummy FCS Bytes
In the gibberish firmware, `assemble_variable_phy_frame` in `crates/gibberish-protocol/src/frame.rs:762-769` counts `FCS_LEN` (2 bytes) in `total_len` and writes two zero bytes at the end, so the hardware PHY overwrites those bytes with the CRC-16 rather than payload bytes:

```rust
let total_len = MHR_LEN + MESH_HEADER_LEN + payload.len() + FCS_LEN;
// ...
out[total_len - FCS_LEN..total_len].copy_from_slice(&[0x00, 0x00]); // Dummy FCS for PHY driver
```

The original c5-light-sync form of the fix (`/home/tscott/Work/esp32/c5-light-sync/src/radio.rs:71-76`, `broadcast`) serialized its 16-byte `SyncPacket` into an 18-byte buffer and passed `&buf[..len + 2]` to `transmit_raw`; that is c5-light-sync history, not gibberish code.

## Why This Works
- **Active-Low Backlight**: On this board, driving GPIO 0 low turns the backlight on and driving it high turns it off (observed in the firmware, `ui/display.rs:54-59`). The underlying circuit (cathode drive vs. P-channel MOSFET gate) is unverified; no schematic is in the repo.
- **CS Framing**: Keeping CS low across `0x2C` and the pixel stream is retained as a precaution. The earlier explanation that any CS edge resets the ST7735 command decoder is contradicted by this project's driver, which raises CS between every command and its parameters (including CASET/RASET in `set_window`, `ui/display.rs:141-161`) with working window addressing. The actual failure mechanism was not established (unverified).
- **Shared Bus Contention**: When the microSD card socket is left with SD_CS floating, the card may respond to traffic on the shared bus (unverified; no scope capture). Holding `SD_CS` high keeps a card's MISO line in high-impedance (Hi-Z), which is standard SD-SPI behaviour.
- **802.15.4 Baseband Transmitter Architecture**: IEEE 802.15.4 packets require a 2-byte Frame Check Sequence (FCS) at the end of the PSDU. The ESP32 hardware radio transmitter is treated here as expecting `total_len = payload_len + 2`, with the radio injecting the CRC into the final 2 bytes; passing `payload_len` was observed to lose the last two payload bytes (c5-light-sync history, unverified).

## Prevention
1. **Always Verify Board Schematics for Backlight Polarity**: Never assume display backlights are active-high. Check factory firmware examples or schematics for active-low FET gates.
2. **Single CS Window for RAMWR and Pixel Data**: Group `RAMWR` and the corresponding framebuffer stream inside a single continuous CS assertion window, as `fill_rect` does (precaution; the failure mechanism is unproven).
3. **Always Park Unused Chip Selects on Shared Busses**: When microcontrollers share SPI lines between multiple peripherals (display, flash, SD card, sensors), explicitly initialize all unselected peripheral CS pins to `Level::High`.
4. **Account for Hardware FCS Overwrite in Raw IEEE 802.15.4**: When working with `esp-radio` raw PHY transmissions, reserve 2 extra bytes at the end of the buffer for the hardware CRC.
