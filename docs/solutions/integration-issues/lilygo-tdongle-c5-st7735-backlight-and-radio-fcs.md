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
  - "ST7735 RAM write aborts if CS is pulsed high after 0x2C command before pixel streaming"
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
When developing bare-metal Rust firmware (`esp-hal` 1.2.1) on the LilyGO T-Dongle-C5 (ESP32-C5 single-core RISC-V), the onboard 0.96-inch ST7735 IPS LCD remains completely pitch black despite standard driver initialization. Additionally, SPI pixel streams experience display corruption if Chip Select is framed per-byte, unasserted SD card pins cause SPI bus contention, and 802.15.4 broadcasts silently truncate trailing payload fields due to hardware FCS CRC overwrites.

## Symptoms
- The ST7735 LCD backlight is unlit and completely dark, even after driving `LCD_BL` (GPIO 0) high.
- The display fails to render pixel rectangles or characters, or renders erratic noise/shifts, when Chip Select (GPIO 10) is toggled high between the `0x2C` (`RAMWR`) command and subsequent pixel data chunks.
- Read/write operations on SPI2 (MOSI GPIO 2, MISO GPIO 7, SCK GPIO 6) suffer intermittent bus contention or floating signal levels because the co-located microSD card socket CS pin (GPIO 23) is uninitialized.
- Receiving nodes on the 802.15.4 radio network receive packets where bytes 14 and 15 (e.g. RGB blue channel and pattern ID) are replaced with unexpected pseudo-random bytes.

## What Didn't Work
- **Driving `LCD_BL` High**: Following common active-high backlight conventions by calling `lcd_bl.set_high()`. On the T-Dongle-C5, the backlight switching transistor is configured active-low; driving GPIO 0 high turns the backlight completely off.
- **Per-Transaction CS Toggling on SPI**: Treating `0x2C` (`RAMWR`) as a standalone command transaction (CS low -> write 0x2C -> CS high) followed by data streaming (CS low -> write pixel buffer -> CS high). The ST7735 controller interprets CS rising as the termination of the RAM write sequence, ignoring subsequent data bytes.
- **Transmitting Exact Payload Length on 802.15.4 PHY**: Passing exact 16-byte buffers to `radio.transmit_raw(&buf[..16], true)`. The ESP32-C5 IEEE 802.15.4 baseband PHY hardware appends its 16-bit CRC by overwriting the last two bytes of the transmitted PSDU rather than extending the packet length.

## Solution

