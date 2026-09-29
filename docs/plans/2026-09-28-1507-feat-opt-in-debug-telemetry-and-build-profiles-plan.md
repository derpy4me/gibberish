---
title: "feat: Opt-in Embedded Debug Telemetry and Build Profiles"
type: feat
date: 2026-09-28
topic: debug-telemetry-and-build-profiles
artifact_contract: ce-unified-plan/v1
product_contract_source: ce-plan-bootstrap
execution: code
---

## Audit corrections (2026-09-29)

> This plan is a historical record. An audit on 2026-09-29 checked its claims against the code. Implementation status: built (79b3f73); line numbers in this plan are now about 4 lines off after 9a3a213, per the audit.

- **Superseded:** this plan's production-RF-telemetry requirements (beacons, telemetry cadence, node-ID/MAC handling, "stealth"/"zero-leakage" prod RF) are superseded by the OPSEC plan (its R27) — see docs/plans/2026-09-29-0837-feat-production-opsec-radio-contract-plan.md (R1-R5, R22, R27).
- **Claimed:** Summary: default build is a "lean, zero-leakage production image (... 60s stealth Trickle beacon)" **Actually:** the default image still transmits plaintext static and delta beacons with the raw MAC tail (`firmware/src/main.rs:~327-430`), a button-triggered `[0xAA;96]` frame with the ASCII tag (`:232-255`), and the MAC in every MHR. Trickle is 10-60 s, static beacon 180 s.
- **Claimed:** design table: debug = "5s rich beacon cadence", prod = "60s" **Actually:** the flag only gates logging, `#PKT#` vs binary CDC and the `build_tier` byte; both builds share one cadence and no 5 s constant exists. The same stale wording is in the `justfile:34,40` comments.
- **Unverified:** verification step `cargo clippy ... -D warnings` on both feature sets, and Definition of Done "hardware flash tested"; no output or artifact is recorded.

## Goal Capsule

- **Objective:** Eliminate the inverted `prod` feature flag anti-pattern, establish clean opt-in `--features debug-telemetry` conditional compilation across the firmware, and configure optimized release debug symbol profiles for ESP32-C5 embedded diagnosis without bloated dev profiles.
- **Means:** Deprecate the `prod` feature flag in `apps/gibberish-firmware/Cargo.toml`; establish `default = []` and `debug-telemetry = []`; replace all 7 dual-gated `#[cfg]` call sites in `apps/gibberish-firmware/src/main.rs`; add zero-cost format-string validation to the silent `log_info!` macro in production; configure `[profile.release-dbg]` for full DWARF symbol retention; update `justfile` recipes to reflect lean default production builds and explicit debug flags.
- **Authority:** User request & architectural reviews with Claude Opus 5.5.
- **Open Blockers:** None.

---

## Product Contract

### Summary
Clean, additive feature flag and build profile architecture for ESP32-C5 firmware. A standard release build (`cargo build --release` or `just build-prod`) defaults to the lean, zero-leakage production image (transmitting binary CDC frames, emitting no verbose `#PKT#` strings, 60s stealth Trickle beacon). Opting into debug instrumentation is explicitly toggled via `--features debug-telemetry` (producing verbose `#PKT#` ASCII decodes, ANSI terminal logs, and 5s rich beacon cadence). Furthermore, binary symbol introspection for debugging is separated from compiler optimizations via `[profile.release-dbg]`, preserving `opt-level = "s"` while emitting DWARF symbols for hardware probes and crash decoding.

### Requirements
- R1. `apps/gibberish-firmware/Cargo.toml` must declare `default = []` and `debug-telemetry = []`. The legacy `prod` feature must be completely removed.
- R2. `apps/gibberish-firmware/src/main.rs` must gate all debug text logging, `#PKT#` ASCII dumps, and debug tier telemetry strictly with `#[cfg(feature = "debug-telemetry")]`.
- R3. `apps/gibberish-firmware/src/main.rs` must gate all production binary CDC streaming and production tier beacons strictly with `#[cfg(not(feature = "debug-telemetry"))]`.
- R4. In non-debug builds, `log_info!` macro must safely consume arguments inside an optimized-away closure/block (`if false { let _ = format_args!($($arg)*); }`) to ensure format-string syntax checking without generating unused-variable warnings or text sections in the resulting ELF.
- R5. `apps/gibberish-firmware/Cargo.toml` must declare `[profile.release-dbg]` inheriting from `release` with `debug = true` and `opt-level = "s"` for symbol inspection without sacrificing timing or ROM footprint.
- R6. `justfile` recipes must be updated: `build-prod` and `flash-prod` must no longer pass `--features prod`; `build-debug` and `flash-debug` must continue passing `--features debug-telemetry`. `flash` recipe comments must explicitly state that default builds are production.

---

## Planning Contract

