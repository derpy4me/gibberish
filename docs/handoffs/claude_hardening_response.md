## Audit corrections (2026-09-29)

> This handoff is a historical record. An audit on 2026-09-29 checked its claims against the code. Implementation status: not built (review response and proposal; untracked file; it states nothing was changed or tested, which matches git); superseded in design by docs/plans/2026-09-29-0837-feat-production-opsec-radio-contract-plan.md, per the audit.

- **Omission:** "protects against outsiders only. Every swarm member holds the swarm key" understates the gap: the swarm key is the constant `Secret::new([0x55u8; 32])` in source (`apps/gibberish-daemon/src/main.rs:130`), and phases P1-P3 do not provide real key provisioning. See the OPSEC plan (R13).
- **Claimed:** "The variable-length PHY is only used for telemetry" **Actually:** `transmit()` always calls `transmit_variable` (`ieee802154.rs:96-98`); data frames are simply full length.

✅ **I agree with the PO, and the problem is larger than the brief says.** Removing the two telemetry beacons does not take the hardware identity off the air. Every data frame carries it, the network tag names the project in plain ASCII, and SACKs send the social graph in cleartext. I also found three existing correctness bugs that the redesign has to fix anyway. Everything below is from reading the code. I haven't changed anything or run any tests.

## Where I disagree with the brief

1. **The dongle can't encrypt presence beacons with the swarm key.** It is deliberately built without keys (`apps/gibberish-firmware/Cargo.toml`: "NEVER import gibberish-crypto"). Anything encrypted has to start on the daemon and go out as a normal frame.
2. **Most of the "must travel" list doesn't need to be in cleartext.** Relays only read `msg_id`, `chunk_idx`, `ttl` and an admission check. `flags`, `total_chunks`, `hop_count` and the sender ID can be masked. `hop_count` isn't read anywhere; it only reveals how far a frame is from its author (`hop_count=0` means the transmitter wrote it).
3. **The beacons do nothing in prod.** The firmware receives them, logs them through `log_info!` (compiled out in prod), and hits `continue` without passing them to USB (`main.rs:455-492`). Only the ASCII log parsers used by the debug sinks read them. In prod they are pure leakage.

## Leaks the brief missed

| Where | What a sniffer gets |
|---|---|
| `DEFAULT_NETWORK_TAG` = ASCII `"GIBBERIS"` (`frame.rs:50`) on beacons and button frames | A trivial filter for "Gibberish mesh here" |
| `SWARM_NETWORK_TAG`, a hard-coded constant (`frame.rs:52`) | The same 8 bytes on every data frame, forever. It links all traffic to one swarm and gives anyone who sniffs one frame the admission credential. |
| MHR bytes 7–10 = the MAC tail on every frame (`ieee802154.rs:113-116`) | A permanent hardware ID for each device. The FCF (`0x41 0x08`) declares no source address, so these are our own bytes and nothing in the standard requires the MAC there. |
| MHR `frame_seq` increments per device (`ieee802154.rs:104`) | Links frames to a device and counts them |
| `msg_id = (node_id & 0xFFFF0000) \| counter` (`chunk.rs:163`) | Half the node ID plus a per-node message count |
| SACK payload in cleartext: `receiver_node_id`, `sender_node_id`, bitmask (`chunk.rs:388`) | **Who talks to whom.** Also, anyone can forge SACKs to trigger retransmits. |
| `flags` in cleartext (`FLAG_DIRECT`/`GROUP`/`CLIPBOARD`/`SACK`) | Message type: DM, group or clipboard sync |
| Button press sends a `[0xAA;96]` payload with the ASCII tag (`main.rs:236-255`) | A fingerprint pattern, plus proof a human is holding the device |
| `StaticMetadataBeacon.node_id` is documented as a "truncated BLAKE3 hash" (`frame.rs:531`) | It is actually the raw MAC (`main.rs:348`). The comment is false. |

**Threat model for the telemetry fields specifically (outsider with a sniffer or SDR, no swarm key):**
- **MAC / node ID:** tracking a device over days, and a link to the hardware (MACs are readable over USB and from eFuse).
- **Uptime:** reboot detection and device fingerprinting.
- **`build_tier`:** tells an attacker which node is a soft target.
- **`storage_mode`:** which node holds messages on an SD card, i.e. the one to seize.
- **rx/tx counts:** activity levels, and by differencing, who is sending.
- **Drops, heap, SRAM:** how close a node is to overflowing, which is a DoS-tuning aid.
- **`last_rssi`:** distance to the nearest peer. Combined across nodes, this maps the network's layout.

