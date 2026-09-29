# Concepts

> Shared domain vocabulary for this project — entities, named processes, and status concepts with project-specific meaning. Seeded with core domain vocabulary, then accretes as ce-compound and ce-compound-refresh process learnings; direct edits are fine. Glossary only, not a spec or catch-all.

## Embedded Tooling & Firmware Architecture

### USB-Serial-JTAG
An on-chip peripheral integrated into modern Espressif RISC-V microcontrollers (including ESP32-C5 and ESP32-C6) that combines native USB CDC ACM serial communications and hardware JTAG debugging directly over the USB D+/D- lines. Distinct from external bridge ICs (such as CP2102 or CH340), its hardware interface is integrated into the silicon core and shares lifecycle state with the microcontroller.

### App Descriptor
A required binary header structure (`esp_app_desc_t`) placed in the application image metadata section that declares application version, project identity, compilation timestamp, and image verification hashes. The ESP-IDF bootloader and tooling (`espflash`) require this structure to validate and boot the partition image.

### Internal USB PHY
The integrated physical transceiver within the microcontroller silicon that interfaces the digital USB controller to the physical USB bus. Because the transceiver shares the chip's power and reset rails, hardware reset operations via flasher tooling cycle the transceiver state and require bus re-enumeration (unverified on the bench).

## Display & Radio Integration

### ST7735 RAMWR CS Framing
The SPI bus transaction discipline required by ST7735 display controllers where Chip Select (CS) must remain continuously asserted low through both the RAM write command (`0x2C`) and the subsequent RGB565 pixel data stream. Deasserting CS high after the command byte aborts the controller's memory autoincrement state and causes subsequent pixel writes to be ignored.

### Active-Low Backlight Gate
A display backlight switching topology where driving the control pin (GPIO 0 on LilyGO T-Dongle-C5) to logic low energizes the backlight circuit (via a P-channel MOSFET or PNP transistor), while driving logic high turns it off.

