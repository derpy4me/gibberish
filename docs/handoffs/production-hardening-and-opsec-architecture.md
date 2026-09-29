---
artifact_contract: "ce-handoff/v1"
created_at: "2026-09-28T22:20:00Z"
title: "Production Hardening & OPSEC Architecture Review (Claude Opus 5.5 Collaboration)"
summary: "Collaborative threat model and architecture review between Antigravity (Gemini Flash) and Claude Opus 5.5 covering RF emissions, information leakage, protocol correctness bugs, and a 5-phase production hardening plan."
keywords: ["opsec", "rf-leakage", "production-hardening", "opus5.5", "claude-code", "threat-model", "encryption"]
cwd: "/home/tscott/Work/esp32/gibberish"
resume_focus: "Review Opus 5.5 architectural decisions with user and execute Phase 0 (Radio silence for telemetry in prod, feature-gated diagnostics)"
repository: "gibberish"
repo_root_sha: "9a3a213980a373466144e59074092b3a985d89ce"
branch: "feat/milestone-10-slint-mesh-messaging-client"
head: "9a3a213980a373466144e59074092b3a985d89ce"
---

## Audit corrections (2026-09-29)

> This handoff is a historical record. An audit on 2026-09-29 checked its claims against the code. Implementation status: not built (proposal and leak audit; untracked file, no phase P0-P4 is implemented at HEAD 9a3a213); superseded in design by docs/plans/2026-09-29-0837-feat-production-opsec-radio-contract-plan.md, per the audit.

- **Omission:** the threat model treats the swarm key as the secret, but the swarm key is the constant `Secret::new([0x55u8; 32])` in source (`apps/gibberish-daemon/src/main.rs:130`), so anyone with the repo is a "member". Also not mentioned: DMs are encrypted under the swarm key, `total_chunks as u8` truncates long payloads (`chunk.rs:151-153`), and there is no BLE stack. See the OPSEC plan (R8, R11, R13).
- **Unverified:** the agreement/attribution to "Antigravity (Gemini Flash)" and "Claude Opus 5.5"; no transcript exists. Bugs 2 and 3 were confirmed by reading code only, not by test or hardware.

# Production Hardening & OPSEC Architecture Review

## 1. Executive Summary & Context
Following user feedback pointing out that previous work merely compressed telemetry payloads without eliminating unnecessary emissions in production, an in-depth OPSEC review was conducted with **Claude Opus 5.5** (`claude-opus-5-5` via Claude Code CLI).

Opus verified the user's critique and expanded it: **the current protocol leaks significant identifying and structural information beyond just the two telemetry beacons**, and contains three dormant correctness bugs in multi-hop, nonce derivation, and headless daemon node ID discovery.

---

## 2. Leaks Identified Across Current Airwaves

| Field / Location | Where in Code | What an SDR / RF Sniffer Learns |
| :--- | :--- | :--- |
| **Static & Delta Beacons** | `main.rs:329-370, 395-430` | Full hardware MAC, uptime, storage mode (SD vs RAM), free heap, SRAM buffer drops, build tier (`0xDB` vs `0x50`), and RSSI link mapping. In prod, receiving nodes simply discard these without forwarding to USB—making them pure leakage. |
| **`DEFAULT_NETWORK_TAG`** | `frame.rs:50` (`"GIBBERIS"`) | Broadcast in plain ASCII on beacons and physical button frames. An adversary can filter specifically for Gibberish devices instantly. |
| **`SWARM_NETWORK_TAG`** | `frame.rs:52` (Hardcoded const) | The same 8-byte static tag appears on every data frame, forever. It links all packets to one swarm and gives any listener the admission credential. |
| **MHR Bytes 7–10 (MAC Tail)** | `ieee802154.rs:113-116` | A permanent hardware identifier for each device. IEEE 802.15.4 FCF `0x41 0x08` declares no source address, so these bytes are arbitrary internal fields that currently leak the MAC. |
| **MHR Sequence Number** | `ieee802154.rs:104` | Increments sequentially per physical device, allowing packet counting and source correlation. |
| **`msg_id` Structure** | `chunk.rs:163` | Formatted as `(node_id & 0xFFFF0000) | counter`, leaking half the node ID and per-node message counts. |
| **SACK Payloads** | `chunk.rs:388` | Transmits `receiver_node_id`, `sender_node_id`, and bitmask in cleartext, leaking the complete social graph of who communicates with whom. |
| **`flags` Cleartext** | `frame.rs` | Leaks message type (`DIRECT`, `GROUP`, `CLIPBOARD`, `SACK`). |
| **Physical Button Frame** | `main.rs:236-255` | Broadcasts `[0xAA; 96]` dummy payload with ASCII tag—fingerprinting that a human physically interacted with the node. |

---