**Q3, do remote peers need any of this?** No. The only remotely plausible case is link quality, and each receiver already measures RSSI/LQI itself when a frame arrives. It was on the air because the telemetry work treated "cheap to encode" as "fine to send".

## Correctness bugs found along the way

1. **Nonce reuse after 65,536 messages per sender.** The nonce is derived from `msg_id` (`ratchet_counter = msg_id`, `chunk.rs:177`), and `msg_id` keeps only 16 bits of the counter. After 65,536 messages the same sender subkey reuses a nonce, which breaks ChaCha20-Poly1305 confidentiality. **This alone blocks a production release.**
2. **Multi-hop decryption fails.** Relays re-send with their own MAC in the MHR (`main.rs:543-549` → `transmit_variable`). The receiver derives the subkey from that MHR source, so in an A→B→C chain, C tries B's key and authentication fails. Only direct reception works.
3. **Prod daemons probably can't learn their own node ID.** Discovery parses ASCII `Node ID:` and heartbeat lines (`daemon main.rs:183-197, 265-287`), and prod firmware compiles those out. Without `--node-id`, `local_node_id` stays 0, so the sender subkey is derived from 0 while receivers derive it from the MAC. I haven't confirmed this on hardware; one OTA run with both dongles on prod builds would settle it.

Bugs 2 and 3 have the same fix as the OPSEC problem: identity belongs to the daemon and travels end-to-end, not in the per-hop header.

## Target production wire format (v2)

**All prod frames are 127 bytes, and data, SACK and presence frames look the same.** The variable-length PHY is only used for telemetry, so it goes away in prod.

| Bytes | Field | On the air | Who reads it |
|---|---|---|---|
| MHR 0–6 | FCF, seq, PAN, dst | `seq` random per frame | nobody |
| MHR 7–10 | Originator ID | **Masked** (XOR with `PRF(header_key, msg_id‖chunk_idx)`) | Daemon only |
| 8B | Admission tag | **New value per frame:** `trunc64(BLAKE3_keyed(admission_key, msg_id‖chunk_idx‖masked fields‖ciphertext))`, with `ttl` excluded | Dongle filter and relay integrity check |
| 4B | `msg_id` | A 32-bit keyed permutation of the sender's 64-bit counter. Looks random, never repeats per sender, and the receiver inverts it to get the nonce counter. **This fixes bug 1.** | Relay dedup |
| 1B / 1B | `chunk_idx`, `ttl` | Clear | Relays |
| 1B / 1B / 2B | `total_chunks`, `hop_count`→reserved, `flags` | **Masked** | Daemon only |
| 96B | Payload | ChaCha20-Poly1305. SACK sender and receiver IDs move inside it. | Daemon only |

**Identity:** the daemon generates a random 32-bit logical node ID once and stores it; the hardware MAC is not used. That fixes bugs 2 and 3, keeps identity when a dongle is swapped, and the dongle never needs to know it. The host-to-dongle USB frame now carries the masked originator bytes, and relays pass them through unchanged. The loopback check switches to the bloom filter, which already records outbound `msg_id`s.

**Keys:** derive `admission_key` and `header_key` one-way from the swarm key. The dongle gets **only** `admission_key`, over USB at daemon start. With it the dongle can check admission and refuse to relay outsider floods, but it cannot decrypt anything. That relaxes the blind-dongle rule, so it's your call (decision 1). It is still strictly better than today, where the admission credential is a constant in the firmware image and in every frame.

**Presence:** nothing by default. Firmware sends nothing on its own in prod. The DTN outbox was designed around "peer beacon overheard" (`dtn_outbox.rs:100`), but it isn't wired into the daemon yet. Feed it from any authenticated inbound frame from peer X instead, which costs zero extra emissions. For peers that only listen, retry with backoff. Add an encrypted presence message from the daemon (identical 127-byte frame, low rate, jittered, opt-in) only if a real DTN scenario shows passive presence isn't enough.

## Debug vs prod boundary

