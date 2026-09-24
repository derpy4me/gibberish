---
title: "ESP32-C5 ST7735 LCD 4 Hz Dirty-Region Rendering & Monotonic Event Loop Decoupling"
date: "2026-09-24"
category: "integration-issues"
module: "firmware"
problem_type: "integration_issue"
component: "display"
severity: "medium"
symptoms:
  - "Full-screen ST7735 LCD redraws saturate shared SPI2 bus and contend with MicroSD card transactions"
  - "System uptime clock and heartbeat telemetry run 4x faster than wall-clock time"
  - "Cargo workspace root build fails with missing bare-metal linker symbols (_rtc_fast_bss_end, _stack_end_cpu0, DefaultHandler)"
root_cause: "architecture_mismatch"
resolution_type: "code_fix"
tags:
  - "esp32-c5"
  - "st7735"
  - "lcd"
  - "dirty-region"
  - "spi-coexistence"
  - "event-loop"
  - "cargo-workspace"
  - "riscv32imac"
---

# ESP32-C5 ST7735 LCD 4 Hz Dirty-Region Rendering & Monotonic Event Loop Decoupling

## Problem
On the LilyGO T-Dongle-C5 (ESP32-C5 RISC-V), the ST7735 0.96-inch color LCD (160×80 RGB565) and the onboard MicroSD card slot share the identical hardware `SPI2` peripheral (SCK: GPIO 6, MOSI: GPIO 2, MISO: GPIO 7, LCD_CS: GPIO 10, SD_CS: GPIO 23). Performing unbuffered full-screen repaints at a standard 4 Hz refresh rate streams 25,600 bytes of pixel data over SPI four times every second (102.4 KB/s), severely saturating the half-duplex SPI bus, introducing high bus contention, and stalling latency-sensitive radio packet ingestion. Furthermore, coupling bare-metal event loop telemetry timestamps to display refresh timers causes telemetry clocks to drift wildly.

## Symptoms
- **SPI Bus Starvation**: MicroSD sector writes and radio packet handling experience latency spikes or buffer overflow drops when the display is constantly redrawing static status lines.
- **4x Fast Clock Telemetry**: When `uptime_secs` was updated inside the 4 Hz LCD refresh block (`loop_tick % 50 == 0` at 5ms per tick), the device incremented `uptime_secs` every 250ms, causing peer devices to observe telemetry running 4x faster than real time.
- **Cargo Linker Failure in Multi-Crate Workspace**: Compiling firmware from the repository root via `cargo build --manifest-path apps/gibberish-firmware/Cargo.toml --target riscv32imac-unknown-none-elf` fails with `rust-lld: error: undefined symbol: _rtc_fast_bss_end, DefaultHandler` because member-level `.cargo/config.toml` rustflags (`-C link-arg=-Tlinkall.x`) are not inherited when Cargo runs outside the member directory.

## What Didn't Work
- **Naive Full-Screen Clearing (`clear_screen` + `render_status`)**: Repainting the full 160×80 grid every 250ms flooded SPI2 with 25.6 KB bursts. This caused visual tearing and choked concurrent SPI transactions with the MicroSD block device.
- **Coupling State Logic to Display Tick**: Tying status update intervals and system health telemetry to display redraw intervals caused timekeeping distortions whenever the display refresh rate or delay pacing changed.
- **Invoking Cargo from Workspace Root without Local Config**: Cargo only looks upward from the current working directory for `.cargo/config.toml`. When invoked from the workspace root without top-level linker script configuration, Cargo completely dropped the target linker arguments.

## Solution