### Key Technical Decisions
- KTD1. **Single Additive Feature over Mutually Exclusive Pair** (session-settled / Opus consensus): Eliminate `prod` entirely and use `#[cfg(feature = "debug-telemetry")]` vs `#[cfg(not(feature = "debug-telemetry"))]`. Prevents invalid multi-feature states (`--all-features` or `--features prod,debug-telemetry`) and enables rustc `unexpected_cfgs` lint to immediately catch stale feature flags.
- KTD2. **Embedded Standalone Workspace Profiles**: `apps/gibberish-firmware` defines an empty `[workspace]` and is excluded from the root workspace. Cargo profiles affecting firmware must reside in `apps/gibberish-firmware/Cargo.toml`, not the root workspace `Cargo.toml`.
- KTD3. **Zero-Cost Syntax Validation in Prod Macro**: Rather than an empty `macro_rules! log_info { ($($arg:tt)*) => {} }` which triggers compiler warnings for unused variables only referenced in logs, use `if false { let _ = format_args!($($arg)*); }` to guarantee format arguments type-check while LLVM dead-code elimination removes all code and rodata strings.
- KTD4. **Preserve CDC Frame Wire Transport**: Ensure both variants maintain their respective wire contracts cleanly: production builds send length-prefixed postcard binary CDC frames (`TelemetryTier::Prod`); debug builds send formatted `#PKT#` and terminal logs (`TelemetryTier::Debug`).

### High-Level Technical Design

```
+-------------------------------------------------------------------------------------------------+
|                                    Cargo Build Configurations                                   |
+-------------------------------------------------------------------------------------------------+
| Command                                  | Features Active   | Output Mode      | Node Tier     |
|------------------------------------------+-------------------+------------------+---------------|
| cargo build --release                    | (none)            | Binary CDC       | Prod (60s)    |
| cargo build --release --features debug-..| debug-telemetry   | ASCII #PKT# Logs | Debug (5s)    |
| cargo build --profile release-dbg        | (none) + DWARF    | Binary CDC       | Prod (60s)    |
+-------------------------------------------------------------------------------------------------+
```

All 7 compilation sites in `apps/gibberish-firmware/src/main.rs`:
1. Line 18: `log_info!` active implementation -> `#[cfg(feature = "debug-telemetry")]`
2. Line 25: `log_info!` silent syntax-checked no-op -> `#[cfg(not(feature = "debug-telemetry"))]`
3. Line 346: `build_tier = TelemetryTier::Prod` -> `#[cfg(not(feature = "debug-telemetry"))]`
4. Line 350: `build_tier = TelemetryTier::Debug` -> `#[cfg(feature = "debug-telemetry")]`
5. Line 511: Radio RX hex `#PKT#` log dump -> `#[cfg(feature = "debug-telemetry")]`
6. Line 526: Radio RX binary CDC frame forwarding -> `#[cfg(not(feature = "debug-telemetry"))]`
7. Line 648: 1 Hz / 60s periodic binary telemetry CDC frame -> `#[cfg(not(feature = "debug-telemetry"))]`

---

## Implementation Units

### U1: Clean Feature Declarations and Profile in Firmware Manifest
- **Files:** `apps/gibberish-firmware/Cargo.toml`
- **Changes:**
  - Remove `prod = []` from `[features]`.
  - Set `default = []` and keep `debug-telemetry = []`.
  - Add `[profile.release-dbg]` inheriting from `release` with `debug = true`.
- **Verification:** `cargo check --manifest-path apps/gibberish-firmware/Cargo.toml` succeeds with no features.

### U2: Migrate Conditional Compilation Sites in Firmware Main
- **Files:** `apps/gibberish-firmware/src/main.rs`
- **Changes:**
  - Update macro definitions (lines 18 & 25) with `debug-telemetry` feature check and `format_args!` dead-code validation.
  - Update static metadata tier initialization (lines 346 & 350).
  - Update radio RX CDC forwarding logic (lines 511 & 526).
  - Update background telemetry loop (line 648).
- **Verification:** Build succeeds cleanly for both `cargo check --release` and `cargo check --release --features debug-telemetry`. No `unexpected_cfgs` or unused-variable warnings.

### U3: Update Justfile Automation Recipes
- **Files:** `justfile`
- **Changes:**
  - Update `build-prod` to `cd apps/gibberish-firmware && cargo build --release`.
  - Update `flash-prod` to `cd apps/gibberish-firmware && espflash flash {{ if port != "" { "--port " + port } else { "" } }} --release`.
  - Document that default `just flash` flashes production firmware.
- **Verification:** Run `just -n build-prod` and `just -n build-debug` to ensure correct command expansion.

---

## Verification Contract

1. **Compilation Check (Default / Prod):**
   ```bash
   cd apps/gibberish-firmware && cargo check --release
   ```
2. **Compilation Check (Debug Telemetry):**
   ```bash
   cd apps/gibberish-firmware && cargo check --release --features debug-telemetry
   ```
3. **Clippy Zero Warnings Check:**
   ```bash
   cd apps/gibberish-firmware && cargo clippy --release --all-targets -- -D warnings
   cd apps/gibberish-firmware && cargo clippy --release --features debug-telemetry --all-targets -- -D warnings
   ```
4. **Binary String Inspection:**
   Verify that `#PKT#` ASCII strings exist in the debug build ELF, and do NOT exist in the default production ELF:
   ```bash
   strings target/riscv32imac-unknown-none-elf/release/gibberish-firmware | grep "#PKT#" || echo "Clean"
   ```
5. **Physical Dual-Dongle Hardware Smoke Test:**
   Flash one dongle with `just flash-prod` and one with `just flash-debug`. Confirm bidirectional over-the-air communication and appropriate CDC logs.

---

## Definition of Done

- All 3 implementation units completed.
- Clippy passes with `-D warnings` on both default and `debug-telemetry` feature sets.
- `justfile` targets invoke clean commands without `--features prod`.
- Hardware flash tested on lilygo-t-dongle-c5 nodes.