### IEEE 802.15.4 Hardware FCS Overwrite
The automatic behavior of the Espressif 802.15.4 radio baseband transmitter where the trailing 16 bits (2 bytes) of the transmit PSDU buffer are overwritten by the hardware CRC-16 Frame Check Sequence. Sizing transmit buffers to include two trailing dummy bytes prevents payload field corruption. (Hardware behavior is unverified beyond the project's own write-up; the code pads two zero bytes.)

### Blind Relay
A node in a zero-trust mesh swarm that receives, buffers, and retransmits any well-formed Gibberish-format frame without holding any key, checking admission, or decrypting payload contents. Headless repeaters are blind relays that carry no credential. Current firmware relays without holding any key, but only frames whose network tag matches one of two fixed constants, and only above a minimum link quality (LQI) with TTL remaining; no headless-repeater build exists. (Planned in the production OPSEC plan; not yet implemented.)

### Slotted TDM Radio Arbiter
A software-scheduled coordination loop that time-slices a single physical 2.4 GHz radio transceiver between distinct protocols (such as IEEE 802.15.4 and Bluetooth Low Energy) using deterministic operational windows separated by guard bands, bypassing hard dependencies on the Wi-Fi stack. The TDM scheduler exists in the firmware, but the BLE slot is an empty stub and there is no BLE stack, so only the 802.15.4 side is implemented.

### Authoritative SRAM Ring
A circular buffer allocated in internal static RAM that immediately ingests and serves live radio packets regardless of external storage availability, treating flash or MicroSD card containers as non-blocking asynchronous sinks.

### Single-Hop Telemetry Clamping
Enforcing `TTL = 1` and orthogonal packet flag segregation (`FLAG_TELEMETRY = 0x0020`) on diagnostic health beacons to restrict propagation strictly to direct one-hop radio reach, preventing multi-hop forwarding loops, network airtime saturation, and sliding bloom filter deduplication cache pollution. The mechanism is implemented, and telemetry frames are never relayed. Production firmware still transmits telemetry on a Trickle timer (10-60 s), not only debug builds. Superseded by the production OPSEC plan (2026-09-29); the plan removes production telemetry entirely.

### Telemetry Preemption
A priority scheduling discipline in a half-duplex radio backoff controller where loss-tolerant operational telemetry frames yield whenever user-interactive or encrypted data payloads are enqueued: the pending low-priority backoff is aborted and the low-priority frame is preserved (not dropped) for later transmission. User frames still wait a short randomized backoff (5-15 ms), so queue delay is reduced, not zero.

### Off-Grid Telemetry Sink
A workstation-attached radio transceiver node that overhears in-band IEEE 802.15.4 broadcast airwaves without IP, cellular, or Wi-Fi infrastructure, decoding raw physical frames from field nodes and streaming structured diagnostic telemetry to host companion daemons over USB CDC. In current builds the sink only receives telemetry from firmware built with the `debug-telemetry` feature; a production dongle decodes peer telemetry but discards it. Superseded by the production OPSEC plan (2026-09-29); the plan removes production telemetry and confines telemetry to debug builds on a lab network that production dongles ignore.

### Dirty-Region Character Caching
A display rendering technique where the driver caches character glyphs, foreground colors, and background colors across each text line, diffing them prior to transmission so that only modified character cells emit SPI address windowing (`CASET`/`RASET`) and pixel stream writes. On microcontroller systems where an LCD shares an SPI bus with high-throughput peripherals like MicroSD card slots, this is estimated to cut bus traffic by more than 95% (an estimate from a size comparison in a project write-up, not a measurement).

### Decoupled Event Loop Timing
The software architectural pattern in bare-metal polling loops of keeping slow display refresh cadences (such as a 4 Hz LCD update, 250ms) separated from system timekeeping (such as 1 Hz uptime counters, 1000ms). In the current firmware the separation is only by cadence, not by clock: both the LCD refresh (every 50 loop iterations) and the uptime counter (every 200 iterations) are counted from the same 5 ms loop tick, so an SPI stall that lengthens loop iterations also slows uptime and the Trickle telemetry timers.


## Zero-Trust Cryptography & Host Synchronization

### Asymmetric USB-CDC Framing
A dual-format serial protocol topology. In production firmware, dongle-to-host streams binary frames (`[0xAA, 0x55, len(2), crc16(2), payload]`), and the host parser handles them with a sliding window. In `debug-telemetry` builds the dongle instead emits line-delimited ASCII (`#PKT# <src_node_id:8hex> <wire_packet:228hex>\n`) so the parser can resynchronize across interleaved firmware debug logs. Host-to-dongle streams length-prefixed binary frames (`[0xAA, 0x55, 0x72, <114B>]`) parsed by an unconditional 4-state byte machine in firmware.

### Per-Sender HKDF Subkeys
A cryptographic key derivation topology where nodes in a shared swarm derive individual sender subkeys from the Swarm Master Key. The implementation is a keyed-BLAKE3 derivation with a context string, not HKDF (the crypto crate has no HKDF dependency). Receiving nodes inspect the 802.15.4 MAC Header (MHR) source address to derive the matching peer subkey. Distinct senders encrypt with distinct keys, avoiding multi-sender ChaCha20-Poly1305 nonce collisions without wire header overhead. The swarm key is currently the hardcoded development constant `[0x55; 32]`, so any swarm member (or anyone who reads the source) can derive every sender's key. Superseded by the production OPSEC plan (2026-09-29); the plan replaces it with per-device keys, directional DM keys, and keyring subkeys, with no key shipped in source.

### Monotonic Nonce Reservation
A disk-backed counter durability pattern where host companion daemons write ahead counter reservations in blocks (e.g. 1,000) committed with atomic rename and `fsync` before allocating any counter values, combined with `epoch_secs = max(wall_clock, last_persisted_epoch + 1)` to guard against nonce reuse across unexpected crashes, reboots, and system clock rollback. The reserved counter is truncated to the 16-bit `msg_id` on the wire and the epoch is discarded, so the nonce for a given sender key repeats after 65,536 messages.

### Provenance Hash Echo Suppression
A loopback suppression mechanism for synchronized clipboard daemons that records `blake3(text)` and a 500ms time window whenever writing remote text to the local OS clipboard. Incoming clipboard events within the window matching the recorded hash are dropped as local echoes, while user-initiated copies or differing content immediately transmit.

### MAC-Layer Promiscuous Loopback Suppression
A packet filtering discipline in raw IEEE 802.15.4 promiscuous mesh transceivers where the low-level radio polling loop compares the MAC Header (MHR) Source Short Address against the local device's hardware MAC address (`raw.data[8..12] == local_mac[4..8]`) and immediately discards self-transmitted airwave reflections before cryptographic decryption, payload processing, or host serial forwarding. The source address is the MAC-derived node identifier. Superseded by the production OPSEC plan (2026-09-29); the plan removes the MAC loopback check and the MAC-derived node ID, and suppresses own echoes with a sent-frame cache and by never trying send-direction keys.

## Mesh Messaging & Client Architecture

### Hybrid Asymmetric Delivery
A traffic-class segregation pattern in half-duplex mesh networks intended to keep broadcast channels (#all Swarm) fire-and-forget without ACKs, avoiding packet collisions and broadcast ACK implosion, while direct messages are acknowledged. As implemented, the acknowledgment is a plaintext, unauthenticated chunk-level bitmask (SACK/NACK) generated for any incomplete multi-chunk message, including #all and clipboard messages; it is not cryptographic. `FLAG_ACK_REQ` on DMs is set but never acted on by a receiver, and the delivery-policy code is not called from runtime paths.

### Open Swarm Channel
The public #all channel that any dongle joins with no setup. Posts travel as plaintext, identify their sender only by a user-chosen, changeable handle, and are shown with a warning that the channel is not encrypted. Confidential group chat belongs to private rooms, not the swarm channel. Current builds differ: #all is encrypted with the hardcoded development swarm key, senders are identified by node ID, there is no handle, and no warning is shown. (Planned in the production OPSEC plan; not yet implemented.)

### Production Radio Silence
The production transmit rule: a dongle transmits only frames its host hands it or frames it relays, so an idle production dongle emits nothing. Beacons, telemetry, and presence frames exist only in debug builds, on a lab network production dongles ignore. Current production firmware does not follow this rule: it emits a static beacon roughly every 3 minutes and Trickle telemetry every 10-60 s. (Planned in the production OPSEC plan; not yet implemented.)

### Beacon-Triggered DTN Outbox
A delay-tolerant networking pattern where messages addressed to offline mesh stations queue in a local bounded buffer and flush opportunistically upon overhearing the recipient's periodic airwave announcement beacon, avoiding blind channel-congesting retries into unreachable links. The outbox engine exists as library code exercised only by tests; the daemon never calls its beacon-flush or eviction paths, and the live send path inserts a pending outbox row and transmits immediately over RF. Announcement beacons carry no public key or alias. Superseded by the production OPSEC plan (2026-09-29); the plan replaces beacon-triggered flushing with a DM retry loop at growing intervals until the TTL, and no periodic presence broadcast exists.

### Short Authentication String (SAS) Verification
An out-of-band cryptographic verification pattern where peers mathematically derive a matching 4-word mnemonic or short numeric code from their respective public keys and compare them verbally or via QR code, proving contact authenticity and eliminating airwave impersonation without centralized certificate authorities. The SAS derivation is implemented, but no key exchange exists: the local key is hardcoded, the peer key is a placeholder derived from the node ID, and stored contacts hold an all-zero public key, so the words currently prove nothing.

### Multi-Dongle Airwave Loopback Suppression
A host-side loopback suppression discipline in multi-transceiver environments where a host companion daemon connected to multiple physical RF dongles tracks all local hardware station IDs, discarding over-the-air packet reflections transmitted by one local dongle and overheard by another before injecting them into the local database, IPC, or UI chat model. The set of local station IDs is filled only from dongle log lines (which only `debug-telemetry` firmware prints) or the `--node-id` option, so with production firmware and no `--node-id` the set is empty and suppression does not fire. Superseded by the production OPSEC plan (2026-09-29); the plan removes MAC-derived node IDs, has the daemon learn its own identity without dongle debug output, and deduplicates frames heard by several dongles on message ID, chunk index, and body hash.

### Payload-Enclosed Destination Routing
An in-band routing mechanism for MTU-constrained physical frames where recipient station addresses are embedded directly inside authenticated payload chunks rather than the unencrypted wire header.

This preserves fixed-size radio frame headers across intermediate forwarding hops, allowing blind relay nodes to handle packets opaquely while endpoints inspect the decrypted prefix to route messages into targeted conversation channels. Because all DMs currently use the shared swarm key, any swarm member can decrypt any DM and read its destination.

### Optimistic Chat Reflection
A user interface synchronization pattern where locally dispatched chat messages are immediately injected into conversation models with a pending status flag prior to transport confirmation.

When background daemon or physical transport acknowledgment arrives, the client reconciles status without re-rendering or duplicating message entries, eliminating human-perceptible transmission lag.
