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
#[cfg(feature = "debug-telemetry")]
macro_rules! log_info {
    ($($arg:tt)*) => {
        esp_println::println!($($arg)*)
    };
}

#[cfg(not(feature = "debug-telemetry"))]
macro_rules! log_info {
    ($($arg:tt)*) => {
        if false {
            let _ = format_args!($($arg)*);
        }
    };
}

#[allow(unused_imports)]
use gibberish_protocol::{
    encode_cdc_frame, is_valid_network_tag, ClosedTelemetry, CompactDeltaPayload,
    DiagnosticEventCode, PeerTable, StaticMetadataBeacon, StorageModeStatus, TelemetryTier,
    DEFAULT_NETWORK_TAG, FLAG_CLIPBOARD, FLAG_TELEMETRY, FLAG_TELEMETRY_STATIC,
};
use gibberish_storage::sram_ring::SramRingBuffer;

use crate::radio::ble_gate::{BleAuthState, BleSecurityGate};
use crate::radio::coex::{RadioSlot, TdmArbiter};
use crate::radio::dedup::SlidingBloomFilter;
use crate::radio::ieee802154::{BackoffController, FrameKind, PendingTx, RadioManager};
use crate::storage::sd_driver::{DynamicStorageManager, SpiSdBlockDevice};
use crate::ui::display::St7735;
use crate::ui::led::Apa102;

esp_bootloader_esp_idf::esp_app_desc!();