## 3. Correctness Bugs Identified by Opus 5.5

1. **Nonce Reuse After 65,536 Messages (Critical Security Vulnerability):**
   - Nonces are derived via `ratchet_counter = msg_id` (`chunk.rs:177`).
   - `msg_id` only allocates 16 bits to the sequential counter.
   - After 65,536 messages, the same sender subkey reuses nonces under ChaCha20-Poly1305, destroying confidentiality.
2. **Multi-Hop Decryption Failure:**
   - Relays re-transmit packets with their *own* MAC in the MHR (`main.rs:543-549`).
   - The recipient derives the ChaCha20-Poly1305 sender subkey from the MHR source address. In an $A \to B \to C$ relay chain, $C$ attempts to use $B$'s key to decrypt $A$'s payload, causing authentication to fail.
3. **Daemon Node ID Discovery in Headless/Prod Mode:**
   - Host daemon currently relies on parsing ASCII `Node ID: ...` lines during serial startup (`daemon/src/main.rs:183-197`).
   - In production firmware where ASCII logging is compiled out, the daemon defaults to node ID `0` unless `--node-id` is passed explicitly, causing sender subkey derivation mismatches.

---

## 4. Architectural Principles Agreed Upon

1. **The Dongle Remains Cryptographically Blind:**
   - The dongle firmware MUST NOT hold swarm master keys or user private keys.
   - All encryption/decryption occurs on the host daemon.
2. **True Radio Silence in Production:**
   - Production firmware transmits **zero autonomous diagnostic beacons** (no static beacon, no Trickle delta beacon).
   - Radio transmissions occur ONLY when:
     a) The local host injects an outbound frame over USB.
     b) An incoming multi-hop frame satisfies relay criteria.
3. **Local Telemetry Remains Local:**
   - Health metrics (SRAM buffer depth, drop counts, SD storage mode) travel exclusively over the USB cable (`ClosedTelemetry` binary frame) to drive the host UI status pill.
4. **Compile-Time Gating:**
   - The `StaticMetadataBeacon` and `CompactDeltaPayload` types in `gibberish-protocol` are placed behind a `diagnostics` cargo feature, enabled in firmware only when `features = ["debug-telemetry"]`. Production builds will fail to compile if any beacon emission code remains active.

---

## 5. Five-Phase Implementation Roadmap

- **Phase 0 (Stop the Bleeding - Immediate):**
  - Gate all RF beacon scheduling in `apps/gibberish-firmware/src/main.rs` behind `#[cfg(feature = "debug-telemetry")]`.
  - Disable physical button RF beacon broadcast in production.
  - Randomize MHR `seq` to eliminate sequential packet counting.
  - Put protocol diagnostic payload structs behind `features = ["diagnostics"]`.
  - Verify with dual dongles: idle production nodes emit 0 RF frames.
- **Phase 1 (Identity & Nonce Hardening):**
  - Move logical node ID generation and persistence to the daemon (independent of hardware MAC).
  - Expand nonce ratchet counter to 64-bit with a 32-bit keyed permutation for `msg_id` to eliminate nonce reuse.
  - Forward originator ID end-to-end through relays to fix multi-hop decryption.
- **Phase 2 (Header Protection & Admission Control):**
  - Replace static `SWARM_NETWORK_TAG` with per-frame keyed admission tags.
  - Optional: Hand ephemeral `admission_key` (derived one-way from swarm key) to dongle RAM over USB so dongles can drop non-swarm floods without decrypting payloads.
- **Phase 3 (Inner Metadata Protection):**
  - Encrypt SACK payloads inside ChaCha20-Poly1305 (hiding social graph).
  - Mask `flags` and chunk metadata.
- **Phase 4 (Presence & Verification Rigor):**
  - Implement passive DTN presence (triggering queued outbox packets off authenticated incoming traffic rather than periodic beacons).
  - Create `just rf-audit` using a promiscuous sniffer node to verify 0 idle frames and 0 identifier leaks.

---

## 6. Open Architectural Decisions for User

1. **Dongle Admission Key in RAM:**
   - *Option A (Recommended):* Host pushes an `admission_key` to dongle RAM over USB at startup. Dongle checks a 64-bit BLAKE3 keyed tag to drop RF noise/floods before waking the USB bus, but cannot decrypt messages.
   - *Option B (Zero-Key Dongle):* Dongle remains 100% keyless. Any well-formed 127B frame is passed to the host; host drops invalid frames. (Vulnerable to local USB bus flooding if adversary jams RF with junk).
2. **Standalone Battery Relays:**
   - Will there be standalone battery-powered dongles operating as relays without a host PC attached? (If yes, they would need `admission_key` in flash).
3. **Message Size Padding:**
   - Should frames be padded to fixed bucket sizes to prevent packet counting from revealing message lengths?
