#![no_std]
#![no_main]

pub mod hal;
pub mod radio;
pub mod storage;
pub mod ui;

use core::cell::RefCell;
use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::efuse::{self, InterfaceMacAddress};
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::main;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode;
use esp_hal::time::Rate;
use esp_println::println;

use gibberish_protocol::{
    is_valid_network_tag, ClosedTelemetry, DebugTelemetryPayload, DiagnosticEventCode, PeerTable,
    TelemetryTier, CIPHERTEXT_LEN, DEFAULT_NETWORK_TAG, FLAG_CLIPBOARD, FLAG_TELEMETRY,
};
use gibberish_storage::sram_ring::SramRingBuffer;

use crate::radio::ble_gate::{BleAuthState, BleSecurityGate};
use crate::radio::coex::{RadioSlot, TdmArbiter};
use crate::radio::dedup::SlidingBloomFilter;
use crate::radio::ieee802154::{BackoffController, RadioManager};
use crate::storage::sd_driver::{DynamicStorageManager, SpiSdBlockDevice};
use crate::ui::display::St7735;
use crate::ui::led::Apa102;

esp_bootloader_esp_idf::esp_app_desc!();

#[main]
fn main() -> ! {
    esp_alloc::heap_allocator!(size: 36 * 1024);
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let mut delay = Delay::new();

    println!("\n=============================================");
    println!(" Project Gibberish - Encrypted Mesh Dongle   ");
    println!(" LilyGO T-Dongle-C5 Bare-Metal Firmware      ");
    println!(" Zero-Trust Blind Transport & Dual-Mode Sink ");
    println!("=============================================\n");

    // 1. Read Hardware MAC for Node Identification
    let mac = efuse::interface_mac_address(InterfaceMacAddress::Station);
    let mac_bytes = mac.as_bytes();
    let mut full_mac = [0u8; 8];
    full_mac[2..8].copy_from_slice(mac_bytes);
    let local_node_id = u32::from_be_bytes([mac_bytes[2], mac_bytes[3], mac_bytes[4], mac_bytes[5]]);
    println!("Node ID: {:08X} (MAC: {:02X?})", local_node_id, mac_bytes);

    // 2. APA102 DotStar RGB LED (GPIO 4 Clock, GPIO 5 Data)
    let led_clk = Output::new(peripherals.GPIO4, Level::Low, OutputConfig::default());
    let led_data = Output::new(peripherals.GPIO5, Level::Low, OutputConfig::default());
    let mut led = Apa102::new(led_clk, led_data);
    led.set_idle_blue();

    // 3. ST7735 LCD Display (160x80) & MicroSD Slot on Shared SPI2 Bus
    let lcd_mosi = peripherals.GPIO2;
    let lcd_sck = peripherals.GPIO6;
    let sd_miso = Input::new(
        peripherals.GPIO7,
        InputConfig::default().with_pull(Pull::Up),
    );
    let lcd_cs = Output::new(peripherals.GPIO10, Level::High, OutputConfig::default());
    let lcd_dc = Output::new(peripherals.GPIO3, Level::High, OutputConfig::default());
    let lcd_rst = Output::new(peripherals.GPIO1, Level::High, OutputConfig::default());
    let lcd_bl = Output::new(peripherals.GPIO0, Level::Low, OutputConfig::default());
    let sd_cs = Output::new(peripherals.GPIO23, Level::High, OutputConfig::default());

    let spi_cfg = SpiConfig::default()
        .with_frequency(Rate::from_mhz(16))
        .with_mode(Mode::_0);
    let spi = Spi::new(peripherals.SPI2, spi_cfg)
        .unwrap()
        .with_sck(lcd_sck)
        .with_mosi(lcd_mosi)
        .with_miso(sd_miso);
    let spi_cell = RefCell::new(spi);

    let mut display = St7735::new(&spi_cell, lcd_cs, lcd_dc, lcd_rst, lcd_bl);
    display.init(&mut delay);
    println!("ST7735 LCD initialized (160x80 Landscape)");

    // 4. BOOT Button (GPIO 28, Active-Low with Pull-Up)
    let btn = Input::new(
        peripherals.GPIO28,
        InputConfig::default().with_pull(Pull::Up),
    );
    let mut btn_was_pressed = false;

    // 5. Authoritative SRAM Ring Buffer (32 KB / 256 chunks)
    let mut sram_ring = SramRingBuffer::new();

    // 6. Dynamic Storage Manager (Probe SD over SPI, fast fallback to RAM-Only)
    let mut storage = match SpiSdBlockDevice::new(&spi_cell, sd_cs, &mut delay) {
        Ok(sd_device) => {
            println!("MicroSD card detected & initialized in SPI mode (SDHC/SDSC active)");
            DynamicStorageManager::new_with_device(sd_device)
        }
        Err(_) => {
            println!("MicroSD card not present or init timeout -> Falling back to Ephemeral RAM-Only Mode");
            DynamicStorageManager::new_ram_only()
        }
    };
    println!("Storage initialized: Mode = {:?}", storage.mode());

    // 7. Active Mesh Peer Table (Recent 4 peers with RSSI dBm & hardware LQI)
    let mut peer_table = PeerTable::new();
    let mut sync_anim_ticks: u8 = 0;

    // Paint initial dashboard immediately
    display.render_dashboard(
        local_node_id,
        15,
        "2.425 GHz",
        storage.mode(),
        0,
        0,
        sram_ring.len(),
        sram_ring.dropped_count(),
        None,
        None,
        None,
        false,
        0,
    );

    // 8. Radio Deduplication (Sliding Bloom Filter + LRU)
    let mut bloom_filter = SlidingBloomFilter::new();

    // 8. TDM Arbiter (200ms cycle: 802.15.4 + BLE Coexistence)
    let mut tdm_arbiter = TdmArbiter::new();

    // 9. CSMA/CA Contention Backoff Controller
    let mut backoff = BackoffController::new();

    // 10. BLE Security Gate
    let mut ble_gate = BleSecurityGate::new();

    // 11. Hardware RNG for jitter and PINs
    let hw_rng = esp_hal::rng::Rng::new();

    // 12. IEEE 802.15.4 Radio Transceiver (Channel 15, 2.425 GHz)
    let mut radio = RadioManager::new(peripherals.IEEE802154, full_mac, local_node_id);
    println!("IEEE 802.15.4 Radio Transceiver active on Channel 15 (2.425 GHz)");

    // 13. USB Serial JTAG receiver for host CDC packets
    let (mut usb_rx, _usb_tx) = esp_hal::usb::usb_serial_jtag::UsbSerialJtag::new(peripherals.USB_DEVICE).split();
    let mut usb_pkt_buf = [0u8; 114];
    let mut usb_pkt_idx: usize = 0;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum UsbRxState {
        Sync0,
        Sync1,
        Length,
        Payload,
    }
    let mut usb_rx_state = UsbRxState::Sync0;

    // 14. Closed Telemetry State
    let mut telemetry = ClosedTelemetry::new();
    telemetry.last_event = DiagnosticEventCode::Boot;

    // Rendering and event timers
    let mut loop_tick: u32 = 0;
    let mut led_pulse_step: u8 = 0;
    let mut btn_flash_ticks: u32 = 0;
    let mut rx_flash_ticks: u32 = 0;
    let mut telem_flash_ticks: u32 = 0;
    let mut beacon_seq: u32 = 0;
    let mut telem_seq: u32 = 0;

    let calc_next_telem_interval = |rng: &esp_hal::rng::Rng| -> u32 {
        #[cfg(feature = "prod")]
        {
            55_000 + (rng.random() % 10_001) // 60s ± 5s
        }
        #[cfg(not(feature = "prod"))]
        {
            4_500 + (rng.random() % 1_001) // 5s ± 500ms
        }
    };
    let mut telem_elapsed_ms: u32 = 0;
    let mut next_telem_interval_ms: u32 = calc_next_telem_interval(&hw_rng);

    println!("Gibberish firmware initialization complete. Starting main loop.\n");

    loop {
        loop_tick = loop_tick.wrapping_add(1);
        let delta_ms: u32 = 5;
        delay.delay_millis(delta_ms);

        // Advance TDM radio arbiter
        let active_slot = tdm_arbiter.advance(delta_ms);

        // Check BOOT button (GPIO 28, active-low)
        let btn_is_pressed = btn.is_low();
        if btn_is_pressed && !btn_was_pressed {
            println!("Physical button press detected on GPIO 28!");
            btn_flash_ticks = 40; // 200ms visual flash

            beacon_seq = beacon_seq.wrapping_add(1);
            let beacon_msg_id = local_node_id.wrapping_add(beacon_seq);

            let beacon_hdr = gibberish_protocol::MeshHeader {
                network_tag: DEFAULT_NETWORK_TAG,
                msg_id: beacon_msg_id,
                chunk_idx: 0,
                total_chunks: 1,
                ttl: 3,
                hop_count: 0,
                flags: gibberish_protocol::FLAG_ACK_REQ,
            };
            let beacon_pkt = gibberish_protocol::MeshPacket {
                header: beacon_hdr,
                payload: [0xAA; 96],
            };
            // Record in bloom filter so this node does not accept its own relayed beacon
            bloom_filter.insert(beacon_msg_id, 0);
            // Broadcast beacon packet over the airwaves (scheduled via CSMA/CA backoff controller)
            backoff.schedule_tx(beacon_pkt, 15);

            if ble_gate.on_button_press() {
                println!("BLE Pairing authorized via physical button!");
                telemetry.last_event = DiagnosticEventCode::BleAuthGranted;
            } else {
                println!("Beacon broadcast scheduled via physical button! (MsgID: {:08X})", beacon_msg_id);
            }
        }
        btn_was_pressed = btn_is_pressed;
        if btn_flash_ticks > 0 {
            btn_flash_ticks -= 1;
        }
        if telem_flash_ticks > 0 {
            telem_flash_ticks -= 1;
        }
        if rx_flash_ticks > 0 {
            rx_flash_ticks -= 1;
        }

        // Service BLE Gate timeouts
        ble_gate.tick(delta_ms);

        // Slot processing
        match active_slot {
            RadioSlot::Ieee802154Mesh => {
                // 1. Service CSMA/CA backoff controller for pending transmissions
                if backoff.tick(delta_ms as u16) {
                    if let Some(packet) = backoff.take_pending() {
                        let tx_ok = radio.transmit(&packet);
                        if tx_ok {
                            telemetry.tx_packet_count = telemetry.tx_packet_count.saturating_add(1);
                            telemetry.last_event = DiagnosticEventCode::RadioTxOk;
                            println!(
                                "[Radio TX] Broadcasted MsgID: {:08X}, Chunk: {}/{}",
                                packet.header.msg_id,
                                packet.header.chunk_idx,
                                packet.header.total_chunks
                            );
                        } else {
                            println!(
                                "[Radio TX FAIL] MsgID: {:08X}, Chunk: {}/{}",
                                packet.header.msg_id,
                                packet.header.chunk_idx,
                                packet.header.total_chunks
                            );
                        }
                    }
                }

                // 2. Periodic Autonomous Telemetry Emitter (R4, R5, R11, R12)
                telem_elapsed_ms = telem_elapsed_ms.saturating_add(delta_ms);
                if telem_elapsed_ms >= next_telem_interval_ms {
                    telem_elapsed_ms = 0;
                    next_telem_interval_ms = calc_next_telem_interval(&hw_rng);

                    telem_seq = telem_seq.wrapping_add(1);
                    let telem_msg_id = telem_seq; // Monotonic sequence per plan R1/wire spec

                    let telem_hdr = gibberish_protocol::MeshHeader {
                        network_tag: DEFAULT_NETWORK_TAG,
                        msg_id: telem_msg_id,
                        chunk_idx: 0,
                        total_chunks: 1,
                        ttl: 1, // TTL=1: direct broadcast, consumed locally without mesh relay (R2)
                        hop_count: 0,
                        flags: FLAG_TELEMETRY,
                    };

                    let mut telem_payload = [0u8; CIPHERTEXT_LEN];
                    #[cfg(not(feature = "prod"))]
                    {
                        let mut dbg = DebugTelemetryPayload::new();
                        dbg.uptime_secs = telemetry.uptime_secs;
                        dbg.rx_count = telemetry.rx_packet_count;
                        dbg.tx_count = telemetry.tx_packet_count;
                        dbg.drop_count = sram_ring.dropped_count();
                        dbg.sram_used = sram_ring.len() as u16;
                        dbg.storage_mode = storage.mode();
                        dbg.last_event = telemetry.last_event;
                        dbg.build_tier = TelemetryTier::Debug;
                        dbg.free_heap_kb = (esp_alloc::HEAP.free() / 1024) as u8;
                        dbg.last_rssi = telemetry.last_rssi;
                        dbg.last_lqi = crate::radio::ieee802154::rssi_to_lqi(telemetry.last_rssi);
                        dbg.node_mac_tail = full_mac;
                        dbg.serialize(&mut telem_payload);
                    }
                    #[cfg(feature = "prod")]
                    {
                        telemetry.sram_ring_used = sram_ring.len() as u16;
                        telemetry.dropped_count = sram_ring.dropped_count();
                        telemetry.storage_mode = storage.mode();
                        let _ = postcard::to_slice(&telemetry, &mut telem_payload[..]);
                    }

                    let telem_pkt = gibberish_protocol::MeshPacket {
                        header: telem_hdr,
                        payload: telem_payload,
                    };

                    // User traffic strictly preempts telemetry; yields if user traffic pending (R11)
                    let telem_jitter = (hw_rng.random() % 46) as u16 + 15; // 15..=60ms contention backoff
                    let _ = backoff.schedule_telemetry(telem_pkt, telem_jitter);
                }

                // 3. Poll incoming 802.15.4 wireless mesh frames
                while let Some(rx) = radio.poll_rx() {
                    let packet = rx.packet;

                    // Verify admission tag
                    if !is_valid_network_tag(packet.header.network_tag) {
                        telemetry.last_event = DiagnosticEventCode::RadioDroppedTagMismatch;
                        continue;
                    }

                    // Determine sender node ID from 802.15.4 PHY header or debug telemetry payload
                    let effective_src_node = if rx.src_node_id != 0 {
                        rx.src_node_id
                    } else if (packet.header.flags & FLAG_TELEMETRY) != 0
                        && packet.payload.get(20).copied() == Some(TelemetryTier::Debug as u8)
                    {
                        DebugTelemetryPayload::deserialize(&packet.payload).node_id()
                    } else {
                        0
                    };

                    // Record peer metrics once per received frame
                    if effective_src_node != 0 && effective_src_node != local_node_id {
                        peer_table.record_peer(effective_src_node, rx.rssi, rx.lqi);
                    }

                    // Check for encrypted clipboard sync frame
                    if (packet.header.flags & FLAG_CLIPBOARD) != 0 {
                        sync_anim_ticks = 8; // 2 seconds pulsating animation at 4 Hz
                    }

                    // Check if this is a TELEMETRY frame (R2, R4, R5, R10)
                    // Telemetry frames have TTL=1, are consumed locally, and MUST NOT pollute the
                    // mesh deduplication bloom filter or cancel pending user packets via overhearing.
                    if (packet.header.flags & FLAG_TELEMETRY) != 0 {
                        telem_flash_ticks = 20; // 100ms magenta visual indicator
                        telemetry.rx_packet_count = telemetry.rx_packet_count.saturating_add(1);
                        telemetry.last_event = DiagnosticEventCode::RadioRxOk;
                        telemetry.last_rssi = rx.rssi;

                        if packet.payload.get(20).copied() == Some(TelemetryTier::Debug as u8) {
                            let dbg = DebugTelemetryPayload::deserialize(&packet.payload);
                            let node_id = if effective_src_node != 0 { effective_src_node } else { dbg.node_id() };
                            println!(
                                "[Telemetry RX] Node: {:08X}, Tier: {:?}, Storage: {:?}, Uptime: {}s, SRAM: {}/256, Drops: {}, RX: {}, TX: {}, RSSI: {} dBm, LQI: {}",
                                node_id,
                                dbg.build_tier,
                                dbg.storage_mode,
                                dbg.uptime_secs,
                                dbg.sram_used,
                                dbg.drop_count,
                                dbg.rx_count,
                                dbg.tx_count,
                                rx.rssi,
                                rx.lqi
                            );
                        } else if let Ok(closed) = postcard::from_bytes::<ClosedTelemetry>(&packet.payload) {
                            println!(
                                "[Telemetry RX] Node: {:08X}, Tier: {:?}, Storage: {:?}, Uptime: {}s, SRAM: {}/256, Drops: {}, RX: {}, TX: {}, RSSI: {} dBm, LQI: {}",
                                effective_src_node,
                                TelemetryTier::Prod,
                                closed.storage_mode,
                                closed.uptime_secs,
                                closed.sram_ring_used,
                                closed.dropped_count,
                                closed.rx_packet_count,
                                closed.tx_packet_count,
                                rx.rssi,
                                rx.lqi
                            );
                        } else {
                            println!(
                                "[Telemetry RX] Node: {:08X}, Tier: {:?}, Uptime: {}s, SRAM: {}/256, Drops: {}, RSSI: {} dBm, LQI: {}",
                                effective_src_node,
                                TelemetryTier::Debug,
                                0,
                                0,
                                0,
                                rx.rssi,
                                rx.lqi
                            );
                        }

                        // Local consumption without mesh relay (TTL=1) or SRAM ring pollution (R2)
                        continue;
                    }

                    // Deduplication check: drop packets already seen
                    if bloom_filter.contains(packet.header.msg_id, packet.header.chunk_idx) {
                        // Overhearing cancellation (R22): cancel pending retransmit if peer sent it
                        backoff.on_overhear(packet.header.msg_id, packet.header.chunk_idx);
                        continue;
                    }
                    bloom_filter.insert(packet.header.msg_id, packet.header.chunk_idx);

                    // Overhearing cancellation for our pending TX
                    backoff.on_overhear(packet.header.msg_id, packet.header.chunk_idx);

                    println!(
                        "[Radio RX] MsgID: {:08X}, Chunk: {}/{} | RSSI: {} dBm, LQI: {}",
                        packet.header.msg_id,
                        packet.header.chunk_idx,
                        packet.header.total_chunks,
                        rx.rssi,
                        rx.lqi
                    );

                    // Forward to host over USB-CDC as line-delimited ASCII:
                    // #PKT# <src_node_id:8hex> <wire_packet:228hex>
                    let mut wire_buf = [0u8; gibberish_protocol::MeshPacket::WIRE_PAYLOAD_LEN];
                    packet.serialize_payload(&mut wire_buf);
                    let mut hex_buf = [0u8; 228];
                    const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";
                    for (i, &b) in wire_buf.iter().enumerate() {
                        hex_buf[i * 2] = HEX_DIGITS[(b >> 4) as usize];
                        hex_buf[i * 2 + 1] = HEX_DIGITS[(b & 0xF) as usize];
                    }
                    if let Ok(hex_str) = core::str::from_utf8(&hex_buf) {
                        println!("#PKT# {:08X} {}", rx.src_node_id, hex_str);
                    }

                    // Push to authoritative SRAM ring buffer (which auto-flushes to SD or buffers in RAM)
                    sram_ring.push(packet);

                    // Mesh relay: forward if TTL > 0 and link quality satisfies minimum threshold (Issue #2)
                    let mut fwd_packet = packet;
                    if rx.lqi >= crate::radio::ieee802154::MIN_RELAY_LQI && fwd_packet.decrement_ttl() {
                        let jitter = BackoffController::calculate_lqi_relay_jitter(
                            rx.lqi,
                            (hw_rng.random() % 15) as u16,
                        );
                        backoff.schedule_tx(fwd_packet, jitter);
                    }

                    // Increment RX counter & update telemetry
                    telemetry.rx_packet_count = telemetry.rx_packet_count.saturating_add(1);
                    telemetry.last_event = DiagnosticEventCode::RadioRxOk;
                    telemetry.last_rssi = rx.rssi;
                    rx_flash_ticks = 20; // 100ms visual reception indicator
                }
            }
            RadioSlot::GuardBand => {
                // Synthesizer quiet time for PHY retuning (2ms)
            }
            RadioSlot::BleCompanion => {
                // Servicing BLE GATT notifications
            }
        }

        // Drain USB CDC RX buffer and ingest packets into SRAM ring with 4-state framing parser:
        // [0xAA, 0x55, 0x72, <114 wire bytes>]
        while let Ok(b) = usb_rx.read_byte() {
            match usb_rx_state {
                UsbRxState::Sync0 => {
                    if b == 0xAA {
                        usb_rx_state = UsbRxState::Sync1;
                    }
                }
                UsbRxState::Sync1 => {
                    if b == 0x55 {
                        usb_rx_state = UsbRxState::Length;
                    } else if b == 0xAA {
                        // Stay in Sync1 (consecutive 0xAA bytes)
                    } else {
                        usb_rx_state = UsbRxState::Sync0;
                    }
                }
                UsbRxState::Length => {
                    if b == 0x72 {
                        usb_pkt_idx = 0;
                        usb_rx_state = UsbRxState::Payload;
                    } else if b == 0xAA {
                        usb_rx_state = UsbRxState::Sync1;
                    } else {
                        usb_rx_state = UsbRxState::Sync0;
                    }
                }
                UsbRxState::Payload => {
                    usb_pkt_buf[usb_pkt_idx] = b;
                    usb_pkt_idx += 1;
                    if usb_pkt_idx == 114 {
                        let packet = gibberish_protocol::MeshPacket::deserialize_payload(&usb_pkt_buf);
                        if is_valid_network_tag(packet.header.network_tag) {
                            if (packet.header.flags & FLAG_CLIPBOARD) != 0 {
                                sync_anim_ticks = 8; // Trigger encrypted sync animation on transmission
                            }
                            println!(
                                "[USB RX] MsgID: {:08X}, Chunk: {}/{}",
                                packet.header.msg_id, packet.header.chunk_idx, packet.header.total_chunks
                            );
                            bloom_filter.insert(packet.header.msg_id, packet.header.chunk_idx);
                            sram_ring.push(packet);
                            backoff.schedule_tx(packet, 15);
                        }
                        usb_pkt_idx = 0;
                        usb_rx_state = UsbRxState::Sync0;
                    }
                }
            }
        }

        // Flush SRAM ring to storage in background (non-blocking)
        storage.flush_from_sram(&mut sram_ring);

        // Update LED status animation
        if btn_flash_ticks > 0 {
            led.set_green();
        } else if telem_flash_ticks > 0 {
            led.set_magenta(); // Magenta visual telemetry reception indicator (R10)
        } else if rx_flash_ticks > 0 {
            led.set_cyan(); // Cyan visual reception indicator
        } else {
            match ble_gate.state() {
                BleAuthState::PairingRequested { .. } => {
                    led_pulse_step = led_pulse_step.wrapping_add(1);
                    led.set_amber_pulse(led_pulse_step);
                }
                BleAuthState::Authorized => {
                    led.set_green();
                }
                _ => {
                    led.set_idle_blue();
                }
            }
        }

        // 1 Hz system uptime & diagnostic heartbeat (every 200 loops * 5ms = 1000ms)
        if loop_tick % 200 == 0 {
            telemetry.uptime_secs = telemetry.uptime_secs.saturating_add(1);

            if telemetry.uptime_secs % 2 == 0 {
                println!(
                    "[Node {:08X}] Uptime: {}s | Storage: {:?} | SRAM: {}/256 pkts | Drops: {}",
                    local_node_id,
                    telemetry.uptime_secs,
                    storage.mode(),
                    sram_ring.len(),
                    sram_ring.dropped_count()
                );
            }
        }

        // Periodic Dashboard Refresh (approx 4 Hz / every 50 loops * 5ms = 250ms)
        if loop_tick % 50 == 0 {
            telemetry.sram_ring_used = sram_ring.len() as u16;
            telemetry.dropped_count = sram_ring.dropped_count();
            telemetry.storage_mode = storage.mode();

            let ble_pin = match ble_gate.state() {
                BleAuthState::PairingRequested { pin, .. } => Some(pin),
                _ => None,
            };

            let sync_phase = if sync_anim_ticks > 0 {
                let phase = ((sync_anim_ticks % 4) + 1) as u8;
                sync_anim_ticks -= 1;
                phase
            } else {
                0
            };

            display.render_dashboard(
                local_node_id,
                15,
                "2.425 GHz",
                storage.mode(),
                telemetry.rx_packet_count,
                telemetry.tx_packet_count,
                sram_ring.len(),
                sram_ring.dropped_count(),
                peer_table.primary_peer(),
                peer_table.secondary_peer(),
                ble_pin,
                btn_flash_ticks > 0,
                sync_phase,
            );
        }
    }
}
