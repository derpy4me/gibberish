# Concepts

> Shared domain vocabulary for this project — entities, named processes, and status concepts with project-specific meaning. Seeded with core domain vocabulary, then accretes as ce-compound and ce-compound-refresh process learnings; direct edits are fine. Glossary only, not a spec or catch-all.

## Embedded Tooling & Firmware Architecture

### USB-Serial-JTAG
An on-chip peripheral integrated into modern Espressif RISC-V microcontrollers (including ESP32-C5 and ESP32-C6) that combines native USB CDC ACM serial communications and hardware JTAG debugging directly over the USB D+/D- lines. Distinct from external bridge ICs (such as CP2102 or CH340), its hardware interface is integrated into the silicon core and shares lifecycle state with the microcontroller.

### App Descriptor
A required binary header structure (`esp_app_desc_t`) placed in the application image metadata section that declares application version, project identity, compilation timestamp, and image verification hashes. The ESP-IDF bootloader and tooling (`espflash`) require this structure to validate and boot the partition image.

### Internal USB PHY
The integrated physical transceiver within the microcontroller silicon that interfaces the digital USB controller to the physical USB bus. Because the transceiver shares the chip's power and reset rails, hardware reset operations via flasher tooling cycle the transceiver state and require bus re-enumeration.

## Display & Radio Integration

### ST7735 RAMWR CS Framing
The SPI bus transaction discipline required by ST7735 display controllers where Chip Select (CS) must remain continuously asserted low through both the RAM write command (`0x2C`) and the subsequent RGB565 pixel data stream. Deasserting CS high after the command byte aborts the controller's memory autoincrement state and causes subsequent pixel writes to be ignored.

### Active-Low Backlight Gate
A display backlight switching topology where driving the control pin (GPIO 0 on LilyGO T-Dongle-C5) to logic low energizes the backlight circuit (via a P-channel MOSFET or PNP transistor), while driving logic high turns it off.

### IEEE 802.15.4 Hardware FCS Overwrite
The automatic behavior of the Espressif 802.15.4 radio baseband transmitter where the trailing 16 bits (2 bytes) of the transmit PSDU buffer are overwritten by the hardware CRC-16 Frame Check Sequence. Sizing transmit buffers to include two trailing dummy bytes prevents payload field corruption.

### Blind Relay
A node in a zero-trust mesh swarm that receives, validates network admission tags on, buffers, and retransmits encrypted packet chunks without holding cryptographic identity keys or decrypting payload contents.

### Slotted TDM Radio Arbiter
A software-scheduled coordination loop that time-slices a single physical 2.4 GHz radio transceiver between distinct protocols (such as IEEE 802.15.4 and Bluetooth Low Energy) using deterministic operational windows separated by guard bands, bypassing hard dependencies on the Wi-Fi stack.

### Authoritative SRAM Ring
A circular buffer allocated in internal static RAM that immediately ingests and serves live radio packets regardless of external storage availability, treating flash or MicroSD card containers as non-blocking asynchronous sinks.

### Single-Hop Telemetry Clamping
Enforcing `TTL = 1` and orthogonal packet flag segregation (`FLAG_TELEMETRY = 0x0020`) on diagnostic health beacons to restrict propagation strictly to direct one-hop radio reach, preventing multi-hop forwarding loops, network airtime saturation, and sliding bloom filter deduplication cache pollution.

### Telemetry Preemption
A priority scheduling discipline in a half-duplex radio backoff controller where loss-tolerant operational telemetry frames immediately yield or drop without retry accumulation whenever user-interactive or encrypted data payloads are enqueued, guaranteeing zero queue delay for user traffic.

### Off-Grid Telemetry Sink
A workstation-attached radio transceiver node that overhears in-band IEEE 802.15.4 broadcast airwaves without IP, cellular, or Wi-Fi infrastructure, decoding raw physical frames from field nodes and streaming structured diagnostic telemetry to host companion daemons over USB CDC.

## Zero-Trust Cryptography & Host Synchronization

### Asymmetric USB-CDC Framing
A dual-format serial protocol topology where dongle-to-host streams line-delimited ASCII (`#PKT# <src_node_id:8hex> <wire_packet:228hex>\n`) to guarantee clean parser resynchronization across interleaved firmware debug logs, while host-to-dongle streams length-prefixed binary frames (`[0xAA, 0x55, 0x72, <114B>]`) parsed by an unconditional 4-state byte machine in firmware, eliminating sliding-window buffer operations and tag collisions in ciphertext.

### Per-Sender HKDF Subkeys
A cryptographic key derivation topology where nodes in a shared swarm derive individual sender subkeys from the Swarm Master Key (`sender_key = HKDF(swarm_key, info = node_id)`). Receiving nodes inspect the 802.15.4 MAC Header (MHR) source address to derive the matching peer subkey. Distinct senders encrypt with mathematically distinct keys, eliminating multi-sender ChaCha20-Poly1305 nonce collisions without wire header overhead.

### Monotonic Nonce Reservation
A disk-backed counter durability pattern where host companion daemons write ahead counter reservations in blocks (e.g. 1,000) committed with atomic rename and `fsync` before allocating any counter values, combined with `epoch_secs = max(wall_clock, last_persisted_epoch + 1)` to eliminate nonce reuse across unexpected crashes, reboots, and system clock rollback.

### Provenance Hash Echo Suppression
A loopback suppression mechanism for synchronized clipboard daemons that records `blake3(text)` and a 500ms time window whenever writing remote text to the local OS clipboard. Incoming clipboard events within the window matching the recorded hash are dropped as local echoes, while user-initiated copies or differing content immediately transmit.