- **Compile-time guarantee:** put `StaticMetadataBeacon`, `CompactDeltaPayload`, `DebugTelemetryPayload` and `FrameKind::StaticMetadata/CompactDelta` behind a `diagnostics` feature in `gibberish-protocol`. Firmware enables it only through `debug-telemetry`. A prod build that tries to build a beacon then fails to compile, rather than depending on a runtime `cfg` branch.
- **Prod RF rule:** the firmware only transmits (a) frames the host handed it and (b) relays. The button drives only the BLE gate.
- **Debug:** everything stays open (ASCII, beacons, cleartext) but on its own `LAB` tag, which prod firmware rejects. **The data-plane format is identical in both builds.** Debug only adds extra frames, so debug-build tests still exercise prod messaging.
- **Guardrails against a debug dongle in the field:**
  - Add `build_tier` and the firmware version to the USB-only `ClosedTelemetry`.
  - A prod daemon refuses a debug dongle unless given `--allow-debug-dongle`.
  - Debug builds show a DEBUG banner on the LCD.
- **USB `ClosedTelemetry`:** keep it as is in prod. The cable is the right channel for local health data.

## Tests

- **Protocol unit and property tests:**
  - Mask, tag and permutation round-trips.
  - The permutation is a bijection, so there are no nonce collisions across the whole counter range.
  - No serialized prod frame contains the node ID, the MAC tail, or `"GIBBERIS"`.
  - Tags differ across frames.
  - A relay's TTL change keeps the tag valid.
  - A tampered byte fails the tag check.
- **Regression tests for bugs 1–3:** a 3-hop relay in `integration-sim`, more than 65,536 messages with no repeated nonce, and daemon startup without `--node-id` against a prod-format dongle.
- **`just rf-audit` (hardware):**
  - New `rf-sniffer` debug firmware: promiscuous mode that forwards every raw frame over USB.
  - Put two prod dongles next to the sniffer, **idle 10 minutes, and assert zero frames**.
  - Then send N messages and assert: all frames are 127 bytes, no identifier bytes, distinct tags, non-sequential `msg_id` and MHR `seq`, and no cleartext SACK fields.
- **Existing suites:** `test-fleet`, `fleet_sink_test` and `telemetry_qa_rigor_test` go behind `diagnostics` and require debug firmware. They should fail clearly if they see a prod dongle.

## Phased plan

- **P0 – Stop the bleeding (small, independent):**
  - Put the beacons, the Trickle beacon loop and the button RF beacon behind `debug-telemetry`.
  - Put the protocol types behind `diagnostics`.
  - Randomize MHR `seq`.
  - Fix the false `node_id` doc comment.
  - The idle half of `rf-audit` can prove this phase is done.
- **P1 – Identity and nonce (fixes bugs 1–3):**
  - Daemon-owned logical ID.
  - Counter-to-`msg_id` permutation.
  - Originator carried end to end (USB framing change, relays pass it through).
- **P2 – Header protection and per-frame admission tag:**
  - Key derivation.
  - `admission_key` handoff over USB.
  - Dongle-side tag check.
  - Remove the static tags.
- **P3 – Inner semantics:**
  - Encrypt SACKs.
  - Mask `flags` and `total_chunks`.
  - The dongle no longer reads `FLAG_CLIPBOARD`; the host tells it when to play the LED animation.
- **P4 – Presence and guardrails:**
  - Passive DTN presence.
  - Debug-dongle refusal.
  - LCD banner.
  - The full `rf-audit`.

⚠️ **Breaking change:** P1–P3 change the RF and USB formats. Mixed-version swarms won't work, so every dongle and daemon has to be updated together.

## Decisions needed from you

1. **`admission_key` on the dongle** (a one-way-derived key that allows injection but not reading). The alternative is a dongle that holds nothing and relays anything well-formed, which lets outsiders flood the mesh through our relays. I recommend putting the key on the dongle, in RAM only.
2. **Does any dongle run without a host, e.g. as a battery relay?** If yes, `admission_key` has to live in flash, and a lost dongle then leaks the ability to inject.
3. **Message size privacy:** masking `total_chunks` hides almost nothing, because anyone can count the frames that share a `msg_id`. Real protection means padding to fixed bucket sizes, at an airtime cost. Pad or not?
4. **Cover traffic:** yes or no. Without it, an observer can still see when the mesh is active.

## What this plan can't hide

- It protects against outsiders only. Every swarm member holds the swarm key and can see, or impersonate, everything.
- A direction-finding adversary can still see that an 802.15.4 network exists on channel 15, its timing and volume, and which radio transmits a new `msg_id` first. No framing change fixes that.
- Future BLE advertising would be a new beacon and falls under the same rule.

I can write this up as `docs/plans/2026-09-28-…-production-hardening-plan.md` once the four decisions are made. P0 can start now, since it doesn't depend on any of them.