#[main]
fn main() -> ! {
    esp_alloc::heap_allocator!(size: 36 * 1024);
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let mut delay = Delay::new();

    log_info!("\n=============================================");
    log_info!(" Project Gibberish - Encrypted Mesh Dongle   ");
    log_info!(" LilyGO T-Dongle-C5 Bare-Metal Firmware      ");
    log_info!(" Zero-Trust Blind Transport & Dual-Mode Sink ");
    log_info!("=============================================\n");

    // 1. Read Hardware MAC for Node Identification
    let mac = efuse::interface_mac_address(InterfaceMacAddress::Station);
    let mac_bytes = mac.as_bytes();
    let local_node_id = u32::from_be_bytes([mac_bytes[2], mac_bytes[3], mac_bytes[4], mac_bytes[5]]);
    let mut full_mac = [0u8; 8];
    full_mac[4..8].copy_from_slice(&local_node_id.to_be_bytes());
    log_info!("Node ID: {:08X} (MAC: {:02X?})", local_node_id, mac_bytes);

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
    log_info!("ST7735 LCD initialized (160x80 Landscape)");

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
            log_info!("MicroSD card detected & initialized in SPI mode (SDHC/SDSC active)");
            DynamicStorageManager::new_with_device(sd_device)
        }
        Err(_) => {
            log_info!("MicroSD card not present or init timeout -> Falling back to Ephemeral RAM-Only Mode");
            DynamicStorageManager::new_ram_only()
        }
    };
    log_info!("Storage initialized: Mode = {:?}", storage.mode());

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
    log_info!("IEEE 802.15.4 Radio Transceiver active on Channel 15 (2.425 GHz)");

    // 13. USB Serial JTAG receiver for host CDC packets
    let (mut usb_rx, mut usb_tx) = esp_hal::usb::usb_serial_jtag::UsbSerialJtag::new(peripherals.USB_DEVICE).split();
    let _ = &mut usb_tx;
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

    // Trickle Cadence (RFC 6206, KTD4): Imin = 10s, Imax = 60s, k = infinity
    let trickle_imin_ms: u32 = 10_000;
    let trickle_imax_ms: u32 = 60_000;
    let mut trickle_interval_ms: u32 = trickle_imin_ms;
    let mut trickle_elapsed_ms: u32 = 0;
    let choose_trickle_t = |interval: u32, rng: &esp_hal::rng::Rng| -> u32 {
        let half = interval / 2;
        let jitter = rng.random() % half.max(1);
        half + jitter
    };
    let mut trickle_t_ms: u32 = choose_trickle_t(trickle_interval_ms, &hw_rng);
    let mut trickle_fired = false;
    let mut reset_debounce_ms: u32 = 0;
    let mut prev_storage_mode = storage.mode();
    let mut prev_event_code = telemetry.last_event;

    // Static Metadata Beacon cadence: 180s ± 15s TRNG jitter (165s..=195s)
    let calc_next_static_interval = |rng: &esp_hal::rng::Rng| -> u32 {
        165_000 + (rng.random() % 30_001)
    };
    let mut static_elapsed_ms: u32 = 0;
    let mut next_static_interval_ms: u32 = calc_next_static_interval(&hw_rng);
    let mut force_static_beacon = true; // Send initial static beacon on boot so peers discover this node immediately

    log_info!("Gibberish firmware initialization complete. Starting main loop.\n");

    loop {
        loop_tick = loop_tick.wrapping_add(1);
        let delta_ms: u32 = 5;
        delay.delay_millis(delta_ms);

        // Advance TDM radio arbiter
        let active_slot = tdm_arbiter.advance(delta_ms);

        // Check BOOT button (GPIO 28, active-low)
        let btn_is_pressed = btn.is_low();
        if btn_is_pressed && !btn_was_pressed {
            log_info!("Physical button press detected on GPIO 28!");
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
            let _ = backoff.schedule_high(beacon_pkt, 15);

            if ble_gate.on_button_press() {
                log_info!("BLE Pairing authorized via physical button!");
                telemetry.last_event = DiagnosticEventCode::BleAuthGranted;
            } else {
                log_info!("Beacon broadcast scheduled via physical button! (MsgID: {:08X})", beacon_msg_id);
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
                // 1. Service CSMA/CA backoff controller for pending transmissions (two-tier priority)
                if backoff.tick(delta_ms as u16) {
                    if let Some(pending) = backoff.take_pending() {
                        match pending {
                            PendingTx::High(packet) => {
                                let tx_ok = radio.transmit(&packet);
                                if tx_ok {
                                    telemetry.tx_packet_count = telemetry.tx_packet_count.saturating_add(1);
                                    telemetry.last_event = DiagnosticEventCode::RadioTxOk;
                                    log_info!(
                                        "[Radio TX] Broadcasted MsgID: {:08X}, Chunk: {}/{}",
                                        packet.header.msg_id,
                                        packet.header.chunk_idx,
                                        packet.header.total_chunks
                                    );
                                } else {
                                    log_info!(
                                        "[Radio TX FAIL] MsgID: {:08X}, Chunk: {}/{}",
                                        packet.header.msg_id,
                                        packet.header.chunk_idx,
                                        packet.header.total_chunks
                                    );
                                }
                            }
                            PendingTx::Low(frame) => {
                                let tx_ok = radio.transmit_variable(&frame.header, frame.payload_slice());
                                if tx_ok {
                                    telemetry.tx_packet_count = telemetry.tx_packet_count.saturating_add(1);
                                    telemetry.last_event = DiagnosticEventCode::RadioTxOk;
                                    log_info!(
                                        "[Radio TX Low] Broadcasted MsgID: {:08X}, Len: {}",
                                        frame.header.msg_id,
                                        frame.payload_len
                                    );
                                } else {
                                    log_info!(
                                        "[Radio TX Low FAIL] MsgID: {:08X}, Len: {}",
                                        frame.header.msg_id,
                                        frame.payload_len
                                    );
                                }
                            }
                        }
                    }
                }

                // 2. Autonomous Static Metadata Beacon (180s ± 15s TRNG jitter or SD mount/unmount)
                static_elapsed_ms = static_elapsed_ms.saturating_add(delta_ms);
                if static_elapsed_ms >= next_static_interval_ms || force_static_beacon {
                    static_elapsed_ms = 0;
                    force_static_beacon = false;
                    next_static_interval_ms = calc_next_static_interval(&hw_rng);

                    telem_seq = telem_seq.wrapping_add(1);
                    let telem_msg_id = telem_seq;

                    let telem_hdr = gibberish_protocol::MeshHeader {
                        network_tag: DEFAULT_NETWORK_TAG,
                        msg_id: telem_msg_id,
                        chunk_idx: 0,
                        total_chunks: 1,
                        ttl: 1,
                        hop_count: 0,
                        flags: FLAG_TELEMETRY_STATIC,
                    };

                    let mut beacon = StaticMetadataBeacon::new();
                    beacon.node_id = full_mac;
                    beacon.uptime_epoch = (telemetry.uptime_secs / u32::MAX) as u16;
                    #[cfg(not(feature = "debug-telemetry"))]
                    {
                        beacon.build_tier = TelemetryTier::Prod;
                    }
                    #[cfg(feature = "debug-telemetry")]
                    {
                        beacon.build_tier = TelemetryTier::Debug;
                    }
                    beacon.storage_mode = storage.mode();
                    beacon.config_epoch = 1;

                    let mut beacon_buf = [0u8; StaticMetadataBeacon::BYTE_LEN];
                    if beacon.serialize(&mut beacon_buf).is_ok() {
                        let telem_jitter = (hw_rng.random() % 46) as u16 + 15;
                        backoff.schedule_low(telem_hdr, &beacon_buf, telem_jitter);
                    }
                }

                // 3. RFC 6206 Trickle Cadence (10s..60s, monotonic consistency, discrete event reset with 10s debounce)
                trickle_elapsed_ms = trickle_elapsed_ms.saturating_add(delta_ms);
                reset_debounce_ms = reset_debounce_ms.saturating_sub(delta_ms);

                let curr_storage_mode = storage.mode();
                let is_discrete_event = (curr_storage_mode != prev_storage_mode)
                    || (telemetry.last_event != prev_event_code
                        && matches!(
                            telemetry.last_event,
                            DiagnosticEventCode::StorageOverflow | DiagnosticEventCode::RadioDroppedTagMismatch
                        ));

                if is_discrete_event {
                    if curr_storage_mode != prev_storage_mode {
                        force_static_beacon = true;
                    }
                    if reset_debounce_ms == 0 {
                        trickle_interval_ms = trickle_imin_ms;
                        trickle_elapsed_ms = 0;
                        trickle_t_ms = choose_trickle_t(trickle_interval_ms, &hw_rng);
                        trickle_fired = false;
                        reset_debounce_ms = 10_000; // 10s debounce dwell per KTD4
                        prev_storage_mode = curr_storage_mode;
                        prev_event_code = telemetry.last_event;
                    }
                }

                if !trickle_fired && trickle_elapsed_ms >= trickle_t_ms {
                    trickle_fired = true;

                    telem_seq = telem_seq.wrapping_add(1);
                    let telem_msg_id = telem_seq;

                    let telem_hdr = gibberish_protocol::MeshHeader {
                        network_tag: DEFAULT_NETWORK_TAG,
                        msg_id: telem_msg_id,
                        chunk_idx: 0,
                        total_chunks: 1,
                        ttl: 1,
                        hop_count: 0,
                        flags: FLAG_TELEMETRY,
                    };

                    let mut delta_payload = CompactDeltaPayload::new();
                    delta_payload.uptime_secs = telemetry.uptime_secs;
                    delta_payload.rx_count = telemetry.rx_packet_count;
                    delta_payload.tx_count = telemetry.tx_packet_count;
                    delta_payload.drop_count = sram_ring.dropped_count();
                    delta_payload.sram_used = sram_ring.len() as u16;
                    delta_payload.config_epoch = 1;
                    delta_payload.free_heap_kb = (esp_alloc::HEAP.free() / 1024) as u8;
                    delta_payload.last_event = telemetry.last_event;
                    delta_payload.last_rssi = telemetry.last_rssi;
                    delta_payload.last_lqi = crate::radio::ieee802154::rssi_to_lqi(telemetry.last_rssi);

                    let mut delta_buf = [0u8; CompactDeltaPayload::BYTE_LEN];
                    if delta_payload.serialize(&mut delta_buf).is_ok() {
                        let telem_jitter = (hw_rng.random() % 46) as u16 + 15;
                        backoff.schedule_low(telem_hdr, &delta_buf, telem_jitter);
                    }
                }

                if trickle_elapsed_ms >= trickle_interval_ms {
                    trickle_elapsed_ms = 0;
                    trickle_interval_ms = (trickle_interval_ms.saturating_mul(2)).min(trickle_imax_ms);
                    trickle_t_ms = choose_trickle_t(trickle_interval_ms, &hw_rng);
                    trickle_fired = false;
                }

                // 4. Poll incoming 802.15.4 wireless mesh frames (variable-length PHY)
                while let Some(rx_frame) = radio.poll_rx_frame() {
                    let header = rx_frame.header;

                    // Verify admission tag
                    if !is_valid_network_tag(header.network_tag) {
                        telemetry.last_event = DiagnosticEventCode::RadioDroppedTagMismatch;
                        continue;
                    }

                    let effective_src_node = rx_frame.src_node_id;

                    // Record peer metrics once per received frame
                    if effective_src_node != 0 && effective_src_node != local_node_id {
                        peer_table.record_peer(effective_src_node, rx_frame.rssi, rx_frame.lqi);
                    }

                    match rx_frame.kind {
                        FrameKind::StaticMetadata(_beacon) => {
                            telem_flash_ticks = 20; // 100ms magenta visual indicator
                            telemetry.rx_packet_count = telemetry.rx_packet_count.saturating_add(1);
                            telemetry.last_event = DiagnosticEventCode::RadioRxOk;
                            telemetry.last_rssi = rx_frame.rssi;

                            log_info!(
                                "[Telemetry RX] Node: {:08X}, Tier: {:?}, Storage: {:?}, Epoch: {}, UptimeEpoch: {}, RSSI: {} dBm, LQI: {}",
                                _beacon.node_id_u32(),
                                _beacon.build_tier,
                                _beacon.storage_mode,
                                _beacon.config_epoch,
                                _beacon.uptime_epoch,
                                rx_frame.rssi,
                                rx_frame.lqi
                            );
                            continue;
                        }
                        FrameKind::CompactDelta(_delta) => {
                            telem_flash_ticks = 20;
                            telemetry.rx_packet_count = telemetry.rx_packet_count.saturating_add(1);
                            telemetry.last_event = DiagnosticEventCode::RadioRxOk;
                            telemetry.last_rssi = rx_frame.rssi;

                            log_info!(
                                "[Telemetry RX] Node: {:08X}, Tier: {:?}, Storage: {:?}, Uptime: {}s, SRAM: {}/256, Drops: {}, RX: {}, TX: {}, RSSI: {} dBm, LQI: {}",
                                effective_src_node,
                                TelemetryTier::Debug,
                                storage.mode(),
                                _delta.uptime_secs,
                                _delta.sram_used,
                                _delta.drop_count,
                                _delta.rx_count,
                                _delta.tx_count,
                                rx_frame.rssi,
                                rx_frame.lqi
                            );
                            continue;
                        }
                        FrameKind::Mesh(packet) => {
                            if (packet.header.flags & FLAG_CLIPBOARD) != 0 {
                                sync_anim_ticks = 8;
                            }

                            if bloom_filter.contains(packet.header.msg_id, packet.header.chunk_idx) {
                                backoff.on_overhear(packet.header.msg_id, packet.header.chunk_idx);
                                continue;
                            }
                            bloom_filter.insert(packet.header.msg_id, packet.header.chunk_idx);
                            backoff.on_overhear(packet.header.msg_id, packet.header.chunk_idx);

                            log_info!(
                                "[Radio RX] MsgID: {:08X}, Chunk: {}/{} | RSSI: {} dBm, LQI: {}",
                                packet.header.msg_id,
                                packet.header.chunk_idx,
                                packet.header.total_chunks,
                                rx_frame.rssi,
                                rx_frame.lqi
                            );

                            #[cfg(feature = "debug-telemetry")]
                            {
                                let mut wire_buf = [0u8; gibberish_protocol::MeshPacket::WIRE_PAYLOAD_LEN];
                                packet.serialize_payload(&mut wire_buf);
                                let mut hex_buf = [0u8; 228];
                                const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";
                                for (i, &b) in wire_buf.iter().enumerate() {
                                    hex_buf[i * 2] = HEX_DIGITS[(b >> 4) as usize];
                                    hex_buf[i * 2 + 1] = HEX_DIGITS[(b & 0xF) as usize];
                                }
                                if let Ok(hex_str) = core::str::from_utf8(&hex_buf) {
                                    log_info!("#PKT# {:08X} {}", rx_frame.src_node_id, hex_str);
                                }
                            }

                            #[cfg(not(feature = "debug-telemetry"))]
                            {
                                let mut wire_buf = [0u8; gibberish_protocol::MeshPacket::WIRE_PAYLOAD_LEN];
                                packet.serialize_payload(&mut wire_buf);
                                let mut cdc_buf = [0u8; 128];
                                if let Ok(len) = gibberish_protocol::encode_cdc_frame(&wire_buf, &mut cdc_buf) {
                                    let _ = usb_tx.write(&cdc_buf[..len]);
                                    let _ = usb_tx.flush_tx();
                                }
                            }

                            sram_ring.push(packet);

                            let mut fwd_packet = packet;
                            if rx_frame.lqi >= crate::radio::ieee802154::MIN_RELAY_LQI && fwd_packet.decrement_ttl() {
                                let jitter = BackoffController::calculate_lqi_relay_jitter(
                                    rx_frame.lqi,
                                    (hw_rng.random() % 15) as u16,
                                );
                                let _ = backoff.schedule_high(fwd_packet, jitter);
                            }

                            telemetry.rx_packet_count = telemetry.rx_packet_count.saturating_add(1);
                            telemetry.last_event = DiagnosticEventCode::RadioRxOk;
                            telemetry.last_rssi = rx_frame.rssi;
                            rx_flash_ticks = 20;
                        }
                    }
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
                            log_info!(
                                "[USB RX] MsgID: {:08X}, Chunk: {}/{}",
                                packet.header.msg_id, packet.header.chunk_idx, packet.header.total_chunks
                            );
                            bloom_filter.insert(packet.header.msg_id, packet.header.chunk_idx);
                            sram_ring.push(packet);
                            if backoff.schedule_high(packet, 15).is_err() {
                                telemetry.dropped_count = telemetry.dropped_count.saturating_add(1);
                                telemetry.last_event = DiagnosticEventCode::StorageOverflow;
                            }
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

            #[cfg(not(feature = "debug-telemetry"))]
            {
                telemetry.sram_ring_used = sram_ring.len() as u16;
                telemetry.dropped_count = sram_ring.dropped_count();
                telemetry.storage_mode = storage.mode();

                let mut postcard_buf = [0u8; 64];
                if let Ok(slice) = postcard::to_slice(&telemetry, &mut postcard_buf) {
                    let mut cdc_buf = [0u8; 80];
                    if let Ok(len) = encode_cdc_frame(slice, &mut cdc_buf) {
                        let _ = usb_tx.write(&cdc_buf[..len]);
                        let _ = usb_tx.flush_tx();
                    }
                }
            }

            if telemetry.uptime_secs % 2 == 0 {
                log_info!(
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