### 1. Character-Cell and Row-Level Dirty Diffing (`draw_dirty_line`)
In [`apps/gibberish-firmware/src/ui/display.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-firmware/src/ui/display.rs), implement cached text, foreground, and background buffers. Only character cells that actually changed since the previous frame emit SPI window addressing (`CASET`/`RASET`) and pixel stream writes:

```rust
pub const NUM_LINES: usize = 7;
pub const LINE_CHARS: usize = 26;

pub struct St7735<'a, 'd> {
    spi: &'a RefCell<Spi<'d, Blocking>>,
    cs: Output<'d>,
    dc: Output<'d>,
    rst: Output<'d>,
    backlight: Output<'d>,
    cached_text: [[u8; LINE_CHARS]; NUM_LINES],
    cached_fg: [u16; NUM_LINES],
    cached_bg: [u16; NUM_LINES],
    cached_valid: [bool; NUM_LINES],
    last_sync_phase: u8,
}

pub fn draw_dirty_line(&mut self, line_idx: usize, text: &str, fg: u16, bg: u16) {
    if line_idx >= NUM_LINES {
        return;
    }

    let max_cols = if line_idx == 0 { 19 } else { LINE_CHARS };
    let mut new_chars = [b' '; LINE_CHARS];
    let bytes = text.as_bytes();
    let to_copy = bytes.len().min(max_cols);
    new_chars[..to_copy].copy_from_slice(&bytes[..to_copy]);

    // Skip entirely if line is already valid and identical
    if self.cached_valid[line_idx]
        && self.cached_fg[line_idx] == fg
        && self.cached_bg[line_idx] == bg
        && self.cached_text[line_idx][..max_cols] == new_chars[..max_cols]
    {
        return;
    }

    let y = 2 + (line_idx as u16) * 11;
    for col in 0..max_cols {
        let c = new_chars[col];
        let dirty = !self.cached_valid[line_idx]
            || self.cached_text[line_idx][col] != c
            || self.cached_fg[line_idx] != fg
            || self.cached_bg[line_idx] != bg;

        if dirty {
            let x = 2 + (col as u16) * 6;
            self.draw_char(x, y, c as char, fg, bg);
            self.cached_text[line_idx][col] = c;
        }
    }

    self.cached_fg[line_idx] = fg;
    self.cached_bg[line_idx] = bg;
    self.cached_valid[line_idx] = true;
}
```

### 2. Decouple 4 Hz Display Refresh from 1 Hz Monotonic Telemetry
In [`apps/gibberish-firmware/src/main.rs`](file:///home/tscott/Work/esp32/gibberish/apps/gibberish-firmware/src/main.rs), separate display redraw cadence (250ms = 50 ticks of 5ms) from the wall-clock telemetry clock (1000ms = 200 ticks of 5ms):

```rust
// 1. Monotonic 1-second system uptime clock (1000ms = 200 ticks of 5ms)
if loop_tick % 200 == 0 {
    telemetry.uptime_secs = telemetry.uptime_secs.saturating_add(1);
}

// 2. 4 Hz Status Display Render (250ms = 50 ticks of 5ms)
if loop_tick % 50 == 0 {
    display.render_status(
        storage.mode(),
        telemetry.rx_packet_count,
        telemetry.tx_packet_count,
        sram_ring.len(),
        sram_ring.dropped_count(),
        local_node_id,
        &peer_table,
        sync_phase,
    );
}
```

### 3. Cargo Workspace Cwd Discipline
When compiling bare-metal firmware crates, invoke `cargo` with the working directory set directly to the crate containing `.cargo/config.toml`:

```bash
cd apps/gibberish-firmware && cargo build --release
```
Or ensure top-level `.cargo/config.toml` at workspace root provides identical `link-arg=-Tlinkall.x` target rustflags.

## Why This Works
1. **Bandwidth Reduction**: Character-cell dirty caching reduces SPI bus traffic from 25.6 KB per frame down to ~150–300 bytes per frame during normal throughput counter increments, reducing SPI bus utilization by >95% and leaving the shared bus free for MicroSD block I/O.
2. **Deterministic Timekeeping**: Decoupling the 1 Hz uptime clock from the 4 Hz rendering timer ensures telemetry payload values match true wall-clock time regardless of display rendering activity.
3. **Linker Discovery**: Running inside the crate directory allows Cargo to pick up `.cargo/config.toml` directives, linking `linkall.x` and resolving the hardware memory layout symbols required by `esp-hal`.

## Prevention & Best Practices
- **Never Repaint Static Text on Shared Microcontroller Buses**: Always implement line or bounding-box caching when driving displays over a bus shared with external memory or sensors.
- **Keep Display Refresh Ticks Orthogonal to Uptime Timers**: Maintain explicit tick division modulo counters (`% 200` for 1 Hz vs `% 50` for 4 Hz) in bare-metal polling loops.
- **Standardize Workspace `.cargo/config.toml`**: Place bare-metal target flags in both the crate-level directory and workspace root, or encapsulate build commands in repository scripts.