### 1. Invert Backlight Polarity (Active-Low GPIO 0)
Configure `GPIO 0` as a push-pull output and drive it `Level::Low` to turn ON the backlight in [`src/display.rs`](file:///home/tscott/Work/esp32/c5-light-sync/src/display.rs#L40-L47):

```rust
pub struct St7735<'d> {
    spi: Spi<'d, Blocking>,
    cs: Output<'d>,
    dc: Output<'d>,
    rst: Output<'d>,
    backlight: Output<'d>,
}

impl<'d> St7735<'d> {
    /// Controls display backlight. Active-low: `true` turns ON (low), `false` turns OFF (high).
    pub fn set_backlight(&mut self, on: bool) {
        if on {
            self.backlight.set_low();
        } else {
            self.backlight.set_high();
        }
    }

    pub fn init(&mut self, delay: &mut Delay) {
        // Backlight OFF during initialization (active-low: High = OFF)
        self.backlight.set_high();

        // Hardware Reset...
        // ... Send ST7735 init command table ...

        // Display ON
        self.write_cmd(0x29);
        delay.delay_millis(100);

        // Clear screen to black before enabling illumination
        self.fill_rect(0, 0, LCD_WIDTH, LCD_HEIGHT, 0x0000);

        // Backlight ON (Active-low on GPIO 0: Low = ON)
        self.backlight.set_low();
    }
}
```

### 2. Assert Continuous CS Framing Across RAMWR and Pixel Streams
Keep Chip Select (GPIO 10) asserted low from the `0x2C` command through the end of the pixel buffer transmission in [`src/display.rs`](file:///home/tscott/Work/esp32/c5-light-sync/src/display.rs#L178-L196):

```rust
// Keep CS LOW continuously for 0x2C command AND pixel data streaming!
self.cs.set_low();

// 1. Issue RAMWR (0x2C) with DC low
self.dc.set_low();
let _ = self.spi.write(&[0x2C]);

// 2. Stream pixels with DC high while CS remains low
self.dc.set_high();
let mut remaining = count;
while remaining > 0 {
    let chunk_pixels = remaining.min(32);
    let _ = self.spi.write(&buf[..(chunk_pixels * 2)]);
    remaining -= chunk_pixels;
}

self.cs.set_high();
```

### 3. Isolate Shared SPI Bus via SD_CS High
In [`src/main.rs`](file:///home/tscott/Work/esp32/c5-light-sync/src/main.rs#L102), drive `GPIO 23` (`SD_CS`) high to deselect the microSD socket sharing MOSI (GPIO 2) and MISO (GPIO 7):

```rust
let _sd_cs = Output::new(peripherals.GPIO23, Level::High, OutputConfig::default());
```

### 4. Pad 802.15.4 Transmit Buffer with 2 Dummy FCS Bytes
In [`src/radio.rs`](file:///home/tscott/Work/esp32/c5-light-sync/src/radio.rs#L89-L97), serialize the 16-byte payload into an 18-byte buffer so the hardware PHY overwrites bytes 16–17 with the CRC-16 rather than payload bytes 14–15:

```rust
pub fn broadcast(&mut self, packet: &SyncPacket) -> bool {
    let mut buf = [0u8; 18];
    let len = packet.serialize(&mut buf[..16]);
    // len is 16; pass total PSDU of 18 bytes to transmit_raw
    self.radio.transmit_raw(&buf[..len + 2], true).is_ok()
}
```

On RX, deserialize strictly the first 16 bytes:
```rust
if len >= 16 {
    if let Some(packet) = SyncPacket::deserialize(&raw.data[1..17]) {
        // Process valid sync packet
    }
}
```

## Why This Works
- **Active-Low Gate Circuit**: The LilyGO T-Dongle-C5 schematic drives the LCD backlight cathode or P-channel MOSFET gate via GPIO 0. Sinking current (pulling low) enables backlight power; driving high de-energizes it.
- **ST7735 RAM Controller State Machine**: The ST7735 internal memory write sequencer enters data capture on `0x2C` while CS is low and stays in memory autoincrement mode until CS transitions high. Any CS edge resets the command decoder.
- **Shared Bus Contention**: When the microSD card socket is left floating, capacitive coupling and floating card inputs can pull down MOSI/MISO, corrupting high-frequency SPI transfers (10–20 MHz). Holding `SD_CS` high guarantees the SD controller keeps its MISO line in high-impedance (Hi-Z).
- **802.15.4 Baseband Transmitter Architecture**: IEEE 802.15.4 packets require a 2-byte Frame Check Sequence (FCS) at the end of the PSDU. The ESP32 hardware radio transmitter expects the user to pass `total_len = payload_len + 2`, where the radio calculates and injects the CRC into the final 2 bytes. Passing `payload_len` causes the last two payload bytes to be overwritten by the CRC.

## Prevention
1. **Always Verify Board Schematics for Backlight Polarity**: Never assume display backlights are active-high. Check factory firmware examples or schematics for active-low FET gates.
2. **Atomic SPI Commands for Display Controllers**: Group `RAMWR` and the corresponding framebuffer stream inside a single continuous CS assertion window.
3. **Always Park Unused Chip Selects on Shared Busses**: When microcontrollers share SPI lines between multiple peripherals (display, flash, SD card, sensors), explicitly initialize all unselected peripheral CS pins to `Level::High`.
4. **Account for Hardware FCS Overwrite in Raw IEEE 802.15.4**: When working with `esp-radio` raw PHY transmissions, reserve 2 extra bytes at the end of the buffer for the hardware CRC.
